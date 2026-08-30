import { describe, expect, it } from "vitest"
import { resolveComicReaderRuntimeState } from "./comic-reader-runtime-state"

describe("comic reader runtime state", () => {
  it("keeps Demo content in mock mode", () => {
    expect(resolveComicReaderRuntimeState("mock")).toBe("demo")
  })

  it("does not select the Demo sequence construction path in production", () => {
    expect(resolveComicReaderRuntimeState("tauri")).not.toBe("demo")
    expect(resolveComicReaderRuntimeState("unavailable")).not.toBe("demo")
  })

  it("uses the real manifest path in Tauri", () => {
    expect(resolveComicReaderRuntimeState("tauri")).toBe("production")
  })

  it("fails closed when no Haven client is available", () => {
    expect(resolveComicReaderRuntimeState("unavailable")).toBe("unavailable")
  })
})
