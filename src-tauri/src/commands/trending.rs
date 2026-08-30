//! Trending 热榜（搜索页技术缓存 Query/Refresh）。

use tauri::State;

use haven_application::wire::{ErrorDto, TrendingBoardsDto};

use crate::{ipc::to_error_dto, state::AppState};

async fn run_trending_boards_get(state: &AppState) -> Result<TrendingBoardsDto, ErrorDto> {
    state.trending.boards().await.map_err(|e| to_error_dto(&e))
}

async fn run_trending_boards_refresh(state: &AppState) -> Result<TrendingBoardsDto, ErrorDto> {
    state.trending.refresh().await.map_err(|e| to_error_dto(&e))
}

#[tauri::command]
pub async fn trending_boards_get(
    state: State<'_, AppState>,
) -> Result<TrendingBoardsDto, ErrorDto> {
    let state = (*state.inner()).clone();
    crate::ipc::run_blocking(move || async move { run_trending_boards_get(&state).await }).await
}

#[tauri::command]
pub async fn trending_boards_refresh(
    state: State<'_, AppState>,
) -> Result<TrendingBoardsDto, ErrorDto> {
    let state = (*state.inner()).clone();
    crate::ipc::run_blocking(move || async move { run_trending_boards_refresh(&state).await }).await
}
