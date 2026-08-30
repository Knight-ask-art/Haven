//! Search History Commands（V02-SETTINGS-PRIVACY-DATA-007）。
//!
//! 搜索历史与播放/阅读历史分开：这些命令只访问 `search_history` 表，
//! 不会清理进度、收藏、标记、离线资源或原始媒体。

use tauri::State;

use haven_application::wire::{
    ErrorDto, SearchHistoryEntryDto, SearchHistoryListRequest, SearchHistoryRecordRequest,
    SearchHistoryRemoveRequest,
};

use crate::ipc::{run_blocking, to_error_dto};
use crate::state::AppState;

const DEFAULT_LIMIT: u32 = 10;

pub async fn run_search_history_list(
    state: &AppState,
    request: SearchHistoryListRequest,
) -> Result<Vec<SearchHistoryEntryDto>, ErrorDto> {
    state
        .search_history
        .list(request.limit.or(Some(DEFAULT_LIMIT)))
        .await
        .map_err(|error| to_error_dto(&error))
}

pub async fn run_search_history_record(
    state: &AppState,
    request: SearchHistoryRecordRequest,
) -> Result<SearchHistoryEntryDto, ErrorDto> {
    state
        .search_history
        .record(&request.term)
        .await
        .map_err(|error| to_error_dto(&error))
}

pub async fn run_search_history_remove(
    state: &AppState,
    request: SearchHistoryRemoveRequest,
) -> Result<bool, ErrorDto> {
    state
        .search_history
        .remove(&request.term)
        .await
        .map_err(|error| to_error_dto(&error))
}

pub async fn run_search_history_clear(state: &AppState) -> Result<(), ErrorDto> {
    state
        .search_history
        .clear()
        .await
        .map(|_| ())
        .map_err(|error| to_error_dto(&error))
}

#[tauri::command]
pub async fn search_history_list(
    state: State<'_, AppState>,
    request: SearchHistoryListRequest,
) -> Result<Vec<SearchHistoryEntryDto>, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_search_history_list(&state, request).await }).await
}

#[tauri::command]
pub async fn search_history_record(
    state: State<'_, AppState>,
    request: SearchHistoryRecordRequest,
) -> Result<SearchHistoryEntryDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_search_history_record(&state, request).await }).await
}

#[tauri::command]
pub async fn search_history_remove(
    state: State<'_, AppState>,
    request: SearchHistoryRemoveRequest,
) -> Result<bool, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_search_history_remove(&state, request).await }).await
}

#[tauri::command]
pub async fn search_history_clear(state: State<'_, AppState>) -> Result<(), ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_search_history_clear(&state).await }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn empty_database_is_safe_and_clear_is_idempotent() {
        let state = AppState::new(Arc::new(
            haven_infrastructure::Db::open_in_memory().unwrap(),
        ));
        assert!(
            run_search_history_list(&state, SearchHistoryListRequest { limit: None })
                .await
                .unwrap()
                .is_empty()
        );
        run_search_history_clear(&state).await.unwrap();
        run_search_history_clear(&state).await.unwrap();
    }

    #[tokio::test]
    async fn invalid_term_is_mapped_to_validation_error() {
        let state = AppState::new(Arc::new(
            haven_infrastructure::Db::open_in_memory().unwrap(),
        ));
        let error =
            run_search_history_record(&state, SearchHistoryRecordRequest { term: "  ".into() })
                .await
                .unwrap_err();
        assert_eq!(error.code, "INVALID_ARGUMENT");
    }
}
