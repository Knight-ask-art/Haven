import { describe, expect, it } from "vitest"
import { resolveMediaDetailRuntimeState } from "./media-detail-runtime-state"

describe("media detail runtime state", () => {
  it("keeps the curated catalog in explicit Demo mode", () => {
    expect(resolveMediaDetailRuntimeState("mock")).toBe("demo")
  })

  it("keeps Tauri on the authoritative Work Detail path", () => {
    expect(resolveMediaDetailRuntimeState("tauri")).toBe("production")
  })

  it("fails closed when no Haven client is available", () => {
    expect(resolveMediaDetailRuntimeState("unavailable")).toBe("unavailable")
  })
})
