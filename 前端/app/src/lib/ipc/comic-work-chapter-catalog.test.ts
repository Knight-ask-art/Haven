import { describe, expect, it } from "vitest"
import type { ComicWorkChapterCatalogDto } from "./generated/wire"
import { isComicWorkChapterCatalogDto } from "./comic-work-chapter-catalog"
import fixture from "../../../../../contracts/ipc/v1/fixtures/comic/work-chapter-catalog.normal.json"

const WORK_ID = "11111111-1111-4111-8111-111111111111"
const MEDIA_ITEM_ID = "22222222-2222-4222-8222-222222222201"

function normal(): ComicWorkChapterCatalogDto {
  return fixture as unknown as ComicWorkChapterCatalogDto
}

/** fixture 首个章节的进度块，去掉 keyframeUri，便于逐个变体覆盖该字段。 */
function progressBlockOf(catalog: ComicWorkChapterCatalogDto): Record<string, unknown> {
  const progress: Record<string, unknown> = { ...catalog.chapters[0].progress }
  delete progress.keyframeUri
  return progress
}

function withFirstChapterProgress(
  catalog: ComicWorkChapterCatalogDto,
  progress: unknown,
): Record<string, unknown> {
  return {
    ...catalog,
    chapters: [{ ...catalog.chapters[0], progress }, ...catalog.chapters.slice(1)],
  }
}

/** 用给定的 Locator 替换 fixture 首个章节的进度块，其余字段保持合法。 */
function withFirstChapterLocator(
  catalog: ComicWorkChapterCatalogDto,
  locator: unknown,
): Record<string, unknown> {
  return withFirstChapterProgress(catalog, { ...progressBlockOf(catalog), locator })
}

/** fixture 只带 comic locator，其余四种形状在此按 wire.ts 字段集合补齐。 */
const VALID_LOCATORS: Record<string, Record<string, unknown>> = {
  video: { version: 1, kind: "video", data: { positionMs: 1500 } },
  book: {
    version: 1,
    kind: "book",
    data: {
      publicationResource: "OEBPS/chapter-1.xhtml",
      progression: 0.25,
      textAnchor: { exact: "锚点", prefix: null, suffix: null },
      formatLocator: null,
    },
  },
  pdf: {
    version: 1,
    kind: "pdf",
    data: { pageIndex: 3, x: null, y: null, zoom: 1.5, textAnchor: null },
  },
  comic: {
    version: 1,
    kind: "comic",
    data: { chapterItemId: MEDIA_ITEM_ID, pageIndex: 9, pageProgression: 0.4 },
  },
  article: {
    version: 1,
    kind: "article",
    data: { blockId: null, progression: 0.1, textAnchor: { exact: "x", prefix: "a", suffix: "b" } },
  },
}

describe("isComicWorkChapterCatalogDto", () => {
  it("accepts the contract fixture", () => {
    expect(isComicWorkChapterCatalogDto(normal())).toBe(true)
  })

  it("accepts the identity echo for both target shapes", () => {
    expect(isComicWorkChapterCatalogDto(normal(), { workId: WORK_ID })).toBe(true)
    expect(isComicWorkChapterCatalogDto(normal(), { mediaItemId: MEDIA_ITEM_ID })).toBe(true)
  })

  it("rejects a catalog that belongs to another work or chapter", () => {
    const other = "99999999-9999-4999-8999-999999999999"
    expect(isComicWorkChapterCatalogDto(normal(), { workId: other })).toBe(false)
    expect(isComicWorkChapterCatalogDto(normal(), { mediaItemId: other })).toBe(false)
  })

  it("rejects unknown or missing fields", () => {
    expect(isComicWorkChapterCatalogDto({ ...normal(), extra: 1 })).toBe(false)
    const missing: Record<string, unknown> = { ...normal() }
    delete missing.truncated
    expect(isComicWorkChapterCatalogDto(missing)).toBe(false)
  })

  it("rejects a non-canonical local identity", () => {
    // 判别数据必须含十六进制字母，否则 toUpperCase() 是恒等变换、断言必然通过。
    const canonicalLowercase = "a1111111-1111-4111-8111-1111111111a1"
    expect(isComicWorkChapterCatalogDto({ ...normal(), workId: canonicalLowercase })).toBe(true)
    expect(isComicWorkChapterCatalogDto({
      ...normal(),
      workId: canonicalLowercase.toUpperCase(),
    })).toBe(false)
    expect(isComicWorkChapterCatalogDto({ ...normal(), workId: "https://example.invalid/work" })).toBe(false)
  })

  it("rejects a chapter that references an edition outside the response", () => {
    const catalog = normal()
    const broken = {
      ...catalog,
      chapters: catalog.chapters.map((chapter, index) => index === 0
        ? { ...chapter, editionId: "88888888-8888-4888-8888-888888888888" }
        : chapter),
    }
    expect(isComicWorkChapterCatalogDto(broken)).toBe(false)
  })

  it("rejects duplicate chapter or edition identities", () => {
    const catalog = normal()
    expect(isComicWorkChapterCatalogDto({
      ...catalog,
      chapters: [catalog.chapters[0], { ...catalog.chapters[1], mediaItemId: catalog.chapters[0].mediaItemId }],
    })).toBe(false)
    expect(isComicWorkChapterCatalogDto({
      ...catalog,
      editions: [catalog.editions[0], { ...catalog.editions[1], editionId: catalog.editions[0].editionId }],
    })).toBe(false)
  })

  it("rejects a status outside the closed aggregate union", () => {
    const catalog = normal()
    expect(isComicWorkChapterCatalogDto({
      ...catalog,
      chapters: [{ ...catalog.chapters[0], status: "partially_available" }],
    })).toBe(false)
    expect(isComicWorkChapterCatalogDto({ ...catalog, refreshStatus: "pending" })).toBe(false)
  })

  it("rejects a progress locator that is not the closed comic shape", () => {
    const catalog = normal()
    const chapter = catalog.chapters[0]
    expect(isComicWorkChapterCatalogDto({
      ...catalog,
      chapters: [{
        ...chapter,
        progress: {
          mediaItemId: chapter.mediaItemId,
          completion: "in_progress",
          progressRatio: 0.4,
          revision: "12",
          updatedAt: "2026-09-04T00:00:00Z",
          locator: { version: 1, kind: "comic", data: { chapterItemId: "not-a-uuid", pageIndex: 0, pageProgression: null } },
        },
      }],
    })).toBe(false)
  })

  it("accepts the explicit keyframeUri: null that Rust emits for a missing keyframe", () => {
    // Rust `ProgressSummaryDto` 只声明 #[serde(default)]，缺失时 None 会被显式
    // 序列化成 null；守卫若只接受字符串或省略，真实响应会被整体判为非法目录。
    const catalog = normal()
    expect(catalog.chapters[0].progress?.keyframeUri).toBeNull()
    expect(isComicWorkChapterCatalogDto(catalog)).toBe(true)
    expect(isComicWorkChapterCatalogDto(
      withFirstChapterProgress(catalog, { ...progressBlockOf(catalog), keyframeUri: null }),
    )).toBe(true)
  })

  it("keeps the keyframeUri text and length boundary", () => {
    const catalog = normal()
    const accepts = (keyframeUri: unknown) => isComicWorkChapterCatalogDto(
      withFirstChapterProgress(catalog, { ...progressBlockOf(catalog), keyframeUri }),
    )

    // 省略仍是合法形状（本地构造与 serde default 都允许）。
    const withoutKey = progressBlockOf(catalog)
    expect(isComicWorkChapterCatalogDto(withFirstChapterProgress(catalog, withoutKey))).toBe(true)
    expect(accepts("data:image/png;base64,AAAA")).toBe(true)
    expect(accepts("")).toBe(false)
    expect(accepts(42)).toBe(false)
    expect(accepts("x".repeat(8193))).toBe(false)
  })

  it("rejects a previous/next pointer that is not a canonical local identity", () => {
    const catalog = normal()
    expect(isComicWorkChapterCatalogDto({
      ...catalog,
      chapters: [{ ...catalog.chapters[0], nextMediaItemId: "remote-chapter-12" }],
    })).toBe(false)
  })

  it("accepts every closed locator kind the wire union allows", () => {
    const catalog = normal()
    for (const [kind, locator] of Object.entries(VALID_LOCATORS)) {
      expect(
        { kind, accepted: isComicWorkChapterCatalogDto(withFirstChapterLocator(catalog, locator)) },
      ).toEqual({ kind, accepted: true })
    }
  })

  it("rejects an unknown field inside any locator data shape", () => {
    const catalog = normal()
    const accepts = (locator: unknown) => isComicWorkChapterCatalogDto(
      withFirstChapterLocator(catalog, locator),
    )

    // 五种 data 形状各自与 generated/wire.ts 字段完全闭合；多余字段一律越界。
    for (const [kind, locator] of Object.entries(VALID_LOCATORS)) {
      const data = locator.data as Record<string, unknown>
      expect({ kind, accepted: accepts({ ...locator, data: { ...data, extra: 1 } }) })
        .toEqual({ kind, accepted: false })
    }

    // 缺失字段同样非法：Rust 侧这些结构体没有 serde(default)。
    expect(accepts({ version: 1, kind: "comic", data: { chapterItemId: MEDIA_ITEM_ID, pageIndex: 0 } }))
      .toBe(false)
    expect(accepts({ version: 1, kind: "video", data: {} })).toBe(false)
  })

  it("rejects an unknown or missing field on the locator envelope", () => {
    const catalog = normal()
    const accepts = (locator: unknown) => isComicWorkChapterCatalogDto(
      withFirstChapterLocator(catalog, locator),
    )

    // 外层信封是手写 Serialize 固定写出的 {version, kind, data}：多余字段越界。
    for (const [kind, locator] of Object.entries(VALID_LOCATORS)) {
      expect({ kind, accepted: accepts({ ...locator, extra: 1 }) })
        .toEqual({ kind, accepted: false })
    }

    const comic = VALID_LOCATORS.comic
    // 三个必需键缺一不可。
    expect(accepts({ kind: "comic", data: comic.data })).toBe(false)
    expect(accepts({ version: 1, data: comic.data })).toBe(false)
    expect(accepts({ version: 1, kind: "comic" })).toBe(false)
    // 版本是契约字面量 1，不是任意 number。
    expect(accepts({ ...comic, version: 2 })).toBe(false)
    // kind 必须落在闭集里；未知 kind 没有对应的 data 形状。
    expect(accepts({ ...comic, kind: "generic" })).toBe(false)
  })

  it("rejects an unknown or missing field inside TextAnchorDto", () => {
    const catalog = normal()
    const accepts = (textAnchor: unknown) => isComicWorkChapterCatalogDto(
      withFirstChapterLocator(catalog, {
        ...VALID_LOCATORS.book,
        data: { ...(VALID_LOCATORS.book.data as Record<string, unknown>), textAnchor },
      }),
    )

    expect(accepts({ exact: "x", prefix: null, suffix: null })).toBe(true)
    expect(accepts({ exact: "x", prefix: null, suffix: null, extra: 1 })).toBe(false)
    expect(accepts({ exact: "x", prefix: null })).toBe(false)
  })
})
