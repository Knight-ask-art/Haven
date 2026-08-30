import { describe, expect, it } from "vitest"
import { isComicPageManifestDto } from "./comic-page-manifest"

const sessionId = "0196f0d2-0000-7000-8000-000000000001"
const mediaItemId = "0196f0d2-0000-7000-8000-000000000002"
const pageId1 = "0196f0d2-0000-7000-8000-000000000011"
const pageId2 = "0196f0d2-0000-7000-8000-000000000012"
const grant1 = "0196f0d2-0000-7000-8000-000000000021"

function manifest(): Record<string, unknown> {
  return {
    schemaVersion: 1,
    sessionId,
    mediaItemId,
    pageCount: 2,
    pages: [
      {
        pageId: pageId1,
        pageIndex: 0,
        availability: "ready",
        contentUri: `haven-resource://comic-page/${grant1}`,
      },
      {
        pageId: pageId2,
        pageIndex: 1,
        availability: "unavailable",
        contentUri: null,
      },
    ],
  }
}

describe("isComicPageManifestDto", () => {
  it("accepts exact ready/unavailable pages and expected identities", () => {
    expect(isComicPageManifestDto(manifest(), { sessionId, mediaItemId })).toBe(true)
    expect(isComicPageManifestDto({ ...manifest(), pageCount: 0, pages: [] })).toBe(true)
  })

  it.each([
    { schemaVersion: 2 },
    { sessionId: sessionId.toUpperCase() },
    { sessionId: "not-a-uuid" },
    { mediaItemId: "" },
    { pageCount: -1 },
    { pageCount: 0.5 },
    { pageCount: 5_001 },
    { pageCount: 1 },
  ])("rejects an invalid top-level field set %#", (change) => {
    expect(isComicPageManifestDto({ ...manifest(), ...change })).toBe(false)
  })

  it("rejects unknown top-level and page fields", () => {
    expect(isComicPageManifestDto({ ...manifest(), locator: "secret" })).toBe(false)
    const value = manifest()
    const pages = value.pages as Array<Record<string, unknown>>
    pages[0] = { ...pages[0], mimeType: "image/jpeg" }
    expect(isComicPageManifestDto(value)).toBe(false)
  })

  it("rejects discontinuous indices and duplicate or intersecting identities", () => {
    const discontinuous = manifest()
    ;(discontinuous.pages as Array<Record<string, unknown>>)[1].pageIndex = 2
    expect(isComicPageManifestDto(discontinuous)).toBe(false)

    const duplicatePage = manifest()
    ;(duplicatePage.pages as Array<Record<string, unknown>>)[1].pageId = pageId1
    expect(isComicPageManifestDto(duplicatePage)).toBe(false)

    const intersecting = manifest()
    ;(intersecting.pages as Array<Record<string, unknown>>)[0].pageId = grant1
    expect(isComicPageManifestDto(intersecting)).toBe(false)
  })

  it.each([
    `haven-resource://comic-page/${grant1}?x=1`,
    `haven-resource://comic-page/${grant1}#x`,
    `haven-resource://comic-page/${grant1}/extra`,
    `haven-resource://comic-page/%2F${grant1}`,
    `haven-resource://COMIC-PAGE/${grant1}`,
    `HAVEN-RESOURCE://comic-page/${grant1}`,
    `haven-resource://comic-page/${grant1.toUpperCase()}`,
    `haven-resource://comic-page\\${grant1}`,
  ])("rejects a non-canonical grant URI: %s", (contentUri) => {
    const value = manifest()
    ;(value.pages as Array<Record<string, unknown>>)[0].contentUri = contentUri
    expect(isComicPageManifestDto(value)).toBe(false)
  })

  it("rejects availability/content mismatches and duplicate grants", () => {
    const readyWithoutUri = manifest()
    ;(readyWithoutUri.pages as Array<Record<string, unknown>>)[0].contentUri = null
    expect(isComicPageManifestDto(readyWithoutUri)).toBe(false)

    const unavailableWithUri = manifest()
    const pages = unavailableWithUri.pages as Array<Record<string, unknown>>
    pages[1].contentUri = pages[0].contentUri
    expect(isComicPageManifestDto(unavailableWithUri)).toBe(false)

    const duplicateGrant = manifest()
    const duplicatePages = duplicateGrant.pages as Array<Record<string, unknown>>
    duplicatePages[1] = {
      ...duplicatePages[1],
      availability: "ready",
      contentUri: duplicatePages[0].contentUri,
    }
    expect(isComicPageManifestDto(duplicateGrant)).toBe(false)
  })

  it("rejects a response bound to another session or media item", () => {
    expect(isComicPageManifestDto(manifest(), {
      sessionId: "0196f0d2-0000-7000-8000-000000000099",
      mediaItemId,
    })).toBe(false)
    expect(isComicPageManifestDto(manifest(), { sessionId, mediaItemId: "other" })).toBe(false)
  })
})
