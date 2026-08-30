import type { HavenClientMode } from "@/lib/ipc/runtime"

export type MediaDetailRuntimeState = "demo" | "production" | "unavailable"

export function resolveMediaDetailRuntimeState(mode: HavenClientMode): MediaDetailRuntimeState {
  if (mode === "mock") return "demo"
  if (mode === "tauri") return "production"
  return "unavailable"
}
