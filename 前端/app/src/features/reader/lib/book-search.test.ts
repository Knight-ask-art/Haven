import { describe, expect, it } from "vitest"
import {
  buildBookSearchIndex,
  fullWidthToHalfWidth,
  isBookSearchOperationCurrent,
  normalizeForMatch,
  normalizeWithMap,
  searchBookWithRemoteFallback,
  searchBook,
  textAnchorFromHit,
  tokenizeForRank,
} from "./book-search"
import type { BookChapter } from "./book-content"

function chapter(id: string, paragraphs: string[]): BookChapter {
  return { id, kicker: `Chapter ${id}`, title: `标题${id}`, paragraphs }
}

describe("normalizeForMatch", () => {
  it("folds whitespace, lowercases and converts full-width", () => {
    expect(normalizeForMatch("  Hello   \u3000 World  ")).toBe("hello world")
    expect(normalizeForMatch("ＡＢＣ１２３")).toBe("abc123")
    expect(normalizeForMatch("Ｈｅｌｌｏ")).toBe("hello")
  })
})

describe("fullWidthToHalfWidth", () => {
  it("maps FF01-FF5E and 3000", () => {
    expect(fullWidthToHalfWidth("！＂＃")).toBe('!"#')
    expect(fullWidthToHalfWidth("\u3000")).toBe(" ")
  })
})

describe("normalizeWithMap", () => {
  it("preserves offset mapping", () => {
    const { norm, map } = normalizeWithMap("a  b")
    expect(norm).toBe("a b")
    expect(map).toEqual([0, -1, 3])
  })

  it("handles full-width", () => {
    const { norm } = normalizeWithMap("Ａ　Ｂ")
    expect(norm).toBe("a b")
  })
})

describe("tokenizeForRank", () => {
  it("emits CJK bigrams", () => {
    expect(tokenizeForRank("人工智能")).toEqual(["人工", "工智", "智能"])
  })

  it("emits single CJK char as token", () => {
    expect(tokenizeForRank("中")).toEqual(["中"])
  })

  it("emits ascii words lowercased", () => {
    expect(tokenizeForRank("Hello World 123")).toEqual(["hello", "world", "123"])
  })
})

describe("buildBookSearchIndex", () => {
  it("builds per-chapter norm and df", () => {
    const chapters = [chapter("c1", ["人工智能 发展"]), chapter("c2", ["机器学习"])]
    const index = buildBookSearchIndex(chapters)
    expect(index.documents).toBe(2)
    expect(index.termDocumentFrequencies.get("人工")).toBe(1)
    expect(index.chapters[0].norm.length).toBeGreaterThan(0)
  })
})

describe("searchBook", () => {
  it("finds hits with exact/prefix/suffix", () => {
    const chapters = [chapter("c1", ["人工智能是未来的方向，人工智能改变世界"])]
    const index = buildBookSearchIndex(chapters)
    const hits = searchBook(chapters, index, { query: "人工智能" })
    expect(hits.length).toBeGreaterThan(0)
    expect(hits[0].exact.length).toBeGreaterThanOrEqual(12)
    expect(hits[0].exact.length).toBeLessThanOrEqual(240)
    expect(hits[0].score).toBeGreaterThan(0)
  })

  it("returns empty for empty or too long query", () => {
    const chapters = [chapter("c1", ["hello world"])]
    const index = buildBookSearchIndex(chapters)
    expect(searchBook(chapters, index, { query: "  " })).toEqual([])
    expect(searchBook(chapters, index, { query: "a".repeat(129) })).toEqual([])
  })

  it("respects cancellation", () => {
    const chapters = [chapter("c1", ["hello world hello world"])]
    const index = buildBookSearchIndex(chapters)
    const controller = new AbortController()
    controller.abort()
    expect(() => searchBook(chapters, index, { query: "hello", signal: controller.signal })).toThrow()
  })

  it("caps per-chapter and global hits", () => {
    const paragraph = Array.from({ length: 30 }, () => "test").join(" ")
    const chapters = [chapter("c1", [paragraph])]
    const index = buildBookSearchIndex(chapters)
    const hits = searchBook(chapters, index, { query: "test" })
    expect(hits.length).toBeLessThanOrEqual(20)
  })

  it("builds TextAnchor from hit", () => {
    const chapters = [chapter("c1", ["人工智能是未来的方向"])]
    const index = buildBookSearchIndex(chapters)
    const [hit] = searchBook(chapters, index, { query: "人工智能" })
    const anchor = textAnchorFromHit(hit)
    expect(anchor.exact).toBe(hit.exact)
    expect(anchor.prefix).toBe(hit.prefix)
    expect(anchor.suffix).toBe(hit.suffix)
  })
})

describe("searchBookWithRemoteFallback", () => {
  it("falls back to parsed chapters when remote reader search fails", async () => {
    const chapters = [chapter("c1", ["火影忍者的故事从这里开始，鸣人踏上旅程"])]
    const index = buildBookSearchIndex(chapters)
    const remoteSearch = async () => { throw new Error("reader_search unavailable") }

    await expect(searchBookWithRemoteFallback({
      query: "火影忍者",
      chapters,
      index,
      remoteSearch,
    })).resolves.toEqual(expect.arrayContaining([
      expect.objectContaining({ chapterId: "c1", paragraphIndex: 0 }),
    ]))
  })

  it("does not let a superseded remote request produce fallback results", async () => {
    const chapters = [chapter("c1", ["旧查询正文应该被丢弃"])]
    const index = buildBookSearchIndex(chapters)
    let currentOperation = 1
    let resolveRemote: ((error: Error) => void) | undefined
    const remoteSearch = () => new Promise<never>((_, reject) => {
      resolveRemote = reject
    })

    const request = searchBookWithRemoteFallback({
      query: "旧查询",
      chapters,
      index,
      remoteSearch,
      isCurrent: () => isBookSearchOperationCurrent(1, currentOperation),
    })
    currentOperation = 2
    resolveRemote?.(new Error("late response"))

    await expect(request).resolves.toBeNull()
    expect(isBookSearchOperationCurrent(1, currentOperation)).toBe(false)
  })
})
