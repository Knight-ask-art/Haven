import { describe, expect, it, vi } from "vitest"
import type { HavenClient } from "@/lib/ipc/client"
import type { MarkerCreateRequest, MarkerDto } from "@/lib/ipc/generated/wire"
import { HavenError } from "@/lib/ipc/errors"
import {
  articleMarkerLocator,
  bookMarkerLocator,
  comicMarkerLocator,
  createMarker,
  videoMarkerLocator,
} from "./marker-gateway"

const MEDIA_ID = "0196f0d2-0000-7000-8000-000000000001"

function markerDto(overrides: Partial<MarkerDto> = {}): MarkerDto {
  return {
    markerId: "0196f0d2-0000-7000-8000-0000000000aa",
    mediaItemId: MEDIA_ID,
    workId: "work-1",
    editionId: "edition-1",
    locator: bookMarkerLocator(MEDIA_ID, 0.5),
    markerType: "bookmark",
    title: null,
    excerpt: null,
    note: null,
    createdAt: "2026-08-18T00:00:00.000Z",
    updatedAt: "2026-08-18T00:00:00.000Z",
    ...overrides,
  }
}

function clientWith(markerCreate: HavenClient["markerCreate"]): HavenClient {
  return { markerCreate } as unknown as HavenClient
}

function request(overrides: Partial<MarkerCreateRequest> = {}): MarkerCreateRequest {
  return {
    mediaItemId: MEDIA_ID,
    locator: bookMarkerLocator(MEDIA_ID, 0.5),
    markerType: "bookmark",
    title: null,
    excerpt: null,
    note: null,
    ...overrides,
  }
}

describe("marker-gateway createMarker", () => {
  it("passes a well-formed request through and returns the DTO", async () => {
    const markerCreate = vi.fn(async () => markerDto())
    const result = await createMarker(request(), clientWith(markerCreate))
    expect(markerCreate).toHaveBeenCalledTimes(1)
    expect(result.markerId).toBe("0196f0d2-0000-7000-8000-0000000000aa")
  })

  it("rejects a non-canonical mediaItemId without calling IPC", async () => {
    const markerCreate = vi.fn(async () => markerDto())
    await expect(createMarker(request({ mediaItemId: "not-a-uuid" }), clientWith(markerCreate)))
      .rejects.toMatchObject({ code: "INVALID_ARGUMENT" })
    expect(markerCreate).not.toHaveBeenCalled()
  })

  it("rejects a comic locator whose chapterItemId does not match mediaItemId", async () => {
    const markerCreate = vi.fn(async () => markerDto())
    const mismatched = request({
      locator: comicMarkerLocator("0196f0d2-0000-7000-8000-000000000009", 3),
    })
    await expect(createMarker(mismatched, clientWith(markerCreate)))
      .rejects.toMatchObject({ code: "INVALID_ARGUMENT" })
    expect(markerCreate).not.toHaveBeenCalled()
  })

  it("rejects an unknown markerType without calling IPC", async () => {
    const markerCreate = vi.fn(async () => markerDto())
    await expect(createMarker(request({ markerType: "sticker" as never }), clientWith(markerCreate)))
      .rejects.toMatchObject({ code: "INVALID_ARGUMENT" })
    expect(markerCreate).not.toHaveBeenCalled()
  })

  it("fails closed when the wire response is malformed", async () => {
    const markerCreate = vi.fn(async () => ({ markerId: "" }) as unknown as MarkerDto)
    await expect(createMarker(request(), clientWith(markerCreate)))
      .rejects.toMatchObject({ code: "MARKER_INVALID_RESPONSE" })
  })

  it("normalizes a backend error through toHavenError", async () => {
    const markerCreate = vi.fn(async (): Promise<MarkerDto> => {
      throw new HavenError({ code: "LOCATOR_KIND_INCOMPATIBLE", userMessage: "不兼容", retryable: false })
    })
    await expect(createMarker(request(), clientWith(markerCreate)))
      .rejects.toMatchObject({ code: "LOCATOR_KIND_INCOMPATIBLE" })
  })
})

describe("marker-gateway locator builders", () => {
  it("builds each media kind with its contract shape", () => {
    expect(bookMarkerLocator(MEDIA_ID, 0.25)).toMatchObject({ version: 1, kind: "book" })
    expect(comicMarkerLocator(MEDIA_ID, 4)).toMatchObject({
      version: 1,
      kind: "comic",
      data: { chapterItemId: MEDIA_ID, pageIndex: 4, pageProgression: null },
    })
    expect(articleMarkerLocator("h2-intro", 0.4)).toMatchObject({
      version: 1,
      kind: "article",
      data: { blockId: "h2-intro", progression: 0.4 },
    })
    expect(videoMarkerLocator(1234.6)).toMatchObject({
      version: 1,
      kind: "video",
      data: { positionMs: 1235 },
    })
  })

  it("clamps a negative video position to zero", () => {
    expect(videoMarkerLocator(-50)).toMatchObject({ data: { positionMs: 0 } })
  })
})
