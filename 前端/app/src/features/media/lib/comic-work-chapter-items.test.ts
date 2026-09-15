import { describe, expect, it } from "vitest"
import type { ComicWorkChapterCatalogDto } from "@/lib/ipc/generated/wire"
import { primaryActionRoute } from "./primary-action-route"
import { projectComicWorkChapterRows, projectComicWorkEditions } from "./comic-work-chapter-items"
import fixture from "../../../../../../contracts/ipc/v1/fixtures/comic/work-chapter-catalog.normal.json"

const catalog = fixture as unknown as ComicWorkChapterCatalogDto

describe("projectComicWorkChapterRows", () => {
  it("keeps the backend chapter order and projects backend facts only", () => {
    const rows = projectComicWorkChapterRows(catalog)

    expect(rows.map((row) => row.id)).toEqual(catalog.chapters.map((c) => c.mediaItemId))
    expect(rows[0].title).toBe("幕间")
    expect(rows[0].number).toBe("第 2 卷 · 第 12.5 话")
    expect(rows[0].pageCountLabel).toBe("24 页")
    expect(rows[0].progressPercent).toBeCloseTo(40)
  })

  it("carries status, canOpen, subjectId, sources and the current marker from the DTO", () => {
    const rows = projectComicWorkChapterRows(catalog)

    expect(rows.map((row) => row.status)).toEqual(catalog.chapters.map((c) => c.status))
    expect(rows.map((row) => row.canOpen)).toEqual(catalog.chapters.map((c) => c.canOpen))
    expect(rows.map((row) => row.subjectId)).toEqual(catalog.chapters.map((c) => c.subjectId))
    expect(rows.map((row) => row.editionId)).toEqual(catalog.chapters.map((c) => c.editionId))
    expect(rows.map((row) => row.sourceCount)).toEqual(catalog.chapters.map((c) => c.sources.length))
    expect(rows[0].statusLabel).toBe("可阅读")
    expect(rows[1].statusLabel).toBe("暂不可用")
    expect(rows[2].statusLabel).toBe("仅外部来源")
    expect(rows[0].sourceSummaryLabel).toBe("1 个来源 · 可阅读")
    // 当前章节由后端 currentMediaItemId 决定，只有一行命中。
    expect(rows.filter((row) => row.isCurrent)).toHaveLength(1)
    expect(rows[0].isCurrent).toBe(true)
  })

  it("keeps the raw chapter DTO on every row instead of dropping backendOrder, sources or matchResult", () => {
    const rows = projectComicWorkChapterRows(catalog)

    // 原始 DTO 按引用原样保留：既不复制成新对象，也不改写字段。
    rows.forEach((row, index) => {
      expect(row.chapter).toBe(catalog.chapters[index])
    })
    expect(rows.map((row) => row.chapter.backendOrder)).toEqual(catalog.chapters.map((c) => c.backendOrder))
    expect(rows.map((row) => row.chapter.sources)).toEqual(catalog.chapters.map((c) => c.sources))
    expect(rows.map((row) => row.chapter.matchResult)).toEqual(catalog.chapters.map((c) => c.matchResult))

    // 被压成 sourceCount / needsMatchReview 的细节仍可从原始 chapter 读回。
    expect(rows.map((row) => row.chapter.sources.length)).toEqual(catalog.chapters.map((c) => c.sources.length))
    expect(rows.map((row) => row.chapter.sources[0].source.remoteChapterId))
      .toEqual(catalog.chapters.map((c) => c.sources[0].source.remoteChapterId))
    expect(rows[2].chapter.matchResult).toEqual(catalog.chapters[2].matchResult)
    expect(rows[2].chapter.matchResult?.confidence).toBe("low")
    expect(rows[2].chapter.matchResult?.progressMigration).toBe("suggested")
  })

  it("flags the low-confidence match the backend wants the user to confirm", () => {
    const rows = projectComicWorkChapterRows(catalog)
    expect(rows.map((row) => row.needsMatchReview)).toEqual([false, false, true])
  })

  it("routes an openable chapter to the comic reader route", () => {
    const [first] = projectComicWorkChapterRows(catalog)
    expect(first.primaryAction?.kind).toBe("comic")
    expect(first.primaryAction?.mediaItemId).toBe(catalog.chapters[0].mediaItemId)
    expect(primaryActionRoute(first.primaryAction)).toBe(`/comic/${catalog.chapters[0].mediaItemId}`)
  })

  it("marks the backend current chapter as the continue target", () => {
    const rows = projectComicWorkChapterRows(catalog)
    const current = rows.find((row) => row.id === catalog.currentMediaItemId)
    expect(current?.primaryAction?.labelHint).toBe("continue")
    expect(rows.filter((row) => row.primaryAction?.labelHint === "continue")).toHaveLength(1)
  })

  it("does not invent a primary action for chapters the backend cannot open", () => {
    const rows = projectComicWorkChapterRows(catalog)
    expect(rows[1].primaryAction).toBeNull()
    expect(rows[2].primaryAction).toBeNull()
  })

  it("carries the backend locator so progress resumes where the server says", () => {
    const [first] = projectComicWorkChapterRows(catalog)
    expect(first.primaryAction?.locator).toEqual(catalog.chapters[0].progress?.locator)
  })

  it("returns an empty list for a catalog without chapters", () => {
    expect(projectComicWorkChapterRows({ ...catalog, chapters: [] })).toEqual([])
  })
})

describe("projectComicWorkEditions", () => {
  it("projects catalog.editions instead of deriving editions from chapters", () => {
    const editions = projectComicWorkEditions(catalog)
    expect(editions).toEqual(catalog.editions.map((edition) => ({
      editionId: edition.editionId,
      label: edition.displayLabel,
      chapterCount: edition.chapterCount,
      profile: edition.profile,
    })))
  })

  it("keeps the backend edition profile instead of only the label and count", () => {
    const editions = projectComicWorkEditions(catalog)
    expect(editions.map((edition) => edition.profile)).toEqual(catalog.editions.map((edition) => edition.profile))
    expect(editions[0].profile).toBe(catalog.editions[0].profile)
    expect(editions[0].profile.language).toBe("zh-hk")
    expect(editions[0].profile.scanGroup).toBe("Fixture Scan Group")
    expect(editions[1].profile.colorMode).toBe("grayscale")
  })
})
