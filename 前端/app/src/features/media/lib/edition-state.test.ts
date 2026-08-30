import { describe, expect, it } from "vitest"
import { HavenError } from "@/lib/ipc/errors"
import { canConsumeEdition, getEditionListState } from "./edition-state"

describe("edition list state", () => {
  it("distinguishes loading, empty, retryable and terminal states", () => {
    expect(getEditionListState(true, true, null, null)).toBe("loading")
    expect(getEditionListState(true, false, [], null)).toBe("empty")
    expect(getEditionListState(true, false, null, new HavenError({ code: "TEMP", userMessage: "retry", retryable: true }))).toBe("retryable_error")
    expect(getEditionListState(true, false, null, new HavenError({ code: "BAD", userMessage: "bad", retryable: false }))).toBe("terminal_error")
  })

  it("requires a backend primary action before production consumption", () => {
    expect(canConsumeEdition(true, "data", false)).toBe(false)
    expect(canConsumeEdition(true, "data", true)).toBe(true)
    expect(canConsumeEdition(false, "empty", false)).toBe(true)
  })
})
