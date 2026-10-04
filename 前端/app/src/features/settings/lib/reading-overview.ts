// 阅读总览的展示模型（页面只消费这一份）。
//
// 这不是后端统计契约，也不保存任何数据。真实聚合由 `readingOverviewGateway` 返回，
// 这里是那条 typed DTO → 页面模型的唯一映射：**所有时长都来自聚合结果本身**
// （`durationMs` 字段），不从 HistoryEntry / progress 的时间戳二次推算分钟数——
// 那会得到一份后端没算过的数字，也会让同一份事实在设置页与足迹页不一致。
//
// 空态是显式的：没有记录时可选统计量为 null（不是 0），页面显示「无 / 暂无数据」。

import type { ContentCategory, ReadingOverviewDto } from "@/lib/ipc/generated/wire"

/** 总览窗口：固定最近 7 天（后端按请求里的显式偏移换算本地日期）。 */
export const READING_OVERVIEW_DAYS = 7

/**
 * 窗口的用户可见名称，由 [`READING_OVERVIEW_DAYS`] 推出。
 *
 * 页头徽标、统计范围兜底文案与「最近 7 天」那张指标卡共用它：三者说的必须是**同一个**
 * 窗口，而这个窗口就是请求里真正发给后端的天数。写死一份「最近 7 天」会让改窗口时
 * 只改请求、不改界面，于是页面上出现一个后端从未算过的范围。
 */
export const READING_OVERVIEW_WINDOW_LABEL = `最近 ${READING_OVERVIEW_DAYS} 天`

export interface ReadingOverviewStats {
  range: {
    startLocalDate: string
    endLocalDate: string
    timezone: string
  }
  summary: {
    preferredType: ContentCategory | null
    longestStreakDays: number | null
    averageDailyMinutes: number | null
    /**
     * 窗口**末尾**最近 7 个本地日的时长合计（`recentWeekDurationMs`）。
     *
     * 它不是自然周：后端算的是「最后 7 个本地日」，跨周也照样取满 7 天。界面上因此
     * 只能叫「最近 7 天」，不能叫「本周」。
     */
    recentSevenDayMinutes: number | null
  }
  dailyMinutes: Array<{
    localDate: string
    label: string
    minutes: number
  }>
  typeShares: Array<{
    category: ContentCategory
    minutes: number
    percentage: number
  }>
  heatmap: {
    peakStartHour: number | null
    peakEndHour: number | null
    cells: Array<{
      dayOfWeek: number
      hour: number
      minutes: number
    }>
  }
}

export type ReadingOverviewState =
  | { status: "loading" }
  /** 读到了一份真实聚合，但窗口内没有任何阅读时长。 */
  | { status: "empty"; stats: ReadingOverviewStats }
  | { status: "ready"; stats: ReadingOverviewStats }
  | { status: "error"; message: string }

/**
 * 明确的空结果：没有阅读记录时使用 null/[]，不把「没有数据」伪装成 0。
 * 这也是加载中与错误态下页面兜底用的那份值。
 */
export const EMPTY_READING_OVERVIEW_STATS: ReadingOverviewStats = {
  range: {
    startLocalDate: "",
    endLocalDate: "",
    timezone: "",
  },
  summary: {
    preferredType: null,
    longestStreakDays: null,
    averageDailyMinutes: null,
    recentSevenDayMinutes: null,
  },
  dailyMinutes: [],
  typeShares: [],
  heatmap: {
    peakStartHour: null,
    peakEndHour: null,
    cells: [],
  },
}

/** ISO 星期序号（1 = 周一 … 7 = 周日）→ 单字标签。 */
const WEEKDAY_LABELS = ["一", "二", "三", "四", "五", "六", "日"] as const

function weekdayLabel(weekday: number): string {
  return WEEKDAY_LABELS[weekday - 1] ?? ""
}

/** 毫秒 → 分钟。聚合里的一天时长就是这一件事，不做任何二次估算。 */
function durationToMinutes(durationMs: number): number {
  return durationMs / 60_000
}

function nullableMinutes(durationMs: number | null): number | null {
  return durationMs === null ? null : durationToMinutes(durationMs)
}

/**
 * 当前运行环境的显式 UTC 偏移（分钟）。
 *
 * `Date#getTimezoneOffset()` 返回的是 UTC − 本地，符号与契约相反，所以这里取反；
 * 偏移由调用方显式交给后端，后端不读运行环境时区——同一份事实在任何机器上聚合出
 * 同一份结果。
 */
export function localUtcOffsetMinutes(now: Date = new Date()): number {
  return -now.getTimezoneOffset()
}

/** 偏移的展示形态（`UTC+08:00` / `UTC-03:30`）。 */
export function formatUtcOffset(minutes: number): string {
  const sign = minutes < 0 ? "-" : "+"
  const absolute = Math.abs(Math.round(minutes))
  const hours = String(Math.floor(absolute / 60)).padStart(2, "0")
  const rest = String(absolute % 60).padStart(2, "0")
  return `UTC${sign}${hours}:${rest}`
}

export function contentCategoryLabel(category: ContentCategory | null): string {
  switch (category) {
    case "book":
      return "小说"
    case "comic":
      return "漫画"
    case "video":
      return "影视"
    case "periodical":
      return "报刊"
    default:
      return "无"
  }
}

export function formatReadingDuration(minutes: number | null): string {
  if (minutes == null) return "无"
  const safeMinutes = Math.max(0, Math.round(minutes))
  const hours = Math.floor(safeMinutes / 60)
  const rest = safeMinutes % 60
  if (hours === 0) return `${rest}M`
  if (rest === 0) return `${hours}H`
  return `${hours}H ${rest}M`
}

export function formatReadingStreak(days: number | null): string {
  if (days == null) return "无"
  return `${Math.max(0, Math.round(days))} 天`
}

/** 是否至少有一项被聚合证实的阅读事实（而不是「窗口里有 7 个 0」）。 */
export function hasReadingOverviewData(stats: ReadingOverviewStats): boolean {
  return stats.dailyMinutes.some((item) => item.minutes > 0)
    || stats.typeShares.some((item) => item.minutes > 0)
    || stats.heatmap.cells.some((item) => item.minutes > 0)
    || stats.summary.preferredType !== null
    || stats.summary.longestStreakDays !== null
    || stats.summary.averageDailyMinutes !== null
    || stats.summary.recentSevenDayMinutes !== null
}

/**
 * typed DTO → 页面模型。
 *
 * 逐字段来自聚合结果：首选类型取时长最大的分类（并列时保持后端给出的顺序），
 * 占比由同一批分钟数导出，热力图的星期/小时直接沿用聚合的格子。
 */
export function mapReadingOverview(dto: ReadingOverviewDto): ReadingOverviewStats {
  const categories = [...dto.categories]
  let preferred: ContentCategory | null = null
  let preferredMs = -1
  for (const entry of categories) {
    if (entry.durationMs > preferredMs) {
      preferredMs = entry.durationMs
      preferred = entry.category
    }
  }
  const totalMs = categories.reduce((total, entry) => total + entry.durationMs, 0)

  return {
    range: {
      startLocalDate: dto.range.startLocalDate,
      endLocalDate: dto.range.endLocalDate,
      timezone: formatUtcOffset(dto.range.utcOffsetMinutes),
    },
    summary: {
      preferredType: preferred,
      longestStreakDays: dto.longestStreakDays,
      averageDailyMinutes: nullableMinutes(dto.averageDailyDurationMs),
      recentSevenDayMinutes: nullableMinutes(dto.recentWeekDurationMs),
    },
    dailyMinutes: dto.daily.map((bucket) => ({
      localDate: bucket.localDate,
      label: weekdayLabel(bucket.weekday),
      minutes: durationToMinutes(bucket.durationMs),
    })),
    typeShares: categories.map((entry) => ({
      category: entry.category,
      minutes: durationToMinutes(entry.durationMs),
      percentage: totalMs > 0 ? (entry.durationMs / totalMs) * 100 : 0,
    })),
    heatmap: {
      peakStartHour: dto.peakStartHour,
      peakEndHour: dto.peakEndHour,
      cells: dto.heatmapCells.map((cell) => ({
        dayOfWeek: cell.weekday,
        hour: cell.hour,
        minutes: durationToMinutes(cell.durationMs),
      })),
    },
  }
}

/** 请求参数：固定的 7 天窗口 + 当前环境的显式偏移。 */
export function readingOverviewRequest(now: Date = new Date()): {
  days: number
  utcOffsetMinutes: number
} {
  return { days: READING_OVERVIEW_DAYS, utcOffsetMinutes: localUtcOffsetMinutes(now) }
}

/** ready / empty 两态都带一份真实 stats；loading / error 用兜底空值。 */
export function readingOverviewStats(state: ReadingOverviewState): ReadingOverviewStats {
  return state.status === "ready" || state.status === "empty"
    ? state.stats
    : EMPTY_READING_OVERVIEW_STATS
}
