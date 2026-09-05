import { describe, expect, it, vi } from "vitest"
import type { HavenClient } from "@/lib/ipc/client"
import { HavenError } from "@/lib/ipc/errors"
import type {
  ComicChapterCatalogDto,
  ComicRegisteredChapterCatalogDto,
} from "@/lib/ipc/generated/wire"
import {
  getComicChapterCatalog,
  getRegisteredComicChapterCatalog,
  refreshComicChapterCatalog,
} from "./comic-chapter-catalog-gateway"

const request = {
  sourceId: "mangadex",
  remoteWorkId: "aaaaaaaa-aaaa-4aaa-8000-aaaaaaaaaaaa",
} as const

const catalog: ComicChapterCatalogDto = {
  schemaVersion: 1,
  sourceId: request.sourceId,
  remoteWorkId: request.remoteWorkId,
  fetchedAt: "2026-09-04T00:00:00Z",
  total: 0,
  truncated: false,
  chapters: [],
}

const registeredCatalog: ComicRegisteredChapterCatalogDto = {
  schemaVersion: 1,
  sourceId: request.sourceId,
  remoteWorkId: request.remoteWorkId,
  refreshState: {
    generation: 1,
    fetchedAt: "2026-09-04T00:00:00Z",
    total: 0,
    truncated: false,
  },
  chapters: [],
}

function clientReturning(value: unknown): HavenClient {
  return {
    comicChapterCatalogGet: vi.fn().mockResolvedValue(value),
    comicChapterCatalogRegisteredGet: vi.fn().mockResolvedValue(value),
    comicChapterCatalogRefresh: vi.fn().mockResolvedValue(value),
  } as unknown as HavenClient
}

describe("getComicChapterCatalog", () => {
  it("requests the source/work identity through HavenClient", async () => {
    const comicChapterCatalogGet = vi.fn<HavenClient["comicChapterCatalogGet"]>().mockResolvedValue(catalog)
    const client = { comicChapterCatalogGet } as unknown as HavenClient

    await expect(getComicChapterCatalog(request, client)).resolves.toBe(catalog)
    expect(comicChapterCatalogGet).toHaveBeenCalledWith(request)
  })

  it("rejects invalid input before invoking the client", async () => {
    const comicChapterCatalogGet = vi.fn<HavenClient["comicChapterCatalogGet"]>()
    await expect(getComicChapterCatalog(
      { sourceId: "other", remoteWorkId: "not-a-uuid" },
      { comicChapterCatalogGet } as unknown as HavenClient,
    )).rejects.toHaveProperty("code", "INVALID_ARGUMENT")
    expect(comicChapterCatalogGet).not.toHaveBeenCalled()
  })

  it("rejects an invalid response without crossing internal fields", async () => {
    await expect(getComicChapterCatalog(request, clientReturning({
      ...catalog,
      chapters: [{ remoteChapterId: "not-a-uuid" }],
    }))).rejects.toMatchObject({ code: "COMIC_CATALOG_INVALID_RESPONSE", retryable: false })
  })

  it("preserves a typed client error", async () => {
    const expected = new HavenError({
      code: "SOURCE_UNAVAILABLE",
      userMessage: "来源暂时不可用",
      retryable: true,
    })
    const client = { comicChapterCatalogGet: vi.fn().mockRejectedValue(expected) } as unknown as HavenClient
    await expect(getComicChapterCatalog(request, client)).rejects.toBe(expected)
  })
})

describe("refreshComicChapterCatalog", () => {
  it("uses the explicit refresh command while keeping the same safe projection", async () => {
    const comicChapterCatalogRefresh = vi.fn<HavenClient["comicChapterCatalogRefresh"]>()
      .mockResolvedValue(catalog)
    const client = { comicChapterCatalogRefresh } as unknown as HavenClient

    await expect(refreshComicChapterCatalog(request, client)).resolves.toBe(catalog)
    expect(comicChapterCatalogRefresh).toHaveBeenCalledWith(request)
  })

  it("rejects invalid input before invoking the refresh command", async () => {
    const comicChapterCatalogRefresh = vi.fn<HavenClient["comicChapterCatalogRefresh"]>()
    await expect(refreshComicChapterCatalog(
      { sourceId: "other", remoteWorkId: "not-a-uuid" },
      { comicChapterCatalogRefresh } as unknown as HavenClient,
    )).rejects.toHaveProperty("code", "INVALID_ARGUMENT")
    expect(comicChapterCatalogRefresh).not.toHaveBeenCalled()
  })
})

describe("getRegisteredComicChapterCatalog", () => {
  it("requests the persisted catalog through the dedicated client method", async () => {
    const comicChapterCatalogRegisteredGet = vi.fn<HavenClient["comicChapterCatalogRegisteredGet"]>()
      .mockResolvedValue(registeredCatalog)
    const client = { comicChapterCatalogRegisteredGet } as unknown as HavenClient

    await expect(getRegisteredComicChapterCatalog(request, client)).resolves.toBe(registeredCatalog)
    expect(comicChapterCatalogRegisteredGet).toHaveBeenCalledWith(request)
  })

  it("rejects an invalid registered projection without exposing internal fields", async () => {
    await expect(getRegisteredComicChapterCatalog(request, clientReturning({
      ...registeredCatalog,
      chapters: [{ mediaItemId: "not-a-uuid" }],
    }))).rejects.toMatchObject({
      code: "COMIC_REGISTERED_CATALOG_INVALID_RESPONSE",
      retryable: false,
    })
  })

  it("rejects invalid input before invoking the registered query", async () => {
    const comicChapterCatalogRegisteredGet = vi.fn<HavenClient["comicChapterCatalogRegisteredGet"]>()
    await expect(getRegisteredComicChapterCatalog(
      { sourceId: "other", remoteWorkId: "not-a-uuid" },
      { comicChapterCatalogRegisteredGet } as unknown as HavenClient,
    )).rejects.toHaveProperty("code", "INVALID_ARGUMENT")
    expect(comicChapterCatalogRegisteredGet).not.toHaveBeenCalled()
  })
})
