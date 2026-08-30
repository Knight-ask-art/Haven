import { describe, expect, it } from "vitest"
import {
  boundPdfPageText,
  flattenPdfSearchHits,
  MAX_PDF_SEARCH_PAGE_CHARS,
  normalizePdfQuery,
  searchPdfPages,
} from "./pdf-search"

function provider(pages: Record<number, string>, calls: number[] = []) {
  return {
    getPageText: async (pageNumber: number) => {
      calls.push(pageNumber)
      return pages[pageNumber] ?? ""
    },
    calls,
  }
}

describe("pdf-search", () => {
  it("normalizes queries by trimming and case folding", () => {
    expect(normalizePdfQuery("  Hello   WORLD \n\t")).toBe("hello world")
    expect(normalizePdfQuery("　")).toBe("")
  })

  it("returns no hits without calling pages for a blank query", async () => {
    const { getPageText, calls } = provider({ 1: "anything" })
    const hits = await searchPdfPages({ pageCount: 3, query: "   ", getPageText })
    expect(hits).toEqual([])
    expect(calls).toEqual([])
  })

  it("counts occurrences per page in order and skips empty pages", async () => {
    const { getPageText } = provider({
      1: "Alpha alpha ALPHA beta",
      2: "nothing relevant",
      3: "gamma",
      4: "alpha in the middle alpha",
    })
    const hits = await searchPdfPages({ pageCount: 4, query: "ALPHA", getPageText })
    expect(hits).toEqual([
      { pageNumber: 1, occurrences: 3 },
      { pageNumber: 4, occurrences: 2 },
    ])
  })

  it("matches CJK substrings without word boundaries", async () => {
    const { getPageText } = provider({ 1: "庆余年 第一卷", 2: "余年再起" })
    const hits = await searchPdfPages({ pageCount: 2, query: "余年", getPageText })
    expect(hits).toEqual([
      { pageNumber: 1, occurrences: 1 },
      { pageNumber: 2, occurrences: 1 },
    ])
  })

  it("aborts between pages and stops issuing page reads", async () => {
    const controller = new AbortController()
    const { getPageText, calls } = provider({ 1: "hit", 2: "hit hit" })
    const pending = searchPdfPages({
      pageCount: 5,
      query: "hit",
      getPageText,
      signal: controller.signal,
    })
    controller.abort()
    await expect(pending).rejects.toMatchObject({ name: "AbortError" })
    expect(calls.length).toBeLessThanOrEqual(2)
  })

  it("bounds each page's searched text to the configured cap", async () => {
    const beyondCap = `filler${"x".repeat(MAX_PDF_SEARCH_PAGE_CHARS)}needle`
    const { getPageText } = provider({ 1: beyondCap })
    const hits = await searchPdfPages({ pageCount: 1, query: "needle", getPageText })
    expect(hits).toEqual([])
  })

  it("bounds cached page text through the shared helper", () => {
    const within = "short text"
    expect(boundPdfPageText(within)).toBe(within)
    const oversized = `a${"b".repeat(MAX_PDF_SEARCH_PAGE_CHARS)}c`
    expect(boundPdfPageText(oversized).length).toBe(MAX_PDF_SEARCH_PAGE_CHARS)
    expect(boundPdfPageText(oversized).startsWith("ab")).toBe(true)
    expect(boundPdfPageText("")).toBe("")
  })

  it("reports progress after every scanned page", async () => {
    const { getPageText } = provider({ 1: "a", 2: "b", 3: "c" })
    const progress: number[] = []
    await searchPdfPages({ pageCount: 3, query: "z", getPageText, onProgress: (n) => progress.push(n) })
    expect(progress).toEqual([1, 2, 3])
  })

  it("flattens hits into cycling occurrence order for navigation", () => {
    const flat = flattenPdfSearchHits([
      { pageNumber: 2, occurrences: 3 },
      { pageNumber: 5, occurrences: 1 },
    ])
    expect(flat).toEqual([
      { pageNumber: 2, occurrence: 1 },
      { pageNumber: 2, occurrence: 2 },
      { pageNumber: 2, occurrence: 3 },
      { pageNumber: 5, occurrence: 1 },
    ])
  })
})
