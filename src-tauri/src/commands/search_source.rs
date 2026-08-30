//! `search_source_start` / `search_source_cancel`
//! （契约 §36.3 / CONTRACT-V02-SEARCH-CHANNEL-001）。
//!
//! - `search_source_start`：绑定 Channel 事件出口 → `SearchSourceService::start`
//!   （登记 + 立即返回；分发在后台任务跑，V2-A 零参与者诚实 completed）。
//! - `search_source_cancel`：幂等取消；未知 operationId → `RESOURCE_NOT_FOUND`。

use tauri::ipc::Channel;
use tauri::State;

use haven_application::services::SearchCancelOutcome;
use haven_application::wire::{
    ErrorDto, SearchSourceCancelRequest, SearchSourceCancelResultDto, SearchSourceEvent,
    SearchSourceStartRequest, SearchStartResultDto,
};

use crate::ipc::{run_blocking, to_error_dto};
use crate::state::AppState;

/// 命令核心逻辑（纯函数，可脱离 Tauri runtime 测试）。
pub async fn run_search_source_start(
    state: &AppState,
    request: SearchSourceStartRequest,
    on_event: Channel<SearchSourceEvent>,
) -> Result<SearchStartResultDto, ErrorDto> {
    // 绑定事件出口必须先于 start：started 事件在 start 内同步发出；
    // 失败时按 route ownership 解绑（不裸 channel_id，防同 ID 新 route 被旧调用误删）。
    let route = state.search_sink.bind(on_event);
    match state.search_source.start(request).await {
        Ok(result) => {
            route.assign_operation(result.operation_id.clone());
            Ok(result)
        }
        Err(e) => {
            state.search_sink.unbind(&route);
            Err(to_error_dto(&e))
        }
    }
}

/// Tauri Command 薄包装（start 内部 tokio::spawn 需 runtime 上下文，
/// 由 run_blocking 的 block_on 提供；与 library_scan_start 同模式）。
#[tauri::command]
pub async fn search_source_start(
    state: State<'_, AppState>,
    request: SearchSourceStartRequest,
    on_event: Channel<SearchSourceEvent>,
) -> Result<SearchStartResultDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_search_source_start(&state, request, on_event).await })
        .await
}

/// 命令核心逻辑：cancel 结果映射（运行中 → 受理；已终态 → alreadyTerminal）。
pub fn run_search_source_cancel(
    state: &AppState,
    request: SearchSourceCancelRequest,
) -> Result<SearchSourceCancelResultDto, ErrorDto> {
    match state.search_source.cancel(&request.operation_id) {
        Ok(outcome) => Ok(SearchSourceCancelResultDto {
            operation_id: request.operation_id,
            already_terminal: matches!(outcome, SearchCancelOutcome::AlreadyTerminal),
        }),
        Err(e) => Err(to_error_dto(&e)),
    }
}

/// Tauri Command 薄包装（纯内存操作，无需 blocking worker）。
#[tauri::command]
pub async fn search_source_cancel(
    state: State<'_, AppState>,
    request: SearchSourceCancelRequest,
) -> Result<SearchSourceCancelResultDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_search_source_cancel(&state, request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_infrastructure::Db;
    use std::sync::Arc;

    #[tokio::test]
    async fn cancel_unknown_operation_returns_resource_not_found() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let err = run_search_source_cancel(
            &state,
            SearchSourceCancelRequest {
                operation_id: "op-search-nonexistent".into(),
            },
        )
        .unwrap_err();
        assert_eq!(err.code, "RESOURCE_NOT_FOUND");
    }
}
