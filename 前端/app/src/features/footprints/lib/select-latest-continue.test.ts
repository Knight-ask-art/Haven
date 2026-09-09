import { describe, expect, it } from "vitest"
import { selectLatestUnfinished } from "./select-latest-continue"
import type { MediaCardProps } from "@/components/ui/haven/MediaCard"

type ContinueCard = MediaCardProps & {
  completion?: "not_started" | "in_progress" | "completed" | "abandoned"
}

function card(
  id: string,
  progress?: number,
  completion?: ContinueCard["completion"],
): ContinueCard {
  return { id, title: id, imageUrl: `http://${id}`, progress, completion }
}

describe("selectLatestUnfinished", () => {
  it("empty returns null hero", () => {
    expect(selectLatestUnfinished([])).toEqual({ hero: null, rest: [] })
  })
  it("strict 0<x<100 picks first unfinished as hero", () => {
    const items = [card("a", 0), card("b", 45), card("c", 89)]
    const { hero, rest } = selectLatestUnfinished(items)
    expect(hero?.id).toBe("b")
    expect(rest.map((r) => r.id)).toEqual(["a", "c"])
  })
  it("ignores 0 and 100", () => {
    const items = [card("a", 0), card("b", 100), card("c", undefined)]
    const { hero, rest } = selectLatestUnfinished(items)
    expect(hero).toBeNull()
    expect(rest.length).toBe(3)
  })
  it("does not promote a completed item that retains a non-terminal percentage", () => {
    const items = [card("completed", 50, "completed"), card("reading", 20, "in_progress")]

    const { hero, rest } = selectLatestUnfinished(items)

    expect(hero?.id).toBe("reading")
    expect(rest.map((item) => item.id)).toEqual(["completed"])
  })
  it("only promotes in-progress items, never reset or abandoned items with retained percentages", () => {
    const items = [
      card("reset", 50, "not_started"),
      card("abandoned", 60, "abandoned"),
      card("reading", 20, "in_progress"),
    ]

    const { hero, rest } = selectLatestUnfinished(items)

    expect(hero?.id).toBe("reading")
    expect(rest.map((item) => item.id)).toEqual(["reset", "abandoned"])
  })
  it("rest preserves time order after removing hero", () => {
    const items = [card("a", 45), card("b", 55), card("c", 60)]
    const { hero, rest } = selectLatestUnfinished(items)
    expect(hero?.id).toBe("a")
    expect(rest.map((r) => r.id)).toEqual(["b", "c"])
  })
})
