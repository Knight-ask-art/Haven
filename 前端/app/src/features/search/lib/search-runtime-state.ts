import type { HavenClientMode } from "@/lib/ipc/runtime"

export type SearchRuntimeState =
  | "ready_empty"
  | "ready_query"
  | "unavailable_empty"
  | "unavailable_query"

export function resolveSearchRuntimeState(mode: HavenClientMode, query: string): SearchRuntimeState {
  // Mock 与 Tauri 共用结果呈现和 Gateway；客户端身份仍由页面明确标注。
  if (mode === "mock" || mode === "tauri") return query.trim() ? "ready_query" : "ready_empty"
  return query.trim() ? "unavailable_query" : "unavailable_empty"
}

export function loadDemoSearchHistory(mode: HavenClientMode, readHistory: () => string[]): string[] {
  return mode === "mock" ? readHistory() : []
}
