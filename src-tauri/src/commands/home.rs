//! `home_get` command（契约 §14.1；G-0.1 首页 Continue / Recently Added 真实投影）。

use tauri::State;

use haven_application::wire::{ErrorDto, HomeDto};

use crate::ipc::{run_blocking, to_error_dto};
use crate::state::AppState;

pub async fn run_home_get(state: &AppState) -> Result<HomeDto, ErrorDto> {
    state.home.get().await.map_err(|e| to_error_dto(&e))
}

#[tauri::command]
pub async fn home_get(state: State<'_, AppState>) -> Result<HomeDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_home_get(&state).await }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn home_get_empty_db_returns_empty_groups() {
        let state = AppState::new(Arc::new(
            haven_infrastructure::Db::open_in_memory().unwrap(),
        ));
        let home = run_home_get(&state).await.unwrap();
        assert_eq!(home.schema_version, 1);
        assert!(home.continue_items.is_empty());
        assert!(home.recently_added.is_empty());
        assert!(home.shelves.is_empty());
    }
}
