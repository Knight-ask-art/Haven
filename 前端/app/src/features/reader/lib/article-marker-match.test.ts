import { describe, expect, it } from "vitest"
import { articleMarkerLocator } from "@/features/markers/ipc/marker-gateway"
import type { MarkerDto } from "@/lib/ipc/generated/wire"
import { findArticleBookmark } from "./article-marker-match"

const MEDIA_ID = "0196f0d2-0000-7000-8000-000000000001"

function articleMarker(
  markerId: string,
  blockId: string | null,
  progression: number,
  markerType: MarkerDto["markerType"] = "bookmark",
): MarkerDto {
  return {
    markerId,
    mediaItemId: MEDIA_ID,
    workId: "work-1",
    editionId: "edition-1",
    locator: articleMarkerLocator(blockId, progression),
    markerType,
    title: null,
    excerpt: null,
    note: null,
    createdAt: "2026-08-19T00:00:00.000Z",
    updatedAt: "2026-08-19T00:00:00.000Z",
  }
}

describe("findArticleBookmark", () => {
  it("matches an exact article block bookmark", () => {
    expect(findArticleBookmark([articleMarker("section", "section-2", 0.2)], MEDIA_ID, "section-2", 0.8)?.markerId)
      .toBe("section")
  })

  it("matches a blockless bookmark by progression", () => {
    expect(findArticleBookmark([articleMarker("progress", null, 0.505)], MEDIA_ID, null, 0.5)?.markerId)
      .toBe("progress")
  })

  it("matches a blockless bookmark by progression when the current block is known", () => {
    expect(findArticleBookmark([articleMarker("progress-known-block", null, 0.505)], MEDIA_ID, "section-2", 0.5)?.markerId)
      .toBe("progress-known-block")
  })

  it("rejects a different block, distant progression, or non-bookmark marker", () => {
    const markers = [
      articleMarker("other-block", "section-3", 0.5),
      articleMarker("outside", null, 0.52),
      articleMarker("note", "section-2", 0.5, "note"),
    ]

    expect(findArticleBookmark(markers, MEDIA_ID, "section-2", 0.5)).toBeNull()
  })
})
