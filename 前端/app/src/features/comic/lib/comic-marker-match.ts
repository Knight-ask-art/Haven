import type { MarkerDto } from "@/lib/ipc/generated/wire"

export function findComicBookmark(
  markers: readonly MarkerDto[],
  mediaItemId: string,
  pageIndex: number,
): MarkerDto | null {
  return markers.find((marker) => (
    marker.mediaItemId === mediaItemId
    && marker.markerType === "bookmark"
    && marker.locator.kind === "comic"
    && marker.locator.data.chapterItemId === mediaItemId
    && marker.locator.data.pageIndex === pageIndex
  )) ?? null
}
