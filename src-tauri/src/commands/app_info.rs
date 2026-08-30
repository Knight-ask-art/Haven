//! About / Diagnostics Commands（V02-SETTINGS-ABOUT-DIAGNOSTICS-008）。
//!
//! 公开接口只有固定的查询和三个语义化目录操作；不接受 path、URL 或其它
//! 文件系统参数。所有平台 IO 由 Infrastructure 的 AppInfoPort 实现。

use tauri::State;

use haven_application::services::DirectoryKind;
use haven_application::wire::{AppInfoDto, ErrorDto};

use crate::ipc::{run_blocking, to_error_dto};
use crate::state::AppState;

pub async fn run_app_info_get(state: &AppState) -> Result<AppInfoDto, ErrorDto> {
    state.app_info.get().map_err(|error| to_error_dto(&error))
}

pub async fn run_open_directory(state: &AppState, kind: DirectoryKind) -> Result<(), ErrorDto> {
    state
        .app_info
        .open_directory(kind)
        .map_err(|error| to_error_dto(&error))
}

#[tauri::command]
pub async fn app_info_get(state: State<'_, AppState>) -> Result<AppInfoDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_app_info_get(&state).await }).await
}

#[tauri::command]
pub async fn open_data_directory(state: State<'_, AppState>) -> Result<(), ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_open_directory(&state, DirectoryKind::Data).await }).await
}

#[tauri::command]
pub async fn open_logs_directory(state: State<'_, AppState>) -> Result<(), ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_open_directory(&state, DirectoryKind::Logs).await }).await
}

#[tauri::command]
pub async fn open_cache_directory(state: State<'_, AppState>) -> Result<(), ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_open_directory(&state, DirectoryKind::Cache).await })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn app_info_query_returns_redacted_runtime_projection() {
        let state = AppState::new(Arc::new(
            haven_infrastructure::Db::open_in_memory().unwrap(),
        ));
        let info = tauri::async_runtime::block_on(run_app_info_get(&state)).unwrap();
        assert_eq!(info.schema_version, 1);
        assert_eq!(info.source_pack_version.as_deref(), Some("builtin-1"));
        assert_eq!(info.directories.len(), 3);
        let encoded = serde_json::to_string(&info).unwrap();
        assert!(!encoded.contains("Users"));
        assert!(!encoded.contains("haven.db"));
    }
}
