//! StorageLocation Commands（IPC-TAURI-001B；P0-1 修复：公开命令不再接收裸路径）。
//!
//! 路径授权模型：
//! - WebView **从不提交或接收路径字符串**。目录选择在 **Rust 侧 Native 对话框**（rfd）完成，
//!   选择结果直接由后端校验并注册；前端只持有 opaque StorageLocationId。
//! - 内部注册函数（`run_register_local` / `run_rebind_local`）只被 Native 选择流程调用，
//!   不暴露为公开 Command；伪造/绕过流程的路径请求无入口。
//! - 扫描接口不接受 rootPath（get_scan_target 由 BE-SCAN-001 内部使用）。

use std::path::PathBuf;

use tauri::{Emitter, Runtime, State};

use haven_application::wire::{ErrorDto, LibraryChangedDto};
use haven_domain::ids::StorageLocationId;

use crate::ipc::{invalid_argument, run_blocking, to_error_dto, LIBRARY_CHANGED_TRANSPORT_EVENT};

/// displayName 长度上限（命令层不可信输入校验；审查 P2）。
const MAX_DISPLAY_NAME_LEN: usize = 100;
use crate::state::AppState;

/// 内部注册：把 **Native 对话框选择的路径** 立即校验并注册（P0-1：唯一路径入口，
/// 仅由 `run_pick_local_directory` 调用；测试直测本函数）。
pub async fn run_register_local(
    state: &AppState,
    display_name: String,
    path: PathBuf,
) -> Result<StorageLocationId, ErrorDto> {
    state
        .storage_location
        .add_local(display_name, &path)
        .await
        .map_err(|e| to_error_dto(&e))
}

/// 内部重新绑定：Native 对话框选择的新路径（唯一路径入口）。
pub async fn run_rebind_local(
    state: &AppState,
    storage_location_id: String,
    new_path: PathBuf,
) -> Result<(), ErrorDto> {
    let id = parse_location_id(&storage_location_id)?;
    state
        .storage_location
        .rebind_local(id, &new_path)
        .await
        .map_err(|e| to_error_dto(&e))
}

/// 列出全部存储位置（设置页"已连接位置"数据源）。
pub async fn run_storage_location_list(
    state: &AppState,
) -> Result<Vec<crate::ipc::StorageLocationDto>, ErrorDto> {
    let locations = state
        .storage_location
        .list()
        .await
        .map_err(|e| to_error_dto(&e))?;
    Ok(locations.into_iter().map(Into::into).collect())
}

/// 断开位置（幂等；保留用户数据）。
pub async fn run_storage_location_disconnect(
    state: &AppState,
    storage_location_id: String,
) -> Result<(), ErrorDto> {
    let id = parse_location_id(&storage_location_id)?;
    state
        .storage_location
        .disconnect(id)
        .await
        .map_err(|e| to_error_dto(&e))
}

/// 移除位置（只删应用内索引；不触碰用户原始文件）。
pub async fn run_storage_location_remove(
    state: &AppState,
    storage_location_id: String,
) -> Result<(), ErrorDto> {
    let id = parse_location_id(&storage_location_id)?;
    state
        .storage_location
        .remove(id)
        .await
        .map_err(|e| to_error_dto(&e))
}

fn parse_location_id(raw: &str) -> Result<StorageLocationId, ErrorDto> {
    raw.parse().map_err(|_| ErrorDto {
        code: "INVALID_ID".into(),
        user_message: "存储位置 ID 格式非法".into(),
        retryable: false,
    })
}

/// Native 目录选择 + 注册（P0-1：路径不经 WebView；用户取消 → OPERATION_CANCELLED）。
async fn run_pick_local_directory(
    state: &AppState,
    display_name: String,
) -> Result<StorageLocationId, ErrorDto> {
    if display_name.trim().chars().count() > MAX_DISPLAY_NAME_LEN {
        return Err(invalid_argument("显示名称过长（不超过 100 字符）"));
    }
    let picked = tauri::async_runtime::spawn_blocking(|| rfd::FileDialog::new().pick_folder())
        .await
        .map_err(|_| ErrorDto {
            code: "INTERNAL_ERROR".into(),
            user_message: "目录选择器执行失败".into(),
            retryable: false,
        })?;
    let Some(path) = picked else {
        return Err(ErrorDto {
            code: "OPERATION_CANCELLED".into(),
            user_message: "已取消选择目录".into(),
            retryable: false,
        });
    };
    run_register_local(state, display_name, path).await
}

/// Native 目录选择 + 重新绑定（用户取消 → OPERATION_CANCELLED）。
async fn run_rebind_picked_directory(
    state: &AppState,
    storage_location_id: String,
) -> Result<(), ErrorDto> {
    let id = parse_location_id(&storage_location_id)?;
    let picked = tauri::async_runtime::spawn_blocking(|| rfd::FileDialog::new().pick_folder())
        .await
        .map_err(|_| ErrorDto {
            code: "INTERNAL_ERROR".into(),
            user_message: "目录选择器执行失败".into(),
            retryable: false,
        })?;
    let Some(path) = picked else {
        return Err(ErrorDto {
            code: "OPERATION_CANCELLED".into(),
            user_message: "已取消选择目录".into(),
            retryable: false,
        });
    };
    state
        .storage_location
        .rebind_local(id, &path)
        .await
        .map_err(|e| to_error_dto(&e))
}

// ---- Tauri Command 薄包装（公开签名不含任何路径参数）----

#[tauri::command]
pub async fn storage_location_list(
    state: State<'_, AppState>,
) -> Result<Vec<crate::ipc::StorageLocationDto>, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_storage_location_list(&state).await }).await
}

#[tauri::command]
pub async fn storage_location_pick_local_directory(
    state: State<'_, AppState>,
    display_name: String,
) -> Result<StorageLocationId, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_pick_local_directory(&state, display_name).await }).await
}

#[tauri::command]
pub async fn storage_location_rebind_local_directory(
    state: State<'_, AppState>,
    storage_location_id: String,
) -> Result<(), ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(
        move || async move { run_rebind_picked_directory(&state, storage_location_id).await },
    )
    .await
}

#[tauri::command]
pub async fn storage_location_disconnect(
    state: State<'_, AppState>,
    storage_location_id: String,
) -> Result<(), ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move {
        run_storage_location_disconnect(&state, storage_location_id).await
    })
    .await
}

#[tauri::command]
pub async fn storage_location_remove<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    storage_location_id: String,
) -> Result<(), ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(
        move || async move { run_storage_location_remove(&state, storage_location_id).await },
    )
    .await?;

    // Removal is a successful library mutation even when no scan task exists.
    // Emit exactly one invalidation after the transaction commits so every
    // window refreshes its authoritative projection.
    let event = LibraryChangedDto {
        schema_version: 1,
        at: chrono::Utc::now().to_rfc3339(),
        operation_id: format!("remove-{}", uuid::Uuid::new_v4()),
        sequence: 1,
        revision: None,
    };
    let _ = app.emit(LIBRARY_CHANGED_TRANSPORT_EVENT, event);
    Ok(())
}
