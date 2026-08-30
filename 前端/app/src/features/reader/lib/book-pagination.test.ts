import { describe, expect, it } from "vitest"
import {
  alignBookOffsetToPage,
  bookOffsetForPageDelta,
  bookOffsetForProgression,
  getBookPaginationMetrics,
  setBookPaginationOffsetInstant,
  type BookPaginationViewport,
} from "./book-pagination"

function viewport(overrides: Partial<BookPaginationViewport> = {}): BookPaginationViewport {
  return {
    scrollLeft: 0,
    scrollTop: 0,
    scrollWidth: 2400,
    scrollHeight: 2600,
    clientWidth: 800,
    clientHeight: 600,
    ...overrides,
  }
}

describe("book-pagination", () => {
  it("keeps scroll mode continuous and vertical", () => {
    expect(getBookPaginationMetrics(viewport({ scrollTop: 1000 }), "scroll")).toMatchObject({
      pageCount: 1,
      pageIndex: 0,
      progression: 0.5,
      offset: 1000,
      maxOffset: 2000,
    })
  })

  it("derives one page per viewport in single-page mode", () => {
    expect(getBookPaginationMetrics(viewport({ scrollLeft: 800 }), "paginated")).toMatchObject({
      pageCount: 3,
      pageIndex: 1,
      progression: 0.5,
      offset: 800,
      maxOffset: 1600,
    })
  })

  it("maps progression to a spread and clamps page movement", () => {
    const current = viewport({ scrollLeft: 800 })
    expect(bookOffsetForProgression(current, 0.5, "double")).toBe(800)
    expect(bookOffsetForPageDelta(current, 1, "double")).toBe(1600)
    expect(bookOffsetForPageDelta(current, -1, "double")).toBe(0)
    expect(bookOffsetForPageDelta(viewport({ scrollLeft: 0 }), -1, "paginated")).toBe(0)
    expect(bookOffsetForPageDelta(viewport({ scrollLeft: 1600 }), 1, "paginated")).toBe(1600)
  })

  it("aligns chapter/search anchors to the containing page", () => {
    const current = viewport()
    expect(alignBookOffsetToPage(current, 1199, "paginated")).toBe(800)
    expect(alignBookOffsetToPage(current, 1601, "double")).toBe(1600)
    expect(alignBookOffsetToPage(current, 999, "scroll")).toBe(999)
  })

  it("restores an offset instantly even when the reader frame is smooth-scrolling", () => {
    const container = { scrollTop: 120, scrollLeft: 40, style: { scrollBehavior: "smooth" } }
    setBookPaginationOffsetInstant(container, 640, "paginated")
    expect(container.scrollLeft).toBe(640)
    expect(container.scrollTop).toBe(0)
    expect(container.style.scrollBehavior).toBe("smooth")
  })
})
