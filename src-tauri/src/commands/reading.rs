//! 阅读总览 Command（契约 §12；Reading Overview 的 Tauri 接线）。
//!
//! Command 边界（ADR-002 / IPC-TAURI-001A）：
//! - 不可信输入（窗口天数 / UTC 偏移）只经领域 `ReadingOverviewRequest::new` 校验；
//!   越界请求在触碰任何存储之前就以稳定 `READING_OVERVIEW_INVALID_RANGE` 被拒绝。
//! - `now` 由命令层**取一次**再传进服务：同一次读取内部自洽，领域聚合因此可复现。
//!   窗口换算（本地日 ↔ UTC 半开区间）留在领域层，命令层不做日历判断。
//! - 聚合结果整体走 `From<ReadingOverview>`，命令层不手工搬字段——避免出现第二条
//!   与事实源漂移的投影路径。
//! - 同步 SQLite 只在 `run_blocking` 提供的 blocking worker 上执行。
//!
//! 空库不是错误：没有任何会话事实时返回**显式空态**——`sessionCount = 0`、可选统计量
//! 保持 `null`（不是 0）、分类与热力图为空；`daily` 仍然逐日展开，因为它描述的是请求
//! 的那个窗口本身，而不是观察到的阅读。

use tauri::State;

use haven_application::wire::{ErrorDto, ReadingOverviewDto, ReadingOverviewGetRequest};
use haven_common::UtcMillis;
use haven_domain::reading_activity::ReadingOverviewRequest;

use crate::ipc::{run_blocking, to_error_dto};
use crate::state::AppState;

/// 命令核心：校验窗口 → 读取聚合 → 映射为 wire DTO。
pub async fn run_reading_overview_get(
    state: &AppState,
    request: ReadingOverviewGetRequest,
) -> Result<ReadingOverviewDto, ErrorDto> {
    let request = ReadingOverviewRequest::new(request.days, request.utc_offset_minutes)
        .map_err(|error| to_error_dto(&error))?;
    let overview = state
        .reading_overview
        .overview(request, UtcMillis::now())
        .await
        .map_err(|error| to_error_dto(&error))?;
    Ok(overview.into())
}

#[tauri::command]
pub async fn reading_overview_get(
    state: State<'_, AppState>,
    request: ReadingOverviewGetRequest,
) -> Result<ReadingOverviewDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_reading_overview_get(&state, request).await }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use haven_infrastructure::Db;

    fn app_state() -> AppState {
        AppState::new(Arc::new(Db::open_in_memory().unwrap()))
    }

    async fn overview(
        state: &AppState,
        days: u32,
        utc_offset_minutes: i32,
    ) -> Result<ReadingOverviewDto, ErrorDto> {
        run_reading_overview_get(
            state,
            ReadingOverviewGetRequest {
                days,
                utc_offset_minutes,
            },
        )
        .await
    }

    /// 空的内存库：显式空态，而不是伪造的 0 分钟。
    #[tokio::test]
    async fn empty_database_reports_an_explicit_empty_overview() {
        let state = app_state();
        let dto = overview(&state, 7, 480).await.unwrap();

        assert_eq!(dto.schema_version, 1);
        assert_eq!(dto.session_count, 0);
        assert_eq!(dto.total_duration_ms, 0);
        assert_eq!(dto.range.days, 7);
        assert_eq!(dto.range.utc_offset_minutes, 480);

        // 可选统计量保持 null：没有记录就不该凭空长出一个数字。
        assert_eq!(dto.peak_start_hour, None);
        assert_eq!(dto.peak_end_hour, None);
        assert_eq!(dto.longest_streak_days, None);
        assert_eq!(dto.average_daily_duration_ms, None);
        assert_eq!(dto.recent_week_duration_ms, None);

        // 分类与热力图只包含真的有时长的条目，空库必须为空。
        assert!(dto.categories.is_empty(), "空库不得有分类时长");
        assert!(dto.heatmap_cells.is_empty(), "空库不得有热力图格子");

        // `daily` 描述的是窗口本身：7 个本地日各一条，时长全为 0。
        assert_eq!(dto.daily.len(), 7);
        assert!(dto.daily.iter().all(|bucket| bucket.duration_ms == 0));
        // 日期升序且首尾与 range 对齐（窗口是确定的，不依赖是否有阅读记录）。
        assert_eq!(
            dto.daily.first().unwrap().local_date,
            dto.range.start_local_date
        );
        assert_eq!(
            dto.daily.last().unwrap().local_date,
            dto.range.end_local_date
        );
    }

    /// 越界的天数 / UTC 偏移在触碰存储前被拒绝，错误码稳定且不可重试。
    #[tokio::test]
    async fn out_of_range_days_and_offset_are_rejected_without_reading() {
        let state = app_state();
        for (days, utc_offset_minutes) in
            [(0u32, 0i32), (91, 0), (7, 14 * 60 + 1), (7, -(14 * 60) - 1)]
        {
            let error = overview(&state, days, utc_offset_minutes)
                .await
                .unwrap_err();
            assert_eq!(
                error.code, "READING_OVERVIEW_INVALID_RANGE",
                "越界窗口必须稳定映射: (days={days}, offset={utc_offset_minutes})"
            );
            assert!(!error.retryable);
        }
    }

    /// 边界值本身合法：最小/最大天数与正负最大偏移都能读出（空库）总览。
    #[tokio::test]
    async fn boundary_windows_are_accepted() {
        let state = app_state();
        for (days, utc_offset_minutes) in [(1u32, 0i32), (90, 14 * 60), (90, -(14 * 60))] {
            let dto = overview(&state, days, utc_offset_minutes).await.unwrap();
            assert_eq!(dto.range.days, days);
            assert_eq!(dto.range.utc_offset_minutes, utc_offset_minutes);
            assert_eq!(dto.daily.len(), days as usize);
        }
    }
}
