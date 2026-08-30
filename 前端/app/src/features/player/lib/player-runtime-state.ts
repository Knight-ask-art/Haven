import type { HavenClientMode } from "@/lib/ipc/runtime"

export type PlayerRuntimeState = "demo" | "production" | "unavailable"

export function resolvePlayerRuntimeState(mode: HavenClientMode): PlayerRuntimeState {
  if (mode === "mock") return "demo"
  if (mode === "tauri") return "production"
  return "unavailable"
}

export function loadDemoPlayerData<T>(mode: HavenClientMode, loadData: () => T): T | null {
  return mode === "mock" ? loadData() : null
}

export function recordDemoPlayerHistory(
  mode: HavenClientMode,
  mediaItemId: string,
  recordHistory: (mediaItemId: string) => void,
): void {
  if (mode === "mock") recordHistory(mediaItemId)
}
