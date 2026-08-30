use tauri::State;

use haven_application::wire::{ErrorDto, ResourceListByMediaItemRequest, ResourceListDto};

use crate::ipc::{run_blocking, to_error_dto};
use crate::state::AppState;

pub async fn run_resource_list_by_media_item(
    state: &AppState,
    request: ResourceListByMediaItemRequest,
) -> Result<ResourceListDto, ErrorDto> {
    state
        .resource
        .list_by_media_item(request)
        .await
        .map_err(|e| to_error_dto(&e))
}

#[tauri::command]
pub async fn resource_list_by_media_item(
    state: State<'_, AppState>,
    request: ResourceListByMediaItemRequest,
) -> Result<ResourceListDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_resource_list_by_media_item(&state, request).await })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn invalid_media_item_id_returns_invalid_id() {
        let state = AppState::new(Arc::new(
            haven_infrastructure::Db::open_in_memory().unwrap(),
        ));
        let error = run_resource_list_by_media_item(
            &state,
            ResourceListByMediaItemRequest {
                media_item_id: "not-a-uuid".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "INVALID_ID");
    }
}
