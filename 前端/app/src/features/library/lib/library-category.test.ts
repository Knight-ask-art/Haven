import { describe, expect, it } from "vitest"
import type { LibraryMediaItemData } from "../components/MediaItem"
import { filterLibraryItemsByCategory } from "./library-category"

const movie: LibraryMediaItemData = { id: "movie", title: "Movie", type: "movie", year: 2026, imageUrl: "" }
const article: LibraryMediaItemData = { id: "article", title: "Article", type: "article", year: 2026, imageUrl: "" }

describe("library display categories", () => {
  it("returns every item for all and includes articles under periodicals", () => {
    expect(filterLibraryItemsByCategory([movie, article], "all")).toEqual([movie, article])
    expect(filterLibraryItemsByCategory([movie, article], "periodical")).toEqual([article])
  })

  it("returns no stale item for an empty category", () => {
    expect(filterLibraryItemsByCategory([movie], "book")).toEqual([])
  })
})
