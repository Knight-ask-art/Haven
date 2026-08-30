import { describe, expect, it } from "vitest"
import { HavenError } from "./ipc/errors"
import {
  deriveLibrarySliceState,
  deriveScanSliceState,
  deriveStorageSliceState,
  type SliceStateKind,
} from "./slice-state"

describe("strict six-state slice mapping", () => {
  it.each([
    [{ loading: true, itemCount: 0 }, "loading"],
    [{ loading: false, itemCount: 0 }, "empty"],
    [{ loading: false, itemCount: 2 }, "data"],
    [{ loading: false, itemCount: 2, partial: true }, "offline_partial"],
    [{ loading: true, itemCount: 2 }, "offline_partial"],
  ] as const)("maps library state %# to %s", (input, expected: SliceStateKind) => {
    expect(deriveLibrarySliceState(input).kind).toBe(expected)
  })

  it("distinguishes retryable and terminal library errors", () => {
    const retryable = new HavenError({
      code: "DATABASE_ERROR",
      userMessage: "数据库暂时不可用",
      retryable: true,
    })
    const terminal = new HavenError({
      code: "INVALID_ARGUMENT",
      userMessage: "请求无效",
      retryable: false,
    })
    expect(deriveLibrarySliceState({ loading: false, itemCount: 0, error: retryable }).kind)
      .toBe("retryable_error")
    expect(deriveLibrarySliceState({ loading: false, itemCount: 0, error: terminal }).kind)
      .toBe("terminal_error")
    expect(deriveLibrarySliceState({ loading: false, itemCount: 2, error: retryable }).kind)
      .toBe("offline_partial")
  })

  it("maps missing storage and scan warnings to offline_partial", () => {
    expect(deriveStorageSliceState("missing").kind).toBe("offline_partial")
    expect(deriveStorageSliceState("auth_expired").kind).toBe("offline_partial")
    expect(deriveStorageSliceState("connected").kind).toBe("data")
    expect(deriveStorageSliceState("unknown_status").kind).toBe("terminal_error")
    expect(deriveScanSliceState("warning").kind).toBe("offline_partial")
    expect(deriveScanSliceState("failed").kind).toBe("terminal_error")
    expect(deriveScanSliceState("completed").kind).toBe("data")
    expect(deriveScanSliceState("future_phase").kind).toBe("terminal_error")
  })
})
