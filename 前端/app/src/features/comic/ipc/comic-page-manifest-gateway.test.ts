import { describe, expect, it, vi } from "vitest"
import type { HavenClient } from "@/lib/ipc/client"
import { HavenError } from "@/lib/ipc/errors"
import type { ComicPageManifestDto, SessionOpenResultDto } from "@/lib/ipc/generated/wire"
import { getComicPageManifest } from "./comic-page-manifest-gateway"

const session: SessionOpenResultDto = {
  schemaVersion: 1,
  sessionId: "0196f0d2-0000-7000-8000-000000000001",
  contentUri: null,
  workId: "work-1",
  editionId: "edition-1",
  mediaItemId: "0196f0d2-0000-7000-8000-000000000002",
  engine: "comic",
  progress: null,
}
const manifest: ComicPageManifestDto = {
  schemaVersion: 1,
  sessionId: session.sessionId,
  mediaItemId: session.mediaItemId,
  pageCount: 0,
  pages: [],
}

function clientReturning(value: unknown): HavenClient {
  return {
    comicPageManifestGet: vi.fn().mockResolvedValue(value),
  } as unknown as HavenClient
}

describe("getComicPageManifest", () => {
  it("requests by session only and validates the returned identities", async () => {
    const comicPageManifestGet = vi.fn<HavenClient["comicPageManifestGet"]>()
      .mockResolvedValue(manifest)
    const client = { comicPageManifestGet } as unknown as HavenClient

    await expect(getComicPageManifest(session, client)).resolves.toBe(manifest)
    expect(comicPageManifestGet).toHaveBeenCalledWith({ sessionId: session.sessionId })
  })

  it.each([
    { ...manifest, sessionId: "0196f0d2-0000-7000-8000-000000000099" },
    { ...manifest, mediaItemId: "other" },
    { ...manifest, schemaVersion: 2 },
    { ...manifest, unexpected: true },
  ])("rejects an invalid manifest response %#", async (value) => {
    await expect(getComicPageManifest(session, clientReturning(value)))
      .rejects.toMatchObject({ code: "COMIC_MANIFEST_INVALID_RESPONSE", retryable: false })
  })

  it.each([
    { ...session, engine: "reader" as const },
    { ...session, contentUri: "haven-resource://session/0196f0d2-0000-7000-8000-000000000001" },
    { ...session, sessionId: "not-a-uuid" },
  ])("rejects an invalid comic session before invoking the client %#", async (value) => {
    const comicPageManifestGet = vi.fn<HavenClient["comicPageManifestGet"]>()
    await expect(getComicPageManifest(value, { comicPageManifestGet } as unknown as HavenClient))
      .rejects.toHaveProperty("code", "INVALID_ARGUMENT")
    expect(comicPageManifestGet).not.toHaveBeenCalled()
  })

  it("preserves a typed client error", async () => {
    const expected = new HavenError({
      code: "SESSION_STALE",
      userMessage: "会话失效",
      retryable: false,
    })
    const client = {
      comicPageManifestGet: vi.fn().mockRejectedValue(expected),
    } as unknown as HavenClient
    await expect(getComicPageManifest(session, client)).rejects.toBe(expected)
  })
})
