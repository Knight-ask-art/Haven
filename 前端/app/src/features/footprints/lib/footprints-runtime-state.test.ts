import { describe, expect, it, vi } from "vitest"
import {
  canLoadFootprintsData,
  loadDemoFootprintMarkers,
  resolveDemoFootprintsEmptyState,
  resolveFootprintsRuntimeState,
} from "./footprints-runtime-state"

describe("footprints runtime state", () => {
  it("maps client modes to explicit Footprints states", () => {
    expect(resolveFootprintsRuntimeState("mock")).toBe("demo")
    expect(resolveFootprintsRuntimeState("tauri")).toBe("production")
    expect(resolveFootprintsRuntimeState("unavailable")).toBe("unavailable")
  })

  it("does not initialize Demo marker data outside the browser Demo", () => {
    const loadMarkers = vi.fn(() => [{ id: "marker-demo" }])

    expect(loadDemoFootprintMarkers("tauri", loadMarkers)).toEqual([])
    expect(loadDemoFootprintMarkers("unavailable", loadMarkers)).toEqual([])
    expect(loadMarkers).not.toHaveBeenCalled()

    expect(loadDemoFootprintMarkers("mock", loadMarkers)).toEqual([{ id: "marker-demo" }])
    expect(loadMarkers).toHaveBeenCalledOnce()
  })

  it("prevents unavailable browsers from loading Footprints data", () => {
    expect(canLoadFootprintsData("mock")).toBe(true)
    expect(canLoadFootprintsData("tauri")).toBe(true)
    expect(canLoadFootprintsData("unavailable")).toBe(false)
  })

  it("accepts synthetic empty state only in the browser Demo", () => {
    expect(resolveDemoFootprintsEmptyState("mock", "empty")).toBe(true)
    expect(resolveDemoFootprintsEmptyState("mock", null)).toBe(false)
    expect(resolveDemoFootprintsEmptyState("tauri", "empty")).toBe(false)
    expect(resolveDemoFootprintsEmptyState("unavailable", "empty")).toBe(false)
  })
})
