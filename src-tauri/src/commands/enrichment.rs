//! `enrichment_status`（契约 §36.8 / CONTRACT-V02-ENRICHMENT-001，V2-F 批次）。
//!
//! Command 只做不可信输入校验、Application 调用与错误映射；
//! 流水线状态持久化由 EnrichmentService 经 SQLite 完成。
//! `metadata.changed` 事件由流水线执行侧广播，本命令只读。

use tauri::State;

use haven_application::services::EnrichmentService;
use haven_application::wire::{EnrichmentStateDto, EnrichmentStatusRequest, ErrorDto};

use crate::ipc::{run_blocking, to_error_dto};
use crate::state::AppState;

/// 命令核心逻辑（纯函数，可脱离 Tauri runtime 测试）。
pub async fn run_enrichment_status(
    service: &EnrichmentService,
    request: EnrichmentStatusRequest,
) -> Result<Vec<EnrichmentStateDto>, ErrorDto> {
    service
        .status(request.work_id)
        .await
        .map_err(|e| to_error_dto(&e))
}

/// Tauri Command 薄包装（SQLite 读取走 blocking worker）。
#[tauri::command]
pub async fn enrichment_status(
    state: State<'_, AppState>,
    request: EnrichmentStatusRequest,
) -> Result<Vec<EnrichmentStateDto>, ErrorDto> {
    let service = state.enrichment.clone();
    run_blocking(move || async move { run_enrichment_status(&service, request).await }).await
}
