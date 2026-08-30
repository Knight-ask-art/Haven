import { describe, expect, it } from "vitest"
import type { HavenClient } from "@/lib/ipc/client"
import type { PageDto, WorkCardDto } from "@/lib/ipc/generated/wire"
import { searchLocalLibrary } from "./search-gateway"

const card = (id: string, title: string): WorkCardDto => ({
  workId: id,
  title,
  originalTitle: null,
  description: null,
  categories: ["book"],
  availableMediaTypes: ["book"],
  posterUri: null,
  backdropUri: null,
  releaseYear: 2025,
  ratingValue: 8.5,
  ratingScale: 10,
  favorite: false,
  progress: null,
  primaryAction: null,
  externalIds: [],
})

const page = (
  items: WorkCardDto[],
  nextCursor: string | null,
  total: number,
): PageDto<WorkCardDto> => ({
  schemaVersion: 1,
  items,
  nextCursor,
  total,
  revision: null,
})

function clientWithPages(
  pages: Array<PageDto<WorkCardDto>>,
  expectedCategory: "all" | "book" = "book",
): HavenClient {
  let index = 0
  return {
    libraryList: async (request) => {
      expect(request.query).toBe("dune")
      expect(request.category).toBe(expectedCategory)
      return pages[index++] ?? page([], null, 0)
    },
  } as HavenClient
}

describe("searchLocalLibrary", () => {
  it("queries the local library and consumes the complete cursor chain", async () => {
    const signal = new AbortController().signal
    const result = await searchLocalLibrary(
      "  dune ",
      "book",
      signal,
      clientWithPages([
        page([card("1", "Dune")], "page-2", 2),
        page([card("2", "Dune Messiah")], null, 2),
      ]),
    )

    expect(result.map((item) => item.title)).toEqual(["Dune", "Dune Messiah"])
  })

  it("returns no results for an empty or cancelled query", async () => {
    const controller = new AbortController()
    controller.abort()
    const client = clientWithPages([page([], null, 0)])

    await expect(searchLocalLibrary("  ", "all", new AbortController().signal, client)).resolves.toEqual([])
    await expect(searchLocalLibrary("dune", "all", controller.signal, client)).resolves.toEqual([])
  })

  it("rejects a repeated cursor instead of looping forever", async () => {
    const signal = new AbortController().signal
    await expect(searchLocalLibrary(
      "dune",
      "all",
      signal,
      clientWithPages([
        page([], "same", 1),
        page([], "same", 1),
      ], "all"),
    )).rejects.toThrow("循环 cursor")
  })
})
