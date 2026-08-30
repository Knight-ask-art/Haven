use tauri::State;

use haven_application::wire::{
    EditionDetailDto, EditionGetRequest, EditionListByWorkRequest, EditionSummaryDto, ErrorDto,
    PageDto, WorkDetailHeaderDto, WorkGetRequest,
};

use crate::ipc::{invalid_id, run_blocking, to_error_dto};
use crate::state::AppState;

pub async fn run_work_get(
    state: &AppState,
    request: WorkGetRequest,
) -> Result<WorkDetailHeaderDto, ErrorDto> {
    let id = request.work_id.parse().map_err(|_| invalid_id())?;
    state.work.get(id).await.map_err(|e| to_error_dto(&e))
}

#[tauri::command]
pub async fn work_get(
    state: State<'_, AppState>,
    request: WorkGetRequest,
) -> Result<WorkDetailHeaderDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_work_get(&state, request).await }).await
}

pub async fn run_edition_list_by_work(
    state: &AppState,
    request: EditionListByWorkRequest,
) -> Result<PageDto<EditionSummaryDto>, ErrorDto> {
    state
        .work
        .list_editions(request)
        .await
        .map_err(|e| to_error_dto(&e))
}

pub async fn run_edition_get(
    state: &AppState,
    request: EditionGetRequest,
) -> Result<EditionDetailDto, ErrorDto> {
    state
        .work
        .get_edition(request)
        .await
        .map_err(|e| to_error_dto(&e))
}

#[tauri::command]
pub async fn edition_get(
    state: State<'_, AppState>,
    request: EditionGetRequest,
) -> Result<EditionDetailDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_edition_get(&state, request).await }).await
}

#[tauri::command]
pub async fn edition_list_by_work(
    state: State<'_, AppState>,
    request: EditionListByWorkRequest,
) -> Result<PageDto<EditionSummaryDto>, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_edition_list_by_work(&state, request).await }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn invalid_work_id_returns_invalid_id() {
        let state = AppState::new(Arc::new(
            haven_infrastructure::Db::open_in_memory().unwrap(),
        ));
        let err = run_work_get(
            &state,
            WorkGetRequest {
                work_id: "not-a-uuid".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "INVALID_ID");
    }

    #[tokio::test]
    async fn invalid_edition_list_work_id_returns_invalid_id() {
        let state = AppState::new(Arc::new(
            haven_infrastructure::Db::open_in_memory().unwrap(),
        ));
        let err = run_edition_list_by_work(
            &state,
            EditionListByWorkRequest {
                work_id: "not-a-uuid".into(),
                cursor: None,
                limit: 10,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "INVALID_ID");
    }

    #[tokio::test]
    async fn invalid_edition_get_id_returns_invalid_id() {
        let state = AppState::new(Arc::new(
            haven_infrastructure::Db::open_in_memory().unwrap(),
        ));
        let err = run_edition_get(
            &state,
            EditionGetRequest {
                edition_id: "not-a-uuid".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "INVALID_ID");
    }
}
