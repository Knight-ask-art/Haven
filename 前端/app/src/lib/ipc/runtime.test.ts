import { describe, expect, it } from "vitest"
import { resolveTauriRuntime, selectHavenClientMode } from "./runtime"

describe("runtime override", () => {
  it("keeps a detected Tauri WebView authoritative over mock overrides", () => {
    expect(resolveTauriRuntime(false, "tauri")).toBe(true)
    expect(resolveTauriRuntime(true, "mock")).toBe(true)
  })

  it("falls back to the detected WebView runtime when unset", () => {
    expect(resolveTauriRuntime(true, undefined)).toBe(true)
    expect(resolveTauriRuntime(false, undefined)).toBe(false)
  })
})

describe("Haven client runtime selection", () => {
  it("always selects the real client in Tauri", () => {
    expect(selectHavenClientMode({ tauri: true, dev: false, mockEnabled: false })).toBe("tauri")
  })

  it("selects Mock only in dev or with an explicit flag", () => {
    expect(selectHavenClientMode({ tauri: false, dev: true, mockEnabled: false })).toBe("mock")
    expect(selectHavenClientMode({ tauri: false, dev: false, mockEnabled: true })).toBe("mock")
  })

  it("fails closed for a browser production build without the flag", () => {
    expect(selectHavenClientMode({ tauri: false, dev: false, mockEnabled: false })).toBe("unavailable")
  })
})
