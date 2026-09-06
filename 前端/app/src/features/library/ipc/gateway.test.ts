import { describe, expect, it, vi } from "vitest"
import type { HavenClient } from "@/lib/ipc/client"
import type { EditionListByWorkRequest, EditionListByWorkResultDto } from "@/features/media/ipc/edition-wire"
import type {
  FavoriteSetResult,
  HomeDto,
  LibraryScanEvent,
  LibraryScanStartRequest,
  LibraryListRequest,
  PageDto,
  ScanStartResult,
  WorkCardDto,
  WorkDetailHeaderDto,
  WorkGetRequest,
  ResourceListByMediaItemRequest,
  ResourceListDto,
} from "@/lib/ipc/generated/wire"
const { saveProgress } = vi.hoisted(() => ({
  saveProgress: vi.fn().mockResolvedValue({ revision: "revision-2" }),
}))

vi.mock("@/features/progress/ipc/progress-gateway", () => ({ saveProgress }))

import { acceptLibraryCursor, findLibraryItemById, loadAllLibraryPages, markLibraryItemCompleted } from "./gateway"

function fakeClient(pages: Array<PageDto<WorkCardDto>>): HavenClient {
  let calls = 0
  return {
    libraryList(request: LibraryListRequest) {
      expect(request.cursor).toBe(calls === 0 ? null : `cursor-${calls}`)
      return Promise.resolve(pages[calls++] ?? pages.at(-1)!)
    },
    favoriteSet: async (): Promise<FavoriteSetResult> => ({
      workId: "work",
      favorite: false,
      revision: null,
    }),
    libraryScanStart: async (
      _request: LibraryScanStartRequest,
      _onEvent?: (event: LibraryScanEvent) => void,
    ): Promise<ScanStartResult> => ({
      schemaVersion: 1,
      operationId: "operation",
      taskId: "task",
      alreadyRunning: false,
    }),
    workGet: async (_request: WorkGetRequest): Promise<WorkDetailHeaderDto> => {
      throw new Error("not used")
    },
    editionListByWork: async (_request: EditionListByWorkRequest): Promise<EditionListByWorkResultDto> => ({
      schemaVersion: 1,
      items: [],
      nextCursor: null,
      total: 0,
      revision: null,
    }),
    resourceListByMediaItem: async (_request: ResourceListByMediaItemRequest): Promise<ResourceListDto> => ({
      schemaVersion: 1,
      items: [],
    }),
    sessionOpen: async () => {
      throw new Error("not used")
    },
    sessionClose: async () => {
      throw new Error("not used")
    },
    progressSave: async () => {
      throw new Error("not used")
    },
    progressRecent: async () => [],
    progressReset: async () => {
      throw new Error("not used")
    },
    historyList: async () => [],
    historyClear: async () => {
      throw new Error("not used")
    },
    markerCreate: async () => {
      throw new Error("not used")
    },
    markerList: async () => [],
    markerListAll: async () => [],
    markerDelete: async () => false,
    homeGet: async (): Promise<HomeDto> => ({
      schemaVersion: 1,
      continueItems: [],
      recentlyAdded: [],
      shelves: [],
    }),
    settingsGet: async () => {
      throw new Error("not used")
    },
    settingsUpdate: async () => {
      throw new Error("not used")
    },
  } as unknown as HavenClient
}

function page(items: WorkCardDto[], nextCursor: string | null): PageDto<WorkCardDto> {
  return {
    schemaVersion: 1,
    items,
    nextCursor,
    total: 2,
    revision: null,
  }
}

const card = (workId: string): WorkCardDto => ({
  workId,
  title: workId,
  originalTitle: null,
  categories: ["video"],
  availableMediaTypes: ["movie"],
  releaseYear: null,
  ratingValue: null,
  ratingScale: null,
  posterUri: null,
  backdropUri: null,
  description: null,
  favorite: false,
  progress: null,
  primaryAction: null,
  externalIds: [],
})

describe("library cursor pagination", () => {
  it("rejects a repeated cursor instead of looping forever", () => {
    const seen = new Set<string>()
    expect(acceptLibraryCursor(seen, "cursor-1")).toBe("cursor-1")
    expect(() => acceptLibraryCursor(seen, "cursor-1")).toThrow(/循环 cursor/)
  })

  it("consumes every cursor page instead of silently stopping at 200", async () => {
    const items = await loadAllLibraryPages(fakeClient([
      page([card("first")], "cursor-1"),
      page([card("second")], null),
    ]))
    expect(items.map((item) => item.id)).toEqual(["first", "second"])
  })

  it("finds the authoritative favorite projection beyond the first page", async () => {
    const second = card("second")
    second.favorite = true
    const item = await findLibraryItemById(fakeClient([
      page([card("first")], "cursor-1"),
      page([second], null),
    ]), "second")
    expect(item?.favorite).toBe(true)
  })

  it("derives document and article types from available media instead of broad category", async () => {
    const document = card("document")
    document.categories = ["book"]
    document.availableMediaTypes = ["document"]
    const article = card("article")
    article.categories = ["periodical"]
    article.availableMediaTypes = ["article"]

    const items = await loadAllLibraryPages(fakeClient([page([document, article], null)]))
    expect(items.map((item) => item.type)).toEqual(["document", "article"])
  })

  it("uses the server-selected media item and locator for batch completion", async () => {
    const item = {
      id: "work-1",
      title: "作品",
      type: "video",
      year: 2026,
      imageUrl: "",
      progressMediaItemId: "11111111-1111-4111-8111-111111111111",
      progressLocator: { version: 1 as const, kind: "video" as const, data: { positionMs: 12_000 } },
    }

    await markLibraryItemCompleted(item)

    expect(saveProgress).toHaveBeenCalledWith({
      mediaItemId: item.progressMediaItemId,
      locator: item.progressLocator,
      completion: "completed",
      expectedRevision: null,
    })
  })

  it("rejects batch completion when the authoritative locator is missing", async () => {
    await expect(markLibraryItemCompleted({ id: "work-2", title: "无定位作品", type: "book", year: 2026, imageUrl: "" }))
      .rejects.toThrow("没有可用的阅读定位")
    expect(saveProgress).not.toHaveBeenCalledWith(expect.objectContaining({ mediaItemId: "work-2" }))
  })
})
