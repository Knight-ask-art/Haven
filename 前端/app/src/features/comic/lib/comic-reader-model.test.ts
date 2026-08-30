import { describe, expect, it } from "vitest"
import type { ComicPageManifestDto } from "@/lib/ipc/generated/wire"
import { createDemoComicPageSequence, mapComicPageManifest, pageAt, pageNumbersAround, resolveComicReaderDefaults } from "./comic-reader-model"

const manifest: ComicPageManifestDto = {
  schemaVersion: 1,
  sessionId: "0196f0d2-0000-7000-8000-000000000001",
  mediaItemId: "0196f0d2-0000-7000-8000-000000000002",
  pageCount: 3,
  pages: [
    { pageId: "0196f0d2-0000-7000-8000-000000000003", pageIndex: 0, availability: "ready", contentUri: "haven-resource://comic-page/0196f0d2-0000-7000-8000-000000000004" },
    { pageId: "0196f0d2-0000-7000-8000-000000000005", pageIndex: 1, availability: "unavailable", contentUri: null },
    { pageId: "0196f0d2-0000-7000-8000-000000000006", pageIndex: 2, availability: "ready", contentUri: "haven-resource://comic-page/0196f0d2-0000-7000-8000-000000000007" },
  ],
}

describe("comic-reader-model", () => {
  it("preserves manifest order and unavailable page positions", () => {
    const sequence = mapComicPageManifest(manifest)
    expect(sequence.pages.map((page) => page.pageNumber)).toEqual([1, 2, 3])
    expect(pageAt(sequence, 2)?.availability).toBe("unavailable")
    expect(pageAt(sequence, 2)?.contentUri).toBeNull()
  })

  it("bounds a prefetch window without changing page identity", () => {
    expect(pageNumbersAround(1, 5, 2)).toEqual([1, 2, 3, 4, 5])
    expect(pageNumbersAround(5000, 5000, 2)).toEqual([4998, 4999, 5000])
  })

  it("creates browser-only demo pages", () => {
    const sequence = createDemoComicPageSequence(["demo://one", "demo://two"], 3)
    expect(sequence.pages.map((page) => page.contentUri)).toEqual(["demo://one", "demo://two", "demo://one"])
  })

  it("maps persisted Comic Settings to bounded reader defaults", () => {
    expect(resolveComicReaderDefaults({
      section: "comic",
      viewMode: "double",
      direction: "ltr",
      pageGap: "twenty_four",
      preloadPages: "five",
    })).toEqual({ viewMode: "double", direction: "ltr", pageGapPx: 24, preloadRadius: 5 })

    expect(resolveComicReaderDefaults({
      section: "comic",
      viewMode: "strip",
      direction: "rtl",
      pageGap: "zero",
      preloadPages: "unlimited",
    })).toEqual({ viewMode: "strip", direction: "rtl", pageGapPx: 0, preloadRadius: 12 })
  })
})
