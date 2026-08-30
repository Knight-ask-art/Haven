import { describe, expect, it, vi } from "vitest"
import {
  loadDemoHistoryValue,
  resolveHistoryRuntimeState,
  shouldApplyHistoryRequest,
} from "./history-runtime-state"

describe("history runtime state", () => {
  it("maps client modes to explicit history states", () => {
    expect(resolveHistoryRuntimeState("mock")).toBe("demo")
    expect(resolveHistoryRuntimeState("tauri")).toBe("production")
    expect(resolveHistoryRuntimeState("unavailable")).toBe("unavailable")
  })

  it("reads Demo history storage only for the mock client", () => {
    const loadValue = vi.fn(() => ["demo-history"])

    expect(loadDemoHistoryValue("tauri", loadValue, [])).toEqual([])
    expect(loadDemoHistoryValue("unavailable", loadValue, [])).toEqual([])
    expect(loadValue).not.toHaveBeenCalled()

    expect(loadDemoHistoryValue("mock", loadValue, [])).toEqual(["demo-history"])
    expect(loadValue).toHaveBeenCalledOnce()
  })

  it("accepts only the latest history request response", () => {
    expect(shouldApplyHistoryRequest(2, 1)).toBe(false)
    expect(shouldApplyHistoryRequest(2, 2)).toBe(true)
  })
})
