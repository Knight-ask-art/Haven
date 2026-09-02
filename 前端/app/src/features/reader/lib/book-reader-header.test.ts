import { describe, expect, it } from "vitest"
import { resolveBookReaderHeaderContext } from "./book-reader-header"

describe("book reader header context", () => {
  it("reports a content failure instead of leaving the reader in loading state", () => {
    expect(resolveBookReaderHeaderContext({
      contentStatus: "terminal_error",
      contentErrorMessage: "EPUB 归档目录损坏",
    })).toBe("读取失败 · EPUB 归档目录损坏")
  })

  it("keeps the chapter title for ready content", () => {
    expect(resolveBookReaderHeaderContext({
      contentStatus: "ready",
      currentChapterTitle: "Chapter 10",
    })).toBe("Chapter 10")
  })

  it("distinguishes loading from an empty chapter label", () => {
    expect(resolveBookReaderHeaderContext({ contentStatus: "loading" })).toBe("正在读取文本…")
    expect(resolveBookReaderHeaderContext({ contentStatus: "idle" })).toBe("正在准备内容")
  })
})
