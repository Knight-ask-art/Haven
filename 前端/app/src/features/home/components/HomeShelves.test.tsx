import { renderToStaticMarkup } from "react-dom/server"
import { MemoryRouter } from "react-router"
import { describe, expect, it } from "vitest"
import type { ShelfDto, WorkCardDto } from "@/lib/ipc/generated/wire"
import { HomeShelves } from "./HomeShelves"

function favoriteCard(): WorkCardDto {
  return {
    workId: "0196f0d2-0000-7000-8000-000000000001",
    title: "沙丘2",
    originalTitle: null,
    description: null,
    categories: ["video"],
    availableMediaTypes: ["movie"],
    posterUri: "haven-resource://artwork/poster",
    backdropUri: null,
    releaseYear: 2024,
    ratingValue: null,
    ratingScale: null,
    favorite: true,
    progress: null,
    primaryAction: null,
    externalIds: [],
  }
}

function renderShelves(shelves: ShelfDto[]): string {
  return renderToStaticMarkup(
    <MemoryRouter>
      <HomeShelves shelves={shelves} />
    </MemoryRouter>,
  )
}

describe("HomeShelves", () => {
  it("renders the localized favorites shelf and its cards", () => {
    const html = renderShelves([{
      shelfId: "shelf-favorites",
      titleKey: "shelf.favorites",
      preview: [favoriteCard()],
      viewMore: null,
    }])

    expect(html).toContain("收藏")
    expect(html).toContain("沙丘2")
    expect(html).toContain('href="/footprints"')
    expect(html).toContain("查看更多")
  })

  it("renders no shelf for an empty projection or empty preview", () => {
    expect(renderShelves([])).toBe("")
    expect(renderShelves([{
      shelfId: "shelf-favorites",
      titleKey: "shelf.favorites",
      preview: [],
      viewMore: null,
    }])).toBe("")
  })

  it("falls back to a generic title for an unknown title key", () => {
    const html = renderShelves([{
      shelfId: "shelf-unknown",
      titleKey: "shelf.unknown",
      preview: [favoriteCard()],
      viewMore: null,
    }])

    expect(html).toContain("内容架")
  })
})
