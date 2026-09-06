import type { HavenClientMode } from "@/lib/ipc/runtime"

export type LibraryRuntimeState = "demo" | "production" | "unavailable"

export function resolveLibraryRuntimeState(mode: HavenClientMode): LibraryRuntimeState {
  if (mode === "mock") return "demo"
  if (mode === "tauri") return "production"
  return "unavailable"
}

export function canLoadLibraryNextPage(input: {
  nextCursor: string | null
  isLoadingMore: boolean
  isFirstPagePending: boolean
  cursorQueryKey: string | null
  activeQueryKey: string
  partialError: boolean
}): boolean {
  return Boolean(input.nextCursor)
    && !input.isLoadingMore
    && !input.isFirstPagePending
    && input.cursorQueryKey === input.activeQueryKey
    && !input.partialError
}
