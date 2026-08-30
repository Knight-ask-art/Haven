import { describe, expect, it } from "vitest"
import { comicMarkerLocator } from "@/features/markers/ipc/marker-gateway"
import type { MarkerDto } from "@/lib/ipc/generated/wire"
import { findComicBookmark } from "./comic-marker-match"

const MEDIA_ID = "0196f0d2-0000-7000-8000-000000000001"

function comicMarker(markerId: string, pageIndex: number, markerType: MarkerDto["markerType"] = "bookmark"): MarkerDto {
  return {
    markerId,
    mediaItemId: MEDIA_ID,
    workId: "work-1",
    editionId: "edition-1",
    locator: comicMarkerLocator(MEDIA_ID, pageIndex),
    markerType,
    title: null,
    excerpt: null,
    note: null,
    createdAt: "2026-08-19T00:00:00.000Z",
    updatedAt: "2026-08-19T00:00:00.000Z",
  }
}

describe("findComicBookmark", () => {
  it("matches a bookmark by chapter media item and zero-based page index", () => {
    expect(findComicBookmark([comicMarker("page-2", 2)], MEDIA_ID, 2)?.markerId).toBe("page-2")
  })

  it("rejects a different page or marker type", () => {
    const markers = [comicMarker("other-page", 3), comicMarker("page-note", 2, "note")]

    expect(findComicBookmark(markers, MEDIA_ID, 2)).toBeNull()
  })
})
