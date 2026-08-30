import { describe, expect, it, vi } from "vitest"
import {
  loadDemoPlayerData,
  recordDemoPlayerHistory,
  resolvePlayerRuntimeState,
} from "./player-runtime-state"

describe("player runtime state", () => {
  it("maps client modes to explicit Player states", () => {
    expect(resolvePlayerRuntimeState("mock")).toBe("demo")
    expect(resolvePlayerRuntimeState("tauri")).toBe("production")
    expect(resolvePlayerRuntimeState("unavailable")).toBe("unavailable")
  })

  it("selects sample presentation data only in the browser Demo", () => {
    const loadData = vi.fn(() => ({ title: "Demo title" }))

    expect(loadDemoPlayerData("tauri", loadData)).toBeNull()
    expect(loadDemoPlayerData("unavailable", loadData)).toBeNull()
    expect(loadData).not.toHaveBeenCalled()

    expect(loadDemoPlayerData("mock", loadData)).toEqual({ title: "Demo title" })
    expect(loadData).toHaveBeenCalledOnce()
  })

  it("records local Demo history only for the mock client", () => {
    const recordHistory = vi.fn()

    recordDemoPlayerHistory("tauri", "media-1", recordHistory)
    recordDemoPlayerHistory("unavailable", "media-1", recordHistory)
    expect(recordHistory).not.toHaveBeenCalled()

    recordDemoPlayerHistory("mock", "media-1", recordHistory)
    expect(recordHistory).toHaveBeenCalledOnce()
    expect(recordHistory).toHaveBeenCalledWith("media-1")
  })
})
