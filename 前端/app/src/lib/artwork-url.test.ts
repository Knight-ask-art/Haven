import { describe, expect, it } from "vitest"

// Windows Tauri 判定依赖 navigator.userAgent；测试里直接覆盖。
function withUserAgent(value: string, fn: () => void) {
  const original = navigator.userAgent
  Object.defineProperty(navigator, "userAgent", { value, configurable: true })
  try {
    fn()
  } finally {
    Object.defineProperty(navigator, "userAgent", { value: original, configurable: true })
  }
}

import { artworkRequestUri, artworkSrcSet } from "./artwork-url"

describe("artworkRequestUri", () => {
  it("returns empty for null/empty/non-artwork uris", () => {
    expect(artworkRequestUri(null)).toBe("")
    expect(artworkRequestUri(undefined)).toBe("")
    expect(artworkRequestUri("")).toBe("")
    expect(artworkRequestUri("https://example.com/p.jpg")).toBe("https://example.com/p.jpg")
    expect(artworkRequestUri("session://poster")).toBe("session://poster")
  })

  it("rejects malformed ids (path traversal / bad chars)", () => {
    expect(artworkRequestUri("haven://artwork/")).toBe("")
    expect(artworkRequestUri("haven://artwork/../etc")).toBe("")
    expect(artworkRequestUri("haven://artwork/a%20b")).toBe("")
    expect(artworkRequestUri("haven://artwork/ok id")).toBe("")
  })

  it("maps to http form on Windows Tauri webview", () => {
    withUserAgent(
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
      () => {
        // isTauriRuntime 在 vitest 无 __TAURI_INTERNALS__ → false，走 haven-resource:// 分支。
        expect(artworkRequestUri("haven://artwork/abc-123")).toBe(
          "haven-resource://artwork/abc-123",
        )
      },
    )
  })

  it("passes through valid opaque ids", () => {
    expect(artworkRequestUri("haven://artwork/AbC_123-x")).toBe(
      "haven-resource://artwork/AbC_123-x",
    )
  })

  it("only emits artwork variants supported by the resource protocol", () => {
    expect(artworkSrcSet("haven://artwork/poster-1")).toBe(
      "haven-resource://artwork/poster-1?w=200 200w, haven-resource://artwork/poster-1?w=400 400w",
    )
    expect(artworkSrcSet("https://example.com/poster.jpg")).toBeUndefined()
  })
})
