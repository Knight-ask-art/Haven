//! 错误诊断报告 Commands（V02-OPEN-SOURCE-DIAGNOSTICS-001）。
//!
//! Command 只负责 DTO 校验/映射、调用 Application 和错误映射。报告等级、
//! 稳定错误码裁剪、用户确认、导出和固定 Issue URL 均由 Application/Infrastructure
//! 负责；前端不能传入 URL、路径、Header 或报告正文。

use tauri::State;

use haven_application::services::ErrorReportService;
use haven_application::wire::{
    ErrorDto, ErrorReportActionRequest, ErrorReportActionResultDto, ErrorReportConfirmRequest,
    ErrorReportConfirmResultDto, ErrorReportPreviewDto, ErrorReportPreviewRequest,
};

use crate::ipc::{run_blocking, to_error_dto};
use crate::state::AppState;

pub async fn run_error_report_preview_get(
    state: &AppState,
    request: ErrorReportPreviewRequest,
) -> Result<ErrorReportPreviewDto, ErrorDto> {
    state
        .error_report
        .preview(request)
        .map_err(|error| to_error_dto(&error))
}

pub async fn run_error_report_confirm(
    state: &AppState,
    request: ErrorReportConfirmRequest,
) -> Result<ErrorReportConfirmResultDto, ErrorDto> {
    state
        .error_report
        .confirm(request)
        .map_err(|error| to_error_dto(&error))
}

pub async fn run_error_report_export(
    state: &AppState,
    request: ErrorReportActionRequest,
) -> Result<ErrorReportActionResultDto, ErrorDto> {
    state
        .error_report
        .export(request)
        .map_err(|error| to_error_dto(&error))
}

pub async fn run_error_report_open_issue(
    state: &AppState,
    request: ErrorReportActionRequest,
) -> Result<ErrorReportActionResultDto, ErrorDto> {
    state
        .error_report
        .open_issue(request)
        .map_err(|error| to_error_dto(&error))
}

#[tauri::command]
pub async fn error_report_preview_get(
    state: State<'_, AppState>,
    request: ErrorReportPreviewRequest,
) -> Result<ErrorReportPreviewDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_error_report_preview_get(&state, request).await }).await
}

#[tauri::command]
pub async fn error_report_confirm(
    state: State<'_, AppState>,
    request: ErrorReportConfirmRequest,
) -> Result<ErrorReportConfirmResultDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_error_report_confirm(&state, request).await }).await
}

#[tauri::command]
pub async fn error_report_export(
    state: State<'_, AppState>,
    request: ErrorReportActionRequest,
) -> Result<ErrorReportActionResultDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_error_report_export(&state, request).await }).await
}

#[tauri::command]
pub async fn error_report_open_issue(
    state: State<'_, AppState>,
    request: ErrorReportActionRequest,
) -> Result<ErrorReportActionResultDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_error_report_open_issue(&state, request).await }).await
}

#[allow(dead_code)]
fn _service_type_is_cloneable(_: ErrorReportService) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn report_command_flow_requires_confirmation() {
        let state = AppState::new(Arc::new(
            haven_infrastructure::Db::open_in_memory().unwrap(),
        ));
        let preview = run_error_report_preview_get(
            &state,
            ErrorReportPreviewRequest {
                level: haven_application::wire::ErrorReportLevelDto::Basic,
                stable_error_codes: vec!["SOURCE_TIMEOUT".into()],
            },
        )
        .await
        .unwrap();
        let request = ErrorReportActionRequest {
            report_id: preview.report_id.clone(),
        };
        let error = run_error_report_export(&state, request.clone())
            .await
            .unwrap_err();
        assert_eq!(error.code, "ERROR_REPORT_CONFIRMATION_REQUIRED");
        run_error_report_confirm(
            &state,
            ErrorReportConfirmRequest {
                report_id: preview.report_id,
            },
        )
        .await
        .unwrap();
        let result = run_error_report_export(&state, request).await.unwrap();
        assert_eq!(
            result.status,
            haven_application::wire::ErrorReportActionStatusDto::Exported
        );
    }
}
