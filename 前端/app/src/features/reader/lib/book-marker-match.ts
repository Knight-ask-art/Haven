import type { MarkerDto } from "@/lib/ipc/generated/wire"

const BOOKMARK_PROGRESSION_TOLERANCE = 0.01

export function findBookBookmark(
  markers: readonly MarkerDto[],
  mediaItemId: string,
  progression: number,
): MarkerDto | null {
  const publicationResource = `haven-resource://text/${mediaItemId}`
  let closest: MarkerDto | null = null
  let closestDistance = Number.POSITIVE_INFINITY

  for (const marker of markers) {
    if (marker.mediaItemId !== mediaItemId || marker.markerType !== "bookmark" || marker.locator.kind !== "book") continue
    if (marker.locator.data.publicationResource !== publicationResource || marker.locator.data.progression === null) continue
    const distance = Math.abs(marker.locator.data.progression - progression)
    if (distance < BOOKMARK_PROGRESSION_TOLERANCE && distance < closestDistance) {
      closest = marker
      closestDistance = distance
    }
  }
  return closest
}
