import { describe, expect, it } from "vitest"
import type { PreferenceGetResult } from "./settings-wire"
import { guardPreferenceGetResult } from "./settings-wire"

const validPreferenceResult = (): PreferenceGetResult => ({
  schemaVersion: 1,
  mediaItemId: "media-1",
  editionId: "edition-1",
  readingPatch: { fontFamily: "serif", pagination: "paginated" },
  comicPatch: { viewMode: "single", direction: "rtl" },
  editionReadingPatch: { fontSize: "large", letterSpacing: "relaxed" },
  editionComicPatch: { pageGap: "twelve" },
  mediaItemReadingPatch: null,
  mediaItemComicPatch: { preloadPages: "three" },
  effectiveReading: {
    section: "reading",
    fontFamily: "serif",
    customFontFamily: null,
    fontSize: "large",
    lineHeight: "comfortable",
    contentWidth: "medium",
    theme: "warm",
    customBackground: null,
    customText: null,
    fontWeight: "regular",
    letterSpacing: "relaxed",
    systemAuto: true,
    pagination: "paginated",
  },
  effectiveComic: {
    section: "comic",
    viewMode: "single",
    direction: "rtl",
    pageGap: "twelve",
    preloadPages: "three",
  },
  mediaItemRevision: null,
  editionRevision: "edition-rev-1",
})
describe("resource preference wire guards", () => {
  it("accepts valid raw edition and media-item patches", () => {
    expect(guardPreferenceGetResult(validPreferenceResult())).toBe(true)
  })

  it("rejects an invalid value in any raw reading patch", () => {
    const value = validPreferenceResult()
    const malformed = {
      ...value,
      editionReadingPatch: { pagination: "book" },
    }
    expect(guardPreferenceGetResult(malformed)).toBe(false)
  })

  it("rejects an invalid value in any raw comic patch", () => {
    const value = validPreferenceResult()
    const malformed = {
      ...value,
      mediaItemComicPatch: { pageGap: "forty_eight" },
    }
    expect(guardPreferenceGetResult(malformed)).toBe(false)
  })

  it("requires all four raw patch fields in the v1 response", () => {
    const value = validPreferenceResult()
    const { mediaItemReadingPatch: _omitted, ...withoutField } = value
    expect(guardPreferenceGetResult(withoutField)).toBe(false)
  })
})
