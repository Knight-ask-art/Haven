import { describe, expect, it, vi } from "vitest"
import {
  canUseDemoArticleReaderTools,
  loadDemoArticleReaderValue,
  recordDemoArticleReaderHistory,
  resolveArticleReaderRuntimeState,
} from "./article-reader-runtime-state"

describe("article reader runtime state", () => {
  it("maps client modes to explicit Article Reader states", () => {
    expect(resolveArticleReaderRuntimeState("mock")).toBe("demo")
    expect(resolveArticleReaderRuntimeState("tauri")).toBe("production")
    expect(resolveArticleReaderRuntimeState("unavailable")).toBe("unavailable")
  })

  it("loads local Demo state only for the mock client", () => {
    const loadValue = vi.fn(() => ["demo-value"])

    expect(loadDemoArticleReaderValue("tauri", loadValue, [])).toEqual([])
    expect(loadDemoArticleReaderValue("unavailable", loadValue, [])).toEqual([])
    expect(loadValue).not.toHaveBeenCalled()

    expect(loadDemoArticleReaderValue("mock", loadValue, [])).toEqual(["demo-value"])
    expect(loadValue).toHaveBeenCalledOnce()
  })

  it("records local history only for the mock client", () => {
    const recordHistory = vi.fn()

    recordDemoArticleReaderHistory("tauri", "media-1", recordHistory)
    recordDemoArticleReaderHistory("unavailable", "media-1", recordHistory)
    expect(recordHistory).not.toHaveBeenCalled()

    recordDemoArticleReaderHistory("mock", "media-1", recordHistory)
    expect(recordHistory).toHaveBeenCalledOnce()
    expect(recordHistory).toHaveBeenCalledWith("media-1")
  })

  it("enables static AI and annotation tools only in the browser Demo", () => {
    expect(canUseDemoArticleReaderTools("mock")).toBe(true)
    expect(canUseDemoArticleReaderTools("tauri")).toBe(false)
    expect(canUseDemoArticleReaderTools("unavailable")).toBe(false)
  })
})
