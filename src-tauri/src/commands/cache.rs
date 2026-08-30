//! 受控技术缓存清理 Command（V02-SETTINGS-PRIVACY-DATA-007）。
//!
//! 只做 Wire 校验、调用 Application Service 和错误映射；不在此处拼路径、写 SQL
//! 或删除文件。当前实际实现为 Artwork Cache，其他枚举值会得到明确不可用错误。

use tauri::State;

use haven_application::wire::{CacheClearResultDto, CacheScopeDto, ErrorDto};

use crate::ipc::{run_blocking, to_error_dto};
use crate::state::AppState;

pub async fn run_cache_clear(
    state: &AppState,
    scope: CacheScopeDto,
) -> Result<CacheClearResultDto, ErrorDto> {
    state
        .cache
        .clear(scope)
        .await
        .map_err(|error| to_error_dto(&error))
}

#[tauri::command]
pub async fn cache_clear(
    state: State<'_, AppState>,
    scope: CacheScopeDto,
) -> Result<CacheClearResultDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_cache_clear(&state, scope).await }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn unsupported_scope_is_reported_without_touching_database() {
        let state = AppState::new(Arc::new(
            haven_infrastructure::Db::open_in_memory().unwrap(),
        ));
        let error = run_cache_clear(&state, CacheScopeDto::Thumbnails)
            .await
            .unwrap_err();
        assert_eq!(error.code, "CACHE_SCOPE_UNAVAILABLE");
    }
}
