import { describe, expect, it } from "vitest"
import { isBookOrPeriodical, pickCardImage } from "./artwork-policy"
import type { WorkCardDto } from "@/lib/ipc/generated/wire"

function card(overrides: Partial<WorkCardDto>): WorkCardDto {
  return {
    workId: "w1",
    title: "t",
    originalTitle: null,
    description: null,
    categories: ["video"],
    availableMediaTypes: ["movie"],
    posterUri: "haven://artwork/poster",
    backdropUri: "haven://artwork/backdrop",
    releaseYear: 2024,
    ratingValue: null,
    ratingScale: null,
    favorite: false,
    progress: null,
    primaryAction: null,
    externalIds: [],
    ...overrides,
  } as WorkCardDto
}

describe("isBookOrPeriodical", () => {
  it("video uses keyframe", () => {
    expect(isBookOrPeriodical(card({ categories: ["video"], availableMediaTypes: ["movie"] }))).toBe(false)
  })
  it("comic uses keyframe", () => {
    expect(isBookOrPeriodical(card({ categories: ["comic"], availableMediaTypes: ["comic"] }))).toBe(false)
  })
  it("book uses poster", () => {
    expect(isBookOrPeriodical(card({ categories: ["book"], availableMediaTypes: ["book"] }))).toBe(true)
  })
  it("periodical uses poster", () => {
    expect(isBookOrPeriodical(card({ categories: ["periodical"], availableMediaTypes: ["document"] }))).toBe(true)
  })
})

describe("pickCardImage", () => {
  it("video picks backdrop over poster", () => {
    const c = card({ categories: ["video"], availableMediaTypes: ["movie"], posterUri: "haven://artwork/p", backdropUri: "haven://artwork/b" })
    // artworkRequestUri will convert to haven-resource, but both valid
    expect(pickCardImage(c)).toContain("b")
  })
  it("book picks poster even if backdrop exists", () => {
    const c = card({ categories: ["book"], availableMediaTypes: ["book"], posterUri: "haven://artwork/p", backdropUri: "haven://artwork/b" })
    expect(pickCardImage(c)).toContain("p")
    expect(pickCardImage(c)).not.toContain("b")
  })
  it("video fallback to poster when backdrop null", () => {
    const c = card({ categories: ["video"], availableMediaTypes: ["movie"], posterUri: "haven://artwork/p", backdropUri: null })
    expect(pickCardImage(c)).toContain("p")
  })
})
