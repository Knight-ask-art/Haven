//! Comic page manifest query. Page bytes remain on the controlled protocol.

use tauri::{Runtime, State, Webview};

use haven_application::wire::{ComicPageManifestDto, ComicPageManifestGetRequest, ErrorDto};

use crate::ipc::{invalid_id, run_blocking, to_error_dto};
use crate::state::AppState;

pub async fn run_comic_page_manifest_get(
    state: &AppState,
    owner_webview_label: &str,
    request: ComicPageManifestGetRequest,
) -> Result<ComicPageManifestDto, ErrorDto> {
    let uuid = uuid::Uuid::parse_str(&request.session_id).map_err(|_| invalid_id())?;
    let session_id = uuid.to_string();
    if session_id != request.session_id {
        return Err(invalid_id());
    }
    state
        .session_registry
        .comic_manifest(&session_id, owner_webview_label)
        .map_err(|error| to_error_dto(&error))
}

#[tauri::command]
pub async fn comic_page_manifest_get<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, AppState>,
    request: ComicPageManifestGetRequest,
) -> Result<ComicPageManifestDto, ErrorDto> {
    let owner_webview_label = webview.label().to_owned();
    let state = (*state.inner()).clone();
    run_blocking(move || async move {
        run_comic_page_manifest_get(&state, &owner_webview_label, request).await
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_infrastructure::Db;
    use std::sync::Arc;

    #[tokio::test]
    async fn invalid_session_id_is_rejected_before_registry_lookup() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let error = run_comic_page_manifest_get(
            &state,
            "main",
            ComicPageManifestGetRequest {
                session_id: "NOT-A-CANONICAL-ID".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "INVALID_ID");
        assert!(!error.retryable);
    }
}
