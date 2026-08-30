import type { HavenClientMode } from "@/lib/ipc/runtime"

export type LibraryRuntimeState = "demo" | "production" | "unavailable"

export function resolveLibraryRuntimeState(mode: HavenClientMode): LibraryRuntimeState {
  if (mode === "mock") return "demo"
  if (mode === "tauri") return "production"
  return "unavailable"
}
