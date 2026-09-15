import type { HavenClientMode } from "@/lib/ipc/runtime"

export type BookReaderRuntimeState = "demo" | "production" | "unavailable"
export type DemoBookReaderKind = "book" | "periodical" | "document"

export function resolveBookReaderRuntimeState(mode: HavenClientMode): BookReaderRuntimeState {
  if (mode === "mock") return "demo"
  if (mode === "tauri") return "production"
  return "unavailable"
}

/**
 * Selects only browser-demo content.  A production mediaItemId is never
 * interpreted as a fixture key, even if a test or imported record happens to
 * look like `p1-1` or `d1-1`.
 */
export function selectDemoBookReaderKind(
  mode: HavenClientMode,
  storageId: string,
): DemoBookReaderKind | null {
  if (mode !== "mock") return null
  if (storageId.startsWith("p")) return "periodical"
  if (storageId.startsWith("d")) return "document"
  return "book"
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
