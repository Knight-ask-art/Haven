import { describe, expect, it } from "vitest"
import { selectNextEpisodeId } from "./select-next-episode"

const episodes = [
  { id: "e1", number: "第01集", title: "一" },
  { id: "e2", number: "第02集", title: "二" },
  { id: "e3", number: "第03集", title: "三" },
] as const

describe("selectNextEpisodeId", () => {
  it("selects the next canonical item and stops at the end", () => {
    expect(selectNextEpisodeId(episodes, "e1")).toBe("e2")
    expect(selectNextEpisodeId(episodes, "e2")).toBe("e3")
    expect(selectNextEpisodeId(episodes, "e3")).toBeNull()
  })

  it("uses the canonical list supplied by the player, not a missing display projection", () => {
    expect(selectNextEpisodeId(episodes, "e2")).toBe("e3")
    expect(selectNextEpisodeId(undefined, "e1")).toBeNull()
    expect(selectNextEpisodeId(episodes, "missing")).toBeNull()
  })
})
