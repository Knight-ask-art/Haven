//! AI Provider Profile Commands（A2 基础切片）。
//!
//! 规范：[`docs/architecture/AI_SYSTEM.md`](../../../docs/architecture/AI_SYSTEM.md) §2、§3、§4。
//!
//! 命令层只做三件事：typed request 解析、调用 Application service、把 `AppError`
//! 映射成 `ErrorDto`。这里**没有** SQL、HTTP、文件系统，也没有
//! `ai_invoke(prompt)` / `ai_chat(...)` 这类把任意提示词直通模型的自由入口。
//!
//! - 出站网络只由 Infrastructure 的 `AiModelCatalogPort` 适配器发起；
//! - API key 从不经过本层：凭据写入仍走既有的 `credential_set`（provider = `ai`），
//!   本层只读取 profile 与模型目录的**非敏感**投影。

use tauri::State;

use haven_application::services::AiSettingsRecommendationRequest;
use haven_application::wire::{
    AiProviderModelsCatalogDto, AiProviderModelsListRequest, AiProviderProfileDeleteRequest,
    AiProviderProfileDeleteResultDto, AiProviderProfileDto, AiProviderProfileGetRequest,
    AiProviderProfileListResultDto, AiProviderProfileUpsertRequest, AiSettingsRecommendationDto,
    AiSettingsRecommendationGenerateRequest, ErrorDto,
};

use crate::ipc::{run_blocking, to_error_dto};
use crate::state::AppState;

/// `ai_provider_profile_list`：列出全部 Provider Profile 与其凭据配置状态。
#[tauri::command]
pub async fn ai_provider_profile_list(
    state: State<'_, AppState>,
) -> Result<AiProviderProfileListResultDto, ErrorDto> {
    let profiles = state.ai_provider.clone();
    run_blocking(move || async move { profiles.list().await.map_err(|error| to_error_dto(&error)) })
        .await
}

/// `ai_provider_profile_get`：读取单个 Provider Profile。
#[tauri::command]
pub async fn ai_provider_profile_get(
    state: State<'_, AppState>,
    request: AiProviderProfileGetRequest,
) -> Result<AiProviderProfileDto, ErrorDto> {
    let profiles = state.ai_provider.clone();
    run_blocking(move || async move {
        profiles
            .get(&request.profile_id)
            .await
            .map_err(|error| to_error_dto(&error))
    })
    .await
}

/// `ai_provider_profile_upsert`：校验 + CAS 写入（不含任何 secret）。
#[tauri::command]
pub async fn ai_provider_profile_upsert(
    state: State<'_, AppState>,
    request: AiProviderProfileUpsertRequest,
) -> Result<AiProviderProfileDto, ErrorDto> {
    let profiles = state.ai_provider.clone();
    run_blocking(move || async move {
        profiles
            .upsert(request)
            .await
            .map_err(|error| to_error_dto(&error))
    })
    .await
}

/// `ai_provider_profile_delete`：先清理凭据、再 CAS 删除 profile 行。
#[tauri::command]
pub async fn ai_provider_profile_delete(
    state: State<'_, AppState>,
    request: AiProviderProfileDeleteRequest,
) -> Result<AiProviderProfileDeleteResultDto, ErrorDto> {
    let profiles = state.ai_provider.clone();
    run_blocking(move || async move {
        profiles
            .delete(request)
            .await
            .map_err(|error| to_error_dto(&error))
    })
    .await
}

/// `ai_provider_models_list`：读取 Provider 的模型目录（只读、有界、非敏感投影）。
///
/// 没有 profile / 没有凭据 / profile 被禁用 / 目录为空都是**空目录 + 明确 state**，
/// 不是错误；只有真正失败（不可达 / 非 2xx / 非法 JSON）才返回 ErrorDto。
#[tauri::command]
pub async fn ai_provider_models_list(
    state: State<'_, AppState>,
    request: AiProviderModelsListRequest,
) -> Result<AiProviderModelsCatalogDto, ErrorDto> {
    let profiles = state.ai_provider.clone();
    run_blocking(move || async move {
        profiles
            .models_catalog(request)
            .await
            .map_err(|error| to_error_dto(&error))
    })
    .await
}

/// `ai_settings_recommendation_generate`：调用已配置 Provider 生成一次结构化推荐，
/// 并只创建 pending Proposal；不执行任何设置写入。
#[tauri::command]
pub async fn ai_settings_recommendation_generate(
    state: State<'_, AppState>,
    request: AiSettingsRecommendationGenerateRequest,
) -> Result<AiSettingsRecommendationDto, ErrorDto> {
    let profiles = state.ai_provider.clone();
    run_blocking(move || async move {
        profiles
            .generate_settings_recommendation(AiSettingsRecommendationRequest {
                profile_id: request.profile_id,
                session_id: request.session_id,
                request_id: request.request_id,
                context_id: request.context_id,
                context_hash: request.context_hash,
                base_revision: request.base_revision,
            })
            .await
            .map(|result| AiSettingsRecommendationDto {
                schema_version: result.schema_version,
                profile_id: result.profile_id,
                model_id: result.model_id,
                explanation: result.explanation,
                recommended_patch: result.recommended_patch,
                proposal: result.proposal,
            })
            .map_err(|error| to_error_dto(&error))
    })
    .await
}
