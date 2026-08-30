import { describe, expect, it } from "vitest"
import {
  defaultCoverCategoryForMediaType,
  defaultCoverFiles,
  defaultCoverForMediaType,
  defaultCoverPath,
} from "./default-cover"

describe("default cover selection", () => {
  it("maps all supported media labels into the four bundled folders", () => {
    expect(defaultCoverCategoryForMediaType("movie")).toBe("video")
    expect(defaultCoverCategoryForMediaType("episode")).toBe("video")
    expect(defaultCoverCategoryForMediaType("book")).toBe("book")
    expect(defaultCoverCategoryForMediaType("comic")).toBe("comic")
    expect(defaultCoverCategoryForMediaType("periodical")).toBe("article")
    expect(defaultCoverCategoryForMediaType("document")).toBe("article")
  })

  it("chooses a stable cover for an item and distributes comic seeds", () => {
    const first = defaultCoverPath("comic", "media-001")
    expect(first).toBe(defaultCoverPath("comic", "media-001"))
    expect(defaultCoverFiles("comic")).toHaveLength(3)
    const chosen = new Set(
      Array.from({ length: 32 }, (_, index) => defaultCoverPath("comic", `media-${index}`)),
    )
    expect(chosen.size).toBeGreaterThan(1)
  })

  it("never returns a remote URL", () => {
    for (const type of ["video", "book", "comic", "article"] as const) {
      expect(defaultCoverForMediaType(type, "safe-id")).toMatch(/^\/默认图片\//)
    }
  })
})
