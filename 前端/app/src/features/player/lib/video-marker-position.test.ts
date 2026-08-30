import { describe, expect, it } from "vitest"
import { videoSecondsToMilliseconds } from "./video-marker-position"

describe("videoSecondsToMilliseconds", () => {
  it("rounds fractional milliseconds to the nearest integer", () => {
    expect(videoSecondsToMilliseconds(1.2346)).toBe(1235)
  })

  it("clamps negative playback time to zero", () => {
    expect(videoSecondsToMilliseconds(-0.25)).toBe(0)
  })
})
