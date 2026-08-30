import type { HavenClientMode } from "@/lib/ipc/runtime"

export type ComicReaderRuntimeState = "demo" | "production" | "unavailable"

export function resolveComicReaderRuntimeState(mode: HavenClientMode): ComicReaderRuntimeState {
  if (mode === "mock") return "demo"
  if (mode === "tauri") return "production"
  return "unavailable"
}
