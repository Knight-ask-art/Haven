import { beforeEach, describe, expect, it, vi } from "vitest"
import type { LibraryMediaItemData } from "@/features/library/components/MediaItem"
import type { PageDto, WorkCardDto } from "@/lib/ipc/generated/wire"

const mocks = vi.hoisted(() => ({
  getHavenClient: vi.fn(),
  getHavenClientMode: vi.fn(),
  getCatalogItems: vi.fn(),
  getStoredFavoriteIds: vi.fn(),
}))

vi.mock("@/lib/ipc/runtime", () => ({
  getHavenClient: mocks.getHavenClient,
  getHavenClientMode: mocks.getHavenClientMode,
}))

vi.mock("@/lib/havenState", () => ({
  getCatalogItems: mocks.getCatalogItems,
  getStoredFavoriteIds: mocks.getStoredFavoriteIds,
}))

import { getContinueFootprintItems, getFavoriteFootprintItems, getRecentActivityFootprintItems } from "./footprints-gateway"

function workCard(): WorkCardDto {
  return {
    workId: "work-tauri",
    title: "真实作品",
    originalTitle: null,
    description: null,
    categories: ["book"],
    availableMediaTypes: ["book"],
    posterUri: "session://poster",
    backdropUri: null,
    releaseYear: 2025,
    ratingValue: null,
    ratingScale: null,
    favorite: true,
    progress: null,
    primaryAction: null,
    externalIds: [],
  }
}

function page(items: WorkCardDto[]): PageDto<WorkCardDto> {
  return {
    schemaVersion: 1,
    items,
    nextCursor: null,
    total: items.length,
    revision: null,
  }
}

describe("footprints gateway runtime boundary", () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it("uses the authoritative Tauri projection without touching Demo storage", async () => {
    mocks.getHavenClientMode.mockReturnValue("tauri")
    const libraryList = vi.fn().mockResolvedValue(page([workCard()]))
    mocks.getHavenClient.mockReturnValue({ libraryList })
    mocks.getStoredFavoriteIds.mockImplementation(() => {
      throw new Error("Demo localStorage must not be read in Tauri")
    })

    await expect(getFavoriteFootprintItems()).resolves.toEqual([
      {
        id: "work-tauri",
        workId: "work-tauri",
        mediaItemId: null,
        primaryAction: null,
        favorite: true,
        title: "真实作品",
        subtitle: "已收藏 · 图书",
        typeBadge: "图书",
        imageUrl: "session://poster",
        artworkCategory: "book",
      },
    ])
    expect(mocks.getStoredFavoriteIds).not.toHaveBeenCalled()
    expect(mocks.getCatalogItems).not.toHaveBeenCalled()
    expect(libraryList).toHaveBeenCalledOnce()
  })

  it("fails closed for an unavailable browser instead of reading Demo storage", async () => {
    mocks.getHavenClientMode.mockReturnValue("unavailable")
    mocks.getStoredFavoriteIds.mockImplementation(() => {
      throw new Error("Demo localStorage must not be read when IPC is unavailable")
    })

    await expect(getFavoriteFootprintItems()).resolves.toEqual([])
    expect(mocks.getStoredFavoriteIds).not.toHaveBeenCalled()
    expect(mocks.getCatalogItems).not.toHaveBeenCalled()
    expect(mocks.getHavenClient).not.toHaveBeenCalled()
  })

  it("keeps Demo storage available only for the explicit Mock client", async () => {
    mocks.getHavenClientMode.mockReturnValue("mock")
    mocks.getStoredFavoriteIds.mockReturnValue(["demo-id"])
    const demoItem: LibraryMediaItemData = {
      id: "demo-id",
      title: "Demo 作品",
      type: "book",
      year: 2025,
      imageUrl: "demo://poster",
      badge: "图书",
    }
    mocks.getCatalogItems.mockReturnValue([demoItem])

    await expect(getFavoriteFootprintItems()).resolves.toEqual([
      {
        id: "demo-id",
        workId: "demo-id",
        mediaItemId: null,
        primaryAction: null,
        favorite: true,
        title: "Demo 作品",
        subtitle: "已收藏 · 图书",
        typeBadge: "图书",
        imageUrl: "demo://poster",
        artworkCategory: "book",
      },
    ])
    expect(mocks.getStoredFavoriteIds).toHaveBeenCalledOnce()
    expect(mocks.getCatalogItems).toHaveBeenCalledWith(["demo-id"])
    expect(mocks.getHavenClient).not.toHaveBeenCalled()
  })

  it("preserves distinct work, media item, and primary action identities for continue cards", async () => {
    mocks.getHavenClientMode.mockReturnValue("tauri")
    const mediaItemId = "11111111-1111-4111-8111-111111111111"
    const card = workCard()
    card.progress = {
      mediaItemId,
      completion: "in_progress",
      progressRatio: 0.4,
      revision: "revision-1",
      updatedAt: "2026-09-06T00:00:00Z",
      locator: { version: 1, kind: "book", data: { publicationResource: "chapter-1", formatLocator: null, progression: 0.4, textAnchor: null } },
    }
    card.primaryAction = {
      kind: "reader",
      labelHint: "continue",
      editionId: "22222222-2222-4222-8222-222222222222",
      mediaItemId,
      locator: card.progress.locator,
    }
    const client = {
      libraryList: vi.fn().mockResolvedValue(page([card])),
      progressRecent: vi.fn().mockResolvedValue([card.progress]),
    }
    mocks.getHavenClient.mockReturnValue(client)

    const [result] = await getContinueFootprintItems()

    expect(result).toEqual(expect.objectContaining({
      id: "work-tauri",
      workId: "work-tauri",
      mediaItemId,
      primaryAction: card.primaryAction,
    }))
  })

  it("uses the history entry's exact media action and progress when the work card projects another episode", async () => {
    mocks.getHavenClientMode.mockReturnValue("tauri")
    const historyMediaItemId = "33333333-3333-4333-8333-333333333333"
    const projectedMediaItemId = "44444444-4444-4444-8444-444444444444"
    const card = workCard()
    card.progress = {
      mediaItemId: projectedMediaItemId,
      completion: "in_progress",
      progressRatio: 0.8,
      revision: "revision-current",
      updatedAt: "2026-09-07T00:00:00Z",
      locator: { version: 1, kind: "book", data: { publicationResource: "chapter-current", formatLocator: null, progression: 0.8, textAnchor: null } },
    }
    card.primaryAction = {
      kind: "reader",
      labelHint: "continue",
      editionId: "22222222-2222-4222-8222-222222222222",
      mediaItemId: projectedMediaItemId,
      locator: null,
    }
    const historyAction = {
      kind: "reader" as const,
      labelHint: "continue" as const,
      editionId: "55555555-5555-4555-8555-555555555555",
      mediaItemId: historyMediaItemId,
      locator: null,
    }
    const historyProgress = {
      ...card.progress,
      mediaItemId: historyMediaItemId,
      progressRatio: 0.4,
      locator: { version: 1, kind: "book" as const, data: { publicationResource: "chapter-history", formatLocator: null, progression: 0.4, textAnchor: null } },
    }
    const entry = {
      historyEntryId: "history-1",
      mediaItemId: historyMediaItemId,
      workId: card.workId,
      editionId: historyAction.editionId,
      startedAt: "2026-09-06T00:00:00Z",
      lastActiveAt: "2026-09-06T00:00:00Z",
      completedAt: null,
      primaryAction: historyAction,
      progress: historyProgress,
    } as unknown as import("@/lib/ipc/generated/wire").HistoryEntryDto
    mocks.getHavenClient.mockReturnValue({
      historyList: vi.fn().mockResolvedValue([entry]),
      libraryList: vi.fn().mockResolvedValue(page([card])),
    })

    const [result] = await getRecentActivityFootprintItems()

    expect(result).toEqual(expect.objectContaining({
      mediaItemId: historyMediaItemId,
      primaryAction: historyAction,
      progress: 40,
    }))
  })
})
