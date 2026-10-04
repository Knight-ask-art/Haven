import { describe, expect, it } from "vitest"
import {
  EMPTY_READING_OVERVIEW_STATS,
  contentCategoryLabel,
  formatReadingDuration,
  formatReadingStreak,
  hasReadingOverviewData,
} from "./reading-overview"

describe("reading overview presentation model", () => {
  it("keeps an unused device as an explicit empty result", () => {
    expect(hasReadingOverviewData(EMPTY_READING_OVERVIEW_STATS)).toBe(false)
    expect(EMPTY_READING_OVERVIEW_STATS.summary.recentSevenDayMinutes).toBeNull()
    expect(EMPTY_READING_OVERVIEW_STATS.dailyMinutes).toEqual([])
    expect(EMPTY_READING_OVERVIEW_STATS.heatmap.cells).toEqual([])
  })

  it("formats only values that are backed by an aggregate", () => {
    expect(formatReadingDuration(null)).toBe("无")
    expect(formatReadingDuration(0)).toBe("0M")
    expect(formatReadingDuration(109)).toBe("1H 49M")
    expect(formatReadingStreak(null)).toBe("无")
    expect(formatReadingStreak(6)).toBe("6 天")
  })

  it("maps the canonical content categories to the overview labels", () => {
    expect(contentCategoryLabel("book")).toBe("小说")
    expect(contentCategoryLabel("comic")).toBe("漫画")
    expect(contentCategoryLabel("video")).toBe("影视")
    expect(contentCategoryLabel("periodical")).toBe("报刊")
    expect(contentCategoryLabel(null)).toBe("无")
  })
})
