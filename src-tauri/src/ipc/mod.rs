//! IPC 错误映射与 DTO（IPC-TAURI-001A/B）。
//!
//! Command 只允许把 `AppError` 映射为冻结的 `ErrorDto`（code/userMessage/retryable）；
//! Domain Entity、DB Row、rusqlite 错误不得直接出 IPC（P0-3：StorageLocationDto 显式映射）。

use haven_application::wire::ErrorDto;
pub use haven_application::wire::{ScanCancelResultDto, StorageLocationDto};
use haven_common::AppError;

/// Tauri transport name for the logical `favorite.changed` contract event.
/// Tauri 2 rejects dotted event names (`IllegalEventName`), so this boundary
/// adapts the contract name to a legal hyphenated transport name.
pub const FAVORITE_CHANGED_TRANSPORT_EVENT: &str = "favorite-changed";

/// Tauri transport name for the logical `settings.changed` contract event.
/// Tauri 2 rejects dotted event names (`IllegalEventName`), so the IPC
/// boundary adapts the stable contract name to this legal hyphenated name.
pub const SETTINGS_CHANGED_TRANSPORT_EVENT: &str = "settings-changed";

/// Tauri transport name for the logical `library.changed` contract event
/// （扫描终态后的库失效通知；BE-SCAN-001 第三步）。
pub const LIBRARY_CHANGED_TRANSPORT_EVENT: &str = "library-changed";
/// 契约 §36.8：复用既有事件名 metadata.changed（transport 连字符适配）。
pub const METADATA_CHANGED_TRANSPORT_EVENT: &str = "metadata-changed";

/// 把后端稳定错误映射为 Wire ErrorDto（C-02 冻结形状）。
pub fn to_error_dto(err: &AppError) -> ErrorDto {
    ErrorDto {
        code: err.code().as_str().to_owned(),
        user_message: err.user_message().to_owned(),
        retryable: err.retryable(),
    }
}

/// ID 解析失败（不可信输入）→ 稳定 `INVALID_ID`。
pub fn invalid_id() -> ErrorDto {
    ErrorDto {
        code: "INVALID_ID".into(),
        user_message: "ID 格式非法".into(),
        retryable: false,
    }
}

/// 参数校验失败（不可信输入）→ 稳定 `INVALID_ARGUMENT`。
/// 文本类公开参数（display name / 搜索词等）必须在命令层限定长度上限，
/// 防止任意长度字符串进入 SQLite 与列表 UI（审查 P2）。
pub fn invalid_argument(message: impl Into<String>) -> ErrorDto {
    ErrorDto {
        code: "INVALID_ARGUMENT".into(),
        user_message: message.into(),
        retryable: false,
    }
}

/// `settings.changed` 事件负载（P1-8：仅实际变化时发布；revision 与 Result 同源）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsChangedDto {
    pub schema_version: u32,
    pub at: String,
    pub operation_id: String,
    pub sequence: u32,
    pub section: String,
    pub revision: String,
}

/// `stream_close` 请求（grant 为不透明 UUID）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamCloseRequest {
    pub session_id: String,
}

/// 在 blocking worker 上执行命令核心逻辑（P1-9：同步 SQLite/FS 不阻塞 Tauri async runtime）。
/// `f` 返回的命令 future 在闭包内由 block_on 驱动完成（借用闭包捕获的 state，
/// 不要求 future 本身 'static）；`state` 必须已 clone 进闭包。
pub async fn run_blocking<F, Fut, T>(f: F) -> Result<T, ErrorDto>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, ErrorDto>>,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || tauri::async_runtime::block_on(f()))
        .await
        .map_err(|_| ErrorDto {
            code: "INTERNAL_ERROR".into(),
            user_message: "后台任务执行失败".into(),
            retryable: false,
        })?
}
