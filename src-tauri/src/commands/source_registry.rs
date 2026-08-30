//! `source_registry_list` / `source_registry_set`
//! （契约 §36.2 / CONTRACT-V02-SOURCE-REGISTRY-001）。
//!
//! Command 只做不可信输入校验、Application 调用与错误映射；
//! 启用状态持久化由 SourceRegistryService 经 SQLite settings 存储完成。

use tauri::State;

use haven_application::services::SourceRegistryService;
use haven_application::wire::{
    ErrorDto, SourceRegistryDto, SourceRegistrySetRequest, SourceRegistrySetResult,
};

use crate::ipc::{invalid_argument, run_blocking, to_error_dto};
use crate::state::AppState;

/// 命令核心逻辑（纯函数，可脱离 Tauri runtime 测试）。
pub async fn run_source_registry_list(
    registry: &SourceRegistryService,
) -> Result<SourceRegistryDto, ErrorDto> {
    registry.list().await.map_err(|e| to_error_dto(&e))
}

/// Tauri Command 薄包装（SQLite KV 读取走 blocking worker）。
#[tauri::command]
pub async fn source_registry_list(
    state: State<'_, AppState>,
) -> Result<SourceRegistryDto, ErrorDto> {
    let registry = state.source_registry.clone();
    run_blocking(move || async move { run_source_registry_list(&registry).await }).await
}

/// 命令核心逻辑（幂等设置；未知 sourceId → INVALID_ARGUMENT）。
pub async fn run_source_registry_set(
    registry: &SourceRegistryService,
    request: SourceRegistrySetRequest,
) -> Result<SourceRegistrySetResult, ErrorDto> {
    if request.source_id.trim().is_empty() || request.source_id.len() > 64 {
        return Err(invalid_argument("sourceId 非法"));
    }
    registry.set(request).await.map_err(|e| to_error_dto(&e))
}

/// Tauri Command 薄包装（SQLite 写入走 blocking worker）。
#[tauri::command]
pub async fn source_registry_set(
    state: State<'_, AppState>,
    request: SourceRegistrySetRequest,
) -> Result<SourceRegistrySetResult, ErrorDto> {
    let registry = state.source_registry.clone();
    run_blocking(move || async move { run_source_registry_set(&registry, request).await }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_application::services::ports::SourceRegistryPorts;
    use haven_infrastructure::db::repos::SqliteRepositories;
    use haven_infrastructure::Db;
    use std::sync::Arc;

    fn service() -> SourceRegistryService {
        let repos = Arc::new(SqliteRepositories::new(Arc::new(
            Db::open_in_memory().unwrap(),
        )));
        let settings: Arc<dyn SourceRegistryPorts> = repos;
        SourceRegistryService::new(settings)
    }

    #[tokio::test]
    async fn list_returns_builtin_catalog() {
        let dto = run_source_registry_list(&service()).await.unwrap();
        assert_eq!(dto.schema_version, 2);
        assert!(!dto.sources.is_empty());
    }

    #[tokio::test]
    async fn set_unknown_source_maps_to_invalid_argument() {
        let err = run_source_registry_set(
            &service(),
            SourceRegistrySetRequest {
                source_id: "nope".into(),
                enabled: true,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT");
    }
}
