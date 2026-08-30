import { describe, expect, it } from "vitest"
import type { ReadingSettingsValue } from "@/lib/ipc/settings-wire"
import { resolveReadingPresentation } from "./reading-settings-mapping"

const base: ReadingSettingsValue = {
  section: "reading",
  fontFamily: "serif",
  customFontFamily: null,
  fontSize: "medium",
  lineHeight: "comfortable",
  contentWidth: "medium",
  theme: "warm",
  customBackground: null,
  customText: null,
  fontWeight: "regular",
  letterSpacing: "normal",
  systemAuto: true,
  pagination: "scroll",
}

describe("reading-settings-mapping", () => {
  it("maps the default global preference to the existing reader presentation", () => {
    expect(resolveReadingPresentation(base, false)).toEqual({
      theme: "warm",
      fontFamily: "serif",
      customFontFamily: null,
      fontSizePx: 18,
      lineHeight: 1.85,
      contentWidthPx: 700,
      fontWeight: 400,
      letterSpacing: 0,
      customBackground: null,
      customText: null,
      systemAuto: true,
      pagination: "scroll",
    })
  })

  it("maps every supported size, line-height, and width option", () => {
    expect(resolveReadingPresentation({ ...base, fontSize: "small", lineHeight: "compact", contentWidth: "narrow" }, false)).toMatchObject({
      fontSizePx: 16,
      lineHeight: 1.65,
      contentWidthPx: 620,
    })
    expect(resolveReadingPresentation({ ...base, fontSize: "large", lineHeight: "airy", contentWidth: "wide" }, false)).toMatchObject({
      fontSizePx: 21,
      lineHeight: 2.05,
      contentWidthPx: 820,
    })
  })

  it("resolves system theme from the supplied operating-system preference", () => {
    expect(resolveReadingPresentation({ ...base, theme: "system" }, true).theme).toBe("dark")
    expect(resolveReadingPresentation({ ...base, theme: "system" }, false).theme).toBe("warm")
    expect(resolveReadingPresentation({ ...base, theme: "paper" }, true).theme).toBe("paper")
    expect(resolveReadingPresentation({ ...base, theme: "dark" }, false).theme).toBe("dark")
  })

  it("keeps the supported font family values, including Kai", () => {
    expect(resolveReadingPresentation({ ...base, fontFamily: "sans" }, false).fontFamily).toBe("sans")
    expect(resolveReadingPresentation({ ...base, fontFamily: "kai" }, false).fontFamily).toBe("kai")
  })

  it("maps the pagination mode and keeps legacy snapshots on scroll", () => {
    expect(resolveReadingPresentation({ ...base, pagination: "paginated" }, false).pagination).toBe("paginated")
    expect(resolveReadingPresentation({ ...base, pagination: "double" }, false).pagination).toBe("double")
    const legacy = { ...base }
    delete legacy.pagination
    expect(resolveReadingPresentation(legacy, false).pagination).toBe("scroll")
  })
})
