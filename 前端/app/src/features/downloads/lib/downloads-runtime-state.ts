import type { HavenClientMode } from "@/lib/ipc/runtime"

export type DownloadsRuntimeState = "ready" | "unavailable"

export function resolveDownloadsRuntimeState(mode: HavenClientMode): DownloadsRuntimeState {
  return mode === "unavailable" ? "unavailable" : "ready"
}
