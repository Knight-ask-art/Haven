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

import { getFavoriteFootprintItems } from "./footprints-gateway"

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
})
