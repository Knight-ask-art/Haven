import { describe, expect, it } from "vitest"
import { isReaderTocResultDto, isTocItemDto } from "./reader-toc"

const item = { id: "9a00d70e24a8f7b1", title: "序言", depth: 0, progression: 0.25 }

describe("isTocItemDto", () => {
  it("accepts a canonical item", () => {
    expect(isTocItemDto(item)).toBe(true)
  })

  it("rejects missing, extra, or mistyped fields", () => {
    expect(isTocItemDto({ ...item, depth: undefined })).toBe(false)
    expect(isTocItemDto({ ...item, extra: 1 })).toBe(false)
    expect(isTocItemDto({ ...item, id: "" })).toBe(false)
    expect(isTocItemDto({ ...item, title: "  " })).toBe(false)
    expect(isTocItemDto({ ...item, depth: 1.5 })).toBe(false)
    expect(isTocItemDto({ ...item, depth: -1 })).toBe(false)
    expect(isTocItemDto({ ...item, depth: 300 })).toBe(false)
  })

  it("clamps progression to the 0..1 closed range and requires finite numbers", () => {
    expect(isTocItemDto({ ...item, progression: 0 })).toBe(true)
    expect(isTocItemDto({ ...item, progression: 1 })).toBe(true)
    expect(isTocItemDto({ ...item, progression: -0.01 })).toBe(false)
    expect(isTocItemDto({ ...item, progression: 1.01 })).toBe(false)
    expect(isTocItemDto({ ...item, progression: Number.NaN })).toBe(false)
    expect(isTocItemDto({ ...item, progression: Number.POSITIVE_INFINITY })).toBe(false)
  })
})

const result = {
  schemaVersion: 1,
  sessionId: "11111111-1111-4111-8111-111111111111",
  items: [item],
}

describe("isReaderTocResultDto", () => {
  it("accepts a canonical result and matches the expected session", () => {
    expect(isReaderTocResultDto(result)).toBe(true)
    expect(isReaderTocResultDto(result, { sessionId: result.sessionId })).toBe(true)
    expect(isReaderTocResultDto(result, { sessionId: "22222222-2222-4222-8222-222222222222" })).toBe(false)
  })

  it("rejects wrong schema version, malformed session, and oversized item lists", () => {
    expect(isReaderTocResultDto({ ...result, schemaVersion: 2 })).toBe(false)
    expect(isReaderTocResultDto({ ...result, sessionId: "not-a-uuid" })).toBe(false)
    const oversized = { ...result, items: Array.from({ length: 8193 }, () => item) }
    expect(isReaderTocResultDto(oversized)).toBe(false)
  })
})