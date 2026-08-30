import type { HavenError } from "./ipc/errors"

/** The only six user-visible data states shared by the first three slices. */
export type SliceStateKind =
  | "loading"
  | "empty"
  | "data"
  | "offline_partial"
  | "retryable_error"
  | "terminal_error"

export interface SliceState {
  kind: SliceStateKind
  message: string | null
  canRetry: boolean
}

interface LibraryStateInput {
  loading: boolean
  itemCount: number
  partial?: boolean
  error?: HavenError | null
}

export function deriveLibrarySliceState(input: LibraryStateInput): SliceState {
  if (input.loading && input.itemCount === 0) return { kind: "loading", message: null, canRetry: false }
  if (input.partial || (input.itemCount > 0 && (input.loading || input.error))) {
    return {
      kind: "offline_partial",
      message: input.error?.message ?? (input.loading ? "正在刷新可用内容" : "部分内容暂时不可用"),
      canRetry: input.error?.retryable ?? true,
    }
  }
  if (input.error) {
    return {
      kind: input.error.retryable ? "retryable_error" : "terminal_error",
      message: input.error.message,
      canRetry: input.error.retryable,
    }
  }
  if (input.itemCount === 0) return { kind: "empty", message: null, canRetry: false }
  return { kind: "data", message: null, canRetry: false }
}

export function deriveStorageSliceState(status: string): SliceState {
  const normalized = status.toLowerCase()
  if (["missing", "disconnected", "offline", "unavailable", "auth_expired"].includes(normalized)) {
    return {
      kind: "offline_partial",
      message: "存储位置暂时不可用",
      canRetry: true,
    }
  }
  if (["connected", "read_only"].includes(normalized)) {
    return { kind: "data", message: null, canRetry: false }
  }
  if (normalized === "error") {
    return { kind: "retryable_error", message: "存储位置读取失败", canRetry: true }
  }
  if (normalized === "disabled") {
    return { kind: "terminal_error", message: "存储位置已停用", canRetry: false }
  }
  return { kind: "terminal_error", message: "未知的存储位置状态", canRetry: false }
}

export function deriveScanSliceState(phase: string): SliceState {
  switch (phase.toLowerCase()) {
    case "warning":
      return { kind: "offline_partial", message: "部分条目未能索引", canRetry: true }
    case "failed":
      return { kind: "terminal_error", message: "扫描失败", canRetry: false }
    case "started":
    case "enumerating":
    case "detecting":
    case "fingerprinting":
    case "indexing":
    case "item_indexed":
      return { kind: "loading", message: null, canRetry: false }
    case "completed":
    case "cancelled":
      return { kind: "data", message: null, canRetry: false }
    default:
      return { kind: "terminal_error", message: "未知的扫描阶段", canRetry: false }
  }
}
