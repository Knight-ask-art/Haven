import { describe, expect, it } from "vitest"
import { resolveLibraryRuntimeState } from "./library-runtime-state"

describe("library runtime state", () => {
  it("allows the representative catalog only in explicit Demo mode", () => {
    expect(resolveLibraryRuntimeState("mock")).toBe("demo")
  })

  it("keeps Tauri on the authoritative library_list path", () => {
    expect(resolveLibraryRuntimeState("tauri")).toBe("production")
  })

  it("fails closed for production browsers without a Haven client", () => {
    expect(resolveLibraryRuntimeState("unavailable")).toBe("unavailable")
  })
})
