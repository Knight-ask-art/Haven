import { describe, expect, it } from "vitest"
import { videoMarkerLocator } from "@/features/markers/ipc/marker-gateway"
import type { MarkerDto } from "@/lib/ipc/generated/wire"
import { findVideoBookmark } from "./video-marker-match"

const MEDIA_ID = "0196f0d2-0000-7000-8000-000000000001"

function videoMarker(markerId: string, positionMs: number, markerType: MarkerDto["markerType"] = "bookmark"): MarkerDto {
  return {
    markerId,
    mediaItemId: MEDIA_ID,
    workId: "work-1",
    editionId: "edition-1",
    locator: videoMarkerLocator(positionMs),
    markerType,
    title: null,
    excerpt: null,
    note: null,
    createdAt: "2026-08-19T00:00:00.000Z",
    updatedAt: "2026-08-19T00:00:00.000Z",
  }
}

describe("findVideoBookmark", () => {
  it("returns the closest bookmark within the current-position tolerance", () => {
    const markers = [videoMarker("farther", 11_000), videoMarker("closer", 10_400)]

    expect(findVideoBookmark(markers, MEDIA_ID, 10)?.markerId).toBe("closer")
  })

  it("includes 1000 ms and excludes positions beyond the tolerance", () => {
    expect(findVideoBookmark([videoMarker("boundary", 11_000)], MEDIA_ID, 10)?.markerId).toBe("boundary")
    expect(findVideoBookmark([videoMarker("outside", 11_001)], MEDIA_ID, 10)).toBeNull()
  })

  it("ignores non-bookmark markers", () => {
    expect(findVideoBookmark([videoMarker("note", 10_000, "note")], MEDIA_ID, 10)).toBeNull()
  })
})
