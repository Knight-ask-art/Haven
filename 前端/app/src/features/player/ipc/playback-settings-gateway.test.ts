import { describe, expect, it } from "vitest"
import { DEFAULT_PLAYBACK_SETTINGS, playbackRateToNumber } from "./playback-settings-gateway"

describe("playback settings gateway", () => {
  it("maps the closed playback-rate wire enum to player values", () => {
    expect(playbackRateToNumber("point_seven_five")).toBe(0.75)
    expect(playbackRateToNumber("one")).toBe(1)
    expect(playbackRateToNumber("one_point_two_five")).toBe(1.25)
    expect(playbackRateToNumber("one_point_five")).toBe(1.5)
    expect(playbackRateToNumber("two")).toBe(2)
  })

  it("keeps automatic next enabled in the safe default", () => {
    expect(DEFAULT_PLAYBACK_SETTINGS.autoNext).toBe(true)
  })
})
