import { describe, expect, it } from "vitest"
import {
  isComicChapterCatalogDto,
  isComicRegisteredChapterCatalogDto,
} from "./comic-chapter-catalog"

const sourceId = "mangadex"
const remoteWorkId = "aaaaaaaa-aaaa-4aaa-8000-aaaaaaaaaaaa"
const chapterId = "bbbbbbbb-bbbb-4bbb-8000-bbbbbbbbbbbb"

function catalog(): Record<string, unknown> {
  return {
    schemaVersion: 1,
    sourceId,
    remoteWorkId,
    fetchedAt: "2026-09-04T00:00:00Z",
    total: 1,
    truncated: false,
    chapters: [
      {
        remoteChapterId: chapterId,
        chapterNumber: 12.5,
        volumeNumber: 2,
        title: "幕间",
        pageCount: 24,
        publishedAt: "2026-08-01T00:00:00Z",
        updatedAt: null,
        availability: "available",
        editionProfile: {
          language: "zh-hk",
          languageKind: "known",
          translationLine: null,
          translationLineKind: "unknown",
          scanGroup: "Fixture Scan Group",
          scanGroupKind: "content_line",
          colorMode: "unknown",
        },
      },
    ],
  }
}

function registeredCatalog(): Record<string, unknown> {
  const value = catalog()
  return {
    schemaVersion: value.schemaVersion,
    sourceId: value.sourceId,
    remoteWorkId: value.remoteWorkId,
    refreshState: {
      generation: 3,
      fetchedAt: value.fetchedAt,
      total: value.total,
      truncated: false,
    },
    chapters: (value.chapters as Array<Record<string, unknown>>).map((chapter, index) => ({
      mediaItemId: `eeeeeeee-eeee-4eee-8eee-${String(index + 1).padStart(12, "0")}`,
      sourceId,
      remoteWorkId,
      remoteChapterId: chapter.remoteChapterId,
      chapterNumber: chapter.chapterNumber,
      volumeNumber: chapter.volumeNumber,
      title: chapter.title,
      pageCount: chapter.pageCount,
      sourceOrder: index,
      availability: chapter.availability,
      publishedAt: chapter.publishedAt,
      sourceUpdatedAt: chapter.updatedAt,
      lastSeenGeneration: 3,
      editionProfile: chapter.editionProfile,
    })),
  }
}

const foreignSymbol = Symbol("foreign")

function addForeignSymbolKey(target: object): void {
  Object.defineProperty(target, foreignSymbol, { value: "extra", enumerable: true })
}

function addHiddenStringKey(target: object): void {
  Object.defineProperty(target, "foreignField", { value: "extra", enumerable: false })
}

function firstChapter(value: Record<string, unknown>): Record<string, unknown> {
  return (value.chapters as Array<Record<string, unknown>>)[0]
}

function editionProfileOf(chapter: Record<string, unknown>): Record<string, unknown> {
  return chapter.editionProfile as Record<string, unknown>
}

describe("isComicChapterCatalogDto", () => {
  it("accepts a catalog with explicit unknown and known Edition facets", () => {
    expect(isComicChapterCatalogDto(catalog(), { sourceId, remoteWorkId })).toBe(true)
  })

  it.each([
    { schemaVersion: 2 },
    { sourceId: "other" },
    { remoteWorkId: "not-a-uuid" },
    { fetchedAt: "" },
    { unexpected: true },
  ])("rejects an invalid catalog envelope %#", (change) => {
    expect(isComicChapterCatalogDto({ ...catalog(), ...change })).toBe(false)
  })

  it("rejects duplicate identities, unknown fields and facet-kind mismatches", () => {
    const duplicate = catalog()
    duplicate.chapters = [
      ...(duplicate.chapters as Array<Record<string, unknown>>),
      { ...(duplicate.chapters as Array<Record<string, unknown>>)[0] },
    ]
    expect(isComicChapterCatalogDto(duplicate)).toBe(false)

    const unknownField = catalog()
    ;(unknownField.chapters as Array<Record<string, unknown>>)[0].url = "https://example.invalid/page"
    expect(isComicChapterCatalogDto(unknownField)).toBe(false)

    const mismatch = catalog()
    const profile = (mismatch.chapters as Array<Record<string, unknown>>)[0].editionProfile as Record<string, unknown>
    profile.languageKind = "unknown"
    expect(isComicChapterCatalogDto(mismatch)).toBe(false)
  })

  it("keeps unavailable and external-only chapters representable", () => {
    const value = catalog()
    const chapters = value.chapters as Array<Record<string, unknown>>
    chapters[0].availability = "temporarily_unavailable"
    chapters[0].pageCount = 0
    expect(isComicChapterCatalogDto(value)).toBe(true)

    chapters[0].availability = "external_only"
    chapters[0].pageCount = null
    expect(isComicChapterCatalogDto(value)).toBe(true)
  })

  it("rejects an unknown symbol own key on the envelope, chapter and Edition profile", () => {
    expect(isComicChapterCatalogDto(catalog(), { sourceId, remoteWorkId })).toBe(true)

    const onEnvelope = catalog()
    addForeignSymbolKey(onEnvelope)
    expect(isComicChapterCatalogDto(onEnvelope, { sourceId, remoteWorkId })).toBe(false)

    const onChapter = catalog()
    addForeignSymbolKey(firstChapter(onChapter))
    expect(isComicChapterCatalogDto(onChapter, { sourceId, remoteWorkId })).toBe(false)

    const onProfile = catalog()
    addForeignSymbolKey(editionProfileOf(firstChapter(onProfile)))
    expect(isComicChapterCatalogDto(onProfile, { sourceId, remoteWorkId })).toBe(false)
  })

  it("rejects an unknown non-enumerable own key on the envelope, chapter and Edition profile", () => {
    expect(isComicChapterCatalogDto(catalog(), { sourceId, remoteWorkId })).toBe(true)

    const onEnvelope = catalog()
    addHiddenStringKey(onEnvelope)
    expect(isComicChapterCatalogDto(onEnvelope, { sourceId, remoteWorkId })).toBe(false)

    const onChapter = catalog()
    addHiddenStringKey(firstChapter(onChapter))
    expect(isComicChapterCatalogDto(onChapter, { sourceId, remoteWorkId })).toBe(false)

    const onProfile = catalog()
    addHiddenStringKey(editionProfileOf(firstChapter(onProfile)))
    expect(isComicChapterCatalogDto(onProfile, { sourceId, remoteWorkId })).toBe(false)
  })
})

describe("isComicRegisteredChapterCatalogDto", () => {
  it("accepts persisted media identities and the refresh state", () => {
    expect(isComicRegisteredChapterCatalogDto(registeredCatalog(), { sourceId, remoteWorkId })).toBe(true)
  })

  it("accepts Missing without treating it as a provider availability value", () => {
    const value = registeredCatalog()
    ;(value.chapters as Array<Record<string, unknown>>)[0].availability = "missing"
    expect(isComicRegisteredChapterCatalogDto(value)).toBe(true)
  })

  it("rejects identity mismatches, unsafe generation and unknown fields", () => {
    const mismatch = registeredCatalog()
    ;(mismatch.chapters as Array<Record<string, unknown>>)[0].remoteWorkId = "other"
    expect(isComicRegisteredChapterCatalogDto(mismatch)).toBe(false)

    const invalidGeneration = registeredCatalog()
    ;(invalidGeneration.refreshState as Record<string, unknown>).generation = Number.MAX_SAFE_INTEGER + 1
    expect(isComicRegisteredChapterCatalogDto(invalidGeneration)).toBe(false)

    const unknownField = registeredCatalog()
    ;(unknownField.chapters as Array<Record<string, unknown>>)[0].url = "https://example.invalid/page"
    expect(isComicRegisteredChapterCatalogDto(unknownField)).toBe(false)
  })

  it("rejects an unknown symbol own key on the envelope, refresh state, chapter and Edition profile", () => {
    expect(isComicRegisteredChapterCatalogDto(registeredCatalog(), { sourceId, remoteWorkId })).toBe(true)

    const onEnvelope = registeredCatalog()
    addForeignSymbolKey(onEnvelope)
    expect(isComicRegisteredChapterCatalogDto(onEnvelope, { sourceId, remoteWorkId })).toBe(false)

    const onRefreshState = registeredCatalog()
    addForeignSymbolKey(onRefreshState.refreshState as Record<string, unknown>)
    expect(isComicRegisteredChapterCatalogDto(onRefreshState, { sourceId, remoteWorkId })).toBe(false)

    const onChapter = registeredCatalog()
    addForeignSymbolKey(firstChapter(onChapter))
    expect(isComicRegisteredChapterCatalogDto(onChapter, { sourceId, remoteWorkId })).toBe(false)

    const onProfile = registeredCatalog()
    addForeignSymbolKey(editionProfileOf(firstChapter(onProfile)))
    expect(isComicRegisteredChapterCatalogDto(onProfile, { sourceId, remoteWorkId })).toBe(false)
  })

  it("rejects an unknown non-enumerable own key on the envelope, refresh state, chapter and Edition profile", () => {
    expect(isComicRegisteredChapterCatalogDto(registeredCatalog(), { sourceId, remoteWorkId })).toBe(true)

    const onEnvelope = registeredCatalog()
    addHiddenStringKey(onEnvelope)
    expect(isComicRegisteredChapterCatalogDto(onEnvelope, { sourceId, remoteWorkId })).toBe(false)

    const onRefreshState = registeredCatalog()
    addHiddenStringKey(onRefreshState.refreshState as Record<string, unknown>)
    expect(isComicRegisteredChapterCatalogDto(onRefreshState, { sourceId, remoteWorkId })).toBe(false)

    const onChapter = registeredCatalog()
    addHiddenStringKey(firstChapter(onChapter))
    expect(isComicRegisteredChapterCatalogDto(onChapter, { sourceId, remoteWorkId })).toBe(false)

    const onProfile = registeredCatalog()
    addHiddenStringKey(editionProfileOf(firstChapter(onProfile)))
    expect(isComicRegisteredChapterCatalogDto(onProfile, { sourceId, remoteWorkId })).toBe(false)
  })
})
