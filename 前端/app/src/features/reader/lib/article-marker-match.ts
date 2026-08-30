import type { MarkerDto } from "@/lib/ipc/generated/wire"

const ARTICLE_PROGRESSION_TOLERANCE = 0.01

export function findArticleBookmark(
  markers: readonly MarkerDto[],
  mediaItemId: string,
  blockId: string | null,
  progression: number,
): MarkerDto | null {
  return markers.find((marker) => {
    if (marker.mediaItemId !== mediaItemId || marker.markerType !== "bookmark" || marker.locator.kind !== "article") return false
    const markerBlockId = marker.locator.data.blockId
    if (markerBlockId !== null) return markerBlockId === blockId
    const markerProgression = marker.locator.data.progression
    return markerProgression !== null
      && Math.abs(markerProgression - progression) < ARTICLE_PROGRESSION_TOLERANCE
  }) ?? null
}
