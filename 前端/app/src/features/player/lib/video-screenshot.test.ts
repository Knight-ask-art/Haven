import { describe, expect, it } from "vitest"
import { isVideoScreenshotShortcut } from "./video-screenshot"

const event = (overrides: Partial<KeyboardEvent> = {}) => ({
  code: "KeyS",
  ctrlKey: true,
  shiftKey: true,
  altKey: false,
  metaKey: false,
  repeat: false,
  ...overrides,
})

describe("video screenshot shortcut", () => {
  it("accepts only the fixed Ctrl+Shift+S combination", () => {
    expect(isVideoScreenshotShortcut(event())).toBe(true)
    expect(isVideoScreenshotShortcut(event({ code: "KeyP" }))).toBe(false)
    expect(isVideoScreenshotShortcut(event({ ctrlKey: false }))).toBe(false)
    expect(isVideoScreenshotShortcut(event({ shiftKey: false }))).toBe(false)
    expect(isVideoScreenshotShortcut(event({ altKey: true }))).toBe(false)
    expect(isVideoScreenshotShortcut(event({ metaKey: true }))).toBe(false)
  })

  it("ignores browser key auto-repeat", () => {
    expect(isVideoScreenshotShortcut(event({ repeat: true }))).toBe(false)
  })
})
