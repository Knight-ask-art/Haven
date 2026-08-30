//! 来源运行时命令（V2-B 实战批次；契约 §36.2/§36.3/§36.4 演进）。
//!
//! - `source_registry_set_endpoint`：端点只写后端持久化，响应不含端点本身。
//! - `source_work_import`：候选入库（幂等），返回真实 Work/MediaItem 身份。
//! - `stream_open` / `stream_close`：远端流受控代理会话（grant 模型）。

use tauri::State;

use haven_application::wire::{
    ErrorDto, SessionOpenRequest, SessionOpenResultDto, SourceEndpointSetRequest,
    SourceEndpointSetResult, SourceWorkImportRequest, SourceWorkImportResult,
};

use crate::ipc::{invalid_argument, run_blocking, to_error_dto, LIBRARY_CHANGED_TRANSPORT_EVENT};
use crate::state::AppState;

/// `source_registry_set_endpoint` 命令核心。
async fn run_source_endpoint_set(
    registry: &haven_application::services::SourceRegistryService,
    request: SourceEndpointSetRequest,
) -> Result<SourceEndpointSetResult, ErrorDto> {
    if request.source_id.trim().is_empty() || request.source_id.len() > 64 {
        return Err(invalid_argument("sourceId 非法"));
    }
    let configured = registry
        .set_endpoint(&request.source_id, &request.endpoint)
        .await
        .map_err(|e| to_error_dto(&e))?;
    Ok(SourceEndpointSetResult {
        source_id: request.source_id,
        endpoint_configured: configured,
    })
}

/// Tauri Command 薄包装。
#[tauri::command]
pub async fn source_registry_set_endpoint(
    state: State<'_, AppState>,
    request: SourceEndpointSetRequest,
) -> Result<SourceEndpointSetResult, ErrorDto> {
    let registry = state.source_registry.clone();
    run_blocking(move || async move { run_source_endpoint_set(&registry, request).await }).await
}

/// `source_work_import` 命令核心：定位候选 → 入库 → 发布 library.changed。
pub async fn run_source_work_import(
    state: &AppState,
    request: SourceWorkImportRequest,
) -> Result<(SourceWorkImportResult, bool), ErrorDto> {
    let Some(external_id) = state
        .search_source
        .candidate_external_id(&request.operation_id, request.index)
    else {
        return Err(ErrorDto {
            code: "RESOURCE_NOT_FOUND".into(),
            user_message: "搜索候选不存在或已过期".into(),
            retryable: false,
        });
    };
    let imported = state
        .source_import
        .import_candidate(&external_id)
        .await
        .map_err(|e| to_error_dto(&e))?;
    Ok((
        SourceWorkImportResult {
            schema_version: 1,
            work_id: imported.work_id.to_string(),
            media_item_id: imported.media_item_id.to_string(),
        },
        true,
    ))
}

/// Tauri Command 薄包装（SQLite 写入走 blocking worker；成功发布一次 library.changed）。
#[tauri::command]
pub async fn source_work_import<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    request: SourceWorkImportRequest,
) -> Result<SourceWorkImportResult, ErrorDto> {
    use tauri::Emitter;
    let state_clone = (*state.inner()).clone();
    let (result, changed) =
        run_blocking(move || async move { run_source_work_import(&state_clone, request).await })
            .await?;
    if changed {
        // 库失效通知（复用既有事件名；envelope 最小化）。
        let _ = app.emit(
            LIBRARY_CHANGED_TRANSPORT_EVENT,
            serde_json::json!({
                "schemaVersion": 1,
                "at": chrono::Utc::now().to_rfc3339(),
                "operationId": format!("import-{}", uuid::Uuid::new_v4()),
                "sequence": 1,
                "revision": serde_json::Value::Null,
            }),
        );
    }
    Ok(result)
}

/// `stream_open` 命令核心：解析远端流资源 → 注册 grant → 返回代理 URI。
pub fn run_stream_open(
    state: &AppState,
    owner_label: &str,
    request: SessionOpenRequest,
) -> Result<SessionOpenResultDto, haven_application::wire::ErrorDto> {
    // prepare 是异步 DB 操作；命令层用 block_on 收敛（协议处理器同模式）。
    let facts = tauri::async_runtime::block_on(state.stream.prepare(request))
        .map_err(|e| to_error_dto(&e))?;
    let grant = state.stream_registry.register(
        crate::stream_registry::StreamGrantFacts {
            work_id: facts.work_id.clone(),
            edition_id: facts.edition_id.clone(),
            media_item_id: facts.media_item_id.clone(),
            mime_type: facts.mime_type.clone(),
            is_hls: facts.is_hls,
            progress: facts.progress,
            upstream_url: facts.upstream_url.clone(),
        },
        &facts.upstream_url,
        owner_label,
    );
    // 历史足迹：远端流打开即记（幂等，同 mediaItem 只一条 last_active_at 刷新）。
    if let Ok(mid) = facts
        .media_item_id
        .parse::<haven_domain::ids::MediaItemId>()
    {
        let _ = tauri::async_runtime::block_on(state.history.record(mid));
    }
    Ok(SessionOpenResultDto {
        schema_version: 1,
        session_id: grant.to_string(),
        content_uri: Some(format!("haven-resource://stream/{grant}")),
        work_id: facts.work_id,
        edition_id: facts.edition_id,
        media_item_id: facts.media_item_id,
        engine: haven_application::wire::SessionEngineDto::Playback,
        progress: None,
    })
}

/// `stream_open` 命令薄包装。
#[tauri::command]
pub async fn stream_open<R: tauri::Runtime>(
    webview: tauri::WebviewWindow<R>,
    state: State<'_, AppState>,
    request: SessionOpenRequest,
) -> Result<SessionOpenResultDto, ErrorDto> {
    let state = (*state.inner()).clone();
    let label = webview.label().to_owned();
    tauri::async_runtime::spawn_blocking(move || run_stream_open(&state, &label, request))
        .await
        .map_err(|_| ErrorDto {
            code: "INTERNAL_ERROR".into(),
            user_message: "后台任务执行失败".into(),
            retryable: false,
        })?
}

/// `stream_close` 幂等撤销。
#[tauri::command]
pub async fn stream_close<R: tauri::Runtime>(
    webview: tauri::WebviewWindow<R>,
    state: State<'_, AppState>,
    request: crate::ipc::StreamCloseRequest,
) -> Result<bool, ErrorDto> {
    let label = webview.label().to_owned();
    Ok(state.stream_registry.revoke(&request.session_id, &label))
}
