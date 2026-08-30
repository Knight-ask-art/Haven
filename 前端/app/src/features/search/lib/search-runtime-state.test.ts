import { describe, expect, it, vi } from "vitest"
import { loadDemoSearchHistory, resolveSearchRuntimeState } from "./search-runtime-state"

describe("resolveSearchRuntimeState", () => {
  it("keeps sample search available only for the explicit mock client", () => {
    expect(resolveSearchRuntimeState("mock", "")).toBe("demo")
    expect(resolveSearchRuntimeState("mock", "haven")).toBe("demo")
  })

  it("uses the local library for Tauri while keeping the empty state explicit", () => {
    expect(resolveSearchRuntimeState("tauri", "")).toBe("ready_empty")
    expect(resolveSearchRuntimeState("tauri", "haven")).toBe("ready_query")
  })

  it("fails closed for production browsers without mock access", () => {
    expect(resolveSearchRuntimeState("unavailable", "")).toBe("unavailable_empty")
    expect(resolveSearchRuntimeState("unavailable", " haven ")).toBe("unavailable_query")
  })

  it("does not read persisted search history outside the browser demo", () => {
    const readHistory = vi.fn(() => ["haven"])

    expect(loadDemoSearchHistory("tauri", readHistory)).toEqual([])
    expect(loadDemoSearchHistory("unavailable", readHistory)).toEqual([])
    expect(readHistory).not.toHaveBeenCalled()

    expect(loadDemoSearchHistory("mock", readHistory)).toEqual(["haven"])
    expect(readHistory).toHaveBeenCalledOnce()
  })
})
