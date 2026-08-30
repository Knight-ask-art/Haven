//! `history_list` / `history_clear` commands（契约 §23）。

use tauri::State;

use haven_application::wire::{ErrorDto, HistoryEntryDto, HistoryListRequest};

use crate::ipc::{run_blocking, to_error_dto};
use crate::state::AppState;

#[cfg(test)]
use haven_domain::ids::HistoryEntryId;

const DEFAULT_LIMIT: u32 = 50;

pub async fn run_history_list(
    state: &AppState,
    request: HistoryListRequest,
) -> Result<Vec<HistoryEntryDto>, ErrorDto> {
    let limit = request.limit.unwrap_or(DEFAULT_LIMIT);
    state
        .history
        .recent(limit)
        .await
        .map_err(|e| to_error_dto(&e))
}

#[tauri::command]
pub async fn history_list(
    state: State<'_, AppState>,
    request: HistoryListRequest,
) -> Result<Vec<HistoryEntryDto>, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_history_list(&state, request).await }).await
}

pub async fn run_history_clear(state: &AppState) -> Result<(), ErrorDto> {
    state.history.clear().await.map_err(|e| to_error_dto(&e))
}

#[tauri::command]
pub async fn history_clear(state: State<'_, AppState>) -> Result<(), ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_history_clear(&state).await }).await
}

/// 解析并强制要求 canonical 文本，避免同一 ID 通过多种字符串形态进入 IPC。
#[cfg(test)]
fn parse_canonical_history_id(value: &str) -> Result<HistoryEntryId, ErrorDto> {
    let id: HistoryEntryId = value.parse().map_err(|_| ErrorDto {
        code: "INVALID_ID".into(),
        user_message: "ID 格式非法".into(),
        retryable: false,
    })?;
    if id.to_string() != value {
        return Err(ErrorDto {
            code: "INVALID_ID".into(),
            user_message: "ID 格式非法".into(),
            retryable: false,
        });
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn history_list_default_limit_no_write() {
        let state = AppState::new(Arc::new(
            haven_infrastructure::Db::open_in_memory().unwrap(),
        ));
        let result = run_history_list(&state, HistoryListRequest { limit: None })
            .await
            .unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn history_clear_is_idempotent_on_empty_db() {
        let state = AppState::new(Arc::new(
            haven_infrastructure::Db::open_in_memory().unwrap(),
        ));
        run_history_clear(&state).await.unwrap();
        run_history_clear(&state).await.unwrap();
    }

    #[test]
    fn rejects_non_canonical_history_id() {
        assert!(parse_canonical_history_id("not-a-uuid").is_err());
    }
}
