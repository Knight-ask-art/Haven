//! Agent 全局设置 Commands（A1「智能配置推荐」垂直切片）。
//!
//! 命令层只做四件事：typed request 解析、ID 转换、调用 Application service、
//! 把 `AppError` 映射成 `ErrorDto`；同步 SQLite 在 blocking worker 上执行。
//! 这里**没有** SQL、Provider、文件系统调用，也没有
//! `agent_invoke(commandName, arbitraryArguments)` 这类自由分发入口。
//!
//! 批准路径是唯一的用户确认写入口：UI 只提交 proposal id 与它显示的那份 canonical
//! digest；一次性 Approval Token 由 Application service 在内部生成并消费，
//! 不经过本层，也不出现在任何响应里。

use tauri::State;

use haven_application::services::{
    AgentContextQueryService, AgentSettingsIpcService, AgentTraceQueryService,
};
use haven_application::wire::{
    AgentCapabilityManifestDto, AgentResourcePreferenceProposalApproveRequest,
    AgentResourcePreferenceProposalApproveResultDto, AgentResourcePreferenceProposalCreateRequest,
    AgentResourcePreferenceProposalDto, AgentResourcePreferenceProposalGetRequest,
    AgentResourcePreferenceProposalGetResultDto, AgentSettingChangeReceiptDto,
    AgentSettingChangeReceiptGetRequest, AgentSettingsContextDto,
    AgentSettingsProposalApproveRequest, AgentSettingsProposalApproveResultDto,
    AgentSettingsProposalCreateRequest, AgentSettingsProposalDto, AgentSettingsProposalGetRequest,
    AgentSettingsProposalGetResultDto, AgentSettingsProposalRejectRequest,
    AgentSettingsProposalRejectResultDto, AgentTraceGetRequest, AgentTraceGetResultDto, ErrorDto,
};
use haven_domain::ids::{AgentRequestId, AgentSessionId, SettingProposalId};

use crate::ipc::{run_blocking, to_error_dto};
use crate::state::AppState;

/// 读取服务端固定的 Agent 能力清单。
///
/// 这是**声明**而不是授权开关：清单只包含本切片已实现的能力，且不接受任何入参，
/// 因此调用方无法通过它放开 secret / 文件系统 / 元数据提案等未实现能力。
pub async fn run_agent_capability_manifest_get() -> Result<AgentCapabilityManifestDto, ErrorDto> {
    Ok(haven_domain::agent::AgentCapabilityManifest::for_current_slice().into())
}

/// 读取全局阅读设置上下文（脱敏快照 + revision + context id/hash + 能力清单）。
pub async fn run_agent_settings_context_get(
    agent_settings: &AgentSettingsIpcService,
) -> Result<AgentSettingsContextDto, ErrorDto> {
    agent_settings
        .context()
        .await
        .map(|context| context.to_dto())
        .map_err(|error| to_error_dto(&error))
}

/// 创建设置提案（只创建，不写任何设置）。
pub async fn run_agent_settings_proposal_create(
    agent_settings: &AgentSettingsIpcService,
    request: AgentSettingsProposalCreateRequest,
) -> Result<AgentSettingsProposalDto, ErrorDto> {
    let session_id = parse_session_id(&request.session_id)?;
    let request_id = parse_request_id(&request.request_id)?;
    agent_settings
        .create_proposal(session_id, request_id, &request)
        .await
        .map_err(|error| to_error_dto(&error))
}

/// 创建资源级偏好提案（只创建，不写资源偏好）。
pub async fn run_agent_resource_preference_proposal_create(
    agent_context: &AgentContextQueryService,
    request: AgentResourcePreferenceProposalCreateRequest,
) -> Result<AgentResourcePreferenceProposalDto, ErrorDto> {
    let session_id = parse_session_id(&request.session_id)?;
    let request_id = parse_request_id(&request.request_id)?;
    agent_context
        .create_resource_preference_proposal(session_id, request_id, &request)
        .await
        .map_err(|error| to_error_dto(&error))
}

/// 回读提案与（可能存在的）回执。
pub async fn run_agent_settings_proposal_get(
    agent_settings: &AgentSettingsIpcService,
    request: AgentSettingsProposalGetRequest,
) -> Result<AgentSettingsProposalGetResultDto, ErrorDto> {
    agent_settings
        .get_proposal(&request)
        .await
        .map_err(|error| to_error_dto(&error))
}

/// 拒绝提案（零设置写入）。
pub async fn run_agent_settings_proposal_reject(
    agent_settings: &AgentSettingsIpcService,
    request: AgentSettingsProposalRejectRequest,
) -> Result<AgentSettingsProposalRejectResultDto, ErrorDto> {
    agent_settings
        .reject_proposal(&request)
        .await
        .map_err(|error| to_error_dto(&error))
}

/// 用户批准：唯一写入口（内部一次性 token + 同一 UoW 内 CAS 与回执）。
pub async fn run_agent_settings_proposal_approve(
    agent_settings: &AgentSettingsIpcService,
    request: AgentSettingsProposalApproveRequest,
) -> Result<AgentSettingsProposalApproveResultDto, ErrorDto> {
    agent_settings
        .approve_proposal(&request)
        .await
        .map_err(|error| to_error_dto(&error))
}

/// 读取回执（未应用返回 null，不伪造"已应用"结论）。
pub async fn run_agent_setting_change_receipt_get(
    agent_settings: &AgentSettingsIpcService,
    request: AgentSettingChangeReceiptGetRequest,
) -> Result<Option<AgentSettingChangeReceiptDto>, ErrorDto> {
    let proposal_id = parse_proposal_id(&request.proposal_id)?;
    agent_settings
        .receipt(proposal_id)
        .await
        .map_err(|error| to_error_dto(&error))
}

/// 按 proposalId 回读资源级 Agent 提案与回执；不提供任何直接写入能力。
pub async fn run_agent_resource_preference_proposal_get(
    agent_context: &haven_application::services::AgentContextQueryService,
    request: AgentResourcePreferenceProposalGetRequest,
) -> Result<AgentResourcePreferenceProposalGetResultDto, ErrorDto> {
    agent_context
        .get_resource_preference_proposal(&request)
        .await
        .map_err(|error| to_error_dto(&error))
}

/// 用户批准资源级 Agent 提案；目标、绑定、token、CAS 与 receipt 仍由 Application 内核
/// 在同一 UoW 中核对和执行。
pub async fn run_agent_resource_preference_proposal_approve(
    agent_context: &haven_application::services::AgentContextQueryService,
    request: AgentResourcePreferenceProposalApproveRequest,
) -> Result<AgentResourcePreferenceProposalApproveResultDto, ErrorDto> {
    agent_context
        .approve_resource_preference_proposal(&request)
        .await
        .map_err(|error| to_error_dto(&error))
}

/// 读取一条 session 绑定的有界 Agent 轨迹；不返回模型正文或敏感材料。
pub async fn run_agent_trace_get(
    agent_trace: &AgentTraceQueryService,
    request: AgentTraceGetRequest,
) -> Result<AgentTraceGetResultDto, ErrorDto> {
    agent_trace
        .get(&request)
        .await
        .map_err(|error| to_error_dto(&error))
}

// ---------- Tauri 命令 ----------

#[tauri::command]
pub async fn agent_capability_manifest_get() -> Result<AgentCapabilityManifestDto, ErrorDto> {
    run_blocking(run_agent_capability_manifest_get).await
}

#[tauri::command]
pub async fn agent_settings_context_get(
    state: State<'_, AppState>,
) -> Result<AgentSettingsContextDto, ErrorDto> {
    let agent_settings = state.agent_settings.clone();
    run_blocking(move || async move { run_agent_settings_context_get(&agent_settings).await }).await
}

#[tauri::command]
pub async fn agent_settings_proposal_create(
    state: State<'_, AppState>,
    request: AgentSettingsProposalCreateRequest,
) -> Result<AgentSettingsProposalDto, ErrorDto> {
    let agent_settings = state.agent_settings.clone();
    run_blocking(move || async move {
        run_agent_settings_proposal_create(&agent_settings, request).await
    })
    .await
}

#[tauri::command]
pub async fn agent_resource_preference_proposal_create(
    state: State<'_, AppState>,
    request: AgentResourcePreferenceProposalCreateRequest,
) -> Result<AgentResourcePreferenceProposalDto, ErrorDto> {
    let agent_context = state.agent_context.clone();
    run_blocking(move || async move {
        run_agent_resource_preference_proposal_create(&agent_context, request).await
    })
    .await
}

#[tauri::command]
pub async fn agent_settings_proposal_get(
    state: State<'_, AppState>,
    request: AgentSettingsProposalGetRequest,
) -> Result<AgentSettingsProposalGetResultDto, ErrorDto> {
    let agent_settings = state.agent_settings.clone();
    run_blocking(
        move || async move { run_agent_settings_proposal_get(&agent_settings, request).await },
    )
    .await
}

#[tauri::command]
pub async fn agent_settings_proposal_reject(
    state: State<'_, AppState>,
    request: AgentSettingsProposalRejectRequest,
) -> Result<AgentSettingsProposalRejectResultDto, ErrorDto> {
    let agent_settings = state.agent_settings.clone();
    run_blocking(move || async move {
        run_agent_settings_proposal_reject(&agent_settings, request).await
    })
    .await
}

#[tauri::command]
pub async fn agent_settings_proposal_approve(
    state: State<'_, AppState>,
    request: AgentSettingsProposalApproveRequest,
) -> Result<AgentSettingsProposalApproveResultDto, ErrorDto> {
    let agent_settings = state.agent_settings.clone();
    run_blocking(move || async move {
        run_agent_settings_proposal_approve(&agent_settings, request).await
    })
    .await
}

#[tauri::command]
pub async fn agent_setting_change_receipt_get(
    state: State<'_, AppState>,
    request: AgentSettingChangeReceiptGetRequest,
) -> Result<Option<AgentSettingChangeReceiptDto>, ErrorDto> {
    let agent_settings = state.agent_settings.clone();
    run_blocking(move || async move {
        run_agent_setting_change_receipt_get(&agent_settings, request).await
    })
    .await
}

#[tauri::command]
pub async fn agent_resource_preference_proposal_get(
    state: State<'_, AppState>,
    request: AgentResourcePreferenceProposalGetRequest,
) -> Result<AgentResourcePreferenceProposalGetResultDto, ErrorDto> {
    let agent_context = state.agent_context.clone();
    run_blocking(move || async move {
        run_agent_resource_preference_proposal_get(&agent_context, request).await
    })
    .await
}

#[tauri::command]
pub async fn agent_resource_preference_proposal_approve(
    state: State<'_, AppState>,
    request: AgentResourcePreferenceProposalApproveRequest,
) -> Result<AgentResourcePreferenceProposalApproveResultDto, ErrorDto> {
    let agent_context = state.agent_context.clone();
    run_blocking(move || async move {
        run_agent_resource_preference_proposal_approve(&agent_context, request).await
    })
    .await
}

#[tauri::command]
pub async fn agent_trace_get(
    state: State<'_, AppState>,
    request: AgentTraceGetRequest,
) -> Result<AgentTraceGetResultDto, ErrorDto> {
    let agent_trace = state.agent_trace_query.clone();
    run_blocking(move || async move { run_agent_trace_get(&agent_trace, request).await }).await
}

// ---------- 解析辅助 ----------

fn parse_session_id(raw: &str) -> Result<AgentSessionId, ErrorDto> {
    raw.parse::<AgentSessionId>()
        .map_err(|_| invalid_argument("会话 ID 非法"))
}

fn parse_request_id(raw: &str) -> Result<AgentRequestId, ErrorDto> {
    raw.parse::<AgentRequestId>()
        .map_err(|_| invalid_argument("请求 ID 非法"))
}

fn parse_proposal_id(raw: &str) -> Result<SettingProposalId, ErrorDto> {
    raw.parse::<SettingProposalId>()
        .map_err(|_| invalid_argument("提案 ID 非法"))
}

fn invalid_argument(message: &'static str) -> ErrorDto {
    ErrorDto {
        code: "INVALID_ARGUMENT".into(),
        user_message: message.into(),
        retryable: false,
    }
}
