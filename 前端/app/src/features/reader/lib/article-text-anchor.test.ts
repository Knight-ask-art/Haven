import { describe, expect, it } from "vitest"
import { buildArticleTextAnchorAtOffset, resolveArticleTextAnchor } from "./article-text-anchor"

describe("article text anchors", () => {
  it("resolves the selected repeated phrase by rendered offset context", () => {
    const text = "alpha repeated middle repeated omega"
    const start = text.lastIndexOf("repeated")
    const anchor = buildArticleTextAnchorAtOffset(text, "repeated", start)
    expect(resolveArticleTextAnchor(text, anchor)).toEqual({ start, end: start + 8 })
  })

  it("fails closed when an anchor is no longer unique", () => {
    expect(resolveArticleTextAnchor("same same", { exact: "same", prefix: null, suffix: null })).toBeNull()
  })

  it("rejects overlong or offset-mismatched selections", () => {
    expect(buildArticleTextAnchorAtOffset("hello", "hello", 1)).toBeNull()
    expect(buildArticleTextAnchorAtOffset("x".repeat(241), "x".repeat(241), 0)).toBeNull()
  })
})
