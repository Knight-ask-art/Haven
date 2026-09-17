//! AgentProposalService：把 Agent 的上下文绑定推理结果桥接成**设置变更提案**
//! （Haven Copilot Agent Runtime 垂直切片 1）。
//!
//! 边界（与领域模型 `haven-domain::agent` 的意图一致）：
//! - Agent **只能**通过既有 `SettingProposalService::create` 产生设置副作用；
//!   本模块不提供任何"直接写设置""直接执行"的方法。
//! - 每个动作（[`AgentActionProposal`]）都绑定了会话、请求、上下文快照 id 与
//!   `context_hash`，以及上下文声明的 subject；创建时校验 subject 覆盖提案目标，
//!   应用时再次核对存储提案的 target 仍落在同一 subject 内，杜绝"拿 A 作品的
//!   上下文去改 B 作品/别的版本"的跨 subject 复用。
//! - provenance 强制为 `Agent / Agent / AgentSuggestion`，且**不携带任何自由文本**
//!   （context hash、原始参数、上下文正文一律不得塞进 provenance）。
//! - 上下文正文不落库；数据库只保存 `043_agent_action_bindings` 规定的最小绑定元数据，
//!   并与设置提案由同一个 UoW 管理生命周期。
//!
//! `context_hash` 的唯一来源是 `AgentContextSnapshot::context_hash()`——它是不含
//! 自身字段的 canonical 载荷的 SHA-256；非法/空摘要无法成为动作绑定。

use std::sync::Arc;

use async_trait::async_trait;
use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::agent::{
    AgentActionBindingInput, AgentActionKind, AgentApprovalToken, AgentCapability,
    AgentCapabilityManifest, AgentContextSnapshot, AgentScopeSubject, AgentSettingsContextSnapshot,
    AgentSettingsSubject, AgentSubject,
};
use haven_domain::contracts::{
    AgentActionBindingRepository, EditionRepository, MediaItemRepository, WorkRepository,
};
use haven_domain::ids::{
    AgentContextSnapshotId, AgentRequestId, AgentSessionId, SettingProposalId,
};
use haven_domain::setting_proposal::{
    ProvenanceActorKind, ProvenanceReason, ProvenanceSourceKind, SettingChangeReceipt,
    SettingProposal, SettingProposalChange, SettingProvenance, SettingTarget, is_canonical_digest,
    setting_proposal_digest_mismatch_error, setting_proposal_not_found_error,
};

use crate::services::setting_proposals::{SettingProposalRequest, SettingProposalService};
use crate::services::settings::SettingsService;

/// Agent 的能力清单是调用方声明，不是授权凭证。
///
/// 这个策略故意没有公开可配置的能力集合：服务端只允许当前已经实现并经过
/// Application Service 保护的设置读取/设置提案能力。未来增加能力时必须显式
/// 修改这里并补齐对应的服务端边界，而不能由模型或客户端的 manifest 自行放开。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentCapabilityPolicy;

impl AgentCapabilityPolicy {
    pub const fn current() -> Self {
        Self
    }

    /// 校验声明并按服务端固定策略授权一项能力。
    pub fn authorize(
        &self,
        manifest: &AgentCapabilityManifest,
        capability: AgentCapability,
    ) -> Result<(), AppError> {
        manifest.validate()?;
        if !matches!(
            capability,
            AgentCapability::SettingsRead | AgentCapability::SettingsProposal
        ) {
            return Err(agent_capability_policy_denied(capability));
        }
        manifest.require(capability)
    }
}

/// Agent Subject 的真实归属校验端口。
///
/// `AgentSubject` 自身只能表达 ID 之间的结构关系，不能证明这些 ID 在当前数据库中
/// 存在或属于同一条 Work → Edition → MediaItem 链。这个端口是 Application 层的强制
/// 依赖，不能用 `Option` 或隐式 no-op 省略；生产组装应注入基于真实 Repository 的
/// 实现，测试则注入显式的测试 double。
#[async_trait]
pub trait AgentSubjectScopePort: Send + Sync {
    async fn verify_subject(&self, subject: AgentSubject) -> Result<(), AppError>;
}

/// 基于现有 Domain Repository 的默认 Subject Scope 实现。
///
/// 它只读取 Work/Edition/MediaItem，不持有写权限，也不把 SQL 带入 Application 层。
#[derive(Clone)]
pub struct RepositoryAgentSubjectScope {
    works: Arc<dyn WorkRepository + Send + Sync>,
    editions: Arc<dyn EditionRepository + Send + Sync>,
    media_items: Arc<dyn MediaItemRepository + Send + Sync>,
}

impl RepositoryAgentSubjectScope {
    pub fn new(
        works: Arc<dyn WorkRepository + Send + Sync>,
        editions: Arc<dyn EditionRepository + Send + Sync>,
        media_items: Arc<dyn MediaItemRepository + Send + Sync>,
    ) -> Self {
        Self {
            works,
            editions,
            media_items,
        }
    }
}

#[async_trait]
impl AgentSubjectScopePort for RepositoryAgentSubjectScope {
    async fn verify_subject(&self, subject: AgentSubject) -> Result<(), AppError> {
        subject.validate()?;

        if self.works.get(subject.work_id).await?.is_none() {
            return Err(agent_subject_not_found(
                "AGENT_SUBJECT_WORK_NOT_FOUND",
                "Agent Subject 对应的作品不存在",
            ));
        }

        let Some(edition_id) = subject.edition_id else {
            return Ok(());
        };
        let edition = self.editions.get(edition_id).await?.ok_or_else(|| {
            agent_subject_not_found(
                "AGENT_SUBJECT_EDITION_NOT_FOUND",
                "Agent Subject 对应的版本不存在",
            )
        })?;
        if edition.work_id != subject.work_id {
            return Err(agent_subject_scope_mismatch(
                "版本不属于 Agent Subject 的作品",
            ));
        }

        let Some(media_item_id) = subject.media_item_id else {
            return Ok(());
        };
        let media_item = self.media_items.get(media_item_id).await?.ok_or_else(|| {
            agent_subject_not_found(
                "AGENT_SUBJECT_MEDIA_ITEM_NOT_FOUND",
                "Agent Subject 对应的媒体条目不存在",
            )
        })?;
        if media_item.edition_id != edition_id {
            return Err(agent_subject_scope_mismatch(
                "媒体条目不属于 Agent Subject 的版本",
            ));
        }
        Ok(())
    }
}

/// Agent 动作：一次上下文推理发起的设置变更提案，绑定会话 / 请求 / 上下文。
///
/// 字段私有：只能由 [`AgentProposalService`] 的两个创建入口构造，外部无法在
/// 绑定后又替换 `context_hash`、subject 或 `expires_at` 而保持自洽；提案侧的
/// id / digest / 过期时刻只能整份取自刚创建的那条提案，没有裸传入口。
///
/// `subject` 是统一的 [`AgentScopeSubject`]：作品内容范围（快照来自
/// [`AgentContextSnapshot`]）或全局设置范围（快照来自
/// [`AgentSettingsContextSnapshot`]）。两个入口都只从快照派生这些字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentActionProposal {
    session_id: AgentSessionId,
    request_id: AgentRequestId,
    context_snapshot_id: AgentContextSnapshotId,
    context_hash: String,
    subject: AgentScopeSubject,
    setting_proposal_id: SettingProposalId,
    setting_proposal_digest: String,
    /// 提案的过期时刻（UTC 毫秒）：取自刚创建的提案，调用方无法裸传。
    expires_at: UtcMillis,
}

impl AgentActionProposal {
    /// 从**真实上下文快照**与**刚创建的真实提案**派生一个上下文绑定的动作。
    ///
    /// 这是唯一的构造路径，且只对 crate 内部可见：`context_snapshot_id`、
    /// `context_hash` 与 `subject` 全部取自 `&AgentContextSnapshot`；
    /// `setting_proposal_id`、`setting_proposal_digest` 与 `expires_at` 全部取自
    /// `&SettingProposal`。调用方没有机会传入裸 `context_hash` / `subject` /
    /// `snapshot_id` / 提案摘要 / 过期时刻去伪造绑定（前者的 `validate` 保证快照
    /// 自洽，后者由领域构造固化 canonical JSON 与 digest，过期时刻只存在于提案载荷里）。
    ///
    /// - `setting_proposal_digest` 必须是闭合的 SHA-256 小写十六进制摘要。
    /// - 会话 / 请求 / 快照 / 提案 ID 全部拒绝 nil UUID。
    pub(crate) fn from_context(
        session_id: AgentSessionId,
        request_id: AgentRequestId,
        context: &AgentContextSnapshot,
        proposal: &SettingProposal,
    ) -> Result<Self, AppError> {
        let setting_proposal_digest = proposal.digest();
        if !is_canonical_digest(setting_proposal_digest) {
            return Err(agent_action_invalid_proposal_digest());
        }
        validate_agent_action_ids(session_id, request_id, context.id(), proposal.id())?;
        let subject = context.subject();
        subject.validate()?;
        Ok(Self {
            session_id,
            request_id,
            context_snapshot_id: context.id(),
            context_hash: context.context_hash().to_owned(),
            subject: AgentScopeSubject::content(subject),
            setting_proposal_id: proposal.id(),
            setting_proposal_digest: setting_proposal_digest.to_owned(),
            expires_at: proposal.expires_at(),
        })
    }

    /// 从**全局设置上下文快照**与**刚创建的真实提案**派生一个设置范围动作。
    ///
    /// 与 [`AgentActionProposal::from_context`] 同构：`context_snapshot_id`、
    /// `context_hash` 与 subject 全部取自 `&AgentSettingsContextSnapshot`，
    /// 提案侧的 id/digest/过期时刻全部取自 `&SettingProposal`。
    pub(crate) fn from_settings_context(
        session_id: AgentSessionId,
        request_id: AgentRequestId,
        context: &AgentSettingsContextSnapshot,
        proposal: &SettingProposal,
    ) -> Result<Self, AppError> {
        let setting_proposal_digest = proposal.digest();
        if !is_canonical_digest(setting_proposal_digest) {
            return Err(agent_action_invalid_proposal_digest());
        }
        validate_agent_action_ids(session_id, request_id, context.id(), proposal.id())?;
        let subject = context.subject();
        subject.validate()?;
        Ok(Self {
            session_id,
            request_id,
            context_snapshot_id: context.id(),
            context_hash: context.context_hash().to_owned(),
            subject: AgentScopeSubject::settings(subject),
            setting_proposal_id: proposal.id(),
            setting_proposal_digest: setting_proposal_digest.to_owned(),
            expires_at: proposal.expires_at(),
        })
    }

    /// 从**持久化绑定**与它的提案还原一个设置范围动作（批准/拒绝路径）。
    ///
    /// 与两个 `from_*_context` 入口同构：所有事实都取自绑定与提案本身，
    /// 调用方只能提供 `proposal_id`（已经用于读取这两条记录），无法裸传任何字段。
    /// 绑定必须是设置范围，否则这里直接拒绝——作品内容范围的提案不会走设置批准入口。
    pub fn from_settings_binding(
        binding: &haven_domain::agent::AgentActionBinding,
        proposal: &SettingProposal,
    ) -> Result<Self, AppError> {
        if binding.proposal_id() != proposal.id() {
            return Err(agent_binding_integrity("绑定与提案不是同一条记录"));
        }
        let subject = binding
            .subject()
            .as_settings()
            .ok_or_else(|| agent_binding_integrity("绑定不是全局设置范围"))?;
        subject.validate()?;
        if !subject.covers(proposal.target()) {
            return Err(agent_action_subject_mismatch());
        }
        let setting_proposal_digest = proposal.digest();
        if !is_canonical_digest(setting_proposal_digest) {
            return Err(agent_action_invalid_proposal_digest());
        }
        validate_agent_action_ids(
            binding.session_id(),
            binding.request_id(),
            binding.context_snapshot_id(),
            proposal.id(),
        )?;
        Ok(Self {
            session_id: binding.session_id(),
            request_id: binding.request_id(),
            context_snapshot_id: binding.context_snapshot_id(),
            context_hash: binding.context_hash().to_owned(),
            subject: binding.subject(),
            setting_proposal_id: proposal.id(),
            setting_proposal_digest: setting_proposal_digest.to_owned(),
            expires_at: binding.expires_at(),
        })
    }

    pub fn session_id(&self) -> AgentSessionId {
        self.session_id
    }

    pub fn request_id(&self) -> AgentRequestId {
        self.request_id
    }

    pub fn context_snapshot_id(&self) -> AgentContextSnapshotId {
        self.context_snapshot_id
    }

    /// 触发本动作的上下文快照摘要（SHA-256 小写十六进制）。
    pub fn context_hash(&self) -> &str {
        &self.context_hash
    }

    /// 上下文声明的推理范围（只表达范围，不授予权限）。
    pub fn subject(&self) -> AgentScopeSubject {
        self.subject
    }

    pub fn setting_proposal_id(&self) -> SettingProposalId {
        self.setting_proposal_id
    }

    /// 存储提案的 digest（展示给用户确认的就是它）。
    pub fn setting_proposal_digest(&self) -> &str {
        &self.setting_proposal_digest
    }

    /// 存储提案的过期时刻（UTC 毫秒），从刚创建的提案派生。
    pub fn expires_at(&self) -> UtcMillis {
        self.expires_at
    }
}

/// 创建 Agent 设置动作的输入。
///
/// `context` 直接引用**已构造的快照**：subject / 快照 id / context_hash 都从快照
/// 派生，调用方没有机会让三者互相不一致。
#[derive(Debug, Clone)]
pub struct AgentSettingsActionRequest<'a> {
    pub session_id: AgentSessionId,
    pub request_id: AgentRequestId,
    pub context: &'a AgentContextSnapshot,
    pub target: SettingTarget,
    pub change: SettingProposalChange,
    /// Agent/模型对能力的声明。它必须再次经过服务端固定策略授权，不能单独
    /// 作为权限来源。
    pub capability_manifest: AgentCapabilityManifest,
    /// 目标作用域当前 authoritative revision；`None` 表示目标行尚未持久化。
    pub base_revision: Option<String>,
    pub ttl_ms: Option<i64>,
}

impl<'a> AgentSettingsActionRequest<'a> {
    pub fn new(
        session_id: AgentSessionId,
        request_id: AgentRequestId,
        context: &'a AgentContextSnapshot,
        target: SettingTarget,
        change: SettingProposalChange,
    ) -> Self {
        Self {
            session_id,
            request_id,
            context,
            target,
            change,
            capability_manifest: AgentCapabilityManifest::for_current_slice(),
            base_revision: None,
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

    pub fn with_capability_manifest(mut self, manifest: AgentCapabilityManifest) -> Self {
        self.capability_manifest = manifest;
        self
    }
}

/// 上下文绑定的 Agent 提案编排。
///
/// 只持有既有的 [`SettingProposalService`]，因此 Agent 的设置副作用与用户界面
/// 走**同一条** create / reject / apply_confirmed 路径（同一套 UoW / CAS / Receipt）。
#[derive(Clone)]
pub struct AgentProposalService {
    setting_proposals: SettingProposalService,
    subject_scope: Arc<dyn AgentSubjectScopePort>,
    bindings: Arc<dyn AgentActionBindingRepository>,
    capability_policy: AgentCapabilityPolicy,
    /// 全局设置上下文只在"读取权威快照"时使用；缺少它时设置范围读入口
    /// fail-closed，而不是回落到默认值假装读到了设置。
    settings: Option<SettingsService>,
}

impl AgentProposalService {
    pub fn new(
        setting_proposals: SettingProposalService,
        subject_scope: Arc<dyn AgentSubjectScopePort>,
        bindings: Arc<dyn AgentActionBindingRepository>,
    ) -> Self {
        Self {
            setting_proposals,
            subject_scope,
            bindings,
            capability_policy: AgentCapabilityPolicy::current(),
            settings: None,
        }
    }

    /// 注入设置读取服务，开启全局设置上下文入口。
    pub fn with_settings_service(mut self, settings: SettingsService) -> Self {
        self.settings = Some(settings);
        self
    }

    /// 由一次上下文推理创建设置变更提案（**只创建提案，不写任何设置**）。
    ///
    /// 校验顺序：
    /// 1. 上下文 subject 必须覆盖提案 target（跨 subject/context 复用直接拒绝）；
    /// 2. `context_hash` 必须是闭合摘要（快照构造保证，仍再挡一层）；
    /// 3. provenance 强制 `Agent / Agent / AgentSuggestion` 且不带自由文本；
    /// 4. 委托既有 [`SettingProposalService::create`] 落库为 `pending` 提案。
    pub async fn create_setting_proposal(
        &self,
        request: AgentSettingsActionRequest<'_>,
    ) -> Result<AgentActionProposal, AppError> {
        // 强类型身份在**任何副作用之前**先校验：nil 会话/请求直接拒绝，
        // 不会先落一条提案再报错（避免非法绑定留下孤儿提案）。
        validate_agent_identity_ids(request.session_id, request.request_id)?;

        self.capability_policy.authorize(
            &request.capability_manifest,
            AgentCapability::SettingsProposal,
        )?;

        let subject = request.context.subject();
        subject.validate()?;
        if !subject.covers(request.target) {
            return Err(agent_action_subject_mismatch());
        }
        self.subject_scope.verify_subject(subject).await?;

        let context_hash = request.context.context_hash();
        if !is_canonical_digest(context_hash) {
            return Err(agent_action_invalid_context_hash());
        }

        // provenance 是**固定的闭合事实**：来源/主体/理由全部由本模块强制，
        // 不接受任何自由文本，避免把 context hash、原始参数或上下文正文当成
        // 旁路信道塞进来。
        let provenance = SettingProvenance::new(
            ProvenanceSourceKind::Agent,
            ProvenanceActorKind::Agent,
            ProvenanceReason::AgentSuggestion,
        );

        let mut proposal_request =
            SettingProposalRequest::new(request.target, request.change, provenance);
        if let Some(base_revision) = request.base_revision {
            proposal_request = proposal_request.with_base_revision(Some(base_revision));
        }
        if let Some(ttl_ms) = request.ttl_ms {
            proposal_request = proposal_request.with_ttl_ms(ttl_ms);
        }

        let binding_input = AgentActionBindingInput::new(
            request.session_id,
            request.request_id,
            request.context.id(),
            request.context.context_hash(),
            AgentScopeSubject::content(subject),
            AgentActionKind::SettingsProposal,
        )?;
        let (proposal, _binding) = self
            .setting_proposals
            .create_with_agent_binding(proposal_request, binding_input)
            .await?;
        AgentActionProposal::from_context(
            request.session_id,
            request.request_id,
            request.context,
            &proposal,
        )
    }

    /// 由一份**已校验过 freshness 的**全局设置上下文创建设置提案。
    ///
    /// 调用方（`agent_settings_ipc`）负责"重新读取 authoritative 设置、重建上下文并
    /// 逐项比对 context id/hash/revision"；本方法只做领域侧的强制约束：
    /// - 能力清单取自**上下文里的服务端固定清单**（调用方无法自带清单）；
    /// - 目标必须是该设置范围覆盖的全局分区（跨范围复用直接拒绝）；
    /// - provenance 固定 `Agent / Agent / AgentSuggestion`，不带任何自由文本。
    ///
    /// 落库走既有的 [`SettingProposalService::create_with_agent_binding`]，因此
    /// 基线版本会在**同一个事务内**再核对一次（上下文过期 fail-closed）。
    pub async fn create_settings_proposal(
        &self,
        session_id: AgentSessionId,
        request_id: AgentRequestId,
        context: &AgentSettingsContextSnapshot,
        target: SettingTarget,
        change: SettingProposalChange,
        base_revision: Option<String>,
        ttl_ms: Option<i64>,
    ) -> Result<AgentActionProposal, AppError> {
        validate_agent_identity_ids(session_id, request_id)?;

        let manifest = context.capabilities();
        self.capability_policy
            .authorize(&manifest, AgentCapability::SettingsProposal)?;

        let subject = context.subject();
        subject.validate()?;
        if !subject.covers(target) {
            return Err(agent_action_subject_mismatch());
        }
        if !is_canonical_digest(context.context_hash()) {
            return Err(agent_action_invalid_context_hash());
        }

        let provenance = SettingProvenance::new(
            ProvenanceSourceKind::Agent,
            ProvenanceActorKind::Agent,
            ProvenanceReason::AgentSuggestion,
        );
        let mut proposal_request = SettingProposalRequest::new(target, change, provenance);
        if let Some(base_revision) = base_revision {
            proposal_request = proposal_request.with_base_revision(Some(base_revision));
        }
        if let Some(ttl_ms) = ttl_ms {
            proposal_request = proposal_request.with_ttl_ms(ttl_ms);
        }

        let binding_input = AgentActionBindingInput::new(
            session_id,
            request_id,
            context.id(),
            context.context_hash(),
            AgentScopeSubject::settings(subject),
            AgentActionKind::SettingsProposal,
        )?;
        let (proposal, _binding) = self
            .setting_proposals
            .create_with_agent_binding(proposal_request, binding_input)
            .await?;
        AgentActionProposal::from_settings_context(session_id, request_id, context, &proposal)
    }

    /// 读取某条提案当前的持久化绑定（设置范围读路径用它回读 context hash 等事实）。
    pub async fn load_binding(
        &self,
        proposal_id: SettingProposalId,
    ) -> Result<Option<haven_domain::agent::AgentActionBinding>, AppError> {
        self.bindings.get(proposal_id).await
    }

    /// 拒绝动作对应的提案（委托既有 [`SettingProposalService::reject`]）。
    ///
    /// 拒绝不写任何设置事实，因此不要求 Subject 仍然存在；已拒绝且 digest 一致时保持幂等。
    pub async fn reject(
        &self,
        action: &AgentActionProposal,
        confirmed_digest: &str,
    ) -> Result<(), AppError> {
        self.require_bound_action(action, confirmed_digest).await?;
        self.setting_proposals
            .reject(action.setting_proposal_id, confirmed_digest)
            .await
    }

    /// 为动作对应的 pending 提案签发一次性审批令牌。
    ///
    /// 返回的明文令牌只由调用方持有一次；绑定、过期、digest 和状态校验仍由
    /// `SettingProposalService` 在同一个 UoW 内完成。
    pub async fn issue_approval_token(
        &self,
        action: &AgentActionProposal,
        confirmed_digest: &str,
    ) -> Result<AgentApprovalToken, AppError> {
        self.require_bound_action(action, confirmed_digest).await?;
        self.setting_proposals
            .issue_agent_approval_token(action.setting_proposal_id, confirmed_digest)
            .await
    }

    /// 显式确认并应用动作对应的提案（必须携带一次性审批令牌）。
    ///
    /// 返回既有 service 产出的 [`SettingChangeReceipt`]；本模块不复制设置执行逻辑，
    /// 也不暴露任何绕过 digest / token / UoW / CAS 的写入入口。
    pub async fn apply_confirmed(
        &self,
        action: &AgentActionProposal,
        confirmed_digest: &str,
        approval_token: &AgentApprovalToken,
    ) -> Result<SettingChangeReceipt, AppError> {
        self.require_bound_action(action, confirmed_digest).await?;
        self.setting_proposals
            .apply_confirmed_with_approval_token(
                action.setting_proposal_id,
                confirmed_digest,
                approval_token,
            )
            .await
    }

    /// 用户批准 Agent 设置提案的**唯一**写入口。
    ///
    /// 令牌、目标 CAS、回执与 Proposal/Binding 状态迁移由
    /// `SettingProposalService` 在同一个 UoW 内完成；这里不接收或暴露明文令牌。
    pub async fn approve_agent_settings_proposal_in_one_uow(
        &self,
        proposal_id: SettingProposalId,
        confirmed_digest: &str,
        subject: AgentSettingsSubject,
    ) -> Result<SettingChangeReceipt, AppError> {
        self.setting_proposals
            .approve_agent_proposal_in_one_uow(proposal_id, confirmed_digest, subject)
            .await
    }

    /// 执行前核对动作与存储提案、上下文的绑定关系：
    /// - 调用方确认的 digest 必须等于动作记录的 digest（防止拿别的确认执行本动作）；
    /// - 存储提案必须存在；
    /// - 存储提案的 target 仍必须落在动作 subject 覆盖范围内（跨 subject 复用拒绝）。
    async fn require_bound_action(
        &self,
        action: &AgentActionProposal,
        confirmed_digest: &str,
    ) -> Result<(), AppError> {
        if action.setting_proposal_digest != confirmed_digest {
            return Err(setting_proposal_digest_mismatch_error());
        }
        let proposal = self
            .setting_proposals
            .get(action.setting_proposal_id)
            .await?
            .ok_or_else(setting_proposal_not_found_error)?;
        let binding = self
            .bindings
            .get(action.setting_proposal_id)
            .await?
            .ok_or_else(agent_binding_missing)?;
        if binding.proposal_id() != action.setting_proposal_id
            || binding.session_id() != action.session_id
            || binding.request_id() != action.request_id
            || binding.context_snapshot_id() != action.context_snapshot_id
            || binding.context_hash() != action.context_hash
            || binding.subject() != action.subject
            || binding.action_kind() != AgentActionKind::SettingsProposal
            || binding.expires_at() != action.expires_at
        {
            return Err(agent_binding_integrity("动作对象与持久化绑定不一致"));
        }
        if !action.subject.covers(proposal.target()) {
            return Err(agent_action_subject_mismatch());
        }
        Ok(())
    }
}

// ---------- 稳定错误 ----------

/// 会话 / 请求身份必须是非 nil UUID（生产路径在产生任何副作用前先挡）。
fn validate_agent_identity_ids(
    session_id: AgentSessionId,
    request_id: AgentRequestId,
) -> Result<(), AppError> {
    if session_id.as_uuid().is_nil() {
        return Err(agent_action_invalid_binding("会话 ID 不能是 nil"));
    }
    if request_id.as_uuid().is_nil() {
        return Err(agent_action_invalid_binding("请求 ID 不能是 nil"));
    }
    Ok(())
}

/// 动作绑定的四个强类型身份都必须是非 nil UUID。
fn validate_agent_action_ids(
    session_id: AgentSessionId,
    request_id: AgentRequestId,
    context_snapshot_id: AgentContextSnapshotId,
    setting_proposal_id: SettingProposalId,
) -> Result<(), AppError> {
    validate_agent_identity_ids(session_id, request_id)?;
    if context_snapshot_id.as_uuid().is_nil() {
        return Err(agent_action_invalid_binding("上下文快照 ID 不能是 nil"));
    }
    if setting_proposal_id.as_uuid().is_nil() {
        return Err(agent_action_invalid_binding("设置提案 ID 不能是 nil"));
    }
    Ok(())
}

/// 动作绑定的 `context_hash` 不是闭合的 SHA-256 小写十六进制摘要。
fn agent_action_invalid_context_hash() -> AppError {
    AppError::new(
        "AGENT_ACTION_INVALID_CONTEXT_HASH",
        ErrorKind::Validation,
        "Agent 动作的 context_hash 非法",
        false,
    )
}

/// 动作记录的提案摘要不是闭合摘要。
fn agent_action_invalid_proposal_digest() -> AppError {
    AppError::new(
        "AGENT_ACTION_INVALID_PROPOSAL_DIGEST",
        ErrorKind::Validation,
        "Agent 动作的提案摘要非法",
        false,
    )
}

/// 动作的会话/请求/上下文绑定非法（例如 nil 快照 ID）。
fn agent_action_invalid_binding(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_ACTION_INVALID_BINDING",
        ErrorKind::Validation,
        format!("Agent 动作绑定非法：{detail}"),
        false,
    )
}

/// Agent 试图用某个上下文去修改它覆盖范围之外的设置（跨 subject/context 复用）。
fn agent_action_subject_mismatch() -> AppError {
    AppError::new(
        "AGENT_ACTION_SUBJECT_MISMATCH",
        ErrorKind::Forbidden,
        "Agent 上下文范围不覆盖该设置目标，已拒绝",
        false,
    )
}

fn agent_capability_policy_denied(capability: AgentCapability) -> AppError {
    AppError::new(
        "AGENT_CAPABILITY_POLICY_DENIED",
        ErrorKind::Forbidden,
        format!("服务端策略未开放 Agent 能力：{}", capability.as_str()),
        false,
    )
}

fn agent_subject_not_found(code: &'static str, message: &'static str) -> AppError {
    AppError::new(code, ErrorKind::NotFound, message, false)
}

fn agent_subject_scope_mismatch(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_SUBJECT_SCOPE_MISMATCH",
        ErrorKind::Forbidden,
        format!("Agent Subject 归属校验失败：{detail}"),
        false,
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex};

    use haven_common::UtcMillis;
    use haven_domain::agent::{
        AgentApprovalState, AgentBoundaryMode, AgentContentKind, AgentContextPayload,
        AgentContextSnapshot, AgentLocatorConfidence, AgentScopeSubject,
    };
    use haven_domain::contracts::{
        EditionPreference, MediaItemPreference, SettingProposalRepository, SettingsRow,
    };
    use haven_domain::ids::{EditionId, MediaItemId, WorkId};
    use haven_domain::setting_proposal::{SettingProposal, SettingProposalStatus};
    use haven_domain::settings::{
        PreferenceData, ReadingFontSize, ReadingPatch, SettingsPatch, SettingsSection,
    };

    use crate::services::setting_proposals::{SettingProposalTxPorts, SettingProposalUoW};

    // ---------- 纯内存假实现（无 SQL） ----------
    //
    // 只实现端口语义（提案/回执/全局设置行 + 写入计数），不复制任何设置执行
    // 逻辑：apply_confirmed / reject 的编排仍由既有 SettingProposalService 负责。

    #[derive(Default)]
    struct FakeState {
        proposals: HashMap<SettingProposalId, SettingProposal>,
        bindings: HashMap<SettingProposalId, haven_domain::agent::AgentActionBinding>,
        receipts: HashMap<SettingProposalId, SettingChangeReceipt>,
        settings: HashMap<String, SettingsRow>,
        /// 目标写入次数：证明"只创建提案"的分支确实没碰目标。
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

        fn status(&self, id: SettingProposalId) -> SettingProposalStatus {
            self.state
                .lock()
                .unwrap()
                .proposals
                .get(&id)
                .unwrap()
                .status()
        }

        fn stored(&self, id: SettingProposalId) -> SettingProposal {
            self.state
                .lock()
                .unwrap()
                .proposals
                .get(&id)
                .cloned()
                .unwrap()
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
            Ok(self.state.lock().unwrap().receipts.get(&id).cloned())
        }
    }

    #[async_trait::async_trait]
    impl AgentActionBindingRepository for FakeStore {
        async fn get(
            &self,
            proposal_id: SettingProposalId,
        ) -> Result<Option<haven_domain::agent::AgentActionBinding>, AppError> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .bindings
                .get(&proposal_id)
                .cloned())
        }
    }

    impl SettingProposalUoW for FakeStore {
        fn run(
            &self,
            f: &dyn Fn(&dyn SettingProposalTxPorts) -> Result<(), AppError>,
        ) -> Result<(), AppError> {
            f(&FakeTx { state: &self.state })
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
            binding: &haven_domain::agent::AgentActionBinding,
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
        ) -> Result<Option<haven_domain::agent::AgentActionBinding>, AppError> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .bindings
                .get(&proposal_id)
                .cloned())
        }

        fn verify_agent_subject(&self, subject: AgentScopeSubject) -> Result<(), AppError> {
            // 作品范围在假实现里不做真实归属查询；全局设置范围必须是 no-op。
            subject.validate()
        }

        fn verify_settings_base_revision(
            &self,
            section: &str,
            expected_revision: Option<&str>,
        ) -> Result<bool, AppError> {
            let state = self.state.lock().unwrap();
            let current = state.settings.get(section);
            Ok(current.map(|row| row.revision.as_str()) == expected_revision)
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
            Ok(self.state.lock().unwrap().receipts.get(&id).cloned())
        }

        fn edition_exists(&self, _edition_id: EditionId) -> Result<bool, AppError> {
            Ok(false)
        }

        fn media_item_edition(
            &self,
            _media_item_id: MediaItemId,
        ) -> Result<Option<EditionId>, AppError> {
            Ok(None)
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
            _edition_id: EditionId,
        ) -> Result<Option<EditionPreference>, AppError> {
            Ok(None)
        }

        fn load_media_item_preference(
            &self,
            _media_item_id: MediaItemId,
        ) -> Result<Option<MediaItemPreference>, AppError> {
            Ok(None)
        }

        fn cas_upsert_edition(
            &self,
            _preference: &EditionPreference,
            _expected_revision: Option<&str>,
        ) -> Result<bool, AppError> {
            Ok(false)
        }

        fn cas_upsert_media_item(
            &self,
            _preference: &MediaItemPreference,
            _expected_revision: Option<&str>,
        ) -> Result<bool, AppError> {
            Ok(false)
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
            approval_token_hash: &haven_domain::agent::AgentApprovalTokenHash,
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
            approval_token_hash: &haven_domain::agent::AgentApprovalTokenHash,
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
            let cleared = haven_domain::agent::AgentActionBinding::from_stored(
                binding.proposal_id(),
                binding.session_id(),
                binding.request_id(),
                binding.context_snapshot_id(),
                binding.context_hash().to_owned(),
                binding.subject(),
                binding.action_kind(),
                binding.created_at(),
                binding.expires_at(),
                binding.approval_state(),
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

    #[derive(Default)]
    struct TestSubjectScope {
        allow_unknown: bool,
        deny: Mutex<bool>,
        works: Mutex<HashSet<WorkId>>,
        edition_work: Mutex<HashMap<EditionId, WorkId>>,
        media_item_edition: Mutex<HashMap<MediaItemId, EditionId>>,
    }

    impl TestSubjectScope {
        fn allow_all() -> Self {
            Self {
                allow_unknown: true,
                ..Self::default()
            }
        }

        fn strict() -> Self {
            Self::default()
        }

        fn add_work(&self, work_id: WorkId) {
            self.works.lock().unwrap().insert(work_id);
        }

        fn add_edition(&self, edition_id: EditionId, work_id: WorkId) {
            self.edition_work
                .lock()
                .unwrap()
                .insert(edition_id, work_id);
        }

        fn add_media_item(&self, media_item_id: MediaItemId, edition_id: EditionId) {
            self.media_item_edition
                .lock()
                .unwrap()
                .insert(media_item_id, edition_id);
        }

        fn set_deny(&self, deny: bool) {
            *self.deny.lock().unwrap() = deny;
        }
    }

    #[async_trait::async_trait]
    impl AgentSubjectScopePort for TestSubjectScope {
        async fn verify_subject(&self, subject: AgentSubject) -> Result<(), AppError> {
            subject.validate()?;
            if *self.deny.lock().unwrap() {
                return Err(agent_subject_scope_mismatch("测试 double 已拒绝后续复核"));
            }
            if self.allow_unknown {
                return Ok(());
            }
            if !self.works.lock().unwrap().contains(&subject.work_id) {
                return Err(agent_subject_not_found(
                    "AGENT_SUBJECT_WORK_NOT_FOUND",
                    "测试作品不存在",
                ));
            }
            let Some(edition_id) = subject.edition_id else {
                return Ok(());
            };
            let Some(owner_work_id) = self.edition_work.lock().unwrap().get(&edition_id).copied()
            else {
                return Err(agent_subject_not_found(
                    "AGENT_SUBJECT_EDITION_NOT_FOUND",
                    "测试版本不存在",
                ));
            };
            if owner_work_id != subject.work_id {
                return Err(agent_subject_scope_mismatch("测试版本归属不匹配"));
            }
            let Some(media_item_id) = subject.media_item_id else {
                return Ok(());
            };
            let Some(owner_edition_id) = self
                .media_item_edition
                .lock()
                .unwrap()
                .get(&media_item_id)
                .copied()
            else {
                return Err(agent_subject_not_found(
                    "AGENT_SUBJECT_MEDIA_ITEM_NOT_FOUND",
                    "测试媒体条目不存在",
                ));
            };
            if owner_edition_id != edition_id {
                return Err(agent_subject_scope_mismatch("测试媒体条目归属不匹配"));
            }
            Ok(())
        }
    }

    fn service(store: &Arc<FakeStore>) -> AgentProposalService {
        service_with_scope(store, Arc::new(TestSubjectScope::allow_all()))
    }

    fn service_with_scope(
        store: &Arc<FakeStore>,
        scope: Arc<dyn AgentSubjectScopePort>,
    ) -> AgentProposalService {
        AgentProposalService::new(
            SettingProposalService::new(store.clone(), store.clone()),
            scope,
            store.clone(),
        )
    }

    fn reading_change(font_size: ReadingFontSize) -> SettingProposalChange {
        SettingProposalChange::SettingsPatch(SettingsPatch::Reading(ReadingPatch {
            font_size: Some(font_size),
            ..ReadingPatch::default()
        }))
    }

    fn work_subject() -> AgentSubject {
        AgentSubject::work(WorkId::new(), AgentContentKind::Book)
    }

    fn snapshot(subject: AgentSubject, revision: &str) -> AgentContextSnapshot {
        AgentContextSnapshot::new(
            AgentContextSnapshotId::new(),
            AgentContextPayload::new(
                subject,
                None,
                AgentLocatorConfidence::Unresolved,
                AgentBoundaryMode::UserProvided,
                revision,
            ),
        )
        .unwrap()
    }

    /// 直接构造一条合法提案（领域构造固化 canonical JSON、digest 与过期时刻）。
    fn stored_proposal(id: SettingProposalId, ttl_ms: i64) -> SettingProposal {
        SettingProposal::new(
            id,
            SettingTarget::global(SettingsSection::Reading),
            reading_change(ReadingFontSize::Large),
            None,
            SettingProvenance::agent(ProvenanceReason::AgentSuggestion),
            UtcMillis(1_000),
            UtcMillis(1_000 + ttl_ms),
        )
        .unwrap()
    }

    async fn create_action(
        svc: &AgentProposalService,
        context: &AgentContextSnapshot,
    ) -> AgentActionProposal {
        svc.create_setting_proposal(AgentSettingsActionRequest::new(
            AgentSessionId::new(),
            AgentRequestId::new(),
            context,
            SettingTarget::global(SettingsSection::Reading),
            reading_change(ReadingFontSize::Large),
        ))
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn create_binds_context_and_forces_agent_provenance_without_writing_target() {
        let store = FakeStore::new();
        let svc = service(&store);
        let subject = work_subject();
        let context = snapshot(subject, "rev-0001");
        let session_id = AgentSessionId::new();
        let request_id = AgentRequestId::new();

        let action = svc
            .create_setting_proposal(AgentSettingsActionRequest::new(
                session_id,
                request_id,
                &context,
                SettingTarget::global(SettingsSection::Reading),
                reading_change(ReadingFontSize::Large),
            ))
            .await
            .unwrap();

        // 返回的动作绑定上下文 / 会话 / 请求 / 范围，以及创建出的提案 id+digest+过期时刻。
        assert_eq!(action.session_id(), session_id);
        assert_eq!(action.request_id(), request_id);
        assert_eq!(action.context_snapshot_id(), context.id());
        assert_eq!(action.context_hash(), context.context_hash());
        assert_eq!(action.subject(), AgentScopeSubject::content(subject));
        assert_eq!(action.context_hash().len(), 64);
        assert!(is_canonical_digest(action.context_hash()));

        let stored = store.stored(action.setting_proposal_id());
        assert_eq!(action.setting_proposal_digest(), stored.digest());
        assert!(is_canonical_digest(action.setting_proposal_digest()));
        // 过期时刻不是调用方传入的裸值：它就是刚落库提案的过期时刻。
        assert_eq!(action.expires_at(), stored.expires_at());
        assert!(
            action.expires_at().0 > stored.created_at().0,
            "动作的过期时刻必须晚于提案创建时刻"
        );
        assert_eq!(stored.status(), SettingProposalStatus::Pending);
        assert_eq!(
            stored.target(),
            SettingTarget::global(SettingsSection::Reading)
        );

        // provenance 强制 Agent / Agent / AgentSuggestion，且不带任何自由文本
        // （context_hash / 原始参数 / 上下文正文一律不得塞进来）。
        let provenance = stored.provenance();
        assert_eq!(provenance.source_kind, ProvenanceSourceKind::Agent);
        assert_eq!(provenance.actor_kind, ProvenanceActorKind::Agent);
        assert_eq!(provenance.reason, ProvenanceReason::AgentSuggestion);
        assert!(provenance.source_ref.is_none());
        assert!(provenance.summary.is_none());

        // 只创建提案：目标零写入。
        assert_eq!(store.target_writes(), 0);
    }

    #[tokio::test]
    async fn create_rejects_subject_mismatch_and_illegal_bindings() {
        let store = FakeStore::new();
        let svc = service(&store);
        let context = snapshot(work_subject(), "rev-0001");

        // 作品级 subject 不覆盖版本级目标：跨 subject/context 复用直接拒绝，
        // 且不会创建任何提案。
        let error = svc
            .create_setting_proposal(AgentSettingsActionRequest::new(
                AgentSessionId::new(),
                AgentRequestId::new(),
                &context,
                SettingTarget::edition(EditionId::new()),
                SettingProposalChange::ResourcePreference(PreferenceData::default()),
            ))
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_ACTION_SUBJECT_MISMATCH");
        assert!(store.state.lock().unwrap().proposals.is_empty());
        assert_eq!(store.target_writes(), 0);

        // 绑定只能从真实快照与真实提案派生：提案侧（id / digest / expires_at）
        // 没有裸传入口，只能整份取自构造好的提案。
        let proposal = stored_proposal(SettingProposalId::new(), 60_000);
        let derived = AgentActionProposal::from_context(
            AgentSessionId::new(),
            AgentRequestId::new(),
            &context,
            &proposal,
        )
        .unwrap();
        assert_eq!(derived.setting_proposal_id(), proposal.id());
        assert_eq!(derived.setting_proposal_digest(), proposal.digest());
        assert_eq!(derived.expires_at(), proposal.expires_at());

        // 会话 / 请求 / 提案强类型身份都必须拒绝 nil UUID
        // （快照 ID 由快照派生，nil 快照在领域边界已被拒绝）。
        let nil = uuid::Uuid::nil();
        for (session_id, request_id, proposal_id) in [
            (
                AgentSessionId::from_uuid(nil),
                AgentRequestId::new(),
                SettingProposalId::new(),
            ),
            (
                AgentSessionId::new(),
                AgentRequestId::from_uuid(nil),
                SettingProposalId::new(),
            ),
            (
                AgentSessionId::new(),
                AgentRequestId::new(),
                SettingProposalId::from_uuid(nil),
            ),
        ] {
            let error = AgentActionProposal::from_context(
                session_id,
                request_id,
                &context,
                &stored_proposal(proposal_id, 60_000),
            )
            .unwrap_err();
            assert_eq!(error.code().as_str(), "AGENT_ACTION_INVALID_BINDING");
        }
    }

    #[test]
    fn action_binding_is_derived_from_the_snapshot_and_the_proposal_not_caller_input() {
        let context = snapshot(work_subject(), "rev-0001");
        let proposal = stored_proposal(SettingProposalId::new(), 60_000);
        let action = AgentActionProposal::from_context(
            AgentSessionId::new(),
            AgentRequestId::new(),
            &context,
            &proposal,
        )
        .unwrap();

        // context_snapshot_id / context_hash / subject 全部由快照派生。
        assert_eq!(action.context_snapshot_id(), context.id());
        assert_eq!(action.context_hash(), context.context_hash());
        assert_eq!(
            action.subject(),
            AgentScopeSubject::content(context.subject())
        );
        assert!(is_canonical_digest(action.context_hash()));

        // 提案侧三项同样只能来自提案本身。
        assert_eq!(action.setting_proposal_id(), proposal.id());
        assert_eq!(action.setting_proposal_digest(), proposal.digest());
        assert_eq!(action.expires_at(), proposal.expires_at());
    }

    #[tokio::test]
    async fn action_expires_at_is_derived_from_the_created_proposal() {
        let store = FakeStore::new();
        let svc = service(&store);
        let context = snapshot(work_subject(), "rev-0001");

        let action = svc
            .create_setting_proposal(
                AgentSettingsActionRequest::new(
                    AgentSessionId::new(),
                    AgentRequestId::new(),
                    &context,
                    SettingTarget::global(SettingsSection::Reading),
                    reading_change(ReadingFontSize::Large),
                )
                .with_ttl_ms(60_000),
            )
            .await
            .unwrap();

        // 动作的过期时刻就是刚落库提案的过期时刻：TTL 也必须来自提案，而不是
        // 另一条可以脱离提案自洽的裸参数。
        let stored = store.stored(action.setting_proposal_id());
        assert_eq!(action.expires_at(), stored.expires_at());
        assert_eq!(
            action.expires_at().0 - stored.created_at().0,
            60_000,
            "动作的过期时刻必须与提案的 TTL 一致"
        );
    }

    #[tokio::test]
    async fn reject_delegates_and_keeps_idempotency() {
        let store = FakeStore::new();
        let svc = service(&store);
        let action = create_action(&svc, &snapshot(work_subject(), "rev-0001")).await;

        // 确认 digest 与动作记录的 digest 不符：拒绝执行、状态不变、零写入。
        let error = svc.reject(&action, &"0".repeat(64)).await.unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_DIGEST_MISMATCH");
        assert_eq!(
            store.status(action.setting_proposal_id()),
            SettingProposalStatus::Pending
        );

        svc.reject(&action, action.setting_proposal_digest())
            .await
            .unwrap();
        assert_eq!(
            store.status(action.setting_proposal_id()),
            SettingProposalStatus::Rejected
        );
        // 幂等：重复拒绝不报错，也不改变状态或写目标。
        svc.reject(&action, action.setting_proposal_digest())
            .await
            .unwrap();
        assert_eq!(
            store.status(action.setting_proposal_id()),
            SettingProposalStatus::Rejected
        );
        assert_eq!(store.target_writes(), 0);
    }

    #[tokio::test]
    async fn apply_confirmed_delegates_and_returns_the_receipt() {
        let store = FakeStore::new();
        let svc = service(&store);
        let action = create_action(&svc, &snapshot(work_subject(), "rev-0001")).await;

        // 错误 digest 仍由既有服务拒绝，且没有目标写入。
        let error = svc
            .apply_confirmed(&action, &"0".repeat(64), &AgentApprovalToken::issue())
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_DIGEST_MISMATCH");
        assert_eq!(store.target_writes(), 0);
        assert_eq!(
            store.status(action.setting_proposal_id()),
            SettingProposalStatus::Pending
        );

        // 正确 digest 仍必须先签发一次性审批令牌。
        let token = svc
            .issue_approval_token(&action, action.setting_proposal_digest())
            .await
            .unwrap();
        let receipt = svc
            .apply_confirmed(&action, action.setting_proposal_digest(), &token)
            .await
            .unwrap();
        assert_eq!(receipt.proposal_id, action.setting_proposal_id());
        assert_eq!(receipt.proposal_digest, action.setting_proposal_digest());
        assert_eq!(receipt.provenance.source_kind, ProvenanceSourceKind::Agent);
        assert!(receipt.changed);
        assert_eq!(store.target_writes(), 1);
        assert_eq!(
            store.status(action.setting_proposal_id()),
            SettingProposalStatus::Applied
        );

        // 一次性令牌不能重放：Agent 不得退化到普通 Proposal 的回执幂等路径。
        let again = svc
            .apply_confirmed(&action, action.setting_proposal_digest(), &token)
            .await
            .unwrap_err();
        assert_eq!(again.code().as_str(), "AGENT_APPROVAL_TOKEN_INVALID");
        assert_eq!(store.target_writes(), 1);
    }

    #[tokio::test]
    async fn create_enforces_server_capability_policy_before_persistence() {
        let store = FakeStore::new();
        let svc = service(&store);
        let context = snapshot(work_subject(), "rev-0001");

        let mut not_granted = AgentCapabilityManifest::for_current_slice();
        not_granted.capabilities.settings_proposal = false;
        let error = svc
            .create_setting_proposal(
                AgentSettingsActionRequest::new(
                    AgentSessionId::new(),
                    AgentRequestId::new(),
                    &context,
                    SettingTarget::global(SettingsSection::Reading),
                    reading_change(ReadingFontSize::Large),
                )
                .with_capability_manifest(not_granted),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_CAPABILITY_NOT_GRANTED");
        assert!(store.state.lock().unwrap().proposals.is_empty());

        let mut wrong_version = AgentCapabilityManifest::for_current_slice();
        wrong_version.agent_api_version += 1;
        let error = svc
            .create_setting_proposal(
                AgentSettingsActionRequest::new(
                    AgentSessionId::new(),
                    AgentRequestId::new(),
                    &context,
                    SettingTarget::global(SettingsSection::Reading),
                    reading_change(ReadingFontSize::Large),
                )
                .with_capability_manifest(wrong_version),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_API_VERSION_UNSUPPORTED");
        assert!(store.state.lock().unwrap().proposals.is_empty());

        let mut unsupported = AgentCapabilityManifest::for_current_slice();
        unsupported.capabilities.metadata_proposal = true;
        let error = svc
            .create_setting_proposal(
                AgentSettingsActionRequest::new(
                    AgentSessionId::new(),
                    AgentRequestId::new(),
                    &context,
                    SettingTarget::global(SettingsSection::Reading),
                    reading_change(ReadingFontSize::Large),
                )
                .with_capability_manifest(unsupported),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_CAPABILITY_NOT_IMPLEMENTED");
        assert!(store.state.lock().unwrap().proposals.is_empty());
    }

    #[tokio::test]
    async fn create_rechecks_real_work_edition_and_media_item_ownership_before_persistence() {
        let store = FakeStore::new();
        let scope = Arc::new(TestSubjectScope::strict());
        let svc = service_with_scope(&store, scope.clone());
        let work = WorkId::new();
        let edition = EditionId::new();
        let media_item = MediaItemId::new();
        let context = snapshot(
            AgentSubject::edition(work, edition, AgentContentKind::Book),
            "rev-0001",
        );

        // 版本不存在：即使结构上 subject 合法，也不能创建提案。
        scope.add_work(work);
        let error = svc
            .create_setting_proposal(AgentSettingsActionRequest::new(
                AgentSessionId::new(),
                AgentRequestId::new(),
                &context,
                SettingTarget::global(SettingsSection::Reading),
                reading_change(ReadingFontSize::Large),
            ))
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_SUBJECT_EDITION_NOT_FOUND");
        assert!(store.state.lock().unwrap().proposals.is_empty());

        // 版本存在但属于另一部作品。
        scope.add_edition(edition, WorkId::new());
        let error = svc
            .create_setting_proposal(AgentSettingsActionRequest::new(
                AgentSessionId::new(),
                AgentRequestId::new(),
                &context,
                SettingTarget::global(SettingsSection::Reading),
                reading_change(ReadingFontSize::Large),
            ))
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_SUBJECT_SCOPE_MISMATCH");
        assert!(store.state.lock().unwrap().proposals.is_empty());

        // 媒体条目归属也必须落在同一版本上。
        scope.add_edition(edition, work);
        scope.add_media_item(media_item, EditionId::new());
        let media_context = snapshot(
            AgentSubject::media_item(work, edition, media_item, AgentContentKind::Book),
            "rev-0002",
        );
        let error = svc
            .create_setting_proposal(AgentSettingsActionRequest::new(
                AgentSessionId::new(),
                AgentRequestId::new(),
                &media_context,
                SettingTarget::global(SettingsSection::Reading),
                reading_change(ReadingFontSize::Large),
            ))
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_SUBJECT_SCOPE_MISMATCH");
        assert!(store.state.lock().unwrap().proposals.is_empty());
    }

    #[tokio::test]
    async fn expired_apply_delegates_to_transaction_and_reject_can_close_stale_action() {
        let store = FakeStore::new();
        let scope = Arc::new(TestSubjectScope::allow_all());
        let svc = service_with_scope(&store, scope.clone());
        let action = create_action(&svc, &snapshot(work_subject(), "rev-0001")).await;
        let apply_action = svc
            .create_setting_proposal(
                AgentSettingsActionRequest::new(
                    AgentSessionId::new(),
                    AgentRequestId::new(),
                    &snapshot(work_subject(), "rev-0002"),
                    SettingTarget::global(SettingsSection::Reading),
                    reading_change(ReadingFontSize::Large),
                )
                .with_ttl_ms(1_000),
            )
            .await
            .unwrap();
        let apply_token = svc
            .issue_approval_token(&apply_action, apply_action.setting_proposal_digest())
            .await
            .unwrap();
        scope.set_deny(true);

        // 拒绝不执行任何设置写入；即使上下文对应的内容已经被删除，用户仍应能
        // 清理这条待审批提案，而不被 live Subject 复核卡住。
        svc.reject(&action, action.setting_proposal_digest())
            .await
            .unwrap();
        assert_eq!(
            store.status(action.setting_proposal_id()),
            SettingProposalStatus::Rejected
        );

        std::thread::sleep(std::time::Duration::from_millis(1_100));
        let error = svc
            .apply_confirmed(
                &apply_action,
                apply_action.setting_proposal_digest(),
                &apply_token,
            )
            .await
            .unwrap_err();
        // AgentProposalService 不再在底层 expired 分支之前做 live Subject 预检；
        // 即使 Subject 已失效，也必须由同一 UoW 原子收敛 proposal/binding 状态。
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_EXPIRED");
        assert_eq!(
            store.status(apply_action.setting_proposal_id()),
            SettingProposalStatus::Expired
        );
        assert_eq!(store.target_writes(), 0);
    }
}
