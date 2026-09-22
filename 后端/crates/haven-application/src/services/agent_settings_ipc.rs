//! `agent_settings_ipc`：全局设置 Agent 的 Typed Application service（A1 垂直切片）。
//!
//! 它是设置页栖伴与 Tauri 命令层之间的**唯一**契约面：命令层只做
//! typed 请求解析、ID 转换、调用本服务、把 `AppError` 映射成 `ErrorDto`。
//! 这里不出现 SQL、Provider、文件系统或任意 JSON。
//!
//! 本模块负责的三件事：
//! 1. **读取边界**：重新读取 authoritative 全局阅读设置，构造脱敏上下文快照
//!    （revision、context id/hash、服务端固定能力清单）。返回的 DTO 里没有
//!    secret、绝对路径、正文或任意 JSON。
//! 2. **提案边界**：调用方必须回传最近一次读取得到的 `context_id` / `context_hash` /
//!    `base_revision`；本服务**重新读取 authoritative 设置、重建上下文并逐项比对**，
//!    旧快照或 hash 不匹配即 fail-closed（零写入）。canonical digest 只由 Rust 生成。
//! 3. **批准边界**：UI 只提交 Proposal ID 与它显示的那份 canonical digest。
//!    一次性 Approval Token 由单一 UoW 入口在服务内部生成、条件写入、消费并清除；
//!    设置 CAS、回执与 Proposal/Binding 状态迁移和它同生共死。令牌原文既不出现在
//!    返回值里，也不进入 wire / 日志 / provenance。

use std::sync::Arc;

use haven_common::AppError;
use haven_domain::agent::{
    AgentSettingsContextSnapshot, AgentSettingsSubject, redact_setting_value_for_agent,
};
use haven_domain::ids::{AgentRequestId, AgentSessionId, SettingProposalId};
use haven_domain::setting_proposal::{
    SettingChangeReceipt, SettingProposal, SettingProposalChange, SettingTarget,
    is_canonical_digest,
};
use haven_domain::settings::{ReadingSettings, SettingsPatch, SettingsSection, SettingsValue};

use crate::services::agent::{AgentProposalService, AgentSettingsScopeActionRequest};
use crate::services::agent_trace::{AgentEventKind, AgentTracePort, record_best_effort};
use crate::services::setting_proposals::SettingProposalService;
use crate::services::settings::SettingsService;
use crate::wire::{
    AgentSettingChangeDto, AgentSettingChangeReceiptDto, AgentSettingsContextDto,
    AgentSettingsProposalApproveRequest, AgentSettingsProposalApproveResultDto,
    AgentSettingsProposalCreateRequest, AgentSettingsProposalDto, AgentSettingsProposalGetRequest,
    AgentSettingsProposalGetResultDto, AgentSettingsProposalRejectRequest,
    AgentSettingsProposalRejectResultDto, AgentSettingsReadingSnapshotDto,
    AgentSettingsRedactedFieldDto, AgentSettingsSectionDto, AgentSettingsSubjectDto, ErrorDto,
};

/// `agent_settings_context_get` 的响应（typed 领域视图；Tauri 层再投影成 DTO）。
#[derive(Debug, Clone)]
pub struct AgentSettingsContext {
    snapshot: AgentSettingsContextSnapshot,
}

impl AgentSettingsContext {
    pub fn snapshot(&self) -> &AgentSettingsContextSnapshot {
        &self.snapshot
    }

    pub fn context_id(&self) -> haven_domain::ids::AgentContextSnapshotId {
        self.snapshot.id()
    }

    pub fn context_hash(&self) -> &str {
        self.snapshot.context_hash()
    }

    pub fn revision(&self) -> Option<&str> {
        self.snapshot.revision()
    }

    /// 投影成 wire DTO。字段全部来自快照本身，因此 DTO 与 `context_hash` 描述的
    /// 是同一份事实：调用方无法拿到"读起来像 A、哈希是 B"的响应。
    pub fn to_dto(&self) -> AgentSettingsContextDto {
        let reading = self.snapshot.reading();
        AgentSettingsContextDto {
            schema_version: 1,
            context_id: self.snapshot.id().to_string(),
            context_hash: self.snapshot.context_hash().to_owned(),
            subject: AgentSettingsSubjectDto {
                section: AgentSettingsSectionDto::Reading,
            },
            revision: self.snapshot.revision().map(str::to_owned),
            reading: AgentSettingsReadingSnapshotDto {
                section: "reading".to_owned(),
                font_family: wire_enum(&reading.font_family),
                custom_font_family: reading.custom_font_family.clone(),
                font_size: wire_enum(&reading.font_size),
                line_height: wire_enum(&reading.line_height),
                content_width: wire_enum(&reading.content_width),
                theme: wire_enum(&reading.theme),
                custom_background: reading.custom_background.clone(),
                custom_text: reading.custom_text.clone(),
                font_weight: wire_enum(&reading.font_weight),
                letter_spacing: wire_enum(&reading.letter_spacing),
                system_auto: reading.system_auto,
                pagination: wire_enum(&reading.pagination),
                redacted_fields: reading
                    .redacted
                    .redacted_fields
                    .iter()
                    .map(|field| AgentSettingsRedactedFieldDto::from(*field))
                    .collect(),
            },
            capabilities: self.snapshot.capabilities().into(),
        }
    }
}

/// 设置建议的 Typed Application service。
#[derive(Clone)]
pub struct AgentSettingsIpcService {
    proposals: SettingProposalService,
    agent: AgentProposalService,
    settings: SettingsService,
    trace: Option<Arc<dyn AgentTracePort>>,
}

impl AgentSettingsIpcService {
    pub fn new(
        proposals: SettingProposalService,
        agent: AgentProposalService,
        settings: SettingsService,
    ) -> Self {
        Self {
            proposals,
            agent,
            settings,
            trace: None,
        }
    }

    /// 注入共享轨迹 collector。轨迹写入始终 best-effort，不参与业务成功/失败判定。
    pub fn with_trace(mut self, trace: Arc<dyn AgentTracePort>) -> Self {
        self.trace = Some(trace);
        self
    }

    // ---------- Read ----------

    /// 读取全局阅读设置上下文：脱敏快照 + revision + 派生 context id/hash + 能力清单。
    ///
    /// 每次调用都重新读取 authoritative 设置并按当前内容派生身份，因此"设置没变"必然
    /// 得到同一个 id/hash，而任何一次设置写入都会让旧上下文立刻不可用于建提案。
    pub async fn context(&self) -> Result<AgentSettingsContext, AppError> {
        let (revision, reading) = self.read_authoritative().await?;
        let context = build_context(revision, &reading)?;
        Ok(context)
    }

    // ---------- Proposal ----------

    /// 创建全局阅读设置提案。**只创建，不写任何设置。**
    ///
    /// 校验顺序（任何一步失败都零写入）：
    /// 1. 重新读取 authoritative 设置并重建上下文；
    /// 2. 调用方回传的 `context_hash` / `context_id` 必须与重建结果逐字相同；
    /// 3. `base_revision` 必须等于重建上下文里的 revision；
    /// 4. patch 必须真的改动了某些字段（空 patch / 无变化的 patch 不产生提案）。
    ///
    /// 随后交给 `AgentProposalService`，由它在同一个 UoW 内再核对一次 revision 并落库。
    pub async fn create_proposal(
        &self,
        session_id: AgentSessionId,
        request_id: AgentRequestId,
        request: &AgentSettingsProposalCreateRequest,
    ) -> Result<AgentSettingsProposalDto, AppError> {
        record_best_effort(
            self.trace.as_ref(),
            session_id,
            request_id,
            &request.context_id,
            &request.context_hash,
            AgentEventKind::RequestStarted,
        )
        .await;
        let requested_context_id = parse_context_id(&request.context_id)?;
        if !is_canonical_digest(&request.context_hash) {
            return Err(invalid_context_hash());
        }

        let (revision, reading) = self.read_authoritative().await?;
        if revision.as_deref() != request.base_revision.as_deref() {
            return Err(base_revision_mismatch());
        }
        let context = build_context(revision, &reading)?;
        if context.context_hash() != request.context_hash {
            return Err(context_stale());
        }
        if context.context_id() != requested_context_id {
            return Err(context_id_mismatch());
        }
        record_best_effort(
            self.trace.as_ref(),
            session_id,
            request_id,
            &request.context_id,
            &request.context_hash,
            AgentEventKind::ContextLoaded,
        )
        .await;

        let patch = domain_reading_patch(&request.patch);
        let change = SettingProposalChange::SettingsPatch(SettingsPatch::Reading(patch.clone()));
        let target = SettingTarget::global(SettingsSection::Reading);
        if describe_changes(&reading, &patch)?.is_empty() {
            return Err(patch_conflict("提案没有产生任何阅读设置改动"));
        }

        let action = self
            .agent
            .create_settings_proposal(AgentSettingsScopeActionRequest::new(
                session_id,
                request_id,
                &context.snapshot,
                target,
                change,
            ))
            .await?;

        let proposal = self
            .proposals
            .get(action.setting_proposal_id())
            .await?
            .ok_or_else(setting_proposal_not_found)?;
        let projected = self.project_proposal(&proposal, &reading)?;
        record_best_effort(
            self.trace.as_ref(),
            session_id,
            request_id,
            &request.context_id,
            &request.context_hash,
            AgentEventKind::ProposalCreated,
        )
        .await;
        record_best_effort(
            self.trace.as_ref(),
            session_id,
            request_id,
            &request.context_id,
            &request.context_hash,
            AgentEventKind::WaitingForApproval,
        )
        .await;
        Ok(projected)
    }

    /// 回读提案（UI 刷新/重开）。回执未产生时为 `None`。
    pub async fn get_proposal(
        &self,
        request: &AgentSettingsProposalGetRequest,
    ) -> Result<AgentSettingsProposalGetResultDto, AppError> {
        let proposal_id = parse_proposal_id(&request.proposal_id)?;
        let proposal = self
            .proposals
            .get(proposal_id)
            .await?
            .ok_or_else(setting_proposal_not_found)?;
        ensure_global_reading_proposal(&proposal)?;
        let receipt = self.proposals.get_receipt(proposal_id).await?;
        let (_, reading) = self.read_authoritative().await?;
        // 已应用提案的当前值就是 after；用它重新投影会把 Diff 压成空列表。
        // Receipt 的 before 是同一 UoW 写入的审计事实，因此优先用它重建提案 Diff。
        let proposal_before = receipt
            .as_ref()
            .map(|receipt| receipt_reading(&receipt.before_canonical_json, "回执改前值无法解析"))
            .transpose()?
            .unwrap_or_else(|| reading.clone());
        let projected_receipt = receipt
            .as_ref()
            .map(|receipt| self.project_receipt(receipt, &proposal, &reading))
            .transpose()?;
        Ok(AgentSettingsProposalGetResultDto {
            proposal: self.project_proposal(&proposal, &proposal_before)?,
            receipt: projected_receipt,
        })
    }

    /// 拒绝提案：必须携带 UI 显示的那份 digest，零设置写入。
    pub async fn reject_proposal(
        &self,
        request: &AgentSettingsProposalRejectRequest,
    ) -> Result<AgentSettingsProposalRejectResultDto, AppError> {
        let proposal_id = parse_proposal_id(&request.proposal_id)?;
        let stored_proposal = self
            .proposals
            .get(proposal_id)
            .await?
            .ok_or_else(setting_proposal_not_found)?;
        ensure_global_reading_proposal(&stored_proposal)?;
        let binding = self.agent.load_binding(proposal_id).await.ok().flatten();
        self.proposals
            .reject(proposal_id, &request.expected_digest)
            .await?;
        let proposal = self
            .proposals
            .get(proposal_id)
            .await?
            .ok_or_else(setting_proposal_not_found)?;
        let (_, reading) = self.read_authoritative().await?;
        if let Some(binding) = binding {
            let context_id = binding.context_snapshot_id().to_string();
            record_best_effort(
                self.trace.as_ref(),
                binding.session_id(),
                binding.request_id(),
                &context_id,
                binding.context_hash(),
                AgentEventKind::ApprovalRejected,
            )
            .await;
        }
        Ok(AgentSettingsProposalRejectResultDto {
            proposal: self.project_proposal(&proposal, &reading)?,
        })
    }

    // ---------- Approval / Receipt ----------

    /// 用户批准：唯一写入口。调用方只提交 Proposal ID 与 UI 显示的 canonical digest。
    ///
    /// 一次性 Approval Token 在这里生成并在同一 UoW 内被消费；它不会出现在返回值、
    /// 日志或任何 DTO 中。任何校验失败（digest 不匹配、上下文过期、revision 冲突、
    /// 已拒绝/已过期、令牌失效或重放）都不会写 Settings。
    pub async fn approve_proposal(
        &self,
        request: &AgentSettingsProposalApproveRequest,
    ) -> Result<AgentSettingsProposalApproveResultDto, AppError> {
        let proposal_id = parse_proposal_id(&request.proposal_id)?;
        let binding = self.agent.load_binding(proposal_id).await.ok().flatten();
        if let Some(binding) = binding.as_ref() {
            let context_id = binding.context_snapshot_id().to_string();
            record_best_effort(
                self.trace.as_ref(),
                binding.session_id(),
                binding.request_id(),
                &context_id,
                binding.context_hash(),
                AgentEventKind::CasStarted,
            )
            .await;
        }
        // 令牌只在这个 UoW 的闭包里存在一次：签发、摘要落库、CAS、条件消费、
        // 回执与状态迁移全部同生共死；明文不出 Application service 边界。
        let receipt = self
            .agent
            .approve_agent_settings_proposal_in_one_uow(
                proposal_id,
                &request.expected_digest,
                AgentSettingsSubject::reading(),
            )
            .await?;

        let proposal = self
            .proposals
            .get(proposal_id)
            .await?
            .ok_or_else(setting_proposal_not_found)?;
        let before = receipt_reading(&receipt.before_canonical_json, "回执改前值无法解析")?;
        if let Some(binding) = binding {
            let context_id = binding.context_snapshot_id().to_string();
            record_best_effort(
                self.trace.as_ref(),
                binding.session_id(),
                binding.request_id(),
                &context_id,
                binding.context_hash(),
                AgentEventKind::Applied,
            )
            .await;
            record_best_effort(
                self.trace.as_ref(),
                binding.session_id(),
                binding.request_id(),
                &context_id,
                binding.context_hash(),
                AgentEventKind::ReceiptCreated,
            )
            .await;
        }
        Ok(AgentSettingsProposalApproveResultDto {
            proposal: self.project_proposal(&proposal, &before)?,
            receipt: self.project_receipt(&receipt, &proposal, &before)?,
        })
    }

    /// 读取变更回执（未应用返回 `None`，不伪造"已应用"结论）。
    pub async fn receipt(
        &self,
        proposal_id: SettingProposalId,
    ) -> Result<Option<AgentSettingChangeReceiptDto>, AppError> {
        let proposal = self
            .proposals
            .get(proposal_id)
            .await?
            .ok_or_else(setting_proposal_not_found)?;
        ensure_global_reading_proposal(&proposal)?;
        let (_, reading) = self.read_authoritative().await?;
        let Some(receipt) = self.proposals.get_receipt(proposal_id).await? else {
            return Ok(None);
        };
        self.project_receipt(&receipt, &proposal, &reading)
            .map(Some)
    }

    // ---------- 内部 ----------

    /// 读取 authoritative 全局阅读设置：`(revision, 值)`。
    ///
    /// `revision` 为 `None` 表示该分区从未保存过（分区默认值），这与"有版本"是两种
    /// 不同事实，都会进入上下文哈希。
    async fn read_authoritative(&self) -> Result<(Option<String>, ReadingSettings), AppError> {
        let snapshot = self.settings.get(SettingsSection::Reading).await?;
        let reading = match snapshot.value {
            SettingsValue::Reading(reading) => reading,
            other => {
                return Err(AppError::new(
                    "AGENT_SETTINGS_CONTEXT_INVALID",
                    haven_common::ErrorKind::Internal,
                    format!("设置分区与读取请求不一致：{}", other.section().as_str()),
                    false,
                ));
            }
        };
        Ok((snapshot.revision, reading))
    }

    fn project_proposal(
        &self,
        proposal: &SettingProposal,
        current: &ReadingSettings,
    ) -> Result<AgentSettingsProposalDto, AppError> {
        let (patch, changes) = proposal_reading_patch(proposal, current)?;
        let _ = patch;
        Ok(AgentSettingsProposalDto {
            schema_version: 1,
            proposal_id: proposal.id().to_string(),
            status: proposal.status().into(),
            subject: AgentSettingsSubjectDto {
                section: AgentSettingsSectionDto::Reading,
            },
            target_label: "全局默认 · 阅读".to_owned(),
            base_revision: proposal.base_revision().map(str::to_owned),
            digest: proposal.digest().to_owned(),
            created_at: format_millis(proposal.created_at()),
            expires_at: format_millis(proposal.expires_at()),
            changes,
        })
    }

    fn project_receipt(
        &self,
        receipt: &SettingChangeReceipt,
        proposal: &SettingProposal,
        current: &ReadingSettings,
    ) -> Result<AgentSettingChangeReceiptDto, AppError> {
        // 回执的改动列表必须来自回执自己的 before/after（它是审计事实），
        // 而不是"现在读到的设置"——批准之后两者恰好相反，用"当前值"会得到一个空列表。
        let _ = current;
        let _ = proposal;
        let before = receipt_reading(&receipt.before_canonical_json, "回执改前值无法解析")?;
        let after = receipt_reading(&receipt.after_canonical_json, "回执改后值无法解析")?;
        let changes = describe_change_list(&before, &after)?;
        Ok(AgentSettingChangeReceiptDto {
            schema_version: 1,
            receipt_id: receipt.id.to_string(),
            proposal_id: receipt.proposal_id.to_string(),
            proposal_digest: receipt.proposal_digest.clone(),
            status: proposal.status().into(),
            applied_revision: receipt.applied_revision.clone(),
            changed: receipt.changed,
            changes,
            applied_at: format_millis(receipt.applied_at),
        })
    }
}

// ---------- 投影辅助 ----------

/// 由 authoritative `revision` 与当前阅读设置构造上下文；失败即领域校验拒绝。
fn build_context(
    revision: Option<String>,
    reading: &ReadingSettings,
) -> Result<AgentSettingsContext, AppError> {
    let mut payload = haven_domain::agent::AgentSettingsContextPayload::new(
        AgentSettingsSubject::reading(),
        revision,
    );
    payload.reading = haven_domain::agent::AgentReadingSnapshot::from_settings(reading);
    payload.capabilities = haven_domain::agent::AgentCapabilityManifest::for_current_slice();
    let snapshot = AgentSettingsContextSnapshot::derive(payload)?;
    Ok(AgentSettingsContext { snapshot })
}

/// 从 Wire patch 构造领域 patch（`deny_unknown_fields` 已在反序列化边界挡掉未知字段）。
fn domain_reading_patch(patch: &crate::wire::PreferenceReadingPatchDto) -> ReadingSettingsPatch {
    ReadingSettingsPatch::from(patch.clone())
}

/// 领域 `ReadingPatch` 的别名：避免与 Wire DTO 同名混淆。
type ReadingSettingsPatch = haven_domain::settings::ReadingPatch;

/// 读取提案载荷里的阅读 patch 并计算可展示的改动列表。
fn proposal_reading_patch(
    proposal: &SettingProposal,
    current: &ReadingSettings,
) -> Result<(ReadingSettingsPatch, Vec<AgentSettingChangeDto>), AppError> {
    let SettingProposalChange::SettingsPatch(SettingsPatch::Reading(patch)) = proposal.change()
    else {
        return Err(agent_scope_mismatch("提案载荷不是全局阅读设置 patch"));
    };
    let changes = describe_changes(current, patch)?;
    Ok((patch.clone(), changes))
}

fn ensure_global_reading_proposal(proposal: &SettingProposal) -> Result<(), AppError> {
    match proposal.change() {
        SettingProposalChange::SettingsPatch(SettingsPatch::Reading(_)) => Ok(()),
        _ => Err(agent_scope_mismatch("提案不是全局阅读设置 patch")),
    }
}

/// 逐字段比较"当前值 vs patch 应用后的值"，产出可展示的改动列表。
///
/// 这里复用领域 `SettingsPatch::apply_to` 的合并语义（trim、空串清除），因此展示的
/// after 值就是 Rust 将要写入的值，UI 不需要自己合并。
fn describe_changes(
    current: &ReadingSettings,
    patch: &ReadingSettingsPatch,
) -> Result<Vec<AgentSettingChangeDto>, AppError> {
    let current_value = SettingsValue::Reading(current.clone());
    let merged = SettingsPatch::Reading(patch.clone()).apply_to(&current_value);
    let SettingsValue::Reading(merged) = &merged else {
        return Err(agent_scope_mismatch("合并结果不是阅读设置"));
    };

    let mut changes = Vec::new();
    let mut push = |key: &str, before: String, after: String| {
        if before != after {
            changes.push(AgentSettingChangeDto {
                key: key.to_owned(),
                before,
                after,
            });
        }
    };

    let _ = patch;
    push(
        "reading.fontFamily",
        token(&current.font_family)?,
        token(&merged.font_family)?,
    );
    push(
        "reading.customFontFamily",
        agent_text(current.custom_font_family.as_deref()),
        agent_text(merged.custom_font_family.as_deref()),
    );
    push(
        "reading.fontSize",
        token(&current.font_size)?,
        token(&merged.font_size)?,
    );
    push(
        "reading.lineHeight",
        token(&current.line_height)?,
        token(&merged.line_height)?,
    );
    push(
        "reading.contentWidth",
        token(&current.content_width)?,
        token(&merged.content_width)?,
    );
    push(
        "reading.theme",
        token(&current.theme)?,
        token(&merged.theme)?,
    );
    push(
        "reading.customBackground",
        agent_text(current.custom_background.as_deref()),
        agent_text(merged.custom_background.as_deref()),
    );
    push(
        "reading.customText",
        agent_text(current.custom_text.as_deref()),
        agent_text(merged.custom_text.as_deref()),
    );
    push(
        "reading.fontWeight",
        token(&current.font_weight)?,
        token(&merged.font_weight)?,
    );
    push(
        "reading.letterSpacing",
        token(&current.letter_spacing)?,
        token(&merged.letter_spacing)?,
    );
    push(
        "reading.systemAuto",
        current.system_auto.to_string(),
        merged.system_auto.to_string(),
    );
    push(
        "reading.pagination",
        token(&current.pagination)?,
        token(&merged.pagination)?,
    );
    Ok(changes)
}

/// 回执里的 before/after 是**值**（全局目标是整段 `SettingsValue`），
/// 因此这里按 `SettingsValue` 解析并取出阅读分区。
fn receipt_reading(raw: &str, detail: &'static str) -> Result<ReadingSettings, AppError> {
    match serde_json::from_str::<SettingsValue>(raw)
        .map_err(|_| setting_proposal_integrity(detail))?
    {
        SettingsValue::Reading(reading) => Ok(reading),
        _ => Err(setting_proposal_integrity("回执值不是阅读分区")),
    }
}

/// 两个 authoritative 阅读设置之间的逐字段差异（回执投影使用）。
fn describe_change_list(
    before: &ReadingSettings,
    after: &ReadingSettings,
) -> Result<Vec<AgentSettingChangeDto>, AppError> {
    let mut changes = Vec::new();
    let mut push = |key: &str, before: String, after: String| {
        if before != after {
            changes.push(AgentSettingChangeDto {
                key: key.to_owned(),
                before,
                after,
            });
        }
    };
    push(
        "reading.fontFamily",
        token(&before.font_family)?,
        token(&after.font_family)?,
    );
    push(
        "reading.customFontFamily",
        agent_text(before.custom_font_family.as_deref()),
        agent_text(after.custom_font_family.as_deref()),
    );
    push(
        "reading.fontSize",
        token(&before.font_size)?,
        token(&after.font_size)?,
    );
    push(
        "reading.lineHeight",
        token(&before.line_height)?,
        token(&after.line_height)?,
    );
    push(
        "reading.contentWidth",
        token(&before.content_width)?,
        token(&after.content_width)?,
    );
    push("reading.theme", token(&before.theme)?, token(&after.theme)?);
    push(
        "reading.customBackground",
        agent_text(before.custom_background.as_deref()),
        agent_text(after.custom_background.as_deref()),
    );
    push(
        "reading.customText",
        agent_text(before.custom_text.as_deref()),
        agent_text(after.custom_text.as_deref()),
    );
    push(
        "reading.fontWeight",
        token(&before.font_weight)?,
        token(&after.font_weight)?,
    );
    push(
        "reading.letterSpacing",
        token(&before.letter_spacing)?,
        token(&after.letter_spacing)?,
    );
    push(
        "reading.systemAuto",
        before.system_auto.to_string(),
        after.system_auto.to_string(),
    );
    push(
        "reading.pagination",
        token(&before.pagination)?,
        token(&after.pagination)?,
    );
    Ok(changes)
}

/// 领域枚举 → Wire 值字符串（与该枚举自身的 serde 表示逐字一致）。
fn token<T: serde::Serialize>(value: &T) -> Result<String, AppError> {
    match serde_json::to_value(value).map_err(|_| setting_proposal_integrity("枚举无法序列化"))?
    {
        serde_json::Value::String(text) => Ok(text),
        _ => Err(setting_proposal_integrity("枚举不是字符串表示")),
    }
}

fn agent_text(value: Option<&str>) -> String {
    value
        .map(redact_setting_value_for_agent)
        .unwrap_or_default()
        .to_owned()
}

/// 领域枚举 → Wire 枚举：两边共用同一个 `snake_case` 字符串表示，
/// 因此这里走 serde 表示而不是手写第二张映射表（新增档位不会静默漏掉）。
///
/// 这是**契约不变量**：快照里保存的就是同一批字符串。字面量测不准、
/// 序列化不出去都属于"这个值根本不存在于闭合集合里"，是编程错误而不是用户输入错误。
fn wire_enum<W, D>(value: &D) -> W
where
    W: serde::de::DeserializeOwned,
    D: serde::Serialize + ?Sized,
{
    let token = serde_json::to_value(value).unwrap_or(serde_json::Value::Null);
    serde_json::from_value(token)
        .unwrap_or_else(|_| unreachable!("领域枚举与 Wire 枚举的字符串表示必须一一对应"))
}

fn format_millis(value: haven_common::UtcMillis) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(value.0)
        .map(|at| at.to_rfc3339())
        .unwrap_or_else(|| value.0.to_string())
}

fn parse_proposal_id(raw: &str) -> Result<SettingProposalId, AppError> {
    raw.parse::<SettingProposalId>()
        .map_err(|_| invalid_argument("提案 ID 非法"))
}

fn parse_context_id(raw: &str) -> Result<haven_domain::ids::AgentContextSnapshotId, AppError> {
    raw.parse::<haven_domain::ids::AgentContextSnapshotId>()
        .map_err(|_| invalid_argument("上下文 ID 非法"))
}

/// 供 Tauri 层把 `AppError` 映射成 `ErrorDto`；本模块只产生稳定错误码。
pub fn to_error_dto(error: &AppError) -> ErrorDto {
    ErrorDto {
        code: error.code().as_str().to_owned(),
        user_message: error.user_message().to_owned(),
        retryable: error.retryable(),
    }
}

// ---------- 稳定错误 ----------

fn invalid_argument(detail: &'static str) -> AppError {
    AppError::new(
        "INVALID_ARGUMENT",
        haven_common::ErrorKind::Validation,
        detail,
        false,
    )
}

fn invalid_context_hash() -> AppError {
    AppError::new(
        "AGENT_SETTINGS_CONTEXT_HASH_INVALID",
        haven_common::ErrorKind::Validation,
        "上下文 hash 不是闭合的 SHA-256 小写十六进制",
        false,
    )
}

fn context_stale() -> AppError {
    AppError::new(
        "AGENT_SETTINGS_CONTEXT_STALE",
        haven_common::ErrorKind::Conflict,
        "设置上下文已过期，请重新读取设置后再生成提案",
        false,
    )
}

fn context_id_mismatch() -> AppError {
    AppError::new(
        "AGENT_SETTINGS_CONTEXT_ID_MISMATCH",
        haven_common::ErrorKind::Conflict,
        "设置上下文 ID 与当前设置不一致，请重新读取设置",
        false,
    )
}

fn base_revision_mismatch() -> AppError {
    AppError::new(
        "AGENT_SETTINGS_BASE_REVISION_MISMATCH",
        haven_common::ErrorKind::Conflict,
        "设置版本在生成提案前已变化，请重新读取设置",
        false,
    )
}

fn patch_conflict(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_SETTINGS_PATCH_CONFLICT",
        haven_common::ErrorKind::Validation,
        detail,
        false,
    )
}

fn agent_scope_mismatch(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_SETTINGS_SCOPE_MISMATCH",
        haven_common::ErrorKind::Forbidden,
        format!("Agent 设置范围校验失败：{detail}"),
        false,
    )
}

fn setting_proposal_not_found() -> AppError {
    haven_domain::setting_proposal::setting_proposal_not_found_error()
}

fn setting_proposal_integrity(detail: &'static str) -> AppError {
    haven_common::AppError::new(
        "SETTING_PROPOSAL_INTEGRITY_MISMATCH",
        haven_common::ErrorKind::Internal,
        detail,
        false,
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use haven_common::{ErrorKind, UtcMillis};
    use haven_domain::agent::{AgentActionBinding, AgentApprovalState, AgentCapabilitySet};
    use haven_domain::contracts::{
        AgentActionBindingRepository, EditionPreference, MediaItemPreference,
        SettingProposalRepository, SettingsRow,
    };
    use haven_domain::ids::{AgentContextSnapshotId, EditionId, MediaItemId};
    use haven_domain::setting_proposal::{SettingProposalStatus, canonical_json_of};
    use haven_domain::settings::{ReadingFontSize, ReadingPatch};
    use serde_json::json;

    use crate::services::agent::{AgentProposalService, AgentSubjectScopePort};
    use crate::services::setting_proposals::{
        SettingProposalService, SettingProposalTxPorts, SettingProposalUoW,
    };
    use crate::services::settings::{SettingsService, SettingsTxPorts, SettingsUoW};
    use crate::wire::AgentSettingsProposalStatusDto;

    // ---------- 纯内存假实现（无 SQL） ----------

    #[derive(Clone, Default)]
    struct FakeState {
        proposals: HashMap<SettingProposalId, SettingProposal>,
        bindings: HashMap<SettingProposalId, AgentActionBinding>,
        receipts: HashMap<SettingProposalId, SettingChangeReceipt>,
        settings: HashMap<String, SettingsRow>,
        /// 目标写入次数：拒绝/冲突/认证失败必须保持为 0。
        target_writes: usize,
        recovery: usize,
    }

    pub(crate) struct FakeStore {
        state: Mutex<FakeState>,
    }

    impl FakeStore {
        pub(crate) fn new() -> Arc<Self> {
            Arc::new(Self {
                state: Mutex::new(FakeState::default()),
            })
        }

        pub(crate) fn target_writes(&self) -> usize {
            self.state.lock().unwrap().target_writes
        }

        pub(crate) fn proposal_count(&self) -> usize {
            self.state.lock().unwrap().proposals.len()
        }

        fn stored_reading(&self) -> ReadingSettings {
            let state = self.state.lock().unwrap();
            let raw = &state.settings.get("reading").unwrap().data_json;
            match serde_json::from_str::<SettingsValue>(raw).unwrap() {
                SettingsValue::Reading(reading) => reading,
                other => panic!("分区不是阅读设置：{}", other.section().as_str()),
            }
        }

        pub(crate) fn revision(&self) -> Option<String> {
            self.state
                .lock()
                .unwrap()
                .settings
                .get("reading")
                .map(|row| row.revision.clone())
        }

        pub(crate) fn write_reading_for_test(&self, patch: ReadingPatch) -> String {
            self.write_reading(patch)
        }

        /// 模拟"别处改了设置"：值变化 + 新 revision。
        fn write_reading(&self, patch: ReadingPatch) -> String {
            let mut state = self.state.lock().unwrap();
            let raw = &state.settings.get("reading").unwrap().data_json;
            let current = match serde_json::from_str::<SettingsValue>(raw).unwrap() {
                SettingsValue::Reading(reading) => reading,
                other => panic!("分区不是阅读设置：{}", other.section().as_str()),
            };
            let merged = SettingsPatch::Reading(patch).apply_to(&SettingsValue::Reading(current));
            let revision = format!("rev-{}", state.recovery);
            state.recovery += 1;
            let data_json = canonical_json_of(&merged).unwrap();
            state.settings.insert(
                "reading".to_owned(),
                SettingsRow {
                    section: "reading".to_owned(),
                    schema_version: 1,
                    revision: revision.clone(),
                    data_json,
                    updated_at: UtcMillis::now(),
                },
            );
            revision
        }
    }

    pub(crate) fn seed_reading(store: &Arc<FakeStore>, revision: Option<&str>) -> Option<String> {
        let mut state = store.state.lock().unwrap();
        match revision {
            Some(revision) => {
                state.settings.insert(
                    "reading".to_owned(),
                    SettingsRow {
                        section: "reading".to_owned(),
                        schema_version: 1,
                        revision: revision.to_owned(),
                        data_json: canonical_json_of(&SettingsValue::Reading(
                            ReadingSettings::default(),
                        ))
                        .unwrap(),
                        updated_at: UtcMillis::now(),
                    },
                );
                Some(revision.to_owned())
            }
            None => None,
        }
    }

    #[async_trait::async_trait]
    impl SettingProposalRepository for FakeStore {
        async fn create(&self, proposal: &SettingProposal) -> Result<(), AppError> {
            let mut state = self.state.lock().unwrap();
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
        ) -> Result<Option<AgentActionBinding>, AppError> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .bindings
                .get(&proposal_id)
                .cloned())
        }
    }

    struct AllowAllScope;

    #[async_trait::async_trait]
    impl AgentSubjectScopePort for AllowAllScope {
        async fn verify_subject(
            &self,
            subject: haven_domain::agent::AgentSubject,
        ) -> Result<(), AppError> {
            subject.validate()
        }
    }

    impl SettingProposalUoW for FakeStore {
        fn run(
            &self,
            f: &dyn Fn(&dyn SettingProposalTxPorts) -> Result<(), AppError>,
        ) -> Result<(), AppError> {
            // 与真实 SQLite UoW 保持一致：任何 Err 都必须撤销闭包内已经发生的
            // token 摘要、目标、回执和状态中间写入。
            let snapshot = self.state.lock().unwrap().clone();
            let result = f(&FakeTx { store: self });
            if result.is_err() {
                *self.state.lock().unwrap() = snapshot;
            }
            result
        }
    }

    impl SettingsUoW for FakeStore {
        fn run(
            &self,
            f: &dyn Fn(&dyn SettingsTxPorts) -> Result<(), AppError>,
        ) -> Result<(), AppError> {
            f(&FakeTx { store: self })
        }

        fn run_read(
            &self,
            f: &dyn Fn(&dyn SettingsTxPorts) -> Result<(), AppError>,
        ) -> Result<(), AppError> {
            f(&FakeTx { store: self })
        }
    }

    struct FakeTx<'a> {
        store: &'a FakeStore,
    }

    impl SettingsTxPorts for FakeTx<'_> {
        fn load(&self, section: &str) -> Result<Option<SettingsRow>, AppError> {
            Ok(self
                .store
                .state
                .lock()
                .unwrap()
                .settings
                .get(section)
                .cloned())
        }

        fn cas_write(
            &self,
            section: &str,
            expected_revision: Option<&str>,
            row: &SettingsRow,
        ) -> Result<bool, AppError> {
            let mut state = self.store.state.lock().unwrap();
            let current = state.settings.get(section).map(|row| row.revision.clone());
            if current.as_deref() != expected_revision {
                return Ok(false);
            }
            state.target_writes += 1;
            state.settings.insert(section.to_owned(), row.clone());
            Ok(true)
        }
    }

    impl SettingProposalTxPorts for FakeTx<'_> {
        fn insert_proposal(&self, proposal: &SettingProposal) -> Result<(), AppError> {
            self.store
                .state
                .lock()
                .unwrap()
                .proposals
                .insert(proposal.id(), proposal.clone());
            Ok(())
        }

        fn insert_agent_action_binding(
            &self,
            binding: &AgentActionBinding,
        ) -> Result<(), AppError> {
            self.store
                .state
                .lock()
                .unwrap()
                .bindings
                .insert(binding.proposal_id(), binding.clone());
            Ok(())
        }

        fn load_agent_action_binding(
            &self,
            proposal_id: SettingProposalId,
        ) -> Result<Option<AgentActionBinding>, AppError> {
            Ok(self
                .store
                .state
                .lock()
                .unwrap()
                .bindings
                .get(&proposal_id)
                .cloned())
        }

        fn verify_agent_subject(
            &self,
            subject: haven_domain::agent::AgentScopeSubject,
        ) -> Result<(), AppError> {
            subject.validate()
        }

        fn verify_settings_base_revision(
            &self,
            section: &str,
            expected_revision: Option<&str>,
        ) -> Result<bool, AppError> {
            let state = self.store.state.lock().unwrap();
            Ok(state.settings.get(section).map(|row| row.revision.as_str()) == expected_revision)
        }

        fn mark_rejected(
            &self,
            id: SettingProposalId,
            expected_digest: &str,
            _rejected_at: UtcMillis,
        ) -> Result<bool, AppError> {
            let mut state = self.store.state.lock().unwrap();
            let Some(proposal) = state.proposals.get(&id).cloned() else {
                return Ok(false);
            };
            if proposal.status() != SettingProposalStatus::Pending
                || proposal.digest() != expected_digest
            {
                return Ok(false);
            }
            let next = SettingProposal::from_stored(
                id,
                proposal.canonical_json().to_owned(),
                proposal.digest().to_owned(),
                SettingProposalStatus::Rejected,
                proposal.created_at(),
            )?;
            state.proposals.insert(id, next);
            Ok(true)
        }

        fn update_agent_action_binding_state(
            &self,
            proposal_id: SettingProposalId,
            expected: AgentApprovalState,
            next: AgentApprovalState,
        ) -> Result<(), AppError> {
            let mut state = self.store.state.lock().unwrap();
            let Some(binding) = state.bindings.get(&proposal_id).cloned() else {
                return Ok(());
            };
            if binding.approval_state() != expected {
                return Err(AppError::new(
                    "AGENT_ACTION_BINDING_INTEGRITY",
                    ErrorKind::Internal,
                    "绑定状态与预期不一致",
                    false,
                ));
            }
            let updated = binding.with_approval_state(next)?;
            state.bindings.insert(proposal_id, updated);
            Ok(())
        }

        fn load_proposal(
            &self,
            id: SettingProposalId,
        ) -> Result<Option<SettingProposal>, AppError> {
            Ok(self.store.state.lock().unwrap().proposals.get(&id).cloned())
        }

        fn load_receipt(
            &self,
            id: SettingProposalId,
        ) -> Result<Option<SettingChangeReceipt>, AppError> {
            Ok(self.store.state.lock().unwrap().receipts.get(&id).cloned())
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
            Ok(self
                .store
                .state
                .lock()
                .unwrap()
                .settings
                .get(section)
                .cloned())
        }

        fn cas_write_settings(
            &self,
            section: &str,
            expected_revision: Option<&str>,
            row: &SettingsRow,
        ) -> Result<bool, AppError> {
            let mut state = self.store.state.lock().unwrap();
            let current = state.settings.get(section).map(|row| row.revision.clone());
            if current.as_deref() != expected_revision {
                return Ok(false);
            }
            state.target_writes += 1;
            state.settings.insert(section.to_owned(), row.clone());
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
            self.store
                .state
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
            let mut state = self.store.state.lock().unwrap();
            let Some(proposal) = state.proposals.get(&id).cloned() else {
                return Ok(false);
            };
            if proposal.status() != SettingProposalStatus::Pending
                || proposal.digest() != expected_digest
            {
                return Ok(false);
            }
            let next = SettingProposal::from_stored(
                id,
                proposal.canonical_json().to_owned(),
                proposal.digest().to_owned(),
                SettingProposalStatus::Applied,
                proposal.created_at(),
            )?;
            state.proposals.insert(id, next);
            Ok(true)
        }

        fn mark_expired(
            &self,
            id: SettingProposalId,
            expected_digest: &str,
            _expired_at: UtcMillis,
        ) -> Result<(), AppError> {
            let mut state = self.store.state.lock().unwrap();
            let Some(proposal) = state.proposals.get(&id).cloned() else {
                return Ok(());
            };
            if proposal.digest() != expected_digest {
                return Ok(());
            }
            if let Ok(next) = SettingProposal::from_stored(
                id,
                proposal.canonical_json().to_owned(),
                proposal.digest().to_owned(),
                SettingProposalStatus::Expired,
                proposal.created_at(),
            ) {
                state.proposals.insert(id, next);
            }
            Ok(())
        }

        fn issue_agent_action_approval_token(
            &self,
            id: SettingProposalId,
            expected_digest: &str,
            approval_token_hash: &haven_domain::agent::AgentApprovalTokenHash,
            issued_at: UtcMillis,
        ) -> Result<bool, AppError> {
            let mut state = self.store.state.lock().unwrap();
            let Some(proposal) = state.proposals.get(&id).cloned() else {
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
            let updated = binding.issue_approval_token(approval_token_hash.clone(), issued_at)?;
            state.bindings.insert(id, updated);
            Ok(true)
        }

        fn consume_agent_action_approval_token(
            &self,
            id: SettingProposalId,
            expected_digest: &str,
            approval_token_hash: &haven_domain::agent::AgentApprovalTokenHash,
            _consumed_at: UtcMillis,
        ) -> Result<bool, AppError> {
            let mut state = self.store.state.lock().unwrap();
            let Some(binding) = state.bindings.get(&id).cloned() else {
                return Ok(false);
            };
            if binding.approval_token_hash().map(|hash| hash.as_str())
                != Some(approval_token_hash.as_str())
            {
                return Ok(false);
            }
            let proposal_ok = state
                .proposals
                .get(&id)
                .map(|proposal| {
                    proposal.status() == SettingProposalStatus::Pending
                        && proposal.digest() == expected_digest
                })
                .unwrap_or(false);
            if !proposal_ok {
                return Ok(false);
            }
            // 消费后摘要必须被清除：重放同一枚令牌在同一事务语义下必然失败。
            let cleared = AgentActionBinding::from_stored_with_token(
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
                None,
                None,
            )?;
            state.bindings.insert(id, cleared);
            Ok(true)
        }
    }

    // ---------- 组装 ----------

    pub(crate) fn service(store: &Arc<FakeStore>) -> AgentSettingsIpcService {
        let proposals = SettingProposalService::new(store.clone(), store.clone());
        let settings = SettingsService::new(store.clone());
        let agent =
            AgentProposalService::new(proposals.clone(), Arc::new(AllowAllScope), store.clone())
                .with_settings_service(settings.clone());
        AgentSettingsIpcService::new(proposals, agent, settings)
    }

    fn patch(font_size: ReadingFontSize) -> crate::wire::PreferenceReadingPatchDto {
        crate::wire::PreferenceReadingPatchDto {
            font_size: Some(font_size.into()),
            ..crate::wire::PreferenceReadingPatchDto::default()
        }
    }

    fn create_request(
        context: &AgentSettingsContext,
        patch: crate::wire::PreferenceReadingPatchDto,
    ) -> crate::wire::AgentSettingsProposalCreateRequest {
        crate::wire::AgentSettingsProposalCreateRequest {
            session_id: AgentSessionId::new().to_string(),
            request_id: AgentRequestId::new().to_string(),
            context_id: context.context_id().to_string(),
            context_hash: context.context_hash().to_owned(),
            base_revision: context.revision().map(str::to_owned),
            patch,
        }
    }

    fn ids() -> (AgentSessionId, AgentRequestId) {
        (AgentSessionId::new(), AgentRequestId::new())
    }

    // ---------- 读取边界 ----------

    #[tokio::test]
    async fn context_is_redacted_and_has_no_secrets_or_paths() {
        let store = FakeStore::new();
        seed_reading(&store, Some("rev-0001"));
        store.write_reading(ReadingPatch {
            custom_font_family: Some("C:/Windows/Fonts/msyh.ttc".into()),
            custom_background: Some("#f7f1e3".into()),
            ..ReadingPatch::default()
        });
        let service = service(&store);

        let context = service.context().await.unwrap();
        let dto = context.to_dto();
        assert_eq!(dto.schema_version, 1);
        assert_eq!(dto.subject.section, AgentSettingsSectionDto::Reading);
        assert_eq!(dto.revision, store.revision());
        // 敏感字段被清空并登记；安全的自由文本保留。
        assert!(dto.reading.custom_font_family.is_none());
        assert_eq!(dto.reading.custom_background.as_deref(), Some("#f7f1e3"));
        assert_eq!(
            dto.reading.redacted_fields,
            vec![AgentSettingsRedactedFieldDto::CustomFontFamily]
        );
        // 服务端固定能力清单：只声明已实现能力。
        assert!(dto.capabilities.capabilities.settings_read);
        assert!(dto.capabilities.capabilities.settings_proposal);
        assert!(!dto.capabilities.capabilities.secret_read);
        assert!(!dto.capabilities.capabilities.filesystem_write);

        let raw = serde_json::to_string(&dto).unwrap();
        for forbidden in [
            "C:/Windows",
            "msyh.ttc",
            "apiKey",
            "api_key",
            "token",
            "password",
        ] {
            assert!(
                !raw.to_ascii_lowercase()
                    .contains(&forbidden.to_ascii_lowercase()),
                "上下文响应不得包含 {forbidden}"
            );
        }
        // 身份与摘要自洽：context id 是 context hash 的派生身份。
        assert_eq!(
            dto.context_id,
            haven_domain::agent::derive_settings_context_id(&dto.context_hash).to_string()
        );
        assert!(is_canonical_digest(&dto.context_hash));

        // "设置没变" → 同一个 id/hash。
        let again = service.context().await.unwrap().to_dto();
        assert_eq!(again.context_id, dto.context_id);
        assert_eq!(again.context_hash, dto.context_hash);
    }

    #[tokio::test]
    async fn unsaved_section_still_yields_a_context_with_null_revision() {
        let store = FakeStore::new();
        let service = service(&store);
        let context = service.context().await.unwrap().to_dto();
        assert!(context.revision.is_none());
        assert!(is_canonical_digest(&context.context_hash));
    }

    // ---------- 提案边界 ----------

    #[tokio::test]
    async fn proposal_creation_never_writes_settings_and_digest_comes_from_rust() {
        let store = FakeStore::new();
        seed_reading(&store, Some("rev-0001"));
        let service = service(&store);

        let context = service.context().await.unwrap();
        let (session_id, request_id) = ids();
        let request = create_request(&context, patch(ReadingFontSize::Large));
        let proposal = service
            .create_proposal(session_id, request_id, &request)
            .await
            .unwrap();

        assert_eq!(proposal.status, AgentSettingsProposalStatusDto::Pending);
        assert!(is_canonical_digest(&proposal.digest));
        assert_eq!(proposal.base_revision.as_deref(), Some("rev-0001"));
        assert_eq!(proposal.subject.section, AgentSettingsSectionDto::Reading);
        assert_eq!(proposal.changes.len(), 1);
        assert_eq!(proposal.changes[0].key, "reading.fontSize");
        assert_eq!(proposal.changes[0].before, "medium");
        assert_eq!(proposal.changes[0].after, "large");
        // 创建提案不写任何设置。
        assert_eq!(store.target_writes(), 0);
        assert_eq!(store.revision().as_deref(), Some("rev-0001"));
        // 未批准 → 没有回执。
        assert!(
            service
                .receipt(proposal.proposal_id.parse().unwrap())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn proposal_creation_fails_closed_on_stale_context_and_bad_revision() {
        let store = FakeStore::new();
        seed_reading(&store, Some("rev-0001"));
        let service = service(&store);
        let context = service.context().await.unwrap();
        let (session_id, request_id) = ids();

        // 上下文 hash 对不上：零写入。
        let mut request = create_request(&context, patch(ReadingFontSize::Large));
        request.context_hash = "c".repeat(64);
        let error = service
            .create_proposal(session_id, request_id, &request)
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_SETTINGS_CONTEXT_STALE");
        assert_eq!(store.proposal_count(), 0);
        assert_eq!(store.target_writes(), 0);

        // hash 形状非法同样拒绝。
        let mut request = create_request(&context, patch(ReadingFontSize::Large));
        request.context_hash = "NOT-A-DIGEST".into();
        assert_eq!(
            service
                .create_proposal(session_id, request_id, &request)
                .await
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_SETTINGS_CONTEXT_HASH_INVALID"
        );

        // 提交**正确**的 hash 但伪造的 context id：仍然 fail-closed。
        let mut request = create_request(&context, patch(ReadingFontSize::Large));
        request.context_id = AgentContextSnapshotId::new().to_string();
        assert_eq!(
            service
                .create_proposal(session_id, request_id, &request)
                .await
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_SETTINGS_CONTEXT_ID_MISMATCH"
        );

        // 设置已经被别处改过：旧上下文整体过期（hash 与 revision 都对不上）。
        store.write_reading(ReadingPatch {
            line_height: Some(haven_domain::settings::ReadingLineHeight::Airy),
            ..ReadingPatch::default()
        });
        let stale = create_request(&context, patch(ReadingFontSize::Large));
        let error = service
            .create_proposal(session_id, request_id, &stale)
            .await
            .unwrap_err();
        assert_eq!(
            error.code().as_str(),
            "AGENT_SETTINGS_BASE_REVISION_MISMATCH"
        );
        assert_eq!(store.proposal_count(), 0);
        assert_eq!(store.target_writes(), 0);

        // 没有真实改动的 patch 不产生提案。
        let fresh = service.context().await.unwrap();
        let noop = create_request(
            &fresh,
            crate::wire::PreferenceReadingPatchDto {
                font_size: Some(ReadingFontSize::Medium.into()),
                ..crate::wire::PreferenceReadingPatchDto::default()
            },
        );
        assert_eq!(
            service
                .create_proposal(session_id, request_id, &noop)
                .await
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_SETTINGS_PATCH_CONFLICT"
        );
        assert_eq!(store.proposal_count(), 0);
    }

    // ---------- 批准 / 回执 ----------

    /// 走完一次真实闭环：Context → Proposal → 用户批准 → Receipt → 设置写入。
    async fn run_full_flow(
        store: &Arc<FakeStore>,
    ) -> (AgentSettingsIpcService, AgentSettingsProposalDto) {
        let service = service(store);
        let context = service.context().await.unwrap();
        let (session_id, request_id) = ids();
        let request = create_request(&context, patch(ReadingFontSize::Large));
        let proposal = service
            .create_proposal(session_id, request_id, &request)
            .await
            .unwrap();
        (service, proposal)
    }

    #[tokio::test]
    async fn approval_writes_settings_once_and_projects_receipt_without_tokens() {
        let store = FakeStore::new();
        seed_reading(&store, Some("rev-0001"));
        let (service, proposal) = run_full_flow(&store).await;

        let result = service
            .approve_proposal(&crate::wire::AgentSettingsProposalApproveRequest {
                proposal_id: proposal.proposal_id.clone(),
                expected_digest: proposal.digest.clone(),
            })
            .await
            .unwrap();

        assert_eq!(
            result.proposal.status,
            AgentSettingsProposalStatusDto::Applied
        );
        assert_eq!(
            result.receipt.status,
            AgentSettingsProposalStatusDto::Applied
        );
        assert!(result.receipt.changed);
        assert!(result.receipt.applied_revision.is_some());
        assert_eq!(result.receipt.proposal_digest, proposal.digest);
        assert_eq!(result.receipt.changes, proposal.changes);

        // 批准后重开提案仍必须保留原始 Diff；不能把当前 authoritative after
        // 当成 before 再次合并，否则 UI 会看到空列表。
        let reread = service
            .get_proposal(&crate::wire::AgentSettingsProposalGetRequest {
                proposal_id: proposal.proposal_id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(reread.proposal.changes, proposal.changes);
        assert_eq!(
            reread.receipt.expect("已批准提案必须回读 receipt").changes,
            proposal.changes
        );
        // 真正的设置写入：目标 CAS 恰好一次，且值就是提案里的档位。
        assert_eq!(store.target_writes(), 1);
        assert_eq!(store.stored_reading().font_size, ReadingFontSize::Large);
        let proposal_id: SettingProposalId = proposal.proposal_id.parse().unwrap();
        let binding = store
            .state
            .lock()
            .unwrap()
            .bindings
            .get(&proposal_id)
            .cloned()
            .expect("应用后的 Agent 提案必须保留绑定");
        assert_eq!(
            binding.approval_state(),
            haven_domain::agent::AgentApprovalState::Applied
        );
        assert!(
            binding.approval_token_hash().is_none(),
            "审批完成后不得残留审批令牌摘要"
        );

        // 回执与响应里没有任何 token 材料。
        let raw = serde_json::to_string(&result).unwrap();
        for forbidden in ["token", "Token", "approvalToken", "secret"] {
            assert!(
                !raw.contains(forbidden),
                "批准响应不得包含 token 字段：{forbidden}"
            );
        }
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys.len(), 2, "批准响应只有 proposal 与 receipt");
    }

    #[tokio::test]
    async fn agent_receipt_redacts_existing_authoritative_free_text_but_keeps_the_setting() {
        let store = FakeStore::new();
        seed_reading(&store, Some("rev-0001"));
        let private_font = "D:/private/font.ttf";
        store.write_reading_for_test(ReadingPatch {
            custom_font_family: Some(private_font.to_owned()),
            ..ReadingPatch::default()
        });

        // Agent 只提议修改安全的档位字段；它不应复制 authoritative 中既有的字体路径。
        let (service, proposal) = run_full_flow(&store).await;
        let proposal_json = serde_json::to_string(&proposal).unwrap();
        assert!(!proposal_json.contains(private_font));
        assert!(!proposal_json.contains("font.ttf"));
        assert_eq!(proposal.changes.len(), 1);
        assert_eq!(proposal.changes[0].key, "reading.fontSize");

        let result = service
            .approve_proposal(&crate::wire::AgentSettingsProposalApproveRequest {
                proposal_id: proposal.proposal_id.clone(),
                expected_digest: proposal.digest.clone(),
            })
            .await
            .unwrap();

        // authoritative 事实源仍保留用户自己的真实设置，并且 Agent 的安全 patch 已生效。
        assert_eq!(
            store.stored_reading().custom_font_family.as_deref(),
            Some(private_font)
        );
        assert_eq!(store.stored_reading().font_size, ReadingFontSize::Large);

        let proposal_id: SettingProposalId = proposal.proposal_id.parse().unwrap();
        let stored_receipt = store
            .state
            .lock()
            .unwrap()
            .receipts
            .get(&proposal_id)
            .cloned()
            .expect("批准后必须保存 Receipt");
        let receipt_json = format!(
            "{}{}",
            stored_receipt.before_canonical_json, stored_receipt.after_canonical_json
        );
        assert!(!receipt_json.contains(private_font));
        assert!(!receipt_json.contains("font.ttf"));
        assert!(receipt_json.contains("[redacted]"));

        // 对外投影同样不能泄漏 authoritative 的自由文本；它只呈现安全的变更事实。
        let projected_json = serde_json::to_string(&result.receipt).unwrap();
        assert!(!projected_json.contains(private_font));
        assert!(!projected_json.contains("font.ttf"));
        assert_eq!(result.receipt.changes.len(), 1);
        assert_eq!(result.receipt.changes[0].key, "reading.fontSize");
    }

    #[tokio::test]
    async fn approval_is_idempotent_for_the_same_digest_but_rejects_replays() {
        let store = FakeStore::new();
        seed_reading(&store, Some("rev-0001"));
        let (service, proposal) = run_full_flow(&store).await;
        let request = crate::wire::AgentSettingsProposalApproveRequest {
            proposal_id: proposal.proposal_id.clone(),
            expected_digest: proposal.digest.clone(),
        };
        service.approve_proposal(&request).await.unwrap();
        assert_eq!(store.target_writes(), 1);

        // 第二次批准：pending → applied 已迁移，令牌不可能再次签发（重放被拒绝）。
        let replay = service.approve_proposal(&request).await.unwrap_err();
        assert!(
            matches!(
                replay.code().as_str(),
                "AGENT_APPROVAL_TOKEN_INVALID" | "SETTING_PROPOSAL_NOT_PENDING"
            ),
            "重放必须稳定失败，实际：{}",
            replay.code().as_str()
        );
        assert_eq!(store.target_writes(), 1, "重放不得再次写设置");
    }

    #[tokio::test]
    async fn approval_fails_closed_on_digest_mismatch_and_writes_nothing() {
        let store = FakeStore::new();
        seed_reading(&store, Some("rev-0001"));
        let (service, proposal) = run_full_flow(&store).await;

        let error = service
            .approve_proposal(&crate::wire::AgentSettingsProposalApproveRequest {
                proposal_id: proposal.proposal_id.clone(),
                expected_digest: "d".repeat(64),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_DIGEST_MISMATCH");
        assert_eq!(store.target_writes(), 0);
        assert_eq!(store.revision().as_deref(), Some("rev-0001"));
    }

    #[tokio::test]
    async fn approval_fails_closed_when_settings_changed_after_proposal() {
        let store = FakeStore::new();
        seed_reading(&store, Some("rev-0001"));
        let (service, proposal) = run_full_flow(&store).await;

        // 提案生成后、批准之前，设置被别处推进：必须冲突且零写入。
        store.write_reading(ReadingPatch {
            theme: Some(haven_domain::settings::ReadingTheme::Slate),
            ..ReadingPatch::default()
        });

        let error = service
            .approve_proposal(&crate::wire::AgentSettingsProposalApproveRequest {
                proposal_id: proposal.proposal_id.clone(),
                expected_digest: proposal.digest.clone(),
            })
            .await
            .unwrap_err();
        // 稳定冲突码（`setting_proposal_revision_conflict_error` 的 code 就是它）。
        assert_eq!(error.code().as_str(), "REVISION_CONFLICT");
        assert_eq!(store.target_writes(), 0);
        // 设置保持别处写入的值（提案的那个档位没有被应用）。
        assert_ne!(store.stored_reading().font_size, ReadingFontSize::Large);
        let proposal_id: SettingProposalId = proposal.proposal_id.parse().unwrap();
        let binding = store
            .state
            .lock()
            .unwrap()
            .bindings
            .get(&proposal_id)
            .cloned()
            .expect("CAS 冲突后的 Agent 提案必须保留绑定");
        assert_eq!(
            binding.approval_state(),
            haven_domain::agent::AgentApprovalState::Pending
        );
        assert!(
            binding.approval_token_hash().is_none(),
            "CAS 失败回滚后不得残留审批令牌摘要"
        );
    }

    #[tokio::test]
    async fn rejection_writes_nothing_and_blocks_later_approval() {
        let store = FakeStore::new();
        seed_reading(&store, Some("rev-0001"));
        let (service, proposal) = run_full_flow(&store).await;

        let rejected = service
            .reject_proposal(&crate::wire::AgentSettingsProposalRejectRequest {
                proposal_id: proposal.proposal_id.clone(),
                expected_digest: proposal.digest.clone(),
            })
            .await
            .unwrap();
        assert_eq!(
            rejected.proposal.status,
            AgentSettingsProposalStatusDto::Rejected
        );
        assert_eq!(store.target_writes(), 0);
        assert_eq!(store.revision().as_deref(), Some("rev-0001"));

        // 已拒绝的提案不能再批准（令牌无法签发），且仍然零写入。
        let error = service
            .approve_proposal(&crate::wire::AgentSettingsProposalApproveRequest {
                proposal_id: proposal.proposal_id.clone(),
                expected_digest: proposal.digest.clone(),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_NOT_PENDING");
        assert_eq!(store.target_writes(), 0);

        // 拒绝也要核对 digest：不一致时不能把拒绝写成事实。
        let store2 = FakeStore::new();
        seed_reading(&store2, Some("rev-0001"));
        let (service2, proposal2) = run_full_flow(&store2).await;
        assert_eq!(
            service2
                .reject_proposal(&crate::wire::AgentSettingsProposalRejectRequest {
                    proposal_id: proposal2.proposal_id.clone(),
                    expected_digest: "e".repeat(64),
                })
                .await
                .unwrap_err()
                .code()
                .as_str(),
            "SETTING_PROPOSAL_DIGEST_MISMATCH"
        );
    }

    #[tokio::test]
    async fn binding_scope_is_settings_and_never_a_fake_work_id() {
        let store = FakeStore::new();
        seed_reading(&store, Some("rev-0001"));
        let (service, proposal) = run_full_flow(&store).await;
        let proposal_id: SettingProposalId = proposal.proposal_id.parse().unwrap();
        let binding = service
            .proposals
            .get(proposal_id)
            .await
            .unwrap()
            .expect("提案必须存在");
        let stored = store
            .state
            .lock()
            .unwrap()
            .bindings
            .get(&proposal_id)
            .cloned()
            .expect("绑定必须存在");

        assert_eq!(
            stored.subject().as_settings(),
            Some(AgentSettingsSubject::reading())
        );
        assert!(stored.subject().as_content().is_none());
        let subject_json = canonical_json_of(&stored.subject()).unwrap();
        assert_eq!(subject_json, json!({"section": "reading"}).to_string());
        for forbidden in [
            "workId",
            "editionId",
            "mediaItemId",
            "00000000-0000-0000-0000-000000000000",
        ] {
            assert!(
                !subject_json.contains(forbidden),
                "设置绑定不得携带作品身份或 nil UUID：{forbidden}"
            );
        }
        // 绑定携带的 context id/hash 就是上下文身份本身。
        let fresh = service.context().await.unwrap();
        assert_eq!(stored.context_snapshot_id(), fresh.context_id());
        assert_eq!(stored.context_hash(), fresh.context_hash());
        let _ = binding;
    }

    #[tokio::test]
    async fn capability_set_only_declares_implemented_abilities() {
        let store = FakeStore::new();
        seed_reading(&store, Some("rev-0001"));
        let service = service(&store);
        let dto = service.context().await.unwrap().to_dto();
        let expected = AgentCapabilitySet {
            settings_read: true,
            settings_proposal: true,
            library_summary_read: true,
            setting_sources_read: true,
            resource_preference_read: true,
            resource_preference_proposal: true,
            media_capabilities_read: true,
            onboarding_read: true,
            metadata_proposal: false,
            rename_proposal: false,
            secret_read: false,
            filesystem_write: false,
        };
        assert_eq!(dto.capabilities.agent_api_version, 1);
        assert_eq!(
            dto.capabilities.capabilities.settings_read,
            expected.settings_read
        );
        assert_eq!(
            dto.capabilities.capabilities.settings_proposal,
            expected.settings_proposal
        );
        assert!(dto.capabilities.capabilities.library_summary_read);
        assert!(dto.capabilities.capabilities.setting_sources_read);
        assert!(dto.capabilities.capabilities.resource_preference_read);
        assert!(dto.capabilities.capabilities.resource_preference_proposal);
        assert!(dto.capabilities.capabilities.media_capabilities_read);
        assert!(dto.capabilities.capabilities.onboarding_read);
        assert!(!dto.capabilities.capabilities.metadata_proposal);
        assert!(!dto.capabilities.capabilities.rename_proposal);
        assert!(!dto.capabilities.capabilities.secret_read);
        assert!(!dto.capabilities.capabilities.filesystem_write);
    }
}
