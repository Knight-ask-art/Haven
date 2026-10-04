//! 阅读活动事实与总览聚合（Reading Overview）。
//!
//! 这一层是「阅读总览」唯一的持久化事实源：每次内容会话闭合时记下**真实观测到的**
//! 开始/结束时刻，总览页只对这些事实做求和，不从 HistoryEntry 的时间戳或条目数量
//! 反推时长。没有记录就是没有记录（`session_count == 0`），不是 0 分钟。
//!
//! 三条边界：
//! 1. 会话时长是 `ended_at - started_at`，不接受调用方单独传入的时长——避免出现与
//!    两个时刻都不自洽的第三个数字；
//! 2. 分类必须来自 MediaItem 的真实 `media_type`（`Unknown` 直接拒绝，不猜）；
//! 3. 本地日期/小时只由「UTC 毫秒 + 显式 UTC 偏移」算得，偏移是请求参数，
//!    不从运行环境隐式读取，因此聚合结果可复现、可测试。

use haven_common::{AppError, ErrorKind, UtcMillis};

use crate::enums::MediaType;
use crate::ids::{MediaItemId, ReadingSessionId};

/// 总览窗口允许的天数范围（含端点）。
pub const MIN_OVERVIEW_DAYS: u32 = 1;
/// 总览窗口上限：90 天足够覆盖「最近 30 天热力图 + 季度对比」，且保证一次查询有界。
pub const MAX_OVERVIEW_DAYS: u32 = 90;
/// 显式 UTC 偏移（分钟）的绝对值上限：真实时区最东 UTC+14、最西 UTC-12。
pub const MAX_UTC_OFFSET_MINUTES: i32 = 14 * 60;
/// 单次会话时长上限。超过它的记录只可能来自时钟跳变或未正常闭合的会话，
/// 这类事实宁可不记，也不写进一个会污染统计的假时长。
pub const MAX_SESSION_DURATION_MS: i64 = 24 * 60 * 60 * 1000;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const HOUR_MS: i64 = 60 * 60 * 1000;

/// 一次内容会话归属的一级分类。
///
/// 与 [`crate::enums::ContentCategory`] 同一套取值，但刻意排除 `all`：一次真实会话
/// 必然属于某一类，「全部」不是一个会话可以归属的分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReadingSessionCategory {
    Video,
    Book,
    Comic,
    Periodical,
}

impl ReadingSessionCategory {
    /// 全部分类的固定顺序（聚合输出的稳定 tie-break 也用它）。
    pub const ALL: [Self; 4] = [Self::Video, Self::Book, Self::Comic, Self::Periodical];

    /// 由真实 `media_type` 推导。`Unknown` 没有可归属的分类，返回 `None`——
    /// 调用方据此放弃记录，而不是猜一个默认分类。
    pub fn from_media_type(media_type: MediaType) -> Option<Self> {
        match media_type {
            MediaType::Movie | MediaType::Series | MediaType::Episode | MediaType::Audio => {
                Some(Self::Video)
            }
            MediaType::Book => Some(Self::Book),
            MediaType::Comic => Some(Self::Comic),
            MediaType::Document | MediaType::Article => Some(Self::Periodical),
            MediaType::Unknown => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Book => "book",
            Self::Comic => "comic",
            Self::Periodical => "periodical",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|item| item.as_str() == raw)
    }

    /// 聚合 tie-break 用的稳定序号（越小越靠前）。
    fn rank(self) -> usize {
        Self::ALL
            .iter()
            .position(|item| *item == self)
            .unwrap_or(Self::ALL.len())
    }
}

impl serde::Serialize for ReadingSessionCategory {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for ReadingSessionCategory {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).ok_or_else(|| serde::de::Error::custom("未知的阅读会话分类"))
    }
}

/// 一次已闭合的内容会话。
///
/// `id` 是会话身份的持久化形态：同一次会话重复上报只会命中同一行（写入幂等），
/// 不会把同一段时间统计两次。
///
/// 只由 [`ReadingSession::new`] 构造：时长必须等于两个时刻之差，且落在
/// `(0, MAX_SESSION_DURATION_MS]` 之内。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadingSession {
    pub id: ReadingSessionId,
    pub media_item_id: MediaItemId,
    pub category: ReadingSessionCategory,
    pub started_at: UtcMillis,
    pub ended_at: UtcMillis,
}

impl ReadingSession {
    pub fn new(
        id: ReadingSessionId,
        media_item_id: MediaItemId,
        category: ReadingSessionCategory,
        started_at: UtcMillis,
        ended_at: UtcMillis,
    ) -> Result<Self, AppError> {
        let duration = ended_at.0 - started_at.0;
        if duration <= 0 {
            return Err(implausible_session("阅读会话的结束时刻必须晚于开始时刻"));
        }
        if duration > MAX_SESSION_DURATION_MS {
            return Err(implausible_session("阅读会话时长超出可信上限"));
        }
        Ok(Self {
            id,
            media_item_id,
            category,
            started_at,
            ended_at,
        })
    }

    /// 会话时长（毫秒）：由两个时刻直接得出，不是独立输入。
    pub fn duration_ms(&self) -> i64 {
        self.ended_at.0 - self.started_at.0
    }
}

fn implausible_session(message: &str) -> AppError {
    AppError::new(
        "READING_SESSION_IMPLAUSIBLE",
        ErrorKind::Validation,
        message,
        false,
    )
}

fn invalid_overview_range(message: &str) -> AppError {
    AppError::new(
        "READING_OVERVIEW_INVALID_RANGE",
        ErrorKind::Validation,
        message,
        false,
    )
}

/// 总览请求：窗口天数 + 显式 UTC 偏移（分钟）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadingOverviewRequest {
    pub days: u32,
    pub utc_offset_minutes: i32,
}

impl ReadingOverviewRequest {
    pub fn new(days: u32, utc_offset_minutes: i32) -> Result<Self, AppError> {
        if !(MIN_OVERVIEW_DAYS..=MAX_OVERVIEW_DAYS).contains(&days) {
            return Err(invalid_overview_range("总览统计天数超出允许范围"));
        }
        if !(-MAX_UTC_OFFSET_MINUTES..=MAX_UTC_OFFSET_MINUTES).contains(&utc_offset_minutes) {
            return Err(invalid_overview_range("总览统计时区偏移超出允许范围"));
        }
        Ok(Self {
            days,
            utc_offset_minutes,
        })
    }

    /// 窗口对应的 UTC 时间半开区间 `[from, to)`（按会话开始时刻过滤）。
    ///
    /// 由「本地日窗口」反推回 UTC，因此调用方（Repository）只需要做一次范围扫描，
    /// 不需要理解本地日历。
    pub fn utc_window(&self, now: UtcMillis) -> (UtcMillis, UtcMillis) {
        let offset_ms = i64::from(self.utc_offset_minutes) * 60_000;
        let end_day = (now.0 + offset_ms).div_euclid(DAY_MS);
        let start_day = end_day - (i64::from(self.days) - 1);
        (
            UtcMillis(start_day * DAY_MS - offset_ms),
            UtcMillis((end_day + 1) * DAY_MS - offset_ms),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadingOverviewRange {
    pub start_local_date: String,
    pub end_local_date: String,
    pub days: u32,
    pub utc_offset_minutes: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadingDailyBucket {
    pub local_date: String,
    /// ISO 星期序号：1 = 周一 … 7 = 周日。
    pub weekday: u8,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadingCategoryTotal {
    pub category: ReadingSessionCategory,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadingHeatmapCell {
    pub weekday: u8,
    /// 本地小时 0..=23。
    pub hour: u8,
    pub duration_ms: u64,
}

/// 总览聚合结果。所有数字都是对已持久化会话事实的求和；没有任何记录时
/// `session_count == 0`，可选统计量保持 `None`（而不是 0）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadingOverview {
    pub range: ReadingOverviewRange,
    pub session_count: u64,
    pub total_duration_ms: u64,
    /// 窗口内每一天一条（升序，含时长为 0 的空日）。
    pub daily: Vec<ReadingDailyBucket>,
    /// 只包含有时长的分类，按时长降序、同长按固定分类顺序。
    pub categories: Vec<ReadingCategoryTotal>,
    /// 只包含有时长的 (星期, 小时) 格子。
    pub heatmap_cells: Vec<ReadingHeatmapCell>,
    pub peak_start_hour: Option<u8>,
    /// 高峰时段的右端点（= 起点 + 1 小时，可能为 24）。
    pub peak_end_hour: Option<u8>,
    /// 统计窗口内最长的连续活跃阅读天数；窗口内没有任何记录时为 `None`。
    pub longest_streak_days: Option<u32>,
    /// 窗口内日均时长；没有任何记录时为 `None`。
    pub average_daily_duration_ms: Option<u64>,
    /// 窗口末尾最近 7 个本地日的时长之和；没有任何记录时为 `None`。
    pub recent_week_duration_ms: Option<u64>,
}

/// 对已持久化的会话事实做聚合。`now` 显式传入，保证结果可复现。
pub fn aggregate(
    sessions: &[ReadingSession],
    request: &ReadingOverviewRequest,
    now: UtcMillis,
) -> ReadingOverview {
    let offset_ms = i64::from(request.utc_offset_minutes) * 60_000;
    let days = i64::from(request.days);
    let end_day = (now.0 + offset_ms).div_euclid(DAY_MS);
    let start_day = end_day - (days - 1);

    let mut daily_duration = vec![0i64; request.days as usize];
    let mut category_duration = [0i64; ReadingSessionCategory::ALL.len()];
    let mut heatmap: std::collections::BTreeMap<(u8, u8), i64> = std::collections::BTreeMap::new();
    let mut session_count: u64 = 0;
    let mut total_duration_ms: i64 = 0;

    for session in sessions {
        let local_start = session.started_at.0 + offset_ms;
        let day_index = local_start.div_euclid(DAY_MS);
        // 领域层再判一次窗口归属：即使 Repository 返回更宽的区间，聚合也不会
        // 把窗口外的会话算进来。
        if day_index < start_day || day_index > end_day {
            continue;
        }
        let duration = session.duration_ms();
        category_duration[session.category.rank()] += duration;
        session_count += 1;
        total_duration_ms += duration;

        // Duration belongs to the day/hour buckets it actually spans, not only to the
        // session's start bucket. A session that crosses midnight or an hour boundary must
        // not inflate one day/heatmap cell while leaving the following buckets empty.
        let local_end = session.ended_at.0 + offset_ms;
        let mut segment_start = local_start;
        while segment_start < local_end {
            let segment_day = segment_start.div_euclid(DAY_MS);
            let day_end = (segment_day + 1) * DAY_MS;
            let segment_end = local_end.min(day_end);
            if (start_day..=end_day).contains(&segment_day) {
                let slot = (segment_day - start_day) as usize;
                daily_duration[slot] += segment_end - segment_start;

                let mut hour_start = segment_start;
                while hour_start < segment_end {
                    let hour_index = hour_start.div_euclid(HOUR_MS);
                    let hour_end = segment_end.min((hour_index + 1) * HOUR_MS);
                    let hour = hour_index.rem_euclid(24) as u8;
                    *heatmap
                        .entry((weekday_of_day_index(segment_day), hour))
                        .or_insert(0) += hour_end - hour_start;
                    hour_start = hour_end;
                }
            }
            segment_start = segment_end;
        }
    }

    let daily: Vec<ReadingDailyBucket> = (0..days)
        .map(|offset| {
            let day_index = start_day + offset;
            ReadingDailyBucket {
                local_date: local_date_of_day_index(day_index),
                weekday: weekday_of_day_index(day_index),
                duration_ms: u64::try_from(daily_duration[offset as usize]).unwrap_or(0),
            }
        })
        .collect();

    let mut categories: Vec<ReadingCategoryTotal> = ReadingSessionCategory::ALL
        .into_iter()
        .filter(|category| category_duration[category.rank()] > 0)
        .map(|category| ReadingCategoryTotal {
            category,
            duration_ms: u64::try_from(category_duration[category.rank()]).unwrap_or(0),
        })
        .collect();
    categories.sort_by(|left, right| {
        right
            .duration_ms
            .cmp(&left.duration_ms)
            .then_with(|| left.category.rank().cmp(&right.category.rank()))
    });

    let heatmap_cells: Vec<ReadingHeatmapCell> = heatmap
        .into_iter()
        .filter(|(_, duration)| *duration > 0)
        .map(|((weekday, hour), duration)| ReadingHeatmapCell {
            weekday,
            hour,
            duration_ms: u64::try_from(duration).unwrap_or(0),
        })
        .collect();

    let peak = peak_hour(&heatmap_cells);

    let has_records = session_count > 0;
    let recent_window = days.min(7) as usize;
    let recent_week: i64 = daily_duration[daily_duration.len() - recent_window..]
        .iter()
        .sum();

    ReadingOverview {
        range: ReadingOverviewRange {
            start_local_date: local_date_of_day_index(start_day),
            end_local_date: local_date_of_day_index(end_day),
            days: request.days,
            utc_offset_minutes: request.utc_offset_minutes,
        },
        session_count,
        total_duration_ms: u64::try_from(total_duration_ms).unwrap_or(0),
        daily,
        categories,
        heatmap_cells,
        peak_start_hour: peak.map(|hour| hour.0),
        peak_end_hour: peak.map(|hour| hour.1),
        longest_streak_days: has_records.then(|| longest_streak(&daily_duration)),
        average_daily_duration_ms: has_records
            .then(|| u64::try_from(total_duration_ms / days).unwrap_or(0)),
        recent_week_duration_ms: has_records.then(|| u64::try_from(recent_week).unwrap_or(0)),
    }
}

/// 时长最大的小时桶；并列取更早的小时，保证同一份事实总是得到同一个高峰。
fn peak_hour(cells: &[ReadingHeatmapCell]) -> Option<(u8, u8)> {
    let mut per_hour = [0i64; 24];
    for cell in cells {
        let slot = usize::from(cell.hour).min(23);
        per_hour[slot] += i64::try_from(cell.duration_ms).unwrap_or(0);
    }
    let mut best: Option<(u8, i64)> = None;
    for hour in 0..24u8 {
        let duration = per_hour[usize::from(hour)];
        if duration <= 0 {
            continue;
        }
        if best.is_none_or(|(_, current)| duration > current) {
            best = Some((hour, duration));
        }
    }
    best.map(|(hour, _)| (hour, hour.saturating_add(1)))
}

/// 窗口内最长连续活跃天数（时长 > 0 视为活跃）。
fn longest_streak(daily_duration: &[i64]) -> u32 {
    let mut longest = 0u32;
    let mut current = 0u32;
    for duration in daily_duration {
        if *duration > 0 {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

/// 1970-01-01 是星期四，因此 day_index 0 → weekday 4（ISO 周一 = 1）。
fn weekday_of_day_index(day_index: i64) -> u8 {
    (((day_index + 3).rem_euclid(7)) + 1) as u8
}

/// `YYYY-MM-DD`（本地日）。用 Howard Hinnant 的 civil_from_days，
/// 避免为一个纯算术问题引入时区数据库依赖。
fn local_date_of_day_index(day_index: i64) -> String {
    let z = day_index + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media_item() -> MediaItemId {
        MediaItemId::new()
    }

    fn session(
        started_at: i64,
        duration_ms: i64,
        category: ReadingSessionCategory,
    ) -> ReadingSession {
        ReadingSession::new(
            ReadingSessionId::new(),
            media_item(),
            category,
            UtcMillis(started_at),
            UtcMillis(started_at + duration_ms),
        )
        .unwrap()
    }

    /// 1970-01-01 是星期四；civil_from_days 与 ISO 星期序号都必须与之一致。
    #[test]
    fn local_calendar_helpers_match_the_known_epoch() {
        assert_eq!(local_date_of_day_index(0), "1970-01-01");
        assert_eq!(weekday_of_day_index(0), 4, "1970-01-01 是星期四");
        assert_eq!(weekday_of_day_index(1), 5);
        assert_eq!(weekday_of_day_index(3), 7, "1970-01-04 是星期日");
        assert_eq!(weekday_of_day_index(4), 1, "1970-01-05 是星期一");
        // 闰日：2024-02-29 位于 day_index 19782。
        assert_eq!(local_date_of_day_index(19_782), "2024-02-29");
        assert_eq!(weekday_of_day_index(19_782), 4, "2024-02-29 是星期四");
    }

    /// 时长由两个时刻得出，非正时长与超长会话都不进入事实集。
    #[test]
    fn sessions_require_a_plausible_positive_duration() {
        assert!(
            ReadingSession::new(
                ReadingSessionId::new(),
                media_item(),
                ReadingSessionCategory::Book,
                UtcMillis(1_000),
                UtcMillis(1_000),
            )
            .is_err()
        );
        assert!(
            ReadingSession::new(
                ReadingSessionId::new(),
                media_item(),
                ReadingSessionCategory::Book,
                UtcMillis(2_000),
                UtcMillis(1_000),
            )
            .is_err()
        );
        assert!(
            ReadingSession::new(
                ReadingSessionId::new(),
                media_item(),
                ReadingSessionCategory::Book,
                UtcMillis(0),
                UtcMillis(MAX_SESSION_DURATION_MS + 1),
            )
            .is_err()
        );
        let ok = session(0, 60_000, ReadingSessionCategory::Book);
        assert_eq!(ok.duration_ms(), 60_000);
    }

    /// `Unknown` 没有可归属的一级分类，只能被拒绝（不猜默认分类）。
    #[test]
    fn category_derivation_refuses_unknown_media_types() {
        assert_eq!(
            ReadingSessionCategory::from_media_type(MediaType::Episode),
            Some(ReadingSessionCategory::Video)
        );
        assert_eq!(
            ReadingSessionCategory::from_media_type(MediaType::Article),
            Some(ReadingSessionCategory::Periodical)
        );
        assert_eq!(
            ReadingSessionCategory::from_media_type(MediaType::Unknown),
            None
        );
        for category in ReadingSessionCategory::ALL {
            assert_eq!(
                ReadingSessionCategory::parse(category.as_str()),
                Some(category)
            );
        }
        assert_eq!(ReadingSessionCategory::parse("all"), None);
        assert_eq!(ReadingSessionCategory::parse(""), None);
    }

    #[test]
    fn request_rejects_out_of_range_windows() {
        assert!(ReadingOverviewRequest::new(0, 0).is_err());
        assert!(ReadingOverviewRequest::new(MAX_OVERVIEW_DAYS + 1, 0).is_err());
        assert!(ReadingOverviewRequest::new(7, MAX_UTC_OFFSET_MINUTES + 1).is_err());
        assert!(ReadingOverviewRequest::new(7, -MAX_UTC_OFFSET_MINUTES - 1).is_err());
        assert!(ReadingOverviewRequest::new(7, 480).is_ok());
    }

    /// 没有任何记录时：空事实集 → 零计数、None 统计量、逐日全 0（不是伪造的分钟数）。
    #[test]
    fn an_unused_device_reports_an_explicit_empty_result() {
        let request = ReadingOverviewRequest::new(7, 480).unwrap();
        // 2024-03-10T12:00:00Z（UTC+8 的 20 点）。
        let now = UtcMillis(1_710_072_000_000);
        let overview = aggregate(&[], &request, now);
        assert_eq!(overview.session_count, 0);
        assert_eq!(overview.total_duration_ms, 0);
        assert_eq!(overview.daily.len(), 7);
        assert!(overview.daily.iter().all(|bucket| bucket.duration_ms == 0));
        assert!(overview.categories.is_empty());
        assert!(overview.heatmap_cells.is_empty());
        assert_eq!(overview.peak_start_hour, None);
        assert_eq!(overview.peak_end_hour, None);
        assert_eq!(overview.longest_streak_days, None);
        assert_eq!(overview.average_daily_duration_ms, None);
        assert_eq!(overview.recent_week_duration_ms, None);
        assert_eq!(overview.range.days, 7);
        assert_eq!(overview.range.utc_offset_minutes, 480);
    }

    /// 真实会话按本地日/小时/分类落到正确的桶，且窗口外的会话被排除。
    #[test]
    fn recorded_sessions_land_in_their_local_buckets() {
        let request = ReadingOverviewRequest::new(3, 480).unwrap();
        // 本地 2024-03-10 20:00 = UTC 12:00（day_index 19792）。
        let now = UtcMillis(1_710_072_000_000);
        let day = 86_400_000i64;
        let sessions = vec![
            // 本地 2024-03-10 20:00 起 30 分钟。
            session(1_710_072_000_000, 30 * 60_000, ReadingSessionCategory::Book),
            // 本地 2024-03-09 08:00（UTC 前一日 00:00）起 90 分钟。
            session(
                1_709_942_400_000,
                90 * 60_000,
                ReadingSessionCategory::Comic,
            ),
            // 本地 2024-03-08 23:00（UTC 15:00）起 15 分钟。
            session(
                1_709_910_000_000,
                15 * 60_000,
                ReadingSessionCategory::Video,
            ),
            // 窗口之外（本地 2024-03-07 23:00）。
            session(
                1_709_910_000_000 - day,
                60 * 60_000,
                ReadingSessionCategory::Book,
            ),
        ];
        let overview = aggregate(&sessions, &request, now);
        assert_eq!(overview.session_count, 3, "窗口外会话不得计入");
        assert_eq!(overview.total_duration_ms, (30 + 90 + 15) * 60_000);
        assert_eq!(overview.range.start_local_date, "2024-03-08");
        assert_eq!(overview.range.end_local_date, "2024-03-10");
        assert_eq!(
            overview
                .daily
                .iter()
                .map(|bucket| (
                    bucket.local_date.as_str(),
                    bucket.weekday,
                    bucket.duration_ms
                ))
                .collect::<Vec<_>>(),
            vec![
                ("2024-03-08", 5, 15 * 60_000),
                ("2024-03-09", 6, 90 * 60_000),
                ("2024-03-10", 7, 30 * 60_000),
            ]
        );
        assert_eq!(
            overview
                .categories
                .iter()
                .map(|total| (total.category, total.duration_ms))
                .collect::<Vec<_>>(),
            vec![
                (ReadingSessionCategory::Comic, 90 * 60_000),
                (ReadingSessionCategory::Book, 30 * 60_000),
                (ReadingSessionCategory::Video, 15 * 60_000),
            ]
        );
        // 时长按小时边界拆分：8 点 60 分、9 点 30 分、20 点 30 分、23 点 15 分。
        assert_eq!(
            overview
                .heatmap_cells
                .iter()
                .map(|cell| (cell.weekday, cell.hour, cell.duration_ms))
                .collect::<Vec<_>>(),
            vec![
                (5, 23, 15 * 60_000),
                (6, 8, 60 * 60_000),
                (6, 9, 30 * 60_000),
                (7, 20, 30 * 60_000),
            ]
        );
        assert_eq!(overview.peak_start_hour, Some(8));
        assert_eq!(overview.peak_end_hour, Some(9));
        assert_eq!(overview.longest_streak_days, Some(3));
        assert_eq!(
            overview.recent_week_duration_ms,
            Some((30 + 90 + 15) * 60_000)
        );
        assert_eq!(
            overview.average_daily_duration_ms,
            Some(u64::try_from(((30 + 90 + 15) * 60_000i64) / 3).unwrap())
        );
    }

    /// 会话横跨小时和本地午夜时，日图与热力图按真实覆盖时长拆分。
    #[test]
    fn session_duration_is_split_across_local_day_and_hour_boundaries() {
        let request = ReadingOverviewRequest::new(3, 0).unwrap();
        let day = DAY_MS;
        let now = UtcMillis(3 * day + 12 * HOUR_MS);
        let overview = aggregate(
            &[session(
                2 * day + 23 * HOUR_MS + 30 * 60_000,
                2 * HOUR_MS,
                ReadingSessionCategory::Book,
            )],
            &request,
            now,
        );

        assert_eq!(overview.session_count, 1);
        assert_eq!(overview.total_duration_ms, 2 * HOUR_MS as u64);
        assert_eq!(
            overview
                .daily
                .iter()
                .map(|bucket| bucket.duration_ms)
                .collect::<Vec<_>>(),
            vec![0, 30 * 60_000, 90 * 60_000]
        );
        assert_eq!(
            overview
                .heatmap_cells
                .iter()
                .map(|cell| (cell.weekday, cell.hour, cell.duration_ms))
                .collect::<Vec<_>>(),
            vec![
                (6, 23, 30 * 60_000),
                (7, 0, 60 * 60_000),
                (7, 1, 30 * 60_000),
            ]
        );
        assert_eq!(
            overview
                .daily
                .iter()
                .map(|day| day.duration_ms)
                .sum::<u64>(),
            overview.total_duration_ms
        );
        assert_eq!(
            overview
                .heatmap_cells
                .iter()
                .map(|cell| cell.duration_ms)
                .sum::<u64>(),
            overview.total_duration_ms
        );
    }

    /// 连续天数按「有活动即连续」计算，中间空一天即断开。
    #[test]
    fn longest_streak_breaks_on_an_idle_day() {
        let request = ReadingOverviewRequest::new(5, 0).unwrap();
        let day = 86_400_000i64;
        let now = UtcMillis(4 * day + 3_600_000);
        let sessions = vec![
            session(0, 60_000, ReadingSessionCategory::Book),
            session(day, 60_000, ReadingSessionCategory::Book),
            // 第 2 天空缺。
            session(3 * day, 60_000, ReadingSessionCategory::Book),
        ];
        let overview = aggregate(&sessions, &request, now);
        assert_eq!(overview.longest_streak_days, Some(2));
    }

    /// 高峰并列时取更早的小时，保证同一份事实总得到同一个高峰。
    #[test]
    fn peak_hour_is_deterministic_on_ties() {
        let request = ReadingOverviewRequest::new(1, 0).unwrap();
        let now = UtcMillis(10 * 3_600_000);
        let sessions = vec![
            session(3_600_000, 60_000, ReadingSessionCategory::Book),
            session(5 * 3_600_000, 60_000, ReadingSessionCategory::Comic),
        ];
        let overview = aggregate(&sessions, &request, now);
        assert_eq!(overview.peak_start_hour, Some(1));
        assert_eq!(overview.peak_end_hour, Some(2));
    }

    /// UTC 偏移直接决定本地日归属：同一时刻在 UTC+8 与 UTC 属于不同的一天。
    #[test]
    fn utc_offset_decides_the_local_day() {
        // 2024-03-10T20:00:00Z
        let now = UtcMillis(1_710_100_800_000);
        let session_start = 1_710_100_800_000 - 3_600_000; // 19:00Z
        let sessions = vec![session(session_start, 60_000, ReadingSessionCategory::Book)];

        let utc = aggregate(&sessions, &ReadingOverviewRequest::new(1, 0).unwrap(), now);
        assert_eq!(utc.range.end_local_date, "2024-03-10");
        assert_eq!(utc.heatmap_cells[0].hour, 19);

        let east = aggregate(
            &sessions,
            &ReadingOverviewRequest::new(1, 480).unwrap(),
            now,
        );
        assert_eq!(east.range.end_local_date, "2024-03-11", "UTC+8 已跨日");
        assert_eq!(east.heatmap_cells[0].hour, 3);
    }
}
