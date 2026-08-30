import type { HavenClientMode } from "@/lib/ipc/runtime"

export type BookReaderRuntimeState = "demo" | "production" | "unavailable"

export function resolveBookReaderRuntimeState(mode: HavenClientMode): BookReaderRuntimeState {
  if (mode === "mock") return "demo"
  if (mode === "tauri") return "production"
  return "unavailable"
}

export function loadDemoBookReaderBookmarks<T>(
  mode: HavenClientMode,
  loadBookmarks: () => T[],
): T[] {
  return mode === "mock" ? loadBookmarks() : []
}

export function recordDemoBookReaderHistory(
  mode: HavenClientMode,
  mediaItemId: string,
  recordHistory: (mediaItemId: string) => void,
): void {
  if (mode === "mock") recordHistory(mediaItemId)
}
