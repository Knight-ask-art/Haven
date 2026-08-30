//! `credential_status` / `credential_set` / `credential_delete`
//! （契约 §36.5 / CONTRACT-V02-CREDENTIAL-PROFILE-001，WebDAV 前置）。
//!
//! 安全边界：Secret 单向写入 Windows Credential Store（ADR-001）；本层不记录、
//! 不回显 secret；错误经 to_error_dto 映射为稳定 ErrorDto（CREDENTIAL_ACCESS_FAILED
//! 等），底层 cause 不出 IPC。

use tauri::State;

use haven_application::wire::{
    CredentialDeleteRequest, CredentialSetRequest, CredentialStatusDto, CredentialStatusRequest,
    ErrorDto,
};

use crate::ipc::{run_blocking, to_error_dto};
use crate::state::AppState;

/// `credential_status`（只读；凭据 IO 走 blocking worker）。
#[tauri::command]
pub async fn credential_status(
    state: State<'_, AppState>,
    request: CredentialStatusRequest,
) -> Result<CredentialStatusDto, ErrorDto> {
    let access = state.credential_access.clone();
    run_blocking(move || async move { access.status(request).await.map_err(|e| to_error_dto(&e)) })
        .await
}

/// `credential_set`（幂等覆盖写入；secret 只进 CredentialStore）。
#[tauri::command]
pub async fn credential_set(
    state: State<'_, AppState>,
    request: CredentialSetRequest,
) -> Result<(), ErrorDto> {
    let access = state.credential_access.clone();
    run_blocking(move || async move { access.set(request).await.map_err(|e| to_error_dto(&e)) })
        .await
}

/// `credential_delete`（幂等；不存在视为成功）。
#[tauri::command]
pub async fn credential_delete(
    state: State<'_, AppState>,
    request: CredentialDeleteRequest,
) -> Result<(), ErrorDto> {
    let access = state.credential_access.clone();
    run_blocking(move || async move { access.delete(request).await.map_err(|e| to_error_dto(&e)) })
        .await
}
