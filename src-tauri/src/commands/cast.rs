//! Cast 投屏（v02-cast-001 双栈：DLNA + Chromecast）。
//! - 发现走 SSDP/mDNS 合并；播放复用 StreamService 上游解析；LAN 媒体服务供电视拉流。

use tauri::State;

use haven_application::wire::{
    CastDiscoverRequest, CastDiscoverResult, CastPlayRequest, CastPlayResult, CastStatusDto,
    CastStatusRequest, CastStopRequest, CastStopResult, ErrorDto,
};

use crate::{ipc::to_error_dto, state::AppState};

async fn run_cast_discover(
    state: &AppState,
    request: CastDiscoverRequest,
) -> Result<CastDiscoverResult, ErrorDto> {
    state
        .cast
        .discover(request)
        .await
        .map_err(|e| to_error_dto(&e))
}

async fn run_cast_play(
    state: &AppState,
    request: CastPlayRequest,
) -> Result<CastPlayResult, ErrorDto> {
    state.cast.play(request).await.map_err(|e| to_error_dto(&e))
}

async fn run_cast_status(
    state: &AppState,
    request: CastStatusRequest,
) -> Result<CastStatusDto, ErrorDto> {
    state
        .cast
        .status(request)
        .await
        .map_err(|e| to_error_dto(&e))
}

async fn run_cast_stop(
    state: &AppState,
    request: CastStopRequest,
) -> Result<CastStopResult, ErrorDto> {
    state.cast.stop(request).await.map_err(|e| to_error_dto(&e))
}

#[tauri::command]
pub async fn cast_discover(
    state: State<'_, AppState>,
    request: CastDiscoverRequest,
) -> Result<CastDiscoverResult, ErrorDto> {
    let state = (*state.inner()).clone();
    crate::ipc::run_blocking(move || async move { run_cast_discover(&state, request).await }).await
}

#[tauri::command]
pub async fn cast_play(
    state: State<'_, AppState>,
    request: CastPlayRequest,
) -> Result<CastPlayResult, ErrorDto> {
    let state = (*state.inner()).clone();
    crate::ipc::run_blocking(move || async move { run_cast_play(&state, request).await }).await
}

#[tauri::command]
pub async fn cast_status(
    state: State<'_, AppState>,
    request: CastStatusRequest,
) -> Result<CastStatusDto, ErrorDto> {
    let state = (*state.inner()).clone();
    crate::ipc::run_blocking(move || async move { run_cast_status(&state, request).await }).await
}

#[tauri::command]
pub async fn cast_stop(
    state: State<'_, AppState>,
    request: CastStopRequest,
) -> Result<CastStopResult, ErrorDto> {
    let state = (*state.inner()).clone();
    crate::ipc::run_blocking(move || async move { run_cast_stop(&state, request).await }).await
}
