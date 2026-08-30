//! 视频截图 Commands（V02-PLAYBACK-HARDWARE-SCREENSHOT-001）。
//!
//! Command 层只负责窗口身份、DTO 校验/映射和 Application 调用；上传状态、
//! JPEG 校验、临时文件与保存对话框分别由 Application/Infrastructure 拥有。

use tauri::{Runtime, State, Webview};

use haven_application::wire::{
    ErrorDto, VideoScreenshotBeginResultDto, VideoScreenshotChunkRequest, VideoScreenshotResultDto,
};

use crate::ipc::{run_blocking, to_error_dto};
use crate::state::AppState;

pub async fn run_video_screenshot_begin(
    state: &AppState,
    owner_webview_label: &str,
) -> Result<VideoScreenshotBeginResultDto, ErrorDto> {
    state
        .video_screenshot
        .begin(owner_webview_label)
        .map_err(|error| to_error_dto(&error))
}

pub async fn run_video_screenshot_chunk(
    state: &AppState,
    owner_webview_label: &str,
    request: VideoScreenshotChunkRequest,
) -> Result<(), ErrorDto> {
    state
        .video_screenshot
        .chunk(owner_webview_label, request)
        .map_err(|error| to_error_dto(&error))
}

pub async fn run_video_screenshot_commit(
    state: &AppState,
    owner_webview_label: &str,
    upload_id: String,
) -> Result<VideoScreenshotResultDto, ErrorDto> {
    state
        .video_screenshot
        .commit(owner_webview_label, &upload_id)
        .map_err(|error| to_error_dto(&error))
}

pub async fn run_video_screenshot_cancel(
    state: &AppState,
    owner_webview_label: &str,
    upload_id: String,
) -> Result<(), ErrorDto> {
    state
        .video_screenshot
        .cancel(owner_webview_label, &upload_id)
        .map_err(|error| to_error_dto(&error))
}

#[tauri::command]
pub async fn video_screenshot_begin<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, AppState>,
) -> Result<VideoScreenshotBeginResultDto, ErrorDto> {
    let owner_webview_label = webview.label().to_owned();
    let state = (*state.inner()).clone();
    run_blocking(
        move || async move { run_video_screenshot_begin(&state, &owner_webview_label).await },
    )
    .await
}

#[tauri::command]
pub async fn video_screenshot_chunk<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, AppState>,
    request: VideoScreenshotChunkRequest,
) -> Result<(), ErrorDto> {
    let owner_webview_label = webview.label().to_owned();
    let state = (*state.inner()).clone();
    run_blocking(move || async move {
        run_video_screenshot_chunk(&state, &owner_webview_label, request).await
    })
    .await
}

#[tauri::command]
pub async fn video_screenshot_commit<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, AppState>,
    upload_id: String,
) -> Result<VideoScreenshotResultDto, ErrorDto> {
    let owner_webview_label = webview.label().to_owned();
    let state = (*state.inner()).clone();
    run_blocking(move || async move {
        run_video_screenshot_commit(&state, &owner_webview_label, upload_id).await
    })
    .await
}

#[tauri::command]
pub async fn video_screenshot_cancel<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, AppState>,
    upload_id: String,
) -> Result<(), ErrorDto> {
    let owner_webview_label = webview.label().to_owned();
    let state = (*state.inner()).clone();
    run_blocking(move || async move {
        run_video_screenshot_cancel(&state, &owner_webview_label, upload_id).await
    })
    .await
}
