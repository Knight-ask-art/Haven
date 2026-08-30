//! 自定义 OPDS 书源命令（V2-H 收尾批次；契约 §36.2 演进）。
//!
//! - `source_add` / `source_update` / `source_remove`：自定义源生命周期管理。
//! - `source_set_credential`：secret 只写系统 keyring（ADR-001）；持久化仅存 credential_ref；
//!   secret 与 target 禁止出 IPC、日志。
//! - `source_remove` 走 ADR-001 删除顺序：先删系统凭据，再清持久化引用。
//!
//! Command 只做不可信输入校验、Application 调用与错误映射。

use tauri::State;

use haven_application::wire::{
    ErrorDto, SourceAddRequest, SourceAddResult, SourceRemoveRequest, SourceRemoveResult,
    SourceSetCredentialRequest, SourceUpdateRequest, SourceUpdateResult,
};

use crate::ipc::{invalid_argument, run_blocking, to_error_dto};
use crate::state::AppState;

/// `source_add` 命令核心。
pub async fn run_source_add(
    state: &AppState,
    request: SourceAddRequest,
) -> Result<SourceAddResult, ErrorDto> {
    if request.display_name.trim().is_empty() || request.display_name.len() > 100 {
        return Err(invalid_argument("显示名不能为空且不超过 100 字符"));
    }
    if request.endpoint.trim().is_empty() || request.endpoint.len() > 500 {
        return Err(invalid_argument("端点地址不能为空且不超过 500 字符"));
    }
    state
        .source_registry
        .add_custom_source(&request.display_name, &request.endpoint)
        .await
        .map_err(|e| to_error_dto(&e))
}

#[tauri::command]
pub async fn source_add(
    state: State<'_, AppState>,
    request: SourceAddRequest,
) -> Result<SourceAddResult, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_source_add(&state, request).await }).await
}

/// `source_update` 命令核心（幂等）。
pub async fn run_source_update(
    state: &AppState,
    request: SourceUpdateRequest,
) -> Result<SourceUpdateResult, ErrorDto> {
    if request.source_id.trim().is_empty() || request.source_id.len() > 64 {
        return Err(invalid_argument("sourceId 非法"));
    }
    if let Some(name) = &request.display_name {
        if name.trim().is_empty() || name.len() > 100 {
            return Err(invalid_argument("显示名不能为空且不超过 100 字符"));
        }
    }
    if let Some(endpoint) = &request.endpoint {
        if endpoint.trim().is_empty() || endpoint.len() > 500 {
            return Err(invalid_argument("端点地址不能为空且不超过 500 字符"));
        }
    }
    state
        .source_registry
        .update_custom_source(request)
        .await
        .map_err(|e| to_error_dto(&e))
}

#[tauri::command]
pub async fn source_update(
    state: State<'_, AppState>,
    request: SourceUpdateRequest,
) -> Result<SourceUpdateResult, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_source_update(&state, request).await }).await
}

/// `source_remove` 命令核心（ADR-001 删除顺序：先 keyring 后持久化引用）。
pub async fn run_source_remove(
    state: &AppState,
    request: SourceRemoveRequest,
) -> Result<SourceRemoveResult, ErrorDto> {
    if request.source_id.trim().is_empty() || request.source_id.len() > 64 {
        return Err(invalid_argument("sourceId 非法"));
    }
    let store =
        haven_infrastructure::credential::credential_store().map_err(|e| to_error_dto(&e))?;
    state
        .source_registry
        .remove_custom_source(&request.source_id, store.as_ref())
        .await
        .map_err(|e| to_error_dto(&e))
}

#[tauri::command]
pub async fn source_remove(
    state: State<'_, AppState>,
    request: SourceRemoveRequest,
) -> Result<SourceRemoveResult, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_source_remove(&state, request).await }).await
}

/// `source_set_credential` 命令核心。secret 单向写入 keyring，响应无 secret。
pub async fn run_source_set_credential(
    state: &AppState,
    request: SourceSetCredentialRequest,
) -> Result<(), ErrorDto> {
    if request.source_id.trim().is_empty() || request.source_id.len() > 64 {
        return Err(invalid_argument("sourceId 非法"));
    }
    if let Some(secret) = &request.secret {
        if secret.is_empty() {
            return Err(invalid_argument("凭据内容不能为空"));
        }
        if secret.len() > 512 {
            return Err(invalid_argument("凭据内容超长"));
        }
    }
    let store =
        haven_infrastructure::credential::credential_store().map_err(|e| to_error_dto(&e))?;
    state
        .source_registry
        .set_custom_source_credential(&request, store.as_ref())
        .await
        .map_err(|e| to_error_dto(&e))
}

#[tauri::command]
pub async fn source_set_credential(
    state: State<'_, AppState>,
    request: SourceSetCredentialRequest,
) -> Result<(), ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_source_set_credential(&state, request).await }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn app_state() -> AppState {
        AppState::new(Arc::new(
            haven_infrastructure::Db::open_in_memory().unwrap(),
        ))
    }

    #[tokio::test]
    async fn add_rejects_blank_and_oversized_inputs() {
        let state = app_state();
        for bad in [
            SourceAddRequest {
                display_name: " ".into(),
                endpoint: "https://a.example.com/opds".into(),
            },
            SourceAddRequest {
                display_name: "x".repeat(101),
                endpoint: "https://a.example.com/opds".into(),
            },
            SourceAddRequest {
                display_name: "ok".into(),
                endpoint: "".into(),
            },
            SourceAddRequest {
                display_name: "ok".into(),
                endpoint: "ftp://a/x".into(),
            },
        ] {
            let err = run_source_add(&state, bad).await.unwrap_err();
            assert_eq!(err.code, "INVALID_ARGUMENT");
        }
    }

    #[tokio::test]
    async fn add_update_remove_lifecycle() {
        let state = app_state();
        let added = run_source_add(
            &state,
            SourceAddRequest {
                display_name: "我的书源".into(),
                endpoint: "https://example.invalid/opds/".into(),
            },
        )
        .await
        .unwrap();
        assert!(added.source_id.starts_with("custom_"));

        // 注册表应包含自定义源且默认停用。
        let registry = state.source_registry.list().await.unwrap();
        let custom = registry
            .sources
            .iter()
            .find(|s| s.source_id == added.source_id)
            .unwrap();
        assert!(!custom.enabled);
        assert!(custom.endpoint_configured);

        // 更新显示名与端点。
        run_source_update(
            &state,
            SourceUpdateRequest {
                source_id: added.source_id.clone(),
                display_name: Some("改名".into()),
                endpoint: Some("https://other.example.org/feed".into()),
            },
        )
        .await
        .unwrap();
        let registry = state.source_registry.list().await.unwrap();
        let custom = registry
            .sources
            .iter()
            .find(|s| s.source_id == added.source_id)
            .unwrap();
        assert_eq!(custom.display_name, "改名");

        // 内置源不可经 update/remove 修改。
        let err = run_source_update(
            &state,
            SourceUpdateRequest {
                source_id: "cms10".into(),
                display_name: Some("hack".into()),
                endpoint: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT");

        // 删除（无凭据 → 幂等 RefCleared 投影）。
        let removed = run_source_remove(
            &state,
            SourceRemoveRequest {
                source_id: added.source_id.clone(),
            },
        )
        .await
        .unwrap();
        assert!(!removed.credential_deleted);
        let registry = state.source_registry.list().await.unwrap();
        assert!(
            !registry
                .sources
                .iter()
                .any(|s| s.source_id == added.source_id),
            "删除后注册表不再包含该源"
        );
    }

    #[tokio::test]
    async fn remove_unknown_maps_to_not_found() {
        let state = app_state();
        let err = run_source_remove(
            &state,
            SourceRemoveRequest {
                source_id: "custom_nope123456".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "RESOURCE_NOT_FOUND");
    }

    #[tokio::test]
    async fn set_credential_rejects_builtin_and_empty_secret() {
        let state = app_state();
        for (source_id, secret) in [("cms10", None), ("custom_missing0000", Some(String::new()))] {
            let err = run_source_set_credential(
                &state,
                SourceSetCredentialRequest {
                    source_id: source_id.into(),
                    secret,
                },
            )
            .await
            .unwrap_err();
            assert_eq!(err.code, "INVALID_ARGUMENT");
        }
    }
}
