import { createElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it } from "vitest"
import {
  articleOutline,
  decodeArticleText,
  MAX_ARTICLE_TEXT_BYTES,
  parseArticleContent,
  parseArticleText,
} from "./article-content"

function utf8(value: string): ArrayBuffer {
  return new TextEncoder().encode(value).buffer as ArrayBuffer
}

describe("article-content", () => {
  it("parses a real title, paragraphs, and an outline from the same stable blocks", () => {
    const source = "# 本地优先\n\n开场第一行\n开场第二行\n\n## 数据主权\n\n第二节正文。"
    const first = parseArticleText(source)
    const second = parseArticleText(source)

    expect(first).not.toBeNull()
    if (!first) throw new Error("expected parsed article")
    expect(first?.title).toBe("本地优先")
    expect(first?.sections.map((section) => section.paragraphs.map((paragraph) => paragraph.text))).toEqual([
      ["开场第一行 开场第二行"],
      ["第二节正文。"],
    ])
    expect(first?.sections.map((section) => section.level)).toEqual([1, 2])
    expect(articleOutline(first)).toEqual(first.sections.map(({ id, title, level }) => ({ id, title, level })))
    expect(second?.sections.map((section) => section.id)).toEqual(first?.sections.map((section) => section.id))
    expect(second?.sections.flatMap((section) => section.paragraphs.map((paragraph) => paragraph.id))).toEqual(
      first?.sections.flatMap((section) => section.paragraphs.map((paragraph) => paragraph.id)),
    )
  })

  it("keeps dangerous HTML and URLs in inert text nodes without an executable channel", () => {
    const dangerous = "<script>alert(1)</script> <img src=x onerror=alert(2)> javascript:alert(3) <iframe src=https://evil.example></iframe>"
    const document = parseArticleText(`# Unsafe\n\n${dangerous}`)
    const text = document?.sections[0].paragraphs[0].text ?? ""
    const markup = renderToStaticMarkup(createElement("p", null, text))

    expect(text).toBe(dangerous)
    expect(markup).not.toContain("<script")
    expect(markup).not.toContain("<img")
    expect(markup).not.toContain("<iframe")
    expect(markup).not.toContain("src=\"https://evil.example\"")
    expect(markup).not.toContain("onerror=\"alert(2)\"")
    expect(markup).not.toContain("href=\"javascript:")
    expect(markup).toContain("&lt;script&gt;")
  })

  it("uses the first plain-text line as the real title and gives duplicate blocks unique stable ids", () => {
    const first = parseArticleText("第一段\n\n第一段\n\n# 同名\n\n正文\n\n# 同名\n\n正文")
    const second = parseArticleText("第一段\n\n第一段\n\n# 同名\n\n正文\n\n# 同名\n\n正文")

    expect(first?.title).toBe("第一段")
    expect(new Set(first?.sections.map((section) => section.id)).size).toBe(first?.sections.length)
    expect(new Set(first?.sections.flatMap((section) => section.paragraphs.map((paragraph) => paragraph.id))).size)
      .toBe(first?.sections.flatMap((section) => section.paragraphs).length)
    expect(second).toEqual(first)
  })

  it("uses a single non-empty plain-text line as the article title", () => {
    const document = parseArticleText("\n  单行真实标题  \n")

    expect(document).not.toBeNull()
    if (!document) throw new Error("expected parsed article")
    expect(document.title).toBe("单行真实标题")
    expect(document.sections).toHaveLength(1)
    expect(document.sections[0].title).toBe("单行真实标题")
    expect(document.sections[0].paragraphs).toEqual([])
    expect(articleOutline(document)[0]?.title).toBe("单行真实标题")
  })

  it("returns empty for whitespace-only content", () => {
    expect(parseArticleText(" \n\n\t ")).toBeNull()
  })

  it("preserves Markdown structure and records the content format", () => {
    const document = parseArticleContent("# 标题\n\n- 第一项\n- 第二项\n\n`代码`", "text/markdown")

    expect(document?.format).toBe("markdown")
    expect(document?.sections[0]?.paragraphs[0]?.text).toBe("- 第一项\n- 第二项")
    expect(document?.sections[0]?.paragraphs[1]?.text).toBe("`代码`")
  })

  it("requires a browser DOM before processing HTML", () => {
    expect(() => parseArticleContent("<p>正文</p>", "text/html")).toThrow("无法安全解析 HTML")
  })

  it("rejects non-text, oversized, invalid UTF-8, and NUL input", () => {
    expect(() => decodeArticleText(utf8("ok"), "application/json")).toThrow("支持纯文本、Markdown 和 HTML")
    expect(() => decodeArticleText(utf8("abcd"), "text/plain", 3)).toThrow("文件过大")
    expect(() => decodeArticleText(new Uint8Array([0xc3, 0x28]).buffer, "text/plain")).toThrow("UTF-8")
    expect(() => decodeArticleText(utf8("a\0b"), "text/plain")).toThrow("格式无效")
  })

  it("accepts content exactly at the byte limit", () => {
    expect(MAX_ARTICLE_TEXT_BYTES).toBe(8 * 1024 * 1024)
    expect(decodeArticleText(utf8("abcd"), "text/plain", 4)).toBe("abcd")
  })
})
