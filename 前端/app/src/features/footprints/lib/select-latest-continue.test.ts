import { describe, expect, it } from "vitest"
import { selectLatestUnfinished } from "./select-latest-continue"
import type { MediaCardProps } from "@/components/ui/haven/MediaCard"

function card(id: string, progress?: number): MediaCardProps {
  return { id, title: id, imageUrl: `http://${id}`, progress }
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
  it("rest preserves time order after removing hero", () => {
    const items = [card("a", 45), card("b", 55), card("c", 60)]
    const { hero, rest } = selectLatestUnfinished(items)
    expect(hero?.id).toBe("a")
    expect(rest.map((r) => r.id)).toEqual(["b", "c"])
  })
})
