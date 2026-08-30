import { describe, expect, it } from "vitest"
import { decodeBookText, parseBookText } from "./book-content"

function utf8(value: string): ArrayBuffer {
  return new TextEncoder().encode(value).buffer as ArrayBuffer
}

describe("book-content", () => {
  it("splits paragraphs and recognizes plain text chapter headings", () => {
    expect(parseBookText("第一段\n\n第一章：开始\n章节正文\n换行\n\n# 尾声\n\n最后一段")).toEqual([
      { id: "chapter-1", kicker: "Chapter 1", title: "全文", paragraphs: ["第一段"] },
      { id: "chapter-2", kicker: "Chapter 2", title: "第一章：开始", paragraphs: ["章节正文 换行"] },
      { id: "chapter-3", kicker: "Chapter 3", title: "尾声", paragraphs: ["最后一段"] },
    ])
  })

  it("ignores consecutive blank lines and trims leading and trailing whitespace", () => {
    expect(parseBookText("\n\n  第一段  \n\n\n\t第二段\t\n\n")).toEqual([
      { id: "chapter-1", kicker: "Chapter 1", title: "全文", paragraphs: ["第一段", "第二段"] },
    ])
  })

  it("recognizes a chapter heading followed immediately by body text", () => {
    expect(parseBookText("第一章：开始\n正文第一行\n正文第二行\nChapter II - Next\nEnglish body")).toEqual([
      { id: "chapter-1", kicker: "Chapter 1", title: "第一章：开始", paragraphs: ["正文第一行 正文第二行"] },
      { id: "chapter-2", kicker: "Chapter 2", title: "Chapter II - Next", paragraphs: ["English body"] },
    ])
  })

  it("preserves Markdown line structure for list and code blocks", () => {
    expect(parseBookText("# 第一章\n\n- 第一项\n- 第二项\n\n```ts\nconst value = 1\n```", "markdown")).toEqual([
      { id: "chapter-1", kicker: "Chapter 1", title: "第一章", paragraphs: ["- 第一项\n- 第二项", "```ts\nconst value = 1\n```"] },
    ])
  })

  it("keeps an overlong line as body text instead of treating it as a heading", () => {
    const longLine = `Chapter 1 ${"x".repeat(170)}`
    expect(parseBookText(longLine)).toEqual([
      { id: "chapter-1", kicker: "Chapter 1", title: "全文", paragraphs: [longLine] },
    ])
  })

  it("returns no chapters for an empty or whitespace-only text", () => {
    expect(parseBookText(" \n\n\t ")).toEqual([])
  })

  it("accepts GB18030 text and rejects unsupported encodings with safe catalog errors", () => {
    expect(() => decodeBookText(utf8("abcd"), "text/plain", 3)).toThrow("文本文件过大")
    expect(() => decodeBookText(utf8("ok"), "application/epub+zip")).toThrow("支持 TXT 和 Markdown")
    expect(decodeBookText(utf8("# Markdown"), "text/markdown")).toBe("# Markdown")
    expect(decodeBookText(new Uint8Array([0xd6, 0xd0, 0xce, 0xc4]).buffer, "text/plain")).toBe("中文")
    expect(() => decodeBookText(new Uint8Array([0xff, 0xff]).buffer, "text/plain")).toThrow("GB18030")
  })

  it("rejects NUL bytes and accepts content exactly at the byte limit", () => {
    expect(() => decodeBookText(utf8("a\0b"), "text/plain")).toThrow("文本内容格式无效")
    expect(decodeBookText(utf8("abcd"), "text/plain", 4)).toBe("abcd")
  })
})
