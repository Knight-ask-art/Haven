//! 自定义来源命令（V2-H 收尾批次；契约 §36.2 演进；Task 1 增补 RSS/Atom kind；
//! Task 2 增补 Komga/Kavita 自托管漫画库）。
//!
//! - `source_add` / `source_update` / `source_remove`：自定义源生命周期管理。
//! - `source_set_credential`：secret 只写系统 keyring（ADR-001）；持久化仅存 credential_ref；
//!   secret 与 target 禁止出 IPC、日志。RSS/Atom 订阅源不使用该路径；
//!   Komga/Kavita 的 API key 走该路径（只作为请求头注入，绝不进 URL）。
//! - `source_remove` 在来源事务中先撤销引用并登记凭据／配置缓存的清理义务，再清理外部
//!   存储；失败的义务保留供重试。此顺序仅适用于来源，不改变其他凭据家族的契约。
//!
//! Command 只做 `sourceId` 路由键形状校验、Application 调用与错误映射；显示名/端点
//! 这类用户文本的长度与格式由 Application 注册表服务按来源种类唯一裁定，命令层不再
//! 用字节数重复判一遍（两层口径不同会让同一份输入在命令层被拒、在服务层通过）。

use tauri::State;

use haven_application::services::source_registry::CustomSourceKind;
use haven_application::wire::{
    ErrorDto, SourceAddKindDto, SourceAddRequest, SourceAddResult, SourceRemoveRequest,
    SourceRemoveResult, SourceSetCredentialRequest, SourceUpdateRequest, SourceUpdateResult,
};

use crate::ipc::{invalid_argument, run_blocking, to_error_dto};
use crate::state::AppState;

/// `source_add` 命令核心。种类由 `request.kind` 决定，缺省为 OPDS，
/// 因此新增该字段之前的调用行为完全不变。
///
/// 显示名与端点的空值/长度**不在这里重复校验**：注册表服务按来源种类给出唯一口径
/// （显示名按字符数计，端点按种类分别计），命令层再用字节数判一遍只会让同一份输入
/// 在两层得到不同结论。
pub async fn run_source_add(
    state: &AppState,
    request: SourceAddRequest,
) -> Result<SourceAddResult, ErrorDto> {
    let added = match request.kind {
        SourceAddKindDto::Opds => {
            state
                .source_registry
                .add_custom_source(&request.display_name, &request.endpoint)
                .await
        }
        SourceAddKindDto::Feed => {
            state
                .source_registry
                .add_feed_source(&request.display_name, &request.endpoint)
                .await
        }
        SourceAddKindDto::Komga | SourceAddKindDto::Kavita => {
            let kind = match request.kind {
                SourceAddKindDto::Komga => CustomSourceKind::Komga,
                _ => CustomSourceKind::Kavita,
            };
            state
                .source_registry
                .add_comic_library_source(&request.display_name, &request.endpoint, kind)
                .await
        }
    };
    added.map_err(|e| to_error_dto(&e))
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
///
/// 显示名/端点的长度与格式交给注册表服务（唯一口径）；这里只保留 `sourceId` 的
/// 字节形状守卫，因为它是路由键而不是用户文本。
pub async fn run_source_update(
    state: &AppState,
    request: SourceUpdateRequest,
) -> Result<SourceUpdateResult, ErrorDto> {
    if request.source_id.trim().is_empty() || request.source_id.len() > 64 {
        // sourceId 是路由键，不是用户文本：这里按字节限制它的形状，长度/格式的
        // 语义校验由注册表服务按来源种类给出。
        return Err(invalid_argument("sourceId 非法"));
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

/// `source_remove` 命令核心：先原子撤销来源引用，再执行可重试的外部清理。
pub async fn run_source_remove(
    state: &AppState,
    request: SourceRemoveRequest,
) -> Result<SourceRemoveResult, ErrorDto> {
    if request.source_id.trim().is_empty() || request.source_id.len() > 64 {
        return Err(invalid_argument("sourceId 非法"));
    }
    // 删除路径同时清掉该来源的 last-known-good 原始配置（TVBox 配置来源会用到）。
    state
        .source_registry
        .remove_custom_source(
            &request.source_id,
            state.source_credential_store.as_ref(),
            &state.repos.source_config_cache,
        )
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
        // 凭据不是路由键：这里保留字节上限，避免把任意大的 secret 交给 keyring；
        // 各来源家族自己的字符数上限由 Application 凭据路径判定。
        if secret.len() > 512 {
            return Err(invalid_argument("凭据内容超长"));
        }
    }
    state
        .source_registry
        .set_custom_source_credential(&request, state.source_credential_store.as_ref())
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
                kind: SourceAddKindDto::Opds,
            },
            SourceAddRequest {
                display_name: "x".repeat(101),
                endpoint: "https://a.example.com/opds".into(),
                kind: SourceAddKindDto::Opds,
            },
            SourceAddRequest {
                display_name: "ok".into(),
                endpoint: "".into(),
                kind: SourceAddKindDto::Opds,
            },
            SourceAddRequest {
                display_name: "ok".into(),
                endpoint: "ftp://a/x".into(),
                kind: SourceAddKindDto::Opds,
            },
            // 订阅源：明文 HTTP 与带 query 的地址都必须被拒绝。
            SourceAddRequest {
                display_name: "ok".into(),
                endpoint: "http://a.example.com/rss.xml".into(),
                kind: SourceAddKindDto::Feed,
            },
            SourceAddRequest {
                display_name: "ok".into(),
                endpoint: "https://a.example.com/rss.xml?token=secret".into(),
                kind: SourceAddKindDto::Feed,
            },
        ] {
            let err = run_source_add(&state, bad).await.unwrap_err();
            assert_eq!(err.code, "INVALID_ARGUMENT");
        }
    }

    #[tokio::test]
    async fn add_accepts_opds_or_feed_kinds_and_defaults_stay_backwards_compatible() {
        let state = app_state();

        // 缺省 kind（旧请求）仍然是 OPDS。
        let legacy: SourceAddRequest = serde_json::from_value(serde_json::json!({
            "displayName": "旧调用",
            "endpoint": "https://legacy.example.invalid/opds/"
        }))
        .expect("缺少 kind 的旧请求必须仍可反序列化");
        assert_eq!(legacy.kind, SourceAddKindDto::Opds);
        let legacy_added = run_source_add(&state, legacy).await.unwrap();
        assert!(legacy_added.source_id.starts_with("custom_"));
        assert!(!legacy_added.source_id.starts_with("custom_feed_"));

        let feed_added = run_source_add(
            &state,
            SourceAddRequest {
                display_name: "我的订阅".into(),
                endpoint: "https://feeds.example.invalid/rss.xml".into(),
                kind: SourceAddKindDto::Feed,
            },
        )
        .await
        .unwrap();
        assert!(feed_added.source_id.starts_with("custom_feed_"));

        let registry = state.source_registry.list().await.unwrap();
        let feed = registry
            .sources
            .iter()
            .find(|source| source.source_id == feed_added.source_id)
            .unwrap();
        assert_eq!(
            feed.categories,
            vec![haven_application::wire::SourceCategoryDto::Periodical]
        );
        assert!(
            !feed.notes.contains("feeds.example.invalid"),
            "来源说明不得回显端点"
        );
    }

    #[tokio::test]
    async fn add_update_remove_lifecycle() {
        let state = app_state();
        let added = run_source_add(
            &state,
            SourceAddRequest {
                display_name: "我的书源".into(),
                endpoint: "https://example.invalid/opds/".into(),
                kind: SourceAddKindDto::Opds,
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
                    username: None,
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
