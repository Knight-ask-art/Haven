//! `session_open` command (Phase A).

use tauri::{Runtime, State, Webview};

use haven_application::wire::{
    ErrorDto, SessionCloseRequest, SessionCloseResultDto, SessionOpenRequest, SessionOpenResultDto,
};

use crate::ipc::{invalid_id, run_blocking, to_error_dto};
use crate::state::AppState;

pub async fn run_session_open(
    state: &AppState,
    owner_webview_label: &str,
    owner_window_label: &str,
    request: SessionOpenRequest,
) -> Result<SessionOpenResultDto, ErrorDto> {
    let prepared = state
        .session
        .prepare(request)
        .await
        .map_err(|e| to_error_dto(&e))?;
    let exposes_root_content = prepared.engine != haven_application::wire::SessionEngineDto::Comic;
    let result = SessionOpenResultDto {
        schema_version: 1,
        session_id: String::new(),
        content_uri: None,
        work_id: prepared.work_id.clone(),
        edition_id: prepared.edition_id.clone(),
        media_item_id: prepared.media_item_id.clone(),
        engine: prepared.engine,
        progress: prepared.progress.clone(),
    };
    let media_item_id_for_history = prepared.media_item_id.clone();
    let session_id = state
        .session_registry
        .register(
            prepared,
            owner_webview_label.to_owned(),
            owner_window_label.to_owned(),
        )
        .map_err(|error| to_error_dto(&error))?;
    // 历史足迹：打开即记（幂等，同 mediaItem 只一条 last_active_at 刷新），失败不影响播放。
    if let Ok(mid) = media_item_id_for_history.parse::<haven_domain::ids::MediaItemId>() {
        let _ = state.history.record(mid).await;
    }
    let mut result = result;
    result.session_id = session_id.clone();
    if exposes_root_content {
        result.content_uri = Some(crate::session_registry::SessionRegistry::uri(&session_id));
    }
    Ok(result)
}

pub async fn run_session_close(
    state: &AppState,
    owner_webview_label: &str,
    request: SessionCloseRequest,
) -> Result<SessionCloseResultDto, ErrorDto> {
    let uuid = uuid::Uuid::parse_str(&request.session_id).map_err(|_| invalid_id())?;
    let session_id = uuid.to_string();
    if session_id != request.session_id {
        return Err(invalid_id());
    }
    state
        .session_registry
        .remove_for_owner(&session_id, owner_webview_label)
        .map_err(|error| to_error_dto(&error))?;
    Ok(SessionCloseResultDto {
        schema_version: 1,
        closed: true,
    })
}

#[tauri::command]
pub async fn session_open<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, AppState>,
    request: SessionOpenRequest,
) -> Result<SessionOpenResultDto, ErrorDto> {
    let owner_webview_label = webview.label().to_owned();
    let owner_window_label = webview.window().label().to_owned();
    let state = (*state.inner()).clone();
    run_blocking(move || async move {
        run_session_open(&state, &owner_webview_label, &owner_window_label, request).await
    })
    .await
}

#[tauri::command]
pub async fn session_close<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, AppState>,
    request: SessionCloseRequest,
) -> Result<SessionCloseResultDto, ErrorDto> {
    let owner_webview_label = webview.label().to_owned();
    let state = (*state.inner()).clone();
    run_blocking(
        move || async move { run_session_close(&state, &owner_webview_label, request).await },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_infrastructure::Db;
    use std::sync::Arc;

    #[tokio::test]
    async fn session_close_rejects_invalid_id_stably() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let error = run_session_close(
            &state,
            "main",
            SessionCloseRequest {
                session_id: "not-a-uuid".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "INVALID_ID");
        assert!(!error.retryable);
    }

    #[tokio::test]
    async fn invalid_id_maps_without_path_or_resource_details() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let error = run_session_open(
            &state,
            "main",
            "main",
            SessionOpenRequest {
                media_item_id: "not-an-id".into(),
                engine: haven_application::wire::SessionEngineDto::Playback,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "INVALID_ID");
        assert!(!error.user_message.contains("/") && !error.user_message.contains("\\"));
    }
}
