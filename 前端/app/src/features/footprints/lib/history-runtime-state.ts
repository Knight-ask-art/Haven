import type { HavenClientMode } from "@/lib/ipc/runtime"

export type HistoryRuntimeState = "demo" | "production" | "unavailable"

export function resolveHistoryRuntimeState(mode: HavenClientMode): HistoryRuntimeState {
  if (mode === "mock") return "demo"
  if (mode === "tauri") return "production"
  return "unavailable"
}

export function loadDemoHistoryValue<T>(
  mode: HavenClientMode,
  loadValue: () => T,
  fallback: T,
): T {
  return mode === "mock" ? loadValue() : fallback
}

export function shouldApplyHistoryRequest(currentRequestId: number, requestId: number): boolean {
  return currentRequestId === requestId
}
