//! `marker_create` / `marker_list` / `marker_list_all` / `marker_delete` commands（契约 §23）。

use tauri::State;

use haven_application::wire::{
    ErrorDto, MarkerCreateRequest, MarkerDeleteRequest, MarkerDto, MarkerListAllRequest,
    MarkerListRequest,
};
use haven_domain::ids::{MarkerId, MediaItemId};

use crate::ipc::{invalid_id, run_blocking, to_error_dto};
use crate::state::AppState;

const DEFAULT_MARKER_LIST_ALL_LIMIT: u32 = 100;

/// 解析并强制要求 canonical UUID 文本，避免同一 ID 通过多种字符串形态进入 IPC。
fn validate_canonical_media_item_id(value: &str) -> Result<(), ErrorDto> {
    let id: MediaItemId = value.parse().map_err(|_| invalid_id())?;
    if id.to_string() != value {
        return Err(invalid_id());
    }
    Ok(())
}

fn validate_canonical_marker_id(value: &str) -> Result<(), ErrorDto> {
    let id: MarkerId = value.parse().map_err(|_| invalid_id())?;
    if id.to_string() != value {
        return Err(invalid_id());
    }
    Ok(())
}

pub async fn run_marker_create(
    state: &AppState,
    request: MarkerCreateRequest,
) -> Result<MarkerDto, ErrorDto> {
    validate_canonical_media_item_id(&request.media_item_id)?;
    state
        .marker
        .create(request)
        .await
        .map_err(|e| to_error_dto(&e))
}

pub async fn run_marker_list(
    state: &AppState,
    request: MarkerListRequest,
) -> Result<Vec<MarkerDto>, ErrorDto> {
    validate_canonical_media_item_id(&request.media_item_id)?;
    let id: MediaItemId = request.media_item_id.parse().map_err(|_| invalid_id())?;
    state.marker.list(id).await.map_err(|e| to_error_dto(&e))
}

pub async fn run_marker_list_all(
    state: &AppState,
    request: MarkerListAllRequest,
) -> Result<Vec<MarkerDto>, ErrorDto> {
    let limit = request.limit.unwrap_or(DEFAULT_MARKER_LIST_ALL_LIMIT);
    state
        .marker
        .list_all(limit)
        .await
        .map_err(|e| to_error_dto(&e))
}

pub async fn run_marker_delete(
    state: &AppState,
    request: MarkerDeleteRequest,
) -> Result<bool, ErrorDto> {
    validate_canonical_marker_id(&request.marker_id)?;
    let id: MarkerId = request.marker_id.parse().map_err(|_| invalid_id())?;
    state.marker.delete(id).await.map_err(|e| to_error_dto(&e))
}

#[tauri::command]
pub async fn marker_create(
    state: State<'_, AppState>,
    request: MarkerCreateRequest,
) -> Result<MarkerDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_marker_create(&state, request).await }).await
}

#[tauri::command]
pub async fn marker_list(
    state: State<'_, AppState>,
    request: MarkerListRequest,
) -> Result<Vec<MarkerDto>, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_marker_list(&state, request).await }).await
}

#[tauri::command]
pub async fn marker_list_all(
    state: State<'_, AppState>,
    request: MarkerListAllRequest,
) -> Result<Vec<MarkerDto>, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_marker_list_all(&state, request).await }).await
}

#[tauri::command]
pub async fn marker_delete(
    state: State<'_, AppState>,
    request: MarkerDeleteRequest,
) -> Result<bool, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_marker_delete(&state, request).await }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn marker_list_invalid_id_is_stable_and_does_not_write() {
        let state = AppState::new(Arc::new(
            haven_infrastructure::Db::open_in_memory().unwrap(),
        ));
        let error = run_marker_list(
            &state,
            MarkerListRequest {
                media_item_id: "not-a-uuid".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "INVALID_ID");
        assert!(!error.retryable);
    }

    #[tokio::test]
    async fn marker_delete_invalid_id_is_stable() {
        let state = AppState::new(Arc::new(
            haven_infrastructure::Db::open_in_memory().unwrap(),
        ));
        let error = run_marker_delete(
            &state,
            MarkerDeleteRequest {
                marker_id: "not-a-uuid".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "INVALID_ID");
        assert!(!error.retryable);
    }

    #[tokio::test]
    async fn marker_list_all_empty_db_returns_empty() {
        let state = AppState::new(Arc::new(
            haven_infrastructure::Db::open_in_memory().unwrap(),
        ));
        let result = run_marker_list_all(&state, MarkerListAllRequest { limit: None })
            .await
            .unwrap();
        assert!(result.is_empty());
    }
}
