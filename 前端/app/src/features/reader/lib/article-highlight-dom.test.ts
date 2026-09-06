// @vitest-environment jsdom

import { describe, expect, it } from "vitest"
import { applyArticleFallbackHighlight, clearArticleFallbackHighlights, createArticleTextRange } from "./article-highlight-dom"

describe("article highlight DOM projection", () => {
  it("projects a range across inline Markdown/HTML nodes without flattening them", () => {
    const root = document.createElement("div")
    root.innerHTML = "hello <strong>brave</strong> <em>new</em> <a>world</a>"
    const range = createArticleTextRange(root, 6, 21)

    expect(range).not.toBeNull()
    if (!range) throw new Error("expected a DOM range")
    applyArticleFallbackHighlight(range, "marker-1")

    expect(root.textContent).toBe("hello brave new world")
    expect(root.querySelector("strong")?.textContent).toBe("brave")
    expect(root.querySelector("em")?.textContent).toBe("new")
    expect(root.querySelector("a")?.textContent).toBe("world")
    expect(root.querySelectorAll('mark[data-haven-marker-id="marker-1"]')).toHaveLength(5)
  })

  it("uses the supplied unique range and removes only fallback marks", () => {
    const root = document.createElement("div")
    root.innerHTML = "repeat repeat"
    const range = createArticleTextRange(root, 7, 13)

    expect(range).not.toBeNull()
    if (!range) throw new Error("expected a DOM range")
    applyArticleFallbackHighlight(range, "marker-2")

    expect(root.querySelector('mark[data-haven-marker-id="marker-2"]')?.textContent).toBe("repeat")
    expect(root.textContent).toBe("repeat repeat")
    clearArticleFallbackHighlights(root)
    expect(root.querySelector("mark")).toBeNull()
    expect(root.textContent).toBe("repeat repeat")
  })
})
