import { describe, expect, it, vi } from "vitest"
import type { HavenClient } from "@/lib/ipc/client"
import { HavenError } from "@/lib/ipc/errors"
import type { ComicWorkChapterCatalogDto } from "@/lib/ipc/generated/wire"
import {
  getComicWorkChapterCatalog,
  refreshComicWorkChapterCatalog,
  type ComicWorkCatalogTarget,
} from "./comic-work-chapter-catalog-gateway"
import fixture from "../../../../../../contracts/ipc/v1/fixtures/comic/work-chapter-catalog.normal.json"

const WORK_ID = "11111111-1111-4111-8111-111111111111"
const MEDIA_ITEM_ID = "22222222-2222-4222-8222-222222222201"

const catalog = fixture as unknown as ComicWorkChapterCatalogDto

function clientReturning(value: unknown): Pick<HavenClient, "comicWorkChapterCatalogGet"> {
  return { comicWorkChapterCatalogGet: vi.fn().mockResolvedValue(value) }
}

describe("getComicWorkChapterCatalog", () => {
  it("sends only the local work identity and returns the validated catalog", async () => {
    const comicWorkChapterCatalogGet = vi.fn().mockResolvedValue(catalog)
    const client = { comicWorkChapterCatalogGet } as unknown as HavenClient

    await expect(getComicWorkChapterCatalog({ workId: WORK_ID, mediaItemId: null }, client))
      .resolves.toBe(catalog)
    expect(comicWorkChapterCatalogGet).toHaveBeenCalledWith({ workId: WORK_ID, mediaItemId: null })
  })

  it("requests the current session media item for the reader path", async () => {
    const comicWorkChapterCatalogGet = vi.fn().mockResolvedValue(catalog)
    const client = { comicWorkChapterCatalogGet } as unknown as HavenClient

    await expect(getComicWorkChapterCatalog({ workId: null, mediaItemId: MEDIA_ITEM_ID }, client))
      .resolves.toBe(catalog)
    expect(comicWorkChapterCatalogGet).toHaveBeenCalledWith({ workId: null, mediaItemId: MEDIA_ITEM_ID })
  })

  it("rejects an ambiguous, empty or non-canonical target before invoking IPC", async () => {
    const comicWorkChapterCatalogGet = vi.fn()
    const client = { comicWorkChapterCatalogGet } as unknown as HavenClient

    // 运行时负例：这些目标要么两个身份同时缺失/同时存在，要么是非规范 UUID，
    // 本就不属于 `ComicWorkCatalogTarget` 联合类型。用 unknown 显式表达“非法输入”，
    // 再在调用边界收窄，避免用类型断言伪造出合法形状。
    const invalidTargets: readonly unknown[] = [
      { workId: null, mediaItemId: null },
      { workId: WORK_ID, mediaItemId: MEDIA_ITEM_ID },
      { workId: "https://example.invalid/manga", mediaItemId: null },
      // 判别数据必须含十六进制字母，否则 toUpperCase() 是恒等变换、断言必然通过。
      { workId: null, mediaItemId: "a2222222-2222-4222-8222-2222222222A1" },
    ]

    for (const target of invalidTargets) {
      await expect(getComicWorkChapterCatalog(target as ComicWorkCatalogTarget, client))
        .rejects.toMatchObject({ code: "INVALID_ARGUMENT", retryable: false })
    }
    expect(comicWorkChapterCatalogGet).not.toHaveBeenCalled()
  })

  it("accepts the canonical lowercase form of an identity that carries hex letters", async () => {
    const lowercase = "a2222222-2222-4222-8222-2222222222a1"
    const echoed = { ...catalog, currentMediaItemId: lowercase }

    await expect(getComicWorkChapterCatalog({ workId: null, mediaItemId: lowercase }, clientReturning(echoed)))
      .resolves.toBe(echoed)
  })

  it("rejects a response that does not echo the requested identity", async () => {
    const other = { ...catalog, workId: "99999999-9999-4999-8999-999999999999" }
    await expect(getComicWorkChapterCatalog({ workId: WORK_ID, mediaItemId: null }, clientReturning(other)))
      .rejects.toMatchObject({ code: "COMIC_WORK_CATALOG_INVALID_RESPONSE", retryable: false })
  })

  it("rejects a response with an unknown field instead of trusting it", async () => {
    await expect(getComicWorkChapterCatalog(
      { workId: WORK_ID, mediaItemId: null },
      clientReturning({ ...catalog, providerUrl: "https://example.invalid" }),
    )).rejects.toMatchObject({ code: "COMIC_WORK_CATALOG_INVALID_RESPONSE", retryable: false })
  })

  it("preserves a typed client error", async () => {
    const expected = new HavenError({
      code: "SOURCE_UNAVAILABLE",
      userMessage: "来源暂时不可用",
      retryable: true,
    })
    const client = { comicWorkChapterCatalogGet: vi.fn().mockRejectedValue(expected) } as unknown as HavenClient
    await expect(getComicWorkChapterCatalog({ workId: WORK_ID, mediaItemId: null }, client)).rejects.toBe(expected)
  })
})

describe("refreshComicWorkChapterCatalog", () => {
  it("uses the explicit refresh command with the same safe projection", async () => {
    const comicWorkChapterCatalogRefresh = vi.fn().mockResolvedValue(catalog)
    const client = { comicWorkChapterCatalogRefresh } as unknown as HavenClient

    await expect(refreshComicWorkChapterCatalog({ workId: WORK_ID, mediaItemId: null }, client))
      .resolves.toBe(catalog)
    expect(comicWorkChapterCatalogRefresh).toHaveBeenCalledWith({ workId: WORK_ID, mediaItemId: null })
  })

  it("does not fall back to the read command when refresh returns an invalid projection", async () => {
    const comicWorkChapterCatalogRefresh = vi.fn().mockResolvedValue({ schemaVersion: 1 })
    const client = { comicWorkChapterCatalogRefresh } as unknown as HavenClient

    await expect(refreshComicWorkChapterCatalog({ workId: null, mediaItemId: MEDIA_ITEM_ID }, client))
      .rejects.toMatchObject({ code: "COMIC_WORK_CATALOG_INVALID_RESPONSE", retryable: false })
    expect(comicWorkChapterCatalogRefresh).toHaveBeenCalledTimes(1)
  })
})
