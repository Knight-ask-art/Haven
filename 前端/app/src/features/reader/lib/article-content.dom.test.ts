// @vitest-environment jsdom

import { describe, expect, it } from "vitest"
import { parseArticleContent } from "./article-content"

describe("article-content HTML block projection", () => {
  it("maps list and table blocks to paragraph ids without shifting later content", () => {
    const document = parseArticleContent(`
      <article>
        <h1>标题</h1>
        <p>开场</p>
        <ul><li>列表一</li><li>列表二</li></ul>
        <blockquote>引用</blockquote>
        <table><thead><tr><th>表头</th></tr></thead><tbody><tr><td>单元格</td></tr></tbody></table>
        <p>结尾</p>
      </article>
    `, "text/html")

    expect(document).not.toBeNull()
    if (!document?.sanitizedHtml) throw new Error("expected sanitized HTML article")
    const paragraphs = document.sections.flatMap((section) => section.paragraphs)
    expect(paragraphs.map((paragraph) => paragraph.text)).toEqual([
      "开场", "列表一", "列表二", "引用", "表头", "单元格", "结尾",
    ])

    const root = new DOMParser().parseFromString(document.sanitizedHtml, "text/html")
    const projected = Array.from(root.querySelectorAll("p,blockquote,li,th,td"))
    expect(projected.map((element) => element.getAttribute("data-article-block-id")))
      .toEqual(paragraphs.map((paragraph) => paragraph.id))
  })

  it("keeps every paragraph when HTML has no heading and ignores nested block wrappers", () => {
    const document = parseArticleContent(`
      <article>
        <p>第一段</p>
        <ul><li><p>列表段落</p></li><li>列表文本</li></ul>
        <table><tbody><tr><td><p>单元格段落</p></td><th>表头</th></tr></tbody></table>
        <blockquote>引用</blockquote>
        <p>最后一段</p>
      </article>
    `, "text/html")

    expect(document?.title).toBe("文章")
    const paragraphs = document?.sections.flatMap((section) => section.paragraphs) ?? []
    expect(paragraphs.map((paragraph) => paragraph.text)).toEqual([
      "第一段", "列表段落", "列表文本", "单元格段落", "表头", "引用", "最后一段",
    ])
    const root = new DOMParser().parseFromString(document?.sanitizedHtml ?? "", "text/html")
    const projected = Array.from(root.querySelectorAll("p,blockquote,li,th,td"))
      .filter((element) => element.hasAttribute("data-article-block-id"))
    expect(projected.map((element) => element.textContent?.trim())).toEqual(paragraphs.map((paragraph) => paragraph.text))
    expect(projected).toHaveLength(paragraphs.length)
  })
})
