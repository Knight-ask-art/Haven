import { useMemo } from "react"
import { useLocation } from "react-router"
import type { LocatorDto } from "@/lib/ipc/generated/wire"
import { isLocator } from "@/features/progress/ipc/progress-gateway"
import type { InitialPosition } from "@/features/progress/lib/restore-position"

export interface MarkerNavigationState {
  markerNavigation: { markerId: string; mediaItemId: string; locator: LocatorDto }
}

/** 只消费匹配当前媒体的显式导航意图，不写 Progress 或另建持久化存储。 */
export function useMarkerNavigation(mediaItemId: string | undefined): InitialPosition | undefined {
  const { state, key } = useLocation()
  return useMemo(() => {
    if (!state || typeof state !== "object" || !("markerNavigation" in state)) return undefined
    const marker: unknown = state.markerNavigation
    if (!marker || typeof marker !== "object") return undefined
    const value = marker as Record<string, unknown>
    if (typeof value.markerId !== "string" || !value.markerId
      || !mediaItemId || value.mediaItemId !== mediaItemId || !isLocator(value.locator)) return undefined
    if (value.locator.kind === "comic" && value.locator.data.chapterItemId !== mediaItemId) return undefined
    return { key: `${key}:${value.markerId}`, mediaItemId, locator: value.locator }
  }, [key, mediaItemId, state])
}
