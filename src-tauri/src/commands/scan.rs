//! `library_scan_start` / `scan_cancel`（BE-SCAN-001 第三步 / IPC-TAURI-001B 扫描部分）。
//!
//! - `library_scan_start`：校验 storageLocationId → 绑定 Channel 事件出口 →
//!   `ScanService::start`（登记 + 立即返回；扫描在后台任务跑）。
//! - `scan_cancel`：协作式取消；未知 taskId → `RESOURCE_NOT_FOUND`（不伪造终态）。

use tauri::ipc::Channel;
use tauri::State;

use haven_application::services::scan::CancelOutcome;
use haven_application::wire::{
    LibraryScanEvent, LibraryScanStartRequest, ScanPhase, ScanStartResult,
};

use crate::ipc::{invalid_id, run_blocking};
use crate::ipc::{to_error_dto, ScanCancelResultDto};
use crate::state::AppState;

/// 命令核心逻辑（纯函数，可脱离 Tauri runtime 测试）。
pub async fn run_library_scan_start<R: tauri::Runtime>(
    state: &AppState,
    app: tauri::AppHandle<R>,
    request: LibraryScanStartRequest,
    on_event: Channel<LibraryScanEvent>,
) -> Result<ScanStartResult, haven_application::wire::ErrorDto> {
    // 不可信输入校验先于一切（含事件出口绑定）。
    let location_id = match request.storage_location_id.parse() {
        Ok(id) => id,
        Err(_) => return Err(invalid_id()),
    };
    // 绑定事件出口必须在 start 之前：started 事件在 start 内同步发出。
    // `route` 即不可复用身份 handle；失败时按 route ownership 解绑（不裸 channel_id，
    // 防同 ID 新 route 被旧调用误删，R-MAIN-11）。
    let route = state.scan_sink.bind(app, on_event);
    match state.scan.start(location_id).await {
        Ok(result) => {
            let terminal_buffered = route.assign_task(result.task_id.clone());
            if terminal_buffered {
                state.scan_sink.unbind(&route);
            }
            Ok(result)
        }
        // 启动失败（未知位置/stale/DB 错误）时回收出口：绑定先于 start，
        // 不解绑会让残留 Channel 继续接收其他任务的事件。
        Err(e) => {
            state.scan_sink.unbind(&route);
            Err(to_error_dto(&e))
        }
    }
}

/// Tauri Command 薄包装（blocking worker：get_scan_target 含 FS 探测）。
#[tauri::command]
pub async fn library_scan_start<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    request: LibraryScanStartRequest,
    on_event: Channel<LibraryScanEvent>,
) -> Result<ScanStartResult, haven_application::wire::ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(
        move || async move { run_library_scan_start(&state, app, request, on_event).await },
    )
    .await
}

/// 命令核心：cancel 结果映射（运行中 → cancelled 受理；已结束 → 真实终态）。
pub async fn run_scan_cancel(
    state: &AppState,
    task_id: String,
) -> Result<ScanCancelResultDto, haven_application::wire::ErrorDto> {
    match state.scan.cancel(&task_id) {
        Ok(CancelOutcome::Cancelled) => Ok(ScanCancelResultDto {
            task_id,
            already_terminal: false,
            phase: ScanPhase::Cancelled,
        }),
        Ok(CancelOutcome::AlreadyTerminal(phase)) => Ok(ScanCancelResultDto {
            task_id,
            already_terminal: true,
            phase,
        }),
        Err(e) => Err(to_error_dto(&e)),
    }
}

/// Tauri Command 薄包装（纯内存操作，无需 blocking worker）。
#[tauri::command]
pub async fn scan_cancel(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<ScanCancelResultDto, haven_application::wire::ErrorDto> {
    let state = (*state.inner()).clone();
    run_scan_cancel(&state, task_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_cancel_result_serializes_snake_case_phase() {
        let dto = ScanCancelResultDto {
            task_id: "task-1".into(),
            already_terminal: true,
            phase: ScanPhase::ItemIndexed,
        };
        let value = serde_json::to_value(dto).unwrap();
        assert_eq!(value["taskId"], "task-1");
        assert_eq!(value["alreadyTerminal"], true);
        assert_eq!(value["phase"], "item_indexed");
    }

    #[tokio::test]
    async fn scan_cancel_unknown_task_returns_resource_not_found() {
        use std::sync::Arc;

        use haven_infrastructure::Db;

        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let err = run_scan_cancel(&state, "nonexistent-task".into())
            .await
            .unwrap_err();
        assert_eq!(err.code.as_str(), "RESOURCE_NOT_FOUND");
    }
}
