import { describe, expect, it, vi } from "vitest"
import {
  loadDemoBookReaderBookmarks,
  recordDemoBookReaderHistory,
  resolveBookReaderRuntimeState,
  selectDemoBookReaderKind,
} from "./book-reader-runtime-state"

describe("book reader runtime state", () => {
  it("maps client modes to explicit Book Reader states", () => {
    expect(resolveBookReaderRuntimeState("mock")).toBe("demo")
    expect(resolveBookReaderRuntimeState("tauri")).toBe("production")
    expect(resolveBookReaderRuntimeState("unavailable")).toBe("unavailable")
  })

  it("loads local bookmarks only in the browser Demo", () => {
    const loadBookmarks = vi.fn(() => [{ id: "bookmark-1" }])

    expect(loadDemoBookReaderBookmarks("tauri", loadBookmarks)).toEqual([])
    expect(loadDemoBookReaderBookmarks("unavailable", loadBookmarks)).toEqual([])
    expect(loadBookmarks).not.toHaveBeenCalled()

    expect(loadDemoBookReaderBookmarks("mock", loadBookmarks)).toEqual([{ id: "bookmark-1" }])
    expect(loadBookmarks).toHaveBeenCalledOnce()
  })

  it("records local history only in the browser Demo", () => {
    const recordHistory = vi.fn()

    recordDemoBookReaderHistory("tauri", "media-1", recordHistory)
    recordDemoBookReaderHistory("unavailable", "media-1", recordHistory)
    expect(recordHistory).not.toHaveBeenCalled()

    recordDemoBookReaderHistory("mock", "media-1", recordHistory)
    expect(recordHistory).toHaveBeenCalledOnce()
    expect(recordHistory).toHaveBeenCalledWith("media-1")
  })

  it("selects bounded demo fixtures by the demo storage id", () => {
    expect(selectDemoBookReaderKind("mock", "p1-1")).toBe("periodical")
    expect(selectDemoBookReaderKind("mock", "d1-1")).toBe("document")
    expect(selectDemoBookReaderKind("mock", "book-jobs")).toBe("book")
    expect(selectDemoBookReaderKind("tauri", "p1-1")).toBeNull()
  })
})
