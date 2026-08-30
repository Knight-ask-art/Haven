import { describe, expect, it } from "vitest"
import { canConsumeDetail } from "./work-detail-state"

describe("canConsumeDetail", () => {
  it("blocks production fallback states but preserves demo behavior", () => {
    expect(canConsumeDetail(true, "loading")).toBe(false)
    expect(canConsumeDetail(true, "retryable_error")).toBe(false)
    expect(canConsumeDetail(true, "terminal_error")).toBe(false)
    expect(canConsumeDetail(true, "data")).toBe(true)
    expect(canConsumeDetail(false, "terminal_error")).toBe(true)
  })
})
