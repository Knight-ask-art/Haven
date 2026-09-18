//! SettingProposalService：设置变更提案的创建/读取/拒绝与**显式确认**应用
//! （V02-SETTING-PROPOSAL-001）。
//!
//! 分工（与领域模型的意图一致）：
//! - Agent / UI 入口通过 Application Service 创建/读取/拒绝提案；用户可见的 Agent
//!   批准必须走 `approve_agent_proposal_in_one_uow`，由同一个 UoW 生成并消费一次性
//!   Approval Token。底层的分步 token 原语只为安全内核测试/内部编排保留，不能被 UI
//!   当成两次独立提交使用。提案本身不改变任何设置事实，只是"要改什么"的候选描述。
//! - 普通用户提案通过 `apply_confirmed` 携带**调用方展示给用户的 digest**；
//!   Agent 提案通过 `approve_agent_proposal_in_one_uow` 携带 digest 与固定设置范围。
//!   两条路径都在这里收敛，digest 与存储不一致即拒绝执行。
//!
//! 用户可见的 Agent 批准以及普通提案应用的全部编排都在**单个 UoW 事务**内完成：
//! 事务内按 id 重读提案 → 校验完整性/状态/时效/digest/目标兼容性/base_revision →
//! Agent 路径在内存生成 token、条件写入摘要、条件消费摘要 → 条件 CAS 写目标 → 插入
//! 回执 → 提案 pending → applied。除过期分支只提交 `pending → expired` 状态迁移外，
//! 任何冲突/已拒绝/digest 不一致/目标非法/token 无效都不会写目标、不会插回执（事务
//! 整体回滚）。`issue_agent_approval_token` 与
//! `apply_confirmed_with_approval_token` 是分步内核原语，不能用于用户可见批准链。
//!
//! 本模块不写 SQL：目标读写、回执与状态迁移都由 `SettingProposalTxPorts` 表达，
//! 由 Infrastructure 在既有 Db 锁 + 单一 IMMEDIATE 事务内实现。

use std::sync::{Arc, Mutex};

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::agent::{
    AgentActionBinding, AgentActionBindingInput, AgentActionKind, AgentApprovalState,
    AgentApprovalToken, AgentApprovalTokenHash, AgentScopeSubject, AgentSettingsSubject,
};
use haven_domain::contracts::{
    EditionPreference, MediaItemPreference, SettingProposalRepository, SettingsRow,
};
use haven_domain::ids::{EditionId, MediaItemId, SettingChangeReceiptId, SettingProposalId};
use haven_domain::setting_proposal::{
    ProvenanceActorKind, ProvenanceReason, ProvenanceSourceKind, SettingChangeReceipt,
    SettingProposal, SettingProposalChange, SettingProposalStatus, SettingProvenance,
    SettingTarget, canonical_json_of, setting_proposal_digest_mismatch_error,
    setting_proposal_expired_error, setting_proposal_integrity_error,
    setting_proposal_invalid_target_error, setting_proposal_not_found_error,
    setting_proposal_not_pending_error, setting_proposal_rejected_error,
    setting_proposal_revision_conflict_error, setting_proposal_target_not_found_error,
};
use haven_domain::settings::{PreferenceData, SettingsPatch, SettingsSection, SettingsValue};

/// 提案默认有效期：24 小时。
///
/// 设置变更的确认动作应该在同一个使用会话里完成；超过一天仍未确认的提案，
/// 其 base_revision 几乎必然已经过期，留着只会制造"点了确认却报冲突"的迷惑。
pub const DEFAULT_SETTING_PROPOSAL_TTL_MS: i64 = 24 * 60 * 60 * 1_000;

/// 创建提案的输入。
///
/// `created_at` 由 Service 决定（"现在"），TTL 由调用方可选覆盖；领域层只校验
/// `expires_at > created_at` 并固化 canonical JSON 与 digest。
#[derive(Debug, Clone)]
pub struct SettingProposalRequest {
    pub target: SettingTarget,
    pub change: SettingProposalChange,
    /// 目标作用域当前 authoritative revision；`None` 表示目标行尚未持久化。
    pub base_revision: Option<String>,
    pub provenance: SettingProvenance,
    pub ttl_ms: Option<i64>,
}

/// Agent 来源的提案必须带有一条自洽绑定，并且绑定状态必须与提案状态一致。
///
/// 这个检查位于 SettingProposal UoW 内，因而 apply/reject 不能只依赖外层动作对象或
/// UoW 之外的快照；缺绑定被视为完整性错误，而不是“没有 Agent 上下文也照常执行”。
fn validate_agent_binding_for_proposal(
    tx: &dyn SettingProposalTxPorts,
    proposal: &SettingProposal,
) -> Result<Option<AgentActionBinding>, AppError> {
    let binding = tx.load_agent_action_binding(proposal.id())?;
    if !provenance_requires_agent_binding(proposal.provenance()) {
        if binding.is_some() {
            return Err(agent_binding_integrity("非 Agent 提案不应存在动作绑定"));
        }
        return Ok(None);
    }
    if proposal.provenance().source_kind != ProvenanceSourceKind::Agent
        || proposal.provenance().actor_kind != ProvenanceActorKind::Agent
        || proposal.provenance().reason != ProvenanceReason::AgentSuggestion
    {
        return Err(agent_binding_integrity("Agent 提案的 provenance 组合非法"));
    }
    let binding = binding.ok_or_else(agent_binding_missing)?;
    if binding.proposal_id() != proposal.id()
        || binding.action_kind() != AgentActionKind::SettingsProposal
        || binding.created_at() != proposal.created_at()
        || binding.expires_at() != proposal.expires_at()
        || binding.approval_state() != agent_state_for_proposal(proposal.status())
    {
        return Err(agent_binding_integrity("绑定与设置提案生命周期不一致"));
    }
    Ok(Some(binding))
}

fn agent_state_for_proposal(status: SettingProposalStatus) -> AgentApprovalState {
    match status {
        SettingProposalStatus::Pending => AgentApprovalState::Pending,
        SettingProposalStatus::Applied => AgentApprovalState::Applied,
        SettingProposalStatus::Rejected => AgentApprovalState::Rejected,
        SettingProposalStatus::Expired => AgentApprovalState::Expired,
    }
}

impl SettingProposalRequest {
    pub fn new(
        target: SettingTarget,
        change: SettingProposalChange,
        provenance: SettingProvenance,
    ) -> Self {
        Self {
            target,
            change,
            base_revision: None,
            provenance,
            ttl_ms: None,
        }
    }

    pub fn with_base_revision(mut self, base_revision: Option<String>) -> Self {
        self.base_revision = base_revision;
        self
    }

    pub fn with_ttl_ms(mut self, ttl_ms: i64) -> Self {
        self.ttl_ms = Some(ttl_ms);
        self
    }
}

/// 事务内可用的提案应用原语。
///
/// 每个方法都必须在**同一个** `SettingProposalUoW` 事务内被调用；这里刻意不提供
/// "直接写目标"的捷径——所有写路径都要求传入期望 revision，由 SQL 条件写兜底。
pub trait SettingProposalTxPorts {
    /// 在当前 UoW 中插入一条全新的 pending 提案。
    fn insert_proposal(&self, proposal: &SettingProposal) -> Result<(), AppError>;

    /// 在当前 UoW 中插入与提案绑定的最小 Agent 元数据。
    fn insert_agent_action_binding(&self, binding: &AgentActionBinding) -> Result<(), AppError>;

    /// 在当前 UoW 内读取 Agent 绑定。`None` 只表示这不是 Agent 提案；对于 Agent 来源
    /// 提案，Application 必须把缺失视为完整性错误。
    fn load_agent_action_binding(
        &self,
        proposal_id: SettingProposalId,
    ) -> Result<Option<AgentActionBinding>, AppError>;

    /// 在同一事务内校验 Subject 的真实 Work → Edition → MediaItem 归属。
    ///
    /// 全局设置范围的 Subject 没有作品身份可校验，实现必须是 no-op；
    /// 设置范围的权威性由 [`SettingProposalTxPorts::verify_settings_base_revision`] 承担。
    fn verify_agent_subject(&self, subject: AgentScopeSubject) -> Result<(), AppError>;

    /// 在同一事务内核对全局分区的 authoritative revision 仍是提案的 base revision。
    ///
    /// 这是"上下文过期"的**事务内**判据：读取（重建上下文）与写入（落库提案）之间
    /// 若有并发写入者推进了 revision，这里必须 fail-closed，而不是先落一条注定冲突的提案。
    fn verify_settings_base_revision(
        &self,
        section: &str,
        expected_revision: Option<&str>,
    ) -> Result<bool, AppError>;

    /// pending → rejected 的条件写。
    fn mark_rejected(
        &self,
        id: SettingProposalId,
        expected_digest: &str,
        rejected_at: UtcMillis,
    ) -> Result<bool, AppError>;

    /// 与提案状态迁移一起更新 Agent 绑定状态；非 Agent 提案没有绑定时必须是幂等 no-op。
    fn update_agent_action_binding_state(
        &self,
        proposal_id: SettingProposalId,
        expected: AgentApprovalState,
        next: AgentApprovalState,
    ) -> Result<(), AppError>;

    /// 事务内按 id 重读提案（含 canonical/digest/冗余 target 列完整性校验）。
    fn load_proposal(&self, id: SettingProposalId) -> Result<Option<SettingProposal>, AppError>;

    /// 读取已存回执（未应用返回 `None`）。
    ///
    /// 实现必须返回**已通过 `SettingChangeReceipt::validate`** 的自洽回执：
    /// `apply_confirmed` 的普通提案幂等重放路径会把它原样交给调用方，未校验的行一旦从这里
    /// 流出，就成了一条无法跨进程重放的"审计事实"。无法自洽的行是完整性错误，
    /// 不是 `None`。
    fn load_receipt(&self, id: SettingProposalId)
    -> Result<Option<SettingChangeReceipt>, AppError>;

    /// 版本是否存在（Edition 目标的稳定 target-not-found 判据）。
    fn edition_exists(&self, edition_id: EditionId) -> Result<bool, AppError>;

    /// 媒体条目真实所属版本（MediaItem 目标的归属交叉校验）。
    fn media_item_edition(&self, media_item_id: MediaItemId)
    -> Result<Option<EditionId>, AppError>;

    /// 读取全局分区原始行（从未保存返回 `None`，由调用方回落到分区默认值）。
    fn load_settings(&self, section: &str) -> Result<Option<SettingsRow>, AppError>;

    /// 全局设置的条件写：`expected_revision` 作为 SQL 条件，affected == 0 → `false`。
    fn cas_write_settings(
        &self,
        section: &str,
        expected_revision: Option<&str>,
        row: &SettingsRow,
    ) -> Result<bool, AppError>;

    fn load_edition_preference(
        &self,
        edition_id: EditionId,
    ) -> Result<Option<EditionPreference>, AppError>;

    fn load_media_item_preference(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Option<MediaItemPreference>, AppError>;

    fn cas_upsert_edition(
        &self,
        preference: &EditionPreference,
        expected_revision: Option<&str>,
    ) -> Result<bool, AppError>;

    fn cas_upsert_media_item(
        &self,
        preference: &MediaItemPreference,
        expected_revision: Option<&str>,
    ) -> Result<bool, AppError>;

    /// 插入变更回执（append-only；同一提案重复插入由唯一约束拒绝）。
    fn insert_receipt(&self, receipt: &SettingChangeReceipt) -> Result<(), AppError>;

    /// 提案 pending → applied 的条件写（status 与 digest 双条件）。
    /// 返回 `false` 表示条件未命中，调用方必须回滚整个事务。
    fn mark_applied(
        &self,
        id: SettingProposalId,
        expected_digest: &str,
        applied_at: UtcMillis,
    ) -> Result<bool, AppError>;

    /// 到期未确认的提案 pending → expired 的条件写（status 与 digest 双条件）。
    ///
    /// 与 `mark_applied` 不同，这里**没有**配套的目标写入要连带回滚：置为 expired
    /// 本身就是全部工作，调用方无论如何都会向用户返回 `SETTING_PROPOSAL_EXPIRED`，
    /// 因此条件未命中不算错误（也无需向外区分）。实现必须保证：条件不满足时
    /// 不写任何行，绝不把已 applied/rejected/expired 的提案改写成别的状态。
    fn mark_expired(
        &self,
        id: SettingProposalId,
        expected_digest: &str,
        expired_at: UtcMillis,
    ) -> Result<(), AppError>;

    /// 为 pending Agent 提案条件签发（或覆盖）一枚审批令牌摘要。
    ///
    /// 令牌明文只在 Application 内存中存在；实现只能持久化摘要，且必须把
    /// proposal digest + pending 状态作为条件的一部分。
    fn issue_agent_action_approval_token(
        &self,
        id: SettingProposalId,
        expected_digest: &str,
        approval_token_hash: &AgentApprovalTokenHash,
        issued_at: UtcMillis,
    ) -> Result<bool, AppError>;

    /// 在同一 UoW 内条件消费审批令牌。成功后摘要与签发时间必须同时清除。
    fn consume_agent_action_approval_token(
        &self,
        id: SettingProposalId,
        expected_digest: &str,
        approval_token_hash: &AgentApprovalTokenHash,
        consumed_at: UtcMillis,
    ) -> Result<bool, AppError>;
}

/// SettingProposal Unit of Work：闭包在**单一 IMMEDIATE 事务**内执行，失败自动回滚。
pub trait SettingProposalUoW: Send + Sync {
    fn run(
        &self,
        f: &dyn Fn(&dyn SettingProposalTxPorts) -> Result<(), AppError>,
    ) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct SettingProposalService {
    repository: Arc<dyn SettingProposalRepository>,
    uow: Arc<dyn SettingProposalUoW>,
}

impl SettingProposalService {
    pub fn new(
        repository: Arc<dyn SettingProposalRepository>,
        uow: Arc<dyn SettingProposalUoW>,
    ) -> Self {
        Self { repository, uow }
    }

    /// 创建提案（status 固定 pending，不触碰任何设置事实）。
    ///
    /// target/change 的结构兼容性、provenance 安全性、base_revision 字符集与
    /// 有效期都由 `SettingProposal::new` 在构造时拒绝，Service 不重复实现这些规则。
    pub async fn create(
        &self,
        request: SettingProposalRequest,
    ) -> Result<SettingProposal, AppError> {
        if provenance_requires_agent_binding(&request.provenance) {
            return Err(agent_binding_required());
        }
        let now = UtcMillis::now();
        let ttl = request.ttl_ms.unwrap_or(DEFAULT_SETTING_PROPOSAL_TTL_MS);
        let proposal = SettingProposal::new(
            SettingProposalId::new(),
            request.target,
            request.change,
            request.base_revision,
            request.provenance,
            now,
            UtcMillis(now.0.saturating_add(ttl)),
        )?;
        self.repository.create(&proposal).await?;
        Ok(proposal)
    }

    /// 原子创建 Agent 设置提案与最小动作绑定。
    ///
    /// 提案 ID、创建/过期时间和 pending 状态都由本方法统一派生；两条记录必须在同一
    /// `BEGIN IMMEDIATE` UoW 中成功，否则全部回滚，不留下没有绑定的 Agent pending 提案。
    pub async fn create_with_agent_binding(
        &self,
        request: SettingProposalRequest,
        binding_input: AgentActionBindingInput,
    ) -> Result<(SettingProposal, AgentActionBinding), AppError> {
        if request.provenance.source_kind != ProvenanceSourceKind::Agent
            || request.provenance.actor_kind != ProvenanceActorKind::Agent
            || request.provenance.reason != ProvenanceReason::AgentSuggestion
        {
            return Err(agent_binding_provenance_mismatch());
        }
        if !binding_input.subject().covers(request.target) {
            return Err(agent_binding_subject_mismatch());
        }

        let now = UtcMillis::now();
        let ttl = request.ttl_ms.unwrap_or(DEFAULT_SETTING_PROPOSAL_TTL_MS);
        let proposal = SettingProposal::new(
            SettingProposalId::new(),
            request.target,
            request.change,
            request.base_revision,
            request.provenance,
            now,
            UtcMillis(now.0.saturating_add(ttl)),
        )?;
        let binding = AgentActionBinding::pending(
            proposal.id(),
            binding_input,
            proposal.created_at(),
            proposal.expires_at(),
        )?;

        self.uow.run(&|tx| {
            tx.verify_agent_subject(binding.subject())?;
            // 全局设置范围没有作品归属，但**必须**在同一个事务里核对基线版本：
            // 上下文是照着某个 revision 读出来的，落库前 revision 已经变了就说明
            // 这份上下文已过期，不能落一条注定冲突的提案。
            if binding.subject().as_settings().is_some() {
                let expected = proposal.base_revision();
                if !tx.verify_settings_base_revision(
                    request
                        .target
                        .section()
                        .map(|s| s.as_str())
                        .unwrap_or_default(),
                    expected,
                )? {
                    return Err(setting_proposal_revision_conflict_error());
                }
            }
            tx.insert_proposal(&proposal)?;
            tx.insert_agent_action_binding(&binding)?;
            Ok(())
        })?;
        Ok((proposal, binding))
    }

    /// 读取提案（含完整性校验；不存在返回 `None`）。
    pub async fn get(&self, id: SettingProposalId) -> Result<Option<SettingProposal>, AppError> {
        self.repository.get(id).await
    }

    /// 读取变更回执（未应用返回 `None`）。
    pub async fn get_receipt(
        &self,
        id: SettingProposalId,
    ) -> Result<Option<SettingChangeReceipt>, AppError> {
        self.repository.get_receipt(id).await
    }

    /// 拒绝提案：必须携带调用方展示的 digest，走 pending + digest 条件写。
    ///
    /// - 已 rejected 且 digest 一致 → 幂等成功（UI 重复点击不报错）；
    /// - applied / expired → 稳定 `SETTING_PROPOSAL_NOT_PENDING`（终态不可再拒绝）；
    /// - digest 不一致 → 稳定 `SETTING_PROPOSAL_DIGEST_MISMATCH`。
    pub async fn reject(
        &self,
        id: SettingProposalId,
        expected_digest: &str,
    ) -> Result<(), AppError> {
        let confirmed = expected_digest.to_owned();
        let result = Arc::new(Mutex::new(None::<Result<(), AppError>>));
        self.uow.run(&|tx| {
            let proposal = tx
                .load_proposal(id)?
                .ok_or_else(setting_proposal_not_found_error)?;
            require_digest(&proposal, &confirmed)?;
            let binding = validate_agent_binding_for_proposal(tx, &proposal)?;

            match proposal.status() {
                SettingProposalStatus::Pending => {}
                SettingProposalStatus::Rejected => {
                    *result.lock().unwrap() = Some(Ok(()));
                    return Ok(());
                }
                status => return Err(setting_proposal_not_pending_error(status)),
            }

            if !tx.mark_rejected(id, &confirmed, UtcMillis::now())? {
                return Err(setting_proposal_revision_conflict_error());
            }
            if binding.is_some() {
                tx.update_agent_action_binding_state(
                    id,
                    AgentApprovalState::Pending,
                    AgentApprovalState::Rejected,
                )?;
            }
            *result.lock().unwrap() = Some(Ok(()));
            Ok(())
        })?;
        result.lock().unwrap().take().expect("拒绝事务必须写入结果")
    }

    /// 为 Agent pending 提案签发一次性审批令牌。
    ///
    /// 明文只通过返回值交给调用方一次；数据库和任何持久化事实只接触绑定了
    /// proposal id + digest 的 SHA-256 摘要。重复签发会覆盖旧摘要，使旧令牌立即失效。
    pub async fn issue_agent_approval_token(
        &self,
        id: SettingProposalId,
        confirmed_digest: &str,
    ) -> Result<AgentApprovalToken, AppError> {
        let confirmed = confirmed_digest.to_owned();
        let result = Arc::new(Mutex::new(None::<Result<AgentApprovalToken, AppError>>));
        self.uow.run(&|tx| {
            let proposal = tx
                .load_proposal(id)?
                .ok_or_else(setting_proposal_not_found_error)?;
            require_digest(&proposal, &confirmed)?;
            let binding = validate_agent_binding_for_proposal(tx, &proposal)?
                .ok_or_else(agent_approval_token_binding_required)?;

            match proposal.status() {
                SettingProposalStatus::Pending => {}
                status => return Err(setting_proposal_not_pending_error(status)),
            }

            let now = UtcMillis::now();
            if proposal.is_expired_at(now) {
                tx.mark_expired(id, &confirmed, now)?;
                tx.update_agent_action_binding_state(
                    id,
                    AgentApprovalState::Pending,
                    AgentApprovalState::Expired,
                )?;
                *result.lock().unwrap() = Some(Err(setting_proposal_expired_error()));
                return Ok(());
            }

            let token = AgentApprovalToken::issue();
            let token_hash = token.bind(id, &confirmed)?;
            // 领域对象再次确认状态、时间窗口和 hash/issued_at 的联合不变量；
            // 持久化端随后用同一 proposal digest + pending 条件做 CAS。
            binding.issue_approval_token(token_hash.clone(), now)?;
            if !tx.issue_agent_action_approval_token(id, &confirmed, &token_hash, now)? {
                return Err(agent_binding_integrity("签发审批令牌的条件更新未命中"));
            }
            *result.lock().unwrap() = Some(Ok(token));
            Ok(())
        })?;
        result
            .lock()
            .unwrap()
            .take()
            .expect("审批令牌签发事务必须写入结果")
    }

    /// 显式确认并应用提案，返回本次（或此前已存的）变更回执。
    ///
    /// 全部步骤在单一事务内：
    /// 1. 事务内按 id 重读提案（含完整性校验）；
    /// 2. `confirmed_digest` 必须等于存储 digest；
    /// 3. 状态：applied → 返回已存回执（幂等，不重复修改）；rejected/expired → 稳定错误；
    /// 4. pending 还要过时效校验（到期未确认 → 同一事务内把 pending 落为 expired，
    ///    再返回稳定 `SETTING_PROPOSAL_EXPIRED`；这是唯一一个"报错但留下状态迁移"
    ///    的分支，且它不写目标、不插回执）；
    /// 5. target/change 兼容性与 base_revision 校验；
    /// 6. 条件 CAS 写目标（Global 走分区默认值 + section 校验；Edition/MediaItem 走
    ///    空 `PreferenceData` 默认值，MediaItem 额外在事务内确认归属版本）；
    /// 7. 插入回执并把提案置为 applied。
    ///
    /// 除第 4 步外，其余步骤任一失败都会让整个事务回滚：目标零写入、无回执、状态不变。
    pub async fn apply_confirmed(
        &self,
        id: SettingProposalId,
        confirmed_digest: &str,
    ) -> Result<SettingChangeReceipt, AppError> {
        self.apply_confirmed_internal(id, confirmed_digest, None)
            .await
    }

    /// 显式确认并应用 Agent 提案：digest 和一次性审批令牌必须同时匹配。
    ///
    /// 令牌校验、条件消费、目标 CAS、回执插入以及 Proposal/Binding 状态迁移
    /// 都在同一 `BEGIN IMMEDIATE` UoW 内完成。消费过的令牌不会走普通 Proposal
    /// 的回执幂等回放路径。
    pub async fn apply_confirmed_with_approval_token(
        &self,
        id: SettingProposalId,
        confirmed_digest: &str,
        approval_token: &AgentApprovalToken,
    ) -> Result<SettingChangeReceipt, AppError> {
        self.apply_confirmed_internal(id, confirmed_digest, Some(approval_token))
            .await
    }

    /// 用户批准 Agent 设置提案的**唯一**写入口：整条链在**同一个** UoW 事务内完成。
    ///
    /// 这里不接收、也不返回明文 Approval Token。令牌只在事务闭包的短生命周期内生成，
    /// 摘要条件写入后执行目标 CAS，随后条件消费摘要；回执与两条生命周期状态也在同一
    /// 事务中提交。因此任何后续步骤失败，已经写入的 token 摘要、目标、回执和状态都会
    /// 一并回滚。
    ///
    /// `required_subject` 由上层的 typed 设置入口固定提供（当前为全局阅读设置），
    /// 事务内仍会与持久化绑定逐项比对，防止把内容范围绑定误用于设置批准。
    pub async fn approve_agent_proposal_in_one_uow(
        &self,
        id: SettingProposalId,
        confirmed_digest: &str,
        required_subject: AgentSettingsSubject,
    ) -> Result<SettingChangeReceipt, AppError> {
        let confirmed = confirmed_digest.to_owned();
        let cell = Arc::new(Mutex::new(None::<Result<SettingChangeReceipt, AppError>>));
        self.uow.run(&|tx| {
            let proposal = tx
                .load_proposal(id)?
                .ok_or_else(setting_proposal_not_found_error)?;
            require_digest(&proposal, &confirmed)?;
            let binding = validate_agent_binding_for_proposal(tx, &proposal)?
                .ok_or_else(agent_approval_token_binding_required)?;

            required_subject.validate()?;
            if binding.subject() != AgentScopeSubject::settings(required_subject)
                || !required_subject.covers(proposal.target())
            {
                return Err(agent_binding_subject_mismatch());
            }
            if proposal.status() != SettingProposalStatus::Pending {
                return Err(setting_proposal_not_pending_error(proposal.status()));
            }

            let now = UtcMillis::now();
            if proposal.is_expired_at(now) {
                // 过期是唯一允许以错误返回但提交状态迁移的分支；它不签发令牌，
                // 也不写目标/回执。
                tx.mark_expired(id, &confirmed, now)?;
                tx.update_agent_action_binding_state(
                    id,
                    AgentApprovalState::Pending,
                    AgentApprovalState::Expired,
                )?;
                *cell.lock().unwrap() = Some(Err(setting_proposal_expired_error()));
                return Ok(());
            }
            if !proposal.target().accepts(proposal.change()) {
                return Err(setting_proposal_invalid_target_error(
                    "目标与操作不匹配，拒绝执行",
                ));
            }
            // 设置范围没有 Work/Edition 归属，但仍通过事务端口保留统一的
            // subject 完整性检查；SQLite 实现对 settings subject 是显式 no-op。
            tx.verify_agent_subject(binding.subject())?;

            // 明文令牌只活在这几行：生成、绑定摘要，然后立即清除明文。
            // 后续（包括持久化端口）只接触摘要。
            let token = AgentApprovalToken::issue();
            let token_hash = token.bind(id, &confirmed)?;
            drop(token);
            // 领域对象再次确认 pending、时间窗口以及摘要/issued_at 联合不变量；
            // 真正的持久化由同一 UoW 中的条件写完成。
            binding.issue_approval_token(token_hash.clone(), now)?;
            if !tx.issue_agent_action_approval_token(id, &confirmed, &token_hash, now)? {
                return Err(agent_binding_integrity("签发审批令牌的条件更新未命中"));
            }

            let applied = apply_target(tx, &proposal, now)?;
            if !tx.consume_agent_action_approval_token(id, &confirmed, &token_hash, now)? {
                return Err(agent_approval_token_invalid("令牌已失效或已被消费"));
            }
            let receipt = SettingChangeReceipt {
                id: SettingChangeReceiptId::new(),
                proposal_id: proposal.id(),
                proposal_digest: proposal.digest().to_owned(),
                target: proposal.target(),
                before_canonical_json: applied.before_canonical_json,
                after_canonical_json: applied.after_canonical_json,
                applied_revision: applied.applied_revision,
                changed: applied.changed,
                provenance: proposal.provenance().clone(),
                applied_at: now,
            };
            receipt.validate()?;
            tx.insert_receipt(&receipt)?;
            if !tx.mark_applied(id, &confirmed, now)? {
                return Err(setting_proposal_revision_conflict_error());
            }
            tx.update_agent_action_binding_state(
                id,
                AgentApprovalState::Pending,
                AgentApprovalState::Applied,
            )?;

            *cell.lock().unwrap() = Some(Ok(receipt));
            Ok(())
        })?;
        cell.lock()
            .unwrap()
            .take()
            .expect("Agent 批准事务必须写入结果")
    }

    async fn apply_confirmed_internal(
        &self,
        id: SettingProposalId,
        confirmed_digest: &str,
        approval_token: Option<&AgentApprovalToken>,
    ) -> Result<SettingChangeReceipt, AppError> {
        let confirmed = confirmed_digest.to_owned();
        let cell = Arc::new(Mutex::new(None::<Result<SettingChangeReceipt, AppError>>));
        self.uow.run(&|tx| {
            let proposal = tx
                .load_proposal(id)?
                .ok_or_else(setting_proposal_not_found_error)?;
            require_digest(&proposal, &confirmed)?;
            let binding = validate_agent_binding_for_proposal(tx, &proposal)?;

            match (binding.as_ref(), approval_token) {
                (Some(_), None) => return Err(agent_approval_token_required()),
                (None, Some(_)) => return Err(agent_approval_token_not_allowed()),
                (Some(binding), Some(token)) => {
                    // 终态不走普通回执幂等回放；已消费/已清除的令牌必须 fail-closed。
                    if proposal.status() != SettingProposalStatus::Pending {
                        return Err(agent_approval_token_invalid("令牌只能用于 pending 提案"));
                    }
                    binding.verify_approval_token(id, &confirmed, token)?;
                }
                (None, None) => {}
            }

            if approval_token.is_none() {
                match proposal.status() {
                    SettingProposalStatus::Applied => {
                        let receipt = tx.load_receipt(id)?.ok_or_else(|| {
                            setting_proposal_integrity_error("已应用的提案缺少变更回执")
                        })?;
                        *cell.lock().unwrap() = Some(Ok(receipt));
                        return Ok(());
                    }
                    SettingProposalStatus::Rejected => {
                        return Err(setting_proposal_rejected_error());
                    }
                    SettingProposalStatus::Expired => {
                        return Err(setting_proposal_expired_error());
                    }
                    SettingProposalStatus::Pending => {}
                }
            } else if proposal.status() != SettingProposalStatus::Pending {
                // Agent 分支已在上面给出不带 token 的专用 fail-closed 语义；这里
                // 只是让编译器和未来新增状态保持显式闭合。
                return Err(agent_approval_token_invalid("令牌只能用于 pending 提案"));
            }

            let now = UtcMillis::now();
            if proposal.is_expired_at(now) {
                // 到期未确认：在同一事务里把 pending **原子地**转为 expired（终态），
                // 但不写目标、不插回执。
                //
                // 错误经 cell 回传、闭包返回 `Ok` 让事务照常提交：闭包返回 Err 会
                // 回滚整个事务，状态迁移就白做了。提交后库里只多出"提案已过期"
                // 这一条事实，调用方仍然收到稳定的 `SETTING_PROPOSAL_EXPIRED`。
                tx.mark_expired(id, &confirmed, now)?;
                if binding.is_some() {
                    tx.update_agent_action_binding_state(
                        id,
                        AgentApprovalState::Pending,
                        AgentApprovalState::Expired,
                    )?;
                }
                *cell.lock().unwrap() = Some(Err(setting_proposal_expired_error()));
                return Ok(());
            }
            if let Some(binding) = &binding {
                tx.verify_agent_subject(binding.subject())?;
            }
            if !proposal.target().accepts(proposal.change()) {
                return Err(setting_proposal_invalid_target_error(
                    "目标与操作不匹配，拒绝执行",
                ));
            }

            let applied = apply_target(tx, &proposal, now)?;
            if let Some(token) = approval_token {
                let token_hash = token.bind(id, &confirmed)?;
                if !tx.consume_agent_action_approval_token(id, &confirmed, &token_hash, now)? {
                    return Err(agent_approval_token_invalid("令牌已失效或已被消费"));
                }
            }
            let receipt = SettingChangeReceipt {
                id: SettingChangeReceiptId::new(),
                proposal_id: proposal.id(),
                proposal_digest: proposal.digest().to_owned(),
                target: proposal.target(),
                before_canonical_json: applied.before_canonical_json,
                after_canonical_json: applied.after_canonical_json,
                applied_revision: applied.applied_revision,
                changed: applied.changed,
                provenance: proposal.provenance().clone(),
                applied_at: now,
            };
            // 回执是审计事实，落库前必须自身自洽（长度/provenance/canonical）。
            receipt.validate()?;
            tx.insert_receipt(&receipt)?;
            if !tx.mark_applied(id, &confirmed, now)? {
                return Err(setting_proposal_revision_conflict_error());
            }
            if binding.is_some() {
                tx.update_agent_action_binding_state(
                    id,
                    AgentApprovalState::Pending,
                    AgentApprovalState::Applied,
                )?;
            }

            *cell.lock().unwrap() = Some(Ok(receipt));
            Ok(())
        })?;
        cell.lock().unwrap().take().expect("闭包必然写入结果")
    }
}

fn provenance_requires_agent_binding(provenance: &SettingProvenance) -> bool {
    provenance.source_kind == ProvenanceSourceKind::Agent
        || provenance.actor_kind == ProvenanceActorKind::Agent
        || provenance.reason == ProvenanceReason::AgentSuggestion
}

fn agent_binding_missing() -> AppError {
    AppError::new(
        "AGENT_ACTION_BINDING_MISSING",
        ErrorKind::Internal,
        "Agent 提案缺少动作绑定，拒绝继续执行",
        false,
    )
}

fn agent_binding_integrity(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_ACTION_BINDING_INTEGRITY",
        ErrorKind::Internal,
        format!("Agent 动作绑定完整性校验失败：{detail}"),
        false,
    )
}

fn agent_binding_provenance_mismatch() -> AppError {
    AppError::new(
        "AGENT_ACTION_BINDING_PROVENANCE_MISMATCH",
        ErrorKind::Validation,
        "Agent 动作绑定只能用于 Agent/Agent/AgentSuggestion 提案",
        false,
    )
}

fn agent_binding_subject_mismatch() -> AppError {
    AppError::new(
        "AGENT_ACTION_SUBJECT_MISMATCH",
        ErrorKind::Forbidden,
        "Agent 上下文范围不覆盖该设置目标，已拒绝",
        false,
    )
}

fn agent_binding_required() -> AppError {
    AppError::new(
        "AGENT_ACTION_BINDING_REQUIRED",
        ErrorKind::Validation,
        "Agent 来源的设置提案必须通过带动作绑定的创建路径",
        false,
    )
}

fn agent_approval_token_binding_required() -> AppError {
    AppError::new(
        "AGENT_APPROVAL_TOKEN_UNAVAILABLE",
        ErrorKind::Validation,
        "只有带 Agent 动作绑定的提案可以签发审批令牌",
        false,
    )
}

fn agent_approval_token_required() -> AppError {
    AppError::new(
        "AGENT_APPROVAL_TOKEN_REQUIRED",
        ErrorKind::Forbidden,
        "Agent 提案必须携带一次性审批令牌才能应用",
        false,
    )
}

fn agent_approval_token_not_allowed() -> AppError {
    AppError::new(
        "AGENT_APPROVAL_TOKEN_NOT_ALLOWED",
        ErrorKind::Validation,
        "普通用户提案不能携带 Agent 审批令牌",
        false,
    )
}

fn agent_approval_token_invalid(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_APPROVAL_TOKEN_INVALID",
        ErrorKind::Validation,
        format!("Agent 审批令牌非法：{detail}"),
        false,
    )
}

/// 一次成功应用后目标侧的权威状态。
struct AppliedTarget {
    before_canonical_json: String,
    after_canonical_json: String,
    applied_revision: Option<String>,
    changed: bool,
}

fn apply_target(
    tx: &dyn SettingProposalTxPorts,
    proposal: &SettingProposal,
    now: UtcMillis,
) -> Result<AppliedTarget, AppError> {
    match (proposal.target(), proposal.change()) {
        (SettingTarget::Global(target), SettingProposalChange::SettingsPatch(patch)) => {
            // target.section 与 patch.section 的一致性已由领域 `accepts` 保证；
            // 这里再挡一次，避免将来有人放宽领域规则后静默把 patch 写进别的分区。
            if patch.section() != target.section {
                return Err(setting_proposal_invalid_target_error("分区与 patch 不一致"));
            }
            apply_global(tx, target.section, patch, proposal.base_revision(), now)
        }
        (SettingTarget::Edition(target), SettingProposalChange::ResourcePreference(data)) => {
            if !tx.edition_exists(target.edition_id)? {
                return Err(setting_proposal_target_not_found_error("版本不存在"));
            }
            apply_edition_preference(tx, target.edition_id, data, proposal.base_revision(), now)
        }
        (SettingTarget::MediaItem(target), SettingProposalChange::ResourcePreference(data)) => {
            if !tx.edition_exists(target.edition_id)? {
                return Err(setting_proposal_target_not_found_error("版本不存在"));
            }
            match tx.media_item_edition(target.media_item_id)? {
                None => {
                    return Err(setting_proposal_target_not_found_error("媒体条目不存在"));
                }
                Some(edition_id) if edition_id != target.edition_id => {
                    return Err(setting_proposal_target_not_found_error(
                        "媒体条目不属于提案指定的版本",
                    ));
                }
                Some(_) => {}
            }
            apply_media_item_preference(
                tx,
                target.edition_id,
                target.media_item_id,
                data,
                proposal.base_revision(),
                now,
            )
        }
        _ => Err(setting_proposal_invalid_target_error("目标与操作不匹配")),
    }
}

fn apply_global(
    tx: &dyn SettingProposalTxPorts,
    section: SettingsSection,
    patch: &SettingsPatch,
    base_revision: Option<&str>,
    now: UtcMillis,
) -> Result<AppliedTarget, AppError> {
    let row = tx.load_settings(section.as_str())?;
    let (current, current_revision) = match &row {
        Some(row) => (
            decode_settings_value(section, &row.data_json)?,
            Some(row.revision.clone()),
        ),
        // 从未保存的分区以**领域默认值**为基线，而不是"空对象"：
        // 否则第一次应用一个字段就会把其余字段写成缺失。
        None => (SettingsValue::default_for(section), None),
    };
    require_base_revision(current_revision.as_deref(), base_revision)?;

    let next = patch.apply_to(&current);
    let before_canonical_json = canonical_json_of(&current)?;
    let after_canonical_json = canonical_json_of(&next)?;
    if next == current {
        // 幂等：值没有变化就不制造新版本，也不写库。
        return Ok(AppliedTarget {
            before_canonical_json,
            after_canonical_json,
            applied_revision: current_revision,
            changed: false,
        });
    }

    let revision = new_revision("set");
    let data_json = serde_json::to_string(&next).map_err(|e| {
        AppError::new(
            "INTERNAL_ERROR",
            ErrorKind::Internal,
            "设置序列化失败",
            false,
        )
        .with_source(e)
    })?;
    let written = tx.cas_write_settings(
        section.as_str(),
        current_revision.as_deref(),
        &SettingsRow {
            section: section.as_str().to_owned(),
            schema_version: 1,
            revision: revision.clone(),
            data_json,
            updated_at: now,
        },
    )?;
    if !written {
        return Err(setting_proposal_revision_conflict_error());
    }
    Ok(AppliedTarget {
        before_canonical_json,
        after_canonical_json,
        applied_revision: Some(revision),
        changed: true,
    })
}

fn apply_edition_preference(
    tx: &dyn SettingProposalTxPorts,
    edition_id: EditionId,
    data: &PreferenceData,
    base_revision: Option<&str>,
    now: UtcMillis,
) -> Result<AppliedTarget, AppError> {
    let current = tx.load_edition_preference(edition_id)?;
    let (current_data, current_revision) = match &current {
        Some(preference) => (preference.data.clone(), Some(preference.revision.clone())),
        None => (PreferenceData::default(), None),
    };
    require_base_revision(current_revision.as_deref(), base_revision)?;

    let before_canonical_json = canonical_json_of(&current_data)?;
    let after_canonical_json = canonical_json_of(data)?;
    if &current_data == data {
        return Ok(AppliedTarget {
            before_canonical_json,
            after_canonical_json,
            applied_revision: current_revision,
            changed: false,
        });
    }

    let revision = new_revision("pref-edition");
    let written = tx.cas_upsert_edition(
        &EditionPreference {
            edition_id,
            data: data.clone(),
            revision: revision.clone(),
            updated_at: now,
        },
        current_revision.as_deref(),
    )?;
    if !written {
        return Err(setting_proposal_revision_conflict_error());
    }
    Ok(AppliedTarget {
        before_canonical_json,
        after_canonical_json,
        applied_revision: Some(revision),
        changed: true,
    })
}

fn apply_media_item_preference(
    tx: &dyn SettingProposalTxPorts,
    edition_id: EditionId,
    media_item_id: MediaItemId,
    data: &PreferenceData,
    base_revision: Option<&str>,
    now: UtcMillis,
) -> Result<AppliedTarget, AppError> {
    let current = tx.load_media_item_preference(media_item_id)?;
    let (current_data, current_revision) = match &current {
        Some(preference) => {
            // 已有偏好行必须属于提案指定的版本。归属在事务内已经与
            // `media_items.edition_id` 交叉校验过，这里再出现不一致，只可能是偏好行
            // 自己损坏（或被人为改写）；顺着 upsert 把它改写成目标版本会把一条错行
            // 静默"洗白"，因此返回稳定的完整性错误，交给修复路径处理。
            if preference.edition_id != edition_id {
                return Err(setting_proposal_integrity_error(
                    "媒体资源内设归属版本与提案目标不一致",
                ));
            }
            (preference.data.clone(), Some(preference.revision.clone()))
        }
        None => (PreferenceData::default(), None),
    };
    require_base_revision(current_revision.as_deref(), base_revision)?;

    let before_canonical_json = canonical_json_of(&current_data)?;
    let after_canonical_json = canonical_json_of(data)?;
    if &current_data == data {
        return Ok(AppliedTarget {
            before_canonical_json,
            after_canonical_json,
            applied_revision: current_revision,
            changed: false,
        });
    }

    let revision = new_revision("pref-media");
    let written = tx.cas_upsert_media_item(
        &MediaItemPreference {
            media_item_id,
            // 归属已在本事务内与 media_items.edition_id 交叉校验过。
            edition_id,
            data: data.clone(),
            revision: revision.clone(),
            updated_at: now,
        },
        current_revision.as_deref(),
    )?;
    if !written {
        return Err(setting_proposal_revision_conflict_error());
    }
    Ok(AppliedTarget {
        before_canonical_json,
        after_canonical_json,
        applied_revision: Some(revision),
        changed: true,
    })
}

/// 调用方确认的 digest 必须与存储 digest 逐字节一致。
fn require_digest(proposal: &SettingProposal, confirmed_digest: &str) -> Result<(), AppError> {
    if proposal.digest() != confirmed_digest {
        return Err(setting_proposal_digest_mismatch_error());
    }
    Ok(())
}

/// base_revision 必须与事务内读到的 authoritative revision 一致。
///
/// 与后面的 SQL 条件写是**两层**保护：这里给出明确语义（目标是否被并发修改），
/// SQL 条件写兜住"读之后、写之前"的竞争窗口。
fn require_base_revision(current: Option<&str>, expected: Option<&str>) -> Result<(), AppError> {
    let matches = match (current, expected) {
        (None, None) => true,
        (Some(current), Some(expected)) => current == expected,
        _ => false,
    };
    if matches {
        Ok(())
    } else {
        Err(setting_proposal_revision_conflict_error())
    }
}

fn decode_settings_value(
    section: SettingsSection,
    data_json: &str,
) -> Result<SettingsValue, AppError> {
    let value: SettingsValue = serde_json::from_str(data_json).map_err(|e| {
        AppError::new("INTERNAL_ERROR", ErrorKind::Internal, "设置数据损坏", false).with_source(e)
    })?;
    if value.section() != section {
        return Err(AppError::new(
            "INTERNAL_ERROR",
            ErrorKind::Internal,
            "设置分区与存储不一致",
            false,
        ));
    }
    Ok(value)
}

/// 新的 authoritative revision：安全不透明 token（只含 `[A-Za-z0-9_-]`）。
///
/// 直接复用领域 ID 生成器（UUID v7，时间有序）作为 token 源，避免再引入一套
/// 自增/时间戳拼接的编号规则；前缀只用于人工排查时区分作用域。
fn new_revision(prefix: &str) -> String {
    format!("{prefix}-{}", SettingChangeReceiptId::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::setting_proposal::ProvenanceReason;
    use haven_domain::settings::{ComicPatch, ReadingFontSize, ReadingPatch};
    use std::collections::{HashMap, HashSet};

    // ---------- 内存假实现（无 SQL）：只验证编排规则 ----------

    #[derive(Clone, Default)]
    struct FakeState {
        proposals: HashMap<SettingProposalId, SettingProposal>,
        bindings: HashMap<SettingProposalId, AgentActionBinding>,
        receipts: HashMap<SettingProposalId, SettingChangeReceipt>,
        settings: HashMap<String, SettingsRow>,
        edition_preferences: HashMap<EditionId, EditionPreference>,
        media_item_preferences: HashMap<MediaItemId, MediaItemPreference>,
        editions: HashSet<EditionId>,
        media_items: HashMap<MediaItemId, EditionId>,
        /// 目标写入次数：证明"零写入"分支确实没碰目标。
        target_writes: usize,
    }

    struct FakeStore {
        state: Mutex<FakeState>,
    }

    impl FakeStore {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                state: Mutex::new(FakeState::default()),
            })
        }

        fn target_writes(&self) -> usize {
            self.state.lock().unwrap().target_writes
        }

        fn receipt_count(&self) -> usize {
            self.state.lock().unwrap().receipts.len()
        }

        fn status(&self, id: SettingProposalId) -> SettingProposalStatus {
            self.state
                .lock()
                .unwrap()
                .proposals
                .get(&id)
                .unwrap()
                .status()
        }
    }

    #[async_trait::async_trait]
    impl SettingProposalRepository for FakeStore {
        async fn create(&self, proposal: &SettingProposal) -> Result<(), AppError> {
            let mut state = self.state.lock().unwrap();
            if state.proposals.contains_key(&proposal.id()) {
                return Err(AppError::new(
                    "SETTING_PROPOSAL_ALREADY_EXISTS",
                    ErrorKind::AlreadyExists,
                    "提案 ID 已存在",
                    false,
                ));
            }
            state.proposals.insert(proposal.id(), proposal.clone());
            Ok(())
        }

        async fn get(&self, id: SettingProposalId) -> Result<Option<SettingProposal>, AppError> {
            Ok(self.state.lock().unwrap().proposals.get(&id).cloned())
        }

        async fn get_receipt(
            &self,
            id: SettingProposalId,
        ) -> Result<Option<SettingChangeReceipt>, AppError> {
            let state = self.state.lock().unwrap();
            let receipt = state.receipts.get(&id).cloned();
            match receipt {
                // 与 SQLite 实现同语义：读取端不把无法自洽的回执交给调用方。
                Some(receipt) => {
                    receipt.validate()?;
                    Ok(Some(receipt))
                }
                // `None` 只能表示"确实没有应用过"。自称 applied 却没有回执的行是
                // 存储被改写，必须和 `apply_confirmed` 报同一个完整性错误。
                None if state.proposals.get(&id).map(|proposal| proposal.status())
                    == Some(SettingProposalStatus::Applied) =>
                {
                    Err(setting_proposal_integrity_error("已应用的提案缺少变更回执"))
                }
                None => Ok(None),
            }
        }
    }

    impl SettingProposalUoW for FakeStore {
        fn run(
            &self,
            f: &dyn Fn(&dyn SettingProposalTxPorts) -> Result<(), AppError>,
        ) -> Result<(), AppError> {
            // SQLite 的真实 UoW 在闭包返回 Err 时会回滚；内存 double 也必须保留
            // 这个语义，否则 token/CAS/receipt 的原子性测试会被假实现掩盖。
            let snapshot = self.state.lock().unwrap().clone();
            let scope = FakeTx { state: &self.state };
            let result = f(&scope);
            if result.is_err() {
                *self.state.lock().unwrap() = snapshot;
            }
            result
        }
    }

    struct FakeTx<'a> {
        state: &'a Mutex<FakeState>,
    }

    impl SettingProposalTxPorts for FakeTx<'_> {
        fn insert_proposal(&self, proposal: &SettingProposal) -> Result<(), AppError> {
            let mut state = self.state.lock().unwrap();
            if state.proposals.contains_key(&proposal.id()) {
                return Err(AppError::new(
                    "SETTING_PROPOSAL_ALREADY_EXISTS",
                    ErrorKind::AlreadyExists,
                    "提案 ID 已存在",
                    false,
                ));
            }
            state.proposals.insert(proposal.id(), proposal.clone());
            Ok(())
        }

        fn insert_agent_action_binding(
            &self,
            binding: &AgentActionBinding,
        ) -> Result<(), AppError> {
            let mut state = self.state.lock().unwrap();
            if state.bindings.contains_key(&binding.proposal_id()) {
                return Err(agent_binding_integrity("测试绑定 ID 重复"));
            }
            state
                .bindings
                .insert(binding.proposal_id(), binding.clone());
            Ok(())
        }

        fn load_agent_action_binding(
            &self,
            proposal_id: SettingProposalId,
        ) -> Result<Option<AgentActionBinding>, AppError> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .bindings
                .get(&proposal_id)
                .cloned())
        }

        fn verify_settings_base_revision(
            &self,
            _section: &str,
            _expected_revision: Option<&str>,
        ) -> Result<bool, AppError> {
            Ok(true)
        }

        fn verify_agent_subject(&self, _subject: AgentScopeSubject) -> Result<(), AppError> {
            // Agent service 的 scope double 覆盖 Application 外层复核；这里的内存
            // UoW 只验证提案/绑定状态编排，SQLite 集成测试覆盖真实层级查询。
            Ok(())
        }

        fn mark_rejected(
            &self,
            id: SettingProposalId,
            expected_digest: &str,
            _rejected_at: UtcMillis,
        ) -> Result<bool, AppError> {
            let mut state = self.state.lock().unwrap();
            let Some(proposal) = state.proposals.get(&id) else {
                return Ok(false);
            };
            if proposal.status() != SettingProposalStatus::Pending
                || proposal.digest() != expected_digest
            {
                return Ok(false);
            }
            let rejected = SettingProposal::from_stored(
                id,
                proposal.canonical_json().to_owned(),
                proposal.digest().to_owned(),
                SettingProposalStatus::Rejected,
                proposal.created_at(),
            )?;
            state.proposals.insert(id, rejected);
            Ok(true)
        }

        fn update_agent_action_binding_state(
            &self,
            proposal_id: SettingProposalId,
            expected: AgentApprovalState,
            next: AgentApprovalState,
        ) -> Result<(), AppError> {
            let mut state = self.state.lock().unwrap();
            let Some(binding) = state.bindings.get(&proposal_id).cloned() else {
                return Ok(());
            };
            if binding.approval_state() != expected {
                return Err(agent_binding_integrity("测试绑定状态不匹配"));
            }
            state
                .bindings
                .insert(proposal_id, binding.with_approval_state(next)?);
            Ok(())
        }

        fn load_proposal(
            &self,
            id: SettingProposalId,
        ) -> Result<Option<SettingProposal>, AppError> {
            Ok(self.state.lock().unwrap().proposals.get(&id).cloned())
        }

        fn load_receipt(
            &self,
            id: SettingProposalId,
        ) -> Result<Option<SettingChangeReceipt>, AppError> {
            let receipt = self.state.lock().unwrap().receipts.get(&id).cloned();
            // 与 SQLite 实现同语义：读取端不把无法自洽的回执交给调用方。
            if let Some(receipt) = &receipt {
                receipt.validate()?;
            }
            Ok(receipt)
        }

        fn edition_exists(&self, edition_id: EditionId) -> Result<bool, AppError> {
            Ok(self.state.lock().unwrap().editions.contains(&edition_id))
        }

        fn media_item_edition(
            &self,
            media_item_id: MediaItemId,
        ) -> Result<Option<EditionId>, AppError> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .media_items
                .get(&media_item_id)
                .copied())
        }

        fn load_settings(&self, section: &str) -> Result<Option<SettingsRow>, AppError> {
            Ok(self.state.lock().unwrap().settings.get(section).cloned())
        }

        fn cas_write_settings(
            &self,
            section: &str,
            expected_revision: Option<&str>,
            row: &SettingsRow,
        ) -> Result<bool, AppError> {
            let mut state = self.state.lock().unwrap();
            let current = state.settings.get(section).map(|row| row.revision.clone());
            if !revision_matches(current.as_deref(), expected_revision) {
                return Ok(false);
            }
            state.settings.insert(section.to_owned(), row.clone());
            state.target_writes += 1;
            Ok(true)
        }

        fn load_edition_preference(
            &self,
            edition_id: EditionId,
        ) -> Result<Option<EditionPreference>, AppError> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .edition_preferences
                .get(&edition_id)
                .cloned())
        }

        fn load_media_item_preference(
            &self,
            media_item_id: MediaItemId,
        ) -> Result<Option<MediaItemPreference>, AppError> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .media_item_preferences
                .get(&media_item_id)
                .cloned())
        }

        fn cas_upsert_edition(
            &self,
            preference: &EditionPreference,
            expected_revision: Option<&str>,
        ) -> Result<bool, AppError> {
            let mut state = self.state.lock().unwrap();
            let current = state
                .edition_preferences
                .get(&preference.edition_id)
                .map(|row| row.revision.clone());
            if !revision_matches(current.as_deref(), expected_revision) {
                return Ok(false);
            }
            state
                .edition_preferences
                .insert(preference.edition_id, preference.clone());
            state.target_writes += 1;
            Ok(true)
        }

        fn cas_upsert_media_item(
            &self,
            preference: &MediaItemPreference,
            expected_revision: Option<&str>,
        ) -> Result<bool, AppError> {
            let mut state = self.state.lock().unwrap();
            let current = state
                .media_item_preferences
                .get(&preference.media_item_id)
                .map(|row| row.revision.clone());
            if !revision_matches(current.as_deref(), expected_revision) {
                return Ok(false);
            }
            state
                .media_item_preferences
                .insert(preference.media_item_id, preference.clone());
            state.target_writes += 1;
            Ok(true)
        }

        fn insert_receipt(&self, receipt: &SettingChangeReceipt) -> Result<(), AppError> {
            self.state
                .lock()
                .unwrap()
                .receipts
                .insert(receipt.proposal_id, receipt.clone());
            Ok(())
        }

        fn mark_applied(
            &self,
            id: SettingProposalId,
            expected_digest: &str,
            _applied_at: UtcMillis,
        ) -> Result<bool, AppError> {
            let mut state = self.state.lock().unwrap();
            let Some(proposal) = state.proposals.get(&id) else {
                return Ok(false);
            };
            if proposal.status() != SettingProposalStatus::Pending
                || proposal.digest() != expected_digest
            {
                return Ok(false);
            }
            let applied = SettingProposal::from_stored(
                id,
                proposal.canonical_json().to_owned(),
                proposal.digest().to_owned(),
                SettingProposalStatus::Applied,
                proposal.created_at(),
            )?;
            state.proposals.insert(id, applied);
            Ok(true)
        }

        fn mark_expired(
            &self,
            id: SettingProposalId,
            expected_digest: &str,
            _expired_at: UtcMillis,
        ) -> Result<(), AppError> {
            let mut state = self.state.lock().unwrap();
            let Some(proposal) = state.proposals.get(&id) else {
                return Ok(());
            };
            // 与 SQL 条件写同语义：只有仍是 pending 且 digest 一致时才迁移。
            if proposal.status() != SettingProposalStatus::Pending
                || proposal.digest() != expected_digest
            {
                return Ok(());
            }
            let expired = SettingProposal::from_stored(
                id,
                proposal.canonical_json().to_owned(),
                proposal.digest().to_owned(),
                SettingProposalStatus::Expired,
                proposal.created_at(),
            )?;
            state.proposals.insert(id, expired);
            Ok(())
        }

        fn issue_agent_action_approval_token(
            &self,
            id: SettingProposalId,
            expected_digest: &str,
            approval_token_hash: &AgentApprovalTokenHash,
            issued_at: UtcMillis,
        ) -> Result<bool, AppError> {
            let mut state = self.state.lock().unwrap();
            let Some(proposal) = state.proposals.get(&id) else {
                return Ok(false);
            };
            if proposal.status() != SettingProposalStatus::Pending
                || proposal.digest() != expected_digest
            {
                return Ok(false);
            }
            let Some(binding) = state.bindings.get(&id).cloned() else {
                return Ok(false);
            };
            let next = binding.issue_approval_token(approval_token_hash.clone(), issued_at)?;
            state.bindings.insert(id, next);
            Ok(true)
        }

        fn consume_agent_action_approval_token(
            &self,
            id: SettingProposalId,
            expected_digest: &str,
            approval_token_hash: &AgentApprovalTokenHash,
            _consumed_at: UtcMillis,
        ) -> Result<bool, AppError> {
            let mut state = self.state.lock().unwrap();
            let Some(proposal) = state.proposals.get(&id) else {
                return Ok(false);
            };
            if proposal.status() != SettingProposalStatus::Pending
                || proposal.digest() != expected_digest
            {
                return Ok(false);
            }
            let Some(binding) = state.bindings.get(&id).cloned() else {
                return Ok(false);
            };
            if binding.approval_token_hash() != Some(approval_token_hash) {
                return Ok(false);
            }
            let next = binding.with_approval_state(AgentApprovalState::Pending)?;
            // `with_approval_state(Pending)` 保持 token；消费是存储层的原子清除，
            // 用 from_stored 路径构造同一条 pending 事实而不保留摘要。
            let cleared = AgentActionBinding::from_stored(
                next.proposal_id(),
                next.session_id(),
                next.request_id(),
                next.context_snapshot_id(),
                next.context_hash().to_owned(),
                next.subject(),
                next.action_kind(),
                next.created_at(),
                next.expires_at(),
                next.approval_state(),
            )?;
            state.bindings.insert(id, cleared);
            Ok(true)
        }
    }

    fn revision_matches(current: Option<&str>, expected: Option<&str>) -> bool {
        match (current, expected) {
            (None, None) => true,
            (Some(current), Some(expected)) => current == expected,
            _ => false,
        }
    }

    fn service(store: &Arc<FakeStore>) -> SettingProposalService {
        SettingProposalService::new(store.clone(), store.clone())
    }

    fn reading_patch(font_size: ReadingFontSize) -> SettingsPatch {
        SettingsPatch::Reading(ReadingPatch {
            font_size: Some(font_size),
            ..ReadingPatch::default()
        })
    }

    async fn global_proposal(
        store: &Arc<FakeStore>,
        font_size: ReadingFontSize,
    ) -> SettingProposal {
        service(store)
            .create(SettingProposalRequest::new(
                SettingTarget::global(SettingsSection::Reading),
                SettingProposalChange::SettingsPatch(reading_patch(font_size)),
                SettingProvenance::user(ProvenanceReason::UserRequest),
            ))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn generic_create_rejects_agent_provenance_without_a_binding() {
        let store = FakeStore::new();
        let error = service(&store)
            .create(SettingProposalRequest::new(
                SettingTarget::global(SettingsSection::Reading),
                SettingProposalChange::SettingsPatch(reading_patch(ReadingFontSize::Large)),
                SettingProvenance::agent(ProvenanceReason::AgentSuggestion),
            ))
            .await
            .unwrap_err();

        assert_eq!(error.code().as_str(), "AGENT_ACTION_BINDING_REQUIRED");
        assert!(store.state.lock().unwrap().proposals.is_empty());
    }

    #[tokio::test]
    async fn create_get_and_reject_go_through_the_repository() {
        let store = FakeStore::new();
        let svc = service(&store);
        let proposal = global_proposal(&store, ReadingFontSize::Large).await;
        assert_eq!(proposal.status(), SettingProposalStatus::Pending);
        assert_eq!(svc.get(proposal.id()).await.unwrap().unwrap(), proposal);
        assert!(svc.get_receipt(proposal.id()).await.unwrap().is_none());

        svc.reject(proposal.id(), proposal.digest()).await.unwrap();
        assert_eq!(store.status(proposal.id()), SettingProposalStatus::Rejected);
        // 重复拒绝是幂等的，且不会改成别的状态。
        svc.reject(proposal.id(), proposal.digest()).await.unwrap();
        assert_eq!(store.status(proposal.id()), SettingProposalStatus::Rejected);
    }

    #[tokio::test]
    async fn reject_requires_the_displayed_digest() {
        let store = FakeStore::new();
        let svc = service(&store);
        let proposal = global_proposal(&store, ReadingFontSize::Large).await;
        let error = svc.reject(proposal.id(), "deadbeef").await.unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_DIGEST_MISMATCH");
        assert_eq!(store.status(proposal.id()), SettingProposalStatus::Pending);
    }

    #[tokio::test]
    async fn global_apply_records_before_after_and_applied_status() {
        let store = FakeStore::new();
        let svc = service(&store);
        let proposal = global_proposal(&store, ReadingFontSize::Large).await;

        let receipt = svc
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap();
        assert_eq!(store.status(proposal.id()), SettingProposalStatus::Applied);
        assert!(receipt.changed);
        assert!(receipt.applied_revision.is_some());
        assert_eq!(receipt.proposal_id, proposal.id());
        assert_eq!(receipt.proposal_digest, proposal.digest());
        assert_eq!(receipt.target, proposal.target());
        // before 是分区默认值，after 是应用后的完整值。
        let before: SettingsValue = serde_json::from_str(&receipt.before_canonical_json).unwrap();
        let after: SettingsValue = serde_json::from_str(&receipt.after_canonical_json).unwrap();
        assert_eq!(before, SettingsValue::default_for(SettingsSection::Reading));
        match after {
            SettingsValue::Reading(value) => {
                assert_eq!(value.font_size, ReadingFontSize::Large);
                assert_eq!(value.font_family, before_section(&before).font_family);
            }
            _ => panic!("分区必须是 reading"),
        }
        assert_eq!(store.target_writes(), 1);
        assert_eq!(store.receipt_count(), 1);

        // 重复 apply：返回同一张已存回执，且不再写目标。
        let again = svc
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap();
        assert_eq!(again.id, receipt.id);
        assert_eq!(again.applied_revision, receipt.applied_revision);
        assert_eq!(store.target_writes(), 1);
        assert_eq!(store.receipt_count(), 1);
    }

    fn before_section(value: &SettingsValue) -> haven_domain::settings::ReadingSettings {
        match value {
            SettingsValue::Reading(settings) => settings.clone(),
            _ => panic!("分区必须是 reading"),
        }
    }

    #[tokio::test]
    async fn digest_mismatch_expiry_and_rejection_write_nothing() {
        let store = FakeStore::new();
        let svc = service(&store);

        // digest 不一致（用户确认的不是这一份内容）。
        let proposal = global_proposal(&store, ReadingFontSize::Large).await;
        let error = svc
            .apply_confirmed(proposal.id(), &"0".repeat(64))
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_DIGEST_MISMATCH");
        assert_eq!(store.target_writes(), 0);
        assert_eq!(store.receipt_count(), 0);
        assert_eq!(store.status(proposal.id()), SettingProposalStatus::Pending);

        // 已拒绝。
        svc.reject(proposal.id(), proposal.digest()).await.unwrap();
        let error = svc
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_REJECTED");
        assert_eq!(store.target_writes(), 0);
        assert_eq!(store.receipt_count(), 0);

        // 已过期（TTL 为负 → 领域拒绝构造，这里直接构造一个到期提案）。
        let expired = SettingProposal::new(
            SettingProposalId::new(),
            SettingTarget::global(SettingsSection::Reading),
            SettingProposalChange::SettingsPatch(reading_patch(ReadingFontSize::Small)),
            None,
            SettingProvenance::user(ProvenanceReason::UserRequest),
            UtcMillis(1),
            UtcMillis(2),
        )
        .unwrap();
        store
            .state
            .lock()
            .unwrap()
            .proposals
            .insert(expired.id(), expired.clone());
        let error = svc
            .apply_confirmed(expired.id(), expired.digest())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_EXPIRED");
        assert_eq!(store.target_writes(), 0);
        assert_eq!(store.receipt_count(), 0);
        // 过期不是"什么都不发生"：pending 必须在同一次调用里落为终态 expired，
        // 否则每读一次就重算一次过期，状态永远停在 pending。
        assert_eq!(store.status(expired.id()), SettingProposalStatus::Expired);

        // 再次确认：已是终态，仍然是同一个稳定错误，且不再有任何写入。
        let error = svc
            .apply_confirmed(expired.id(), expired.digest())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_EXPIRED");
        assert_eq!(store.target_writes(), 0);
        assert_eq!(store.receipt_count(), 0);
        assert_eq!(store.status(expired.id()), SettingProposalStatus::Expired);

        // 过期是**已提交的终态事实**，别的入口看到的是同一个状态：拒绝路径必须
        // 回报"已不是 pending"，而不是把它当成一条还能拒绝的 pending 提案。
        let error = svc
            .reject(expired.id(), expired.digest())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_NOT_PENDING");
        assert_eq!(store.status(expired.id()), SettingProposalStatus::Expired);
    }

    #[tokio::test]
    async fn expiry_needs_the_displayed_digest_to_change_state() {
        let store = FakeStore::new();
        let svc = service(&store);
        let expired = SettingProposal::new(
            SettingProposalId::new(),
            SettingTarget::global(SettingsSection::Reading),
            SettingProposalChange::SettingsPatch(reading_patch(ReadingFontSize::Small)),
            None,
            SettingProvenance::user(ProvenanceReason::UserRequest),
            UtcMillis(1),
            UtcMillis(2),
        )
        .unwrap();
        store
            .state
            .lock()
            .unwrap()
            .proposals
            .insert(expired.id(), expired.clone());

        // digest 不一致：连状态都不许动（用户确认的不是这一份内容）。
        let error = svc
            .apply_confirmed(expired.id(), &"0".repeat(64))
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_DIGEST_MISMATCH");
        assert_eq!(store.status(expired.id()), SettingProposalStatus::Pending);
        assert_eq!(store.receipt_count(), 0);
    }

    #[tokio::test]
    async fn stale_base_revision_conflicts_without_touching_the_target() {
        let store = FakeStore::new();
        let svc = service(&store);
        let proposal = global_proposal(&store, ReadingFontSize::Large).await;
        // 目标在提案之后被别的会话改过：base_revision（None）不再匹配。
        store.state.lock().unwrap().settings.insert(
            "reading".to_owned(),
            SettingsRow {
                section: "reading".to_owned(),
                schema_version: 1,
                revision: "set-other-session".to_owned(),
                data_json: serde_json::to_string(&SettingsValue::default_for(
                    SettingsSection::Reading,
                ))
                .unwrap(),
                updated_at: UtcMillis(1),
            },
        );

        let error = svc
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "REVISION_CONFLICT");
        assert_eq!(store.target_writes(), 0);
        assert_eq!(store.receipt_count(), 0);
        assert_eq!(store.status(proposal.id()), SettingProposalStatus::Pending);
    }

    #[tokio::test]
    async fn media_item_target_must_belong_to_the_proposed_edition() {
        let store = FakeStore::new();
        let svc = service(&store);
        let edition = EditionId::new();
        let other_edition = EditionId::new();
        let media_item = MediaItemId::new();
        {
            let mut state = store.state.lock().unwrap();
            state.editions.insert(edition);
            state.editions.insert(other_edition);
            state.media_items.insert(media_item, other_edition);
        }
        let proposal = svc
            .create(SettingProposalRequest::new(
                SettingTarget::media_item(edition, media_item),
                SettingProposalChange::ResourcePreference(PreferenceData {
                    reading: Some(ReadingPatch {
                        font_size: Some(ReadingFontSize::Large),
                        ..ReadingPatch::default()
                    }),
                    comic: Some(ComicPatch::default()),
                }),
                SettingProvenance::user(ProvenanceReason::UserRequest),
            ))
            .await
            .unwrap();

        let error = svc
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_TARGET_NOT_FOUND");
        assert_eq!(store.target_writes(), 0);
        assert_eq!(store.receipt_count(), 0);

        // 媒体条目根本不存在同样是稳定的 target-not-found。
        let missing = svc
            .create(SettingProposalRequest::new(
                SettingTarget::edition(edition),
                SettingProposalChange::ResourcePreference(PreferenceData::default()),
                SettingProvenance::user(ProvenanceReason::UserRequest),
            ))
            .await
            .unwrap();
        let receipt = svc
            .apply_confirmed(missing.id(), missing.digest())
            .await
            .unwrap();
        assert!(!receipt.changed, "空覆盖写到空覆盖是幂等 no-op");
    }

    #[tokio::test]
    async fn media_item_preference_row_with_foreign_edition_is_an_integrity_error() {
        let store = FakeStore::new();
        let svc = service(&store);
        let edition = EditionId::new();
        let stranger = EditionId::new();
        let media_item = MediaItemId::new();
        {
            let mut state = store.state.lock().unwrap();
            state.editions.insert(edition);
            state.editions.insert(stranger);
            state.media_items.insert(media_item, edition);
            // 已存在的偏好行自称属于另一个版本：这是损坏行，不是"待覆盖的旧值"。
            state.media_item_preferences.insert(
                media_item,
                MediaItemPreference {
                    media_item_id: media_item,
                    edition_id: stranger,
                    data: PreferenceData::default(),
                    revision: "pref-media-corrupt".to_owned(),
                    updated_at: UtcMillis(1),
                },
            );
        }
        let proposal = svc
            .create(SettingProposalRequest::new(
                SettingTarget::media_item(edition, media_item),
                SettingProposalChange::ResourcePreference(PreferenceData {
                    reading: Some(ReadingPatch {
                        font_size: Some(ReadingFontSize::Large),
                        ..ReadingPatch::default()
                    }),
                    comic: None,
                }),
                SettingProvenance::user(ProvenanceReason::UserRequest),
            ))
            .await
            .unwrap();

        let error = svc
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_INTEGRITY_MISMATCH");
        assert_eq!(store.target_writes(), 0);
        assert_eq!(store.receipt_count(), 0);
        assert_eq!(store.status(proposal.id()), SettingProposalStatus::Pending);
        // 错行必须原样保留（含它错误的归属），不能被静默覆盖成目标版本。
        let kept = store
            .state
            .lock()
            .unwrap()
            .media_item_preferences
            .get(&media_item)
            .cloned()
            .unwrap();
        assert_eq!(kept.edition_id, stranger);
        assert_eq!(kept.revision, "pref-media-corrupt");
    }

    #[tokio::test]
    async fn edition_preference_apply_is_scoped_and_idempotent() {
        let store = FakeStore::new();
        let svc = service(&store);
        let edition = EditionId::new();
        store.state.lock().unwrap().editions.insert(edition);
        let data = PreferenceData {
            reading: Some(ReadingPatch {
                font_size: Some(ReadingFontSize::Large),
                ..ReadingPatch::default()
            }),
            comic: None,
        };
        let proposal = svc
            .create(SettingProposalRequest::new(
                SettingTarget::edition(edition),
                SettingProposalChange::ResourcePreference(data.clone()),
                SettingProvenance::user(ProvenanceReason::UserRequest),
            ))
            .await
            .unwrap();
        let receipt = svc
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap();
        assert!(receipt.changed);
        assert_eq!(
            store
                .state
                .lock()
                .unwrap()
                .edition_preferences
                .get(&edition)
                .unwrap()
                .data,
            data
        );
        // before 是空 PreferenceData（作用域此前没有覆盖）。
        assert_eq!(
            receipt.before_canonical_json,
            canonical_json_of(&PreferenceData::default()).unwrap()
        );
    }

    #[tokio::test]
    async fn edition_target_must_exist() {
        let store = FakeStore::new();
        let svc = service(&store);
        let missing_edition = EditionId::new();
        let proposal = svc
            .create(SettingProposalRequest::new(
                SettingTarget::edition(missing_edition),
                SettingProposalChange::ResourcePreference(PreferenceData {
                    reading: Some(ReadingPatch {
                        font_size: Some(ReadingFontSize::Large),
                        ..ReadingPatch::default()
                    }),
                    comic: None,
                }),
                SettingProvenance::user(ProvenanceReason::UserRequest),
            ))
            .await
            .unwrap();

        let error = svc
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_TARGET_NOT_FOUND");
        // "目标不存在"是没执行，不是执行到一半：目标零写入、无回执，提案仍在 pending。
        assert_eq!(store.target_writes(), 0);
        assert_eq!(store.receipt_count(), 0);
        assert_eq!(store.status(proposal.id()), SettingProposalStatus::Pending);
    }

    #[tokio::test]
    async fn applied_proposal_without_a_receipt_is_an_integrity_error() {
        let store = FakeStore::new();
        let svc = service(&store);
        let proposal = global_proposal(&store, ReadingFontSize::Large).await;
        svc.apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap();
        assert_eq!(store.receipt_count(), 1);
        assert_eq!(store.target_writes(), 1);

        // 回执行被抹掉：状态自称 applied 却拿不出凭证，这不是"已经应用过"，
        // 而是存储被改写。既不能静默返回 Ok，也不能借幂等之名再写一次目标。
        assert!(
            store
                .state
                .lock()
                .unwrap()
                .receipts
                .remove(&proposal.id())
                .is_some()
        );
        let error = svc
            .apply_confirmed(proposal.id(), proposal.digest())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_INTEGRITY_MISMATCH");
        assert_eq!(store.target_writes(), 1);
        assert_eq!(store.receipt_count(), 0);
        assert_eq!(store.status(proposal.id()), SettingProposalStatus::Applied);

        // 读取端必须给出同一个结论：`None` 被契约钉死为"确实没有应用过"，
        // 不能把"存储被改写"报成"还没应用"。
        let error = svc.get_receipt(proposal.id()).await.unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_INTEGRITY_MISMATCH");
        // 提案不存在仍然只是"没有回执"，不是完整性错误。
        assert!(
            svc.get_receipt(SettingProposalId::new())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn expiry_gate_only_applies_to_pending_proposals() {
        let store = FakeStore::new();
        let svc = service(&store);
        // 直接落一条"按时间早已过期、但已应用"的提案 + 回执：过期判定只约束
        // pending；已应用的提案必须继续返回同一张回执，不能被时钟重新判死。
        let stale = SettingProposal::new(
            SettingProposalId::new(),
            SettingTarget::global(SettingsSection::Reading),
            SettingProposalChange::SettingsPatch(reading_patch(ReadingFontSize::Large)),
            None,
            SettingProvenance::user(ProvenanceReason::UserRequest),
            UtcMillis(1),
            UtcMillis(2),
        )
        .unwrap();
        assert!(
            stale.is_expired_at(UtcMillis::now()),
            "测试前提：这条提案按时间已经过期"
        );
        let applied = SettingProposal::from_stored(
            stale.id(),
            stale.canonical_json().to_owned(),
            stale.digest().to_owned(),
            SettingProposalStatus::Applied,
            stale.created_at(),
        )
        .unwrap();
        // 从未持久化的目标 + 空改动：before/after 相同且没有版本号，回执自身合法。
        let before =
            canonical_json_of(&SettingsValue::default_for(SettingsSection::Reading)).unwrap();
        let receipt = SettingChangeReceipt {
            id: SettingChangeReceiptId::new(),
            proposal_id: applied.id(),
            proposal_digest: applied.digest().to_owned(),
            target: applied.target(),
            before_canonical_json: before.clone(),
            after_canonical_json: before,
            applied_revision: None,
            changed: false,
            provenance: applied.provenance().clone(),
            applied_at: UtcMillis(3),
        };
        receipt.validate().unwrap();
        {
            let mut state = store.state.lock().unwrap();
            state.proposals.insert(applied.id(), applied.clone());
            state.receipts.insert(applied.id(), receipt.clone());
        }

        let replayed = svc
            .apply_confirmed(applied.id(), applied.digest())
            .await
            .unwrap();
        assert_eq!(replayed.id, receipt.id);
        assert_eq!(replayed.proposal_digest, applied.digest());
        assert_eq!(store.target_writes(), 0);
        assert_eq!(store.receipt_count(), 1);
        assert_eq!(store.status(applied.id()), SettingProposalStatus::Applied);
    }
}
