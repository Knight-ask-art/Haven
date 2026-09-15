import { describe, expect, it } from "vitest"
import type {
  ComicChapterAggregateStatusDto,
  ComicWorkCatalogStatusDto,
  ComicWorkChapterCatalogDto,
} from "@/lib/ipc/generated/wire"
import {
  comicChapterDisplayTitle,
  comicChapterNumberLabel,
  comicChapterPageCountLabel,
  comicChapterProgressRatio,
  comicChapterStatusLabel,
  comicWorkCatalogStatusLabel,
  currentComicWorkChapter,
  hasSuggestedProgressMigration,
  resolveComicChapterNeighbour,
  selectComicWorkChapters,
} from "./comic-work-catalog-view"
import fixture from "../../../../../../contracts/ipc/v1/fixtures/comic/work-chapter-catalog.normal.json"

const catalog = fixture as unknown as ComicWorkChapterCatalogDto
const FIRST_EDITION = "33333333-3333-4333-8333-333333333301"
const SECOND_EDITION = "33333333-3333-4333-8333-333333333302"

describe("selectComicWorkChapters", () => {
  it("keeps the backend order and only filters by edition", () => {
    expect(selectComicWorkChapters(catalog, null).map((c) => c.backendOrder)).toEqual([0, 1, 2])
    expect(selectComicWorkChapters(catalog, FIRST_EDITION).map((c) => c.backendOrder)).toEqual([0, 1])
    expect(selectComicWorkChapters(catalog, SECOND_EDITION).map((c) => c.backendOrder)).toEqual([2])
    expect(selectComicWorkChapters(catalog, "66666666-6666-4666-8666-666666666666")).toEqual([])
  })

  it("returns a copy so callers cannot mutate the catalog order", () => {
    const selected = selectComicWorkChapters(catalog, null)
    selected.length = 0
    expect(catalog.chapters).toHaveLength(3)
  })
})

describe("currentComicWorkChapter", () => {
  it("prefers the backend currentMediaItemId", () => {
    expect(currentComicWorkChapter(catalog, undefined)?.mediaItemId).toBe(catalog.currentMediaItemId)
  })

  it("falls back to the session media item when the backend omits the current chapter", () => {
    const withoutCurrent = { ...catalog, currentMediaItemId: null }
    const second = catalog.chapters[1].mediaItemId
    expect(currentComicWorkChapter(withoutCurrent, second)?.mediaItemId).toBe(second)
    expect(currentComicWorkChapter(withoutCurrent, "99999999-9999-4999-8999-999999999999")).toBeNull()
  })
})

describe("resolveComicChapterNeighbour", () => {
  it("returns backend chapters and refuses unknown pointers", () => {
    expect(resolveComicChapterNeighbour(catalog, catalog.chapters[1].mediaItemId)?.backendOrder).toBe(1)
    expect(resolveComicChapterNeighbour(catalog, null)).toBeNull()
    expect(resolveComicChapterNeighbour(catalog, "99999999-9999-4999-8999-999999999999")).toBeNull()
  })
})

describe("labels", () => {
  it("covers every closed aggregate and catalog status", () => {
    const chapterStatuses: ComicChapterAggregateStatusDto[] = [
      "available",
      "temporarily_unavailable",
      "external_only",
      "unknown",
      "missing",
    ]
    for (const status of chapterStatuses) {
      expect(comicChapterStatusLabel(status).length).toBeGreaterThan(0)
    }
    const catalogStatuses: ComicWorkCatalogStatusDto[] = [
      "never_synced",
      "synced",
      "refresh_failed",
      "truncated",
    ]
    for (const status of catalogStatuses) {
      expect(comicWorkCatalogStatusLabel(status).length).toBeGreaterThan(0)
    }
  })

  it("describes the chapter without inventing a number", () => {
    const [first, second] = catalog.chapters
    expect(comicChapterNumberLabel(first)).toBe("第 2 卷 · 第 12.5 话")
    expect(comicChapterNumberLabel(second)).toBe("第 13 话")
    expect(comicChapterDisplayTitle(first, "占位")).toBe("幕间")
    expect(comicChapterDisplayTitle({ ...first, title: null }, "占位")).toBe("第 2 卷 · 第 12.5 话")
    expect(comicChapterDisplayTitle({ ...first, title: null, chapterNumber: null }, "占位")).toBe("占位")
    expect(comicChapterPageCountLabel(second)).toBe("0 页")
    expect(comicChapterPageCountLabel(catalog.chapters[2])).toBe("页数未知")
  })
})

describe("progress facts", () => {
  it("exposes the backend ratio only when present", () => {
    expect(comicChapterProgressRatio(catalog.chapters[0])).toBeCloseTo(0.4)
    expect(comicChapterProgressRatio(catalog.chapters[1])).toBeNull()
  })

  it("flags only backend-suggested migrations", () => {
    expect(hasSuggestedProgressMigration(catalog.chapters[0])).toBe(false)
    expect(hasSuggestedProgressMigration(catalog.chapters[2])).toBe(true)
  })
})
