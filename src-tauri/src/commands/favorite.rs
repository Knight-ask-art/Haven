//! `favorite_set` Command（CONTRACT-FAVORITE-SET-001；P1-9 blocking worker）。
//!
//! - 调用 `FavoriteService::set_with_outcome`（状态版本语义 R-FAV-001）。
//! - 仅 `changed=true` 时产生一次 `favorite.changed` 事件；幂等重复设置不产生第二个。

use tauri::{Emitter, State};

use haven_application::wire::{FavoriteChangedDto, FavoriteSetRequest, FavoriteSetResult};

use crate::ipc::{invalid_id, run_blocking, to_error_dto, FAVORITE_CHANGED_TRANSPORT_EVENT};
use crate::state::AppState;

/// 命令核心结果：成功响应 + 待发布事件（changed=true 才有，否则 None）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FavoriteSetOutcome {
    pub result: FavoriteSetResult,
    pub event: Option<FavoriteChangedDto>,
}

/// 命令核心逻辑（纯函数，可脱离 Tauri runtime 测试）。
pub async fn run_favorite_set(
    state: &AppState,
    request: FavoriteSetRequest,
) -> Result<FavoriteSetOutcome, haven_application::wire::ErrorDto> {
    let work_id = request.work_id.parse().map_err(|_| invalid_id())?;
    let outcome = state
        .favorite
        .set_with_outcome(work_id, request.favorite)
        .await
        .map_err(|e| to_error_dto(&e))?;

    // 状态实际变化才构造事件（幂等重复设置不发第二个 Event）。
    let event = if outcome.changed {
        Some(FavoriteChangedDto {
            schema_version: 1,
            at: chrono::Utc::now().to_rfc3339(),
            operation_id: format!("fav-{}", uuid::Uuid::new_v4()),
            sequence: 1,
            work_id: outcome.result.work_id.clone(),
            favorite: outcome.result.favorite,
            revision: outcome.result.revision.clone().unwrap_or_default(),
        })
    } else {
        None
    };

    Ok(FavoriteSetOutcome {
        result: outcome.result,
        event,
    })
}

/// Tauri Command 薄包装：blocking worker 执行 + 发布事件（发布失败不影响结果）。
#[tauri::command]
pub async fn favorite_set<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    request: FavoriteSetRequest,
) -> Result<FavoriteSetResult, haven_application::wire::ErrorDto> {
    let state = (*state.inner()).clone();
    let outcome =
        run_blocking(move || async move { run_favorite_set(&state, request).await }).await?;
    if let Some(event) = outcome.event {
        let _ = app.emit(FAVORITE_CHANGED_TRANSPORT_EVENT, event);
    }
    Ok(outcome.result)
}
