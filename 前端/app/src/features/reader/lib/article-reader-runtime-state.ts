import type { HavenClientMode } from "@/lib/ipc/runtime"

export type ArticleReaderRuntimeState = "demo" | "production" | "unavailable"

export function resolveArticleReaderRuntimeState(mode: HavenClientMode): ArticleReaderRuntimeState {
  if (mode === "mock") return "demo"
  if (mode === "tauri") return "production"
  return "unavailable"
}

export function loadDemoArticleReaderValue<T>(
  mode: HavenClientMode,
  loadValue: () => T,
  fallback: T,
): T {
  return mode === "mock" ? loadValue() : fallback
}

export function recordDemoArticleReaderHistory(
  mode: HavenClientMode,
  mediaItemId: string,
  recordHistory: (mediaItemId: string) => void,
): void {
  if (mode === "mock") recordHistory(mediaItemId)
}

export function canUseDemoArticleReaderTools(mode: HavenClientMode): boolean {
  return mode === "mock"
}
