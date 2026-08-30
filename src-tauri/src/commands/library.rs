//! `library_list` Command（CONTRACT-LIBRARY-LIST-001；P1-9 blocking worker）。

use tauri::State;

use haven_application::wire::{LibraryListRequest, PageDto, WorkCardDto};

use crate::ipc::{invalid_argument, run_blocking, to_error_dto};

/// 搜索词长度上限（命令层不可信输入校验；审查 P2——LIKE 查询与 UI 均不应
/// 接收任意长度文本）。
const MAX_QUERY_LEN: usize = 200;
use crate::state::AppState;

/// 命令核心逻辑（纯函数，可脱离 Tauri runtime 测试）。
pub async fn run_library_list(
    state: &AppState,
    request: LibraryListRequest,
) -> Result<PageDto<WorkCardDto>, haven_application::wire::ErrorDto> {
    if let Some(query) = request.query.as_deref() {
        if query.trim().chars().count() > MAX_QUERY_LEN {
            return Err(invalid_argument("搜索词过长（不超过 200 字符）"));
        }
    }
    state
        .library
        .list(request)
        .await
        .map_err(|e| to_error_dto(&e))
}

/// Tauri Command 薄包装（blocking worker 执行，不阻塞 async runtime）。
#[tauri::command]
pub async fn library_list(
    state: State<'_, AppState>,
    request: LibraryListRequest,
) -> Result<PageDto<WorkCardDto>, haven_application::wire::ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_library_list(&state, request).await }).await
}
