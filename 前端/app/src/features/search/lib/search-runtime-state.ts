import type { HavenClientMode } from "@/lib/ipc/runtime"

export type SearchRuntimeState =
  | "demo"
  | "ready_empty"
  | "ready_query"
  | "unavailable_empty"
  | "unavailable_query"

export function resolveSearchRuntimeState(mode: HavenClientMode, query: string): SearchRuntimeState {
  if (mode === "mock") return "demo"
  if (mode === "tauri") return query.trim() ? "ready_query" : "ready_empty"
  return query.trim() ? "unavailable_query" : "unavailable_empty"
}

export function loadDemoSearchHistory(mode: HavenClientMode, readHistory: () => string[]): string[] {
  return mode === "mock" ? readHistory() : []
}
