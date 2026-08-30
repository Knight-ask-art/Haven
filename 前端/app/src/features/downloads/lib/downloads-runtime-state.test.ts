import { describe, expect, it } from "vitest"
import { resolveDownloadsRuntimeState } from "./downloads-runtime-state"

describe("downloads runtime state", () => {
  it("keeps the real task flow available in Tauri and the typed mock", () => {
    expect(resolveDownloadsRuntimeState("mock")).toBe("ready")
    expect(resolveDownloadsRuntimeState("tauri")).toBe("ready")
    expect(resolveDownloadsRuntimeState("unavailable")).toBe("unavailable")
  })
})
