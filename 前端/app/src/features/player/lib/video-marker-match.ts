import type { MarkerDto } from "@/lib/ipc/generated/wire"
import { videoSecondsToMilliseconds } from "./video-marker-position"

const VIDEO_BOOKMARK_TOLERANCE_MS = 1000

export function findVideoBookmark(
  markers: readonly MarkerDto[],
  mediaItemId: string,
  positionSeconds: number,
): MarkerDto | null {
  const positionMs = videoSecondsToMilliseconds(positionSeconds)
  let closest: MarkerDto | null = null
  let closestDistance = Number.POSITIVE_INFINITY

  for (const marker of markers) {
    if (marker.mediaItemId !== mediaItemId || marker.markerType !== "bookmark" || marker.locator.kind !== "video") continue
    const distance = Math.abs(marker.locator.data.positionMs - positionMs)
    if (distance <= VIDEO_BOOKMARK_TOLERANCE_MS && distance < closestDistance) {
      closest = marker
      closestDistance = distance
    }
  }
  return closest
}
