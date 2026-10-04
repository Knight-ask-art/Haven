// @vitest-environment jsdom

import { act, cleanup, renderHook, waitFor } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"
import type { ReadingOverviewDto } from "@/lib/ipc/generated/wire"
import { HavenError } from "@/lib/ipc/errors"
import { guardReadingOverview, guardReadingOverviewGetRequest } from "@/lib/ipc/settings-wire"
import type { ReadingOverviewGateway } from "../ipc/reading-overview-gateway"
import { readingOverviewGet } from "../ipc/reading-overview-gateway"
import {
  EMPTY_READING_OVERVIEW_STATS,
  READING_OVERVIEW_DAYS,
  formatUtcOffset,
  hasReadingOverviewData,
  localUtcOffsetMinutes,
  mapReadingOverview,
  readingOverviewRequest,
  readingOverviewStats,
} from "./reading-overview"
import { loadReadingOverview, useReadingOverview } from "./useReadingOverview"

const OFFSET_MINUTES = 8 * 60

/**
 * 一份契约合法的聚合样例。
 *
 * 先过 `guardReadingOverview` 再当 DTO 用：映射测试的输入必须是后端真的会返回的形状，
 * 否则这里测的只是「一个想象出来的对象」。
 */
function overviewFixture(): ReadingOverviewDto {
  const dto: ReadingOverviewDto = {
    schemaVersion: 1,
    range: {
      startLocalDate: "2026-09-17",
      endLocalDate: "2026-09-23",
      days: 7,
      utcOffsetMinutes: OFFSET_MINUTES,
    },
    sessionCount: 12,
    totalDurationMs: 7 * 3_600_000,
    daily: [
      { localDate: "2026-09-17", weekday: 4, durationMs: 0 },
      { localDate: "2026-09-18", weekday: 5, durationMs: 1_800_000 },
      { localDate: "2026-09-19", weekday: 6, durationMs: 3_600_000 },
      { localDate: "2026-09-20", weekday: 7, durationMs: 0 },
      { localDate: "2026-09-21", weekday: 1, durationMs: 7_200_000 },
      { localDate: "2026-09-22", weekday: 2, durationMs: 0 },
      { localDate: "2026-09-23", weekday: 3, durationMs: 14_400_000 },
    ],
    categories: [
      { category: "book", durationMs: 18_000_000 },
      { category: "comic", durationMs: 7_200_000 },
      { category: "video", durationMs: 3_600_000 },
    ],
    heatmapCells: [
      { weekday: 3, hour: 21, durationMs: 14_400_000 },
      { weekday: 1, hour: 8, durationMs: 7_200_000 },
    ],
    peakStartHour: 21,
    peakEndHour: 22,
    longestStreakDays: 2,
    averageDailyDurationMs: 3_600_000,
    recentWeekDurationMs: 5_400_000,
  }
  if (!guardReadingOverview(dto)) throw new Error("样例必须是合法聚合")
  return dto
}

/** 空库形态：sessionCount=0、可选统计量为 null、daily 仍逐日展开。 */
function emptyOverviewFixture(): ReadingOverviewDto {
  return {
    schemaVersion: 1,
    range: {
      startLocalDate: "2026-09-17",
      endLocalDate: "2026-09-23",
      days: 7,
      utcOffsetMinutes: OFFSET_MINUTES,
    },
    sessionCount: 0,
    totalDurationMs: 0,
    daily: [
      { localDate: "2026-09-17", weekday: 4, durationMs: 0 },
      { localDate: "2026-09-18", weekday: 5, durationMs: 0 },
      { localDate: "2026-09-19", weekday: 6, durationMs: 0 },
      { localDate: "2026-09-20", weekday: 7, durationMs: 0 },
      { localDate: "2026-09-21", weekday: 1, durationMs: 0 },
      { localDate: "2026-09-22", weekday: 2, durationMs: 0 },
      { localDate: "2026-09-23", weekday: 3, durationMs: 0 },
    ],
    categories: [],
    heatmapCells: [],
    peakStartHour: null,
    peakEndHour: null,
    longestStreakDays: null,
    averageDailyDurationMs: null,
    recentWeekDurationMs: null,
  }
}

function stubGateway(dto: ReadingOverviewDto): { gateway: ReadingOverviewGateway; calls: Array<{ days: number; utcOffsetMinutes: number }> } {
  const calls: Array<{ days: number; utcOffsetMinutes: number }> = []
  return {
    calls,
    gateway: {
      readingOverviewGet: async (request) => {
        calls.push(request)
        return dto
      },
    },
  }
}

afterEach(cleanup)

describe("reading overview mapping", () => {
  it("asks for a bounded seven-day window with the caller's explicit offset", () => {
    // 固定时刻，避免测试依赖运行机器的时区。
    const fixed = new Date("2026-09-23T12:00:00Z")
    const request = readingOverviewRequest(fixed)
    expect(request.days).toBe(READING_OVERVIEW_DAYS)
    expect(request.days).toBe(7)
    expect(request.utcOffsetMinutes).toBe(localUtcOffsetMinutes(fixed))
    expect(guardReadingOverviewGetRequest(request)).toBe(true)
    // 浏览器偏移的符号与契约一致（UTC − 本地 取反）。
    expect(localUtcOffsetMinutes(new Date("2026-09-23T12:00:00Z"))).toBe(-fixed.getTimezoneOffset())
  })

  it("formats the explicit offset for display", () => {
    expect(formatUtcOffset(480)).toBe("UTC+08:00")
    expect(formatUtcOffset(-210)).toBe("UTC-03:30")
    expect(formatUtcOffset(0)).toBe("UTC+00:00")
  })

  it("derives every number from the aggregate, never from history timestamps", () => {
    const stats = mapReadingOverview(overviewFixture())
    expect(stats.range).toEqual({
      startLocalDate: "2026-09-17",
      endLocalDate: "2026-09-23",
      timezone: "UTC+08:00",
    })
    expect(stats.summary.preferredType).toBe("book")
    expect(stats.summary.longestStreakDays).toBe(2)
    expect(stats.summary.averageDailyMinutes).toBe(60)
    expect(stats.summary.recentSevenDayMinutes).toBe(90)
    expect(stats.dailyMinutes.map((item) => item.minutes)).toEqual([0, 30, 60, 0, 120, 0, 240])
    expect(stats.dailyMinutes.map((item) => item.label)).toEqual(["四", "五", "六", "日", "一", "二", "三"])
    expect(stats.typeShares.map((share) => share.minutes)).toEqual([300, 120, 60])
    // 占比由同一批分钟数导出：5H / 8H = 62.5%。
    expect(stats.typeShares[0]?.percentage).toBeCloseTo(62.5, 5)
    expect(stats.heatmap.peakStartHour).toBe(21)
    expect(stats.heatmap.peakEndHour).toBe(22)
    expect(stats.heatmap.cells).toEqual([
      { dayOfWeek: 3, hour: 21, minutes: 240 },
      { dayOfWeek: 1, hour: 8, minutes: 120 },
    ])
  })

  it("keeps an unused device empty instead of fabricating zeros", () => {
    const stats = mapReadingOverview(emptyOverviewFixture())
    expect(hasReadingOverviewData(stats)).toBe(false)
    expect(stats.summary.preferredType).toBeNull()
    expect(stats.summary.longestStreakDays).toBeNull()
    expect(stats.summary.averageDailyMinutes).toBeNull()
    expect(stats.summary.recentSevenDayMinutes).toBeNull()
    expect(stats.typeShares).toEqual([])
    expect(stats.heatmap.cells).toEqual([])
    // 逐日桶仍然按窗口展开（描述的是窗口本身），且时长真的是 0。
    expect(stats.dailyMinutes).toHaveLength(7)
    expect(stats.dailyMinutes.every((item) => item.minutes === 0)).toBe(true)
  })

  it("distinguishes loading, empty, ready and error states", async () => {
    const ready = await loadReadingOverview(stubGateway(overviewFixture()).gateway)
    expect(ready.status).toBe("ready")
    const empty = await loadReadingOverview(stubGateway(emptyOverviewFixture()).gateway)
    expect(empty.status).toBe("empty")

    const failing: ReadingOverviewGateway = {
      readingOverviewGet: async () => {
        throw new HavenError({ code: "DATABASE_ERROR", userMessage: "数据库暂时不可用", retryable: true })
      },
    }
    const failed = await loadReadingOverview(failing)
    expect(failed.status).toBe("error")
    if (failed.status === "error") expect(failed.message).toBe("数据库暂时不可用")

    // loading / error 两态不会伪装成一份空统计。
    expect(readingOverviewStats({ status: "loading" })).toBe(EMPTY_READING_OVERVIEW_STATS)
    expect(readingOverviewStats({ status: "error", message: "x" })).toBe(EMPTY_READING_OVERVIEW_STATS)
    expect(empty.status).toBe("empty")
    if (empty.status === "empty") {
      expect(readingOverviewStats(empty)).toBe(empty.stats)
      expect(readingOverviewStats(empty).range.startLocalDate).toBe("2026-09-17")
    }
  })

  it("passes the explicit offset all the way to the client", async () => {
    const { gateway, calls } = stubGateway(overviewFixture())
    await readingOverviewGet({ days: 7, utcOffsetMinutes: 480 }, {
      readingOverviewGet: gateway.readingOverviewGet,
    } as never)
    expect(calls).toEqual([{ days: 7, utcOffsetMinutes: 480 }])
  })
})

describe("useReadingOverview", () => {
  it("goes loading → ready with the aggregate-backed stats", async () => {
    const { gateway, calls } = stubGateway(overviewFixture())
    const { result } = renderHook(() => useReadingOverview(gateway))
    expect(result.current.state.status).toBe("loading")
    await waitFor(() => expect(result.current.state.status).toBe("ready"))
    expect(calls).toHaveLength(1)
    expect(calls[0]?.days).toBe(7)
    if (result.current.state.status === "ready") {
      expect(result.current.state.stats.summary.preferredType).toBe("book")
    }
  })

  it("lands on the explicit empty state for an unused device", async () => {
    const { gateway } = stubGateway(emptyOverviewFixture())
    const { result } = renderHook(() => useReadingOverview(gateway))
    await waitFor(() => expect(result.current.state.status).toBe("empty"))
  })

  it("surfaces a failure and can retry it", async () => {
    let attempt = 0
    const gateway: ReadingOverviewGateway = {
      readingOverviewGet: async () => {
        attempt += 1
        if (attempt === 1) {
          throw new HavenError({ code: "DATABASE_ERROR", userMessage: "读取失败", retryable: true })
        }
        return overviewFixture()
      },
    }
    const { result } = renderHook(() => useReadingOverview(gateway))
    await waitFor(() => expect(result.current.state.status).toBe("error"))
    act(() => result.current.reload())
    await waitFor(() => expect(result.current.state.status).toBe("ready"))
    expect(attempt).toBe(2)
  })

  it("ignores a stale response that lands after a newer request", async () => {
    const resolvers: Array<(dto: ReadingOverviewDto) => void> = []
    const gateway: ReadingOverviewGateway = {
      readingOverviewGet: () => new Promise<ReadingOverviewDto>((resolve) => { resolvers.push(resolve) }),
    }
    const { result } = renderHook(() => useReadingOverview(gateway))
    await waitFor(() => expect(resolvers).toHaveLength(1))
    act(() => result.current.reload())
    await waitFor(() => expect(resolvers).toHaveLength(2))
    // 先发出的那次晚落地：它不得覆盖第二次的结果。
    await act(async () => { resolvers[1]?.(overviewFixture()) })
    await waitFor(() => expect(result.current.state.status).toBe("ready"))
    await act(async () => { resolvers[0]?.(emptyOverviewFixture()) })
    expect(result.current.state.status).toBe("ready")
  })

  it("refuses an out-of-range window before any IPC round trip", async () => {
    const inner = vi.fn(async () => emptyOverviewFixture())
    expect(guardReadingOverviewGetRequest({ days: 0, utcOffsetMinutes: 0 })).toBe(false)
    expect(guardReadingOverviewGetRequest({ days: 7, utcOffsetMinutes: 100_000 })).toBe(false)
    await expect(
      readingOverviewGet({ days: 0, utcOffsetMinutes: 0 }, { readingOverviewGet: inner } as never),
    ).rejects.toMatchObject({ code: "READING_OVERVIEW_INVALID_RANGE" })
    expect(inner).not.toHaveBeenCalled()
  })
})
