import type { HavenClientMode } from "@/lib/ipc/runtime"

export type FootprintsRuntimeState = "demo" | "production" | "unavailable"

export function resolveFootprintsRuntimeState(mode: HavenClientMode): FootprintsRuntimeState {
  if (mode === "mock") return "demo"
  if (mode === "tauri") return "production"
  return "unavailable"
}

export function canLoadFootprintsData(mode: HavenClientMode): boolean {
  return mode !== "unavailable"
}

export function resolveDemoFootprintsEmptyState(
  mode: HavenClientMode,
  requestedState: string | null,
): boolean {
  return mode === "mock" && requestedState === "empty"
}

export function loadDemoFootprintMarkers<T>(mode: HavenClientMode, loadMarkers: () => T[]): T[] {
  return mode === "mock" ? loadMarkers() : []
}
