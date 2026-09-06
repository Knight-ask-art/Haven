import { describe, expect, it } from "vitest"
import { canLoadLibraryNextPage, resolveLibraryRuntimeState } from "./library-runtime-state"

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

describe("library pagination ownership", () => {
  const ready = {
    nextCursor: "200",
    isLoadingMore: false,
    isFirstPagePending: false,
    cursorQueryKey: "query-b",
    activeQueryKey: "query-b",
    partialError: false,
  }

  it("allows a cursor only after its own first page succeeds", () => {
    expect(canLoadLibraryNextPage(ready)).toBe(true)
    expect(canLoadLibraryNextPage({ ...ready, isFirstPagePending: true })).toBe(false)
    expect(canLoadLibraryNextPage({ ...ready, cursorQueryKey: "query-a" })).toBe(false)
  })

  it("stops automatic pagination after an error until explicit retry clears it", () => {
    expect(canLoadLibraryNextPage({ ...ready, partialError: true })).toBe(false)
    expect(canLoadLibraryNextPage({ ...ready, partialError: false })).toBe(true)
  })
})
