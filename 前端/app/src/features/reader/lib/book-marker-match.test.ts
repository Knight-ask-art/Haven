import { describe, expect, it } from "vitest"
import { bookMarkerLocator } from "@/features/markers/ipc/marker-gateway"
import type { MarkerDto } from "@/lib/ipc/generated/wire"
import { findBookBookmark } from "./book-marker-match"

const MEDIA_ID = "0196f0d2-0000-7000-8000-000000000001"

function bookMarker(markerId: string, progression: number, markerType: MarkerDto["markerType"] = "bookmark"): MarkerDto {
  return {
    markerId,
    mediaItemId: MEDIA_ID,
    workId: "work-1",
    editionId: "edition-1",
    locator: bookMarkerLocator(MEDIA_ID, progression),
    markerType,
    title: null,
    excerpt: null,
    note: null,
    createdAt: "2026-08-19T00:00:00.000Z",
    updatedAt: "2026-08-19T00:00:00.000Z",
  }
}

describe("findBookBookmark", () => {
  it("matches the closest bookmark for the publication resource and progression", () => {
    const markers = [bookMarker("farther", 0.508), bookMarker("closer", 0.503)]

    expect(findBookBookmark(markers, MEDIA_ID, 0.5)?.markerId).toBe("closer")
  })

  it("rejects progression outside the tolerance or a non-bookmark marker", () => {
    const markers = [bookMarker("outside", 0.52), bookMarker("note", 0.5, "note")]

    expect(findBookBookmark(markers, MEDIA_ID, 0.5)).toBeNull()
  })
})
