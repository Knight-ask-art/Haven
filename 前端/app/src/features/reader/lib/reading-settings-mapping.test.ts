import { describe, expect, it } from "vitest"
import type { ReadingSettingsValue } from "@/lib/ipc/settings-wire"
import {
  MAX_CUSTOM_FONT_FAMILY_LENGTH,
  READING_CUSTOM_BACKGROUND_FALLBACK,
  READING_CUSTOM_TEXT_FALLBACK,
  isDarkReadingTheme,
  resolveCustomFontFamilyCss,
  resolveReadingCustomAppearance,
  resolveReadingCustomColors,
  resolveReadingCustomRender,
  resolveReadingPresentation,
  sanitizeCustomFontFamilyName,
  sanitizeCustomReadingColor,
} from "./reading-settings-mapping"

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

const safeAppearance = { background: "#101418", text: "#e8e6e1", fontFamily: "思源宋体" }

describe("custom reading colour sanitising", () => {
  it("accepts only the contract's #rrggbb form and normalises the case", () => {
    expect(sanitizeCustomReadingColor("#A1B2C3")).toBe("#a1b2c3")
    expect(sanitizeCustomReadingColor("  #0f0f11  ")).toBe("#0f0f11")
  })

  it("treats every other input as unset instead of handing it to CSS", () => {
    expect(sanitizeCustomReadingColor("#fff")).toBeNull()
    expect(sanitizeCustomReadingColor("#12345")).toBeNull()
    expect(sanitizeCustomReadingColor("red")).toBeNull()
    expect(sanitizeCustomReadingColor("rgb(1,2,3)")).toBeNull()
    expect(sanitizeCustomReadingColor("")).toBeNull()
    expect(sanitizeCustomReadingColor(null)).toBeNull()
    expect(sanitizeCustomReadingColor(undefined)).toBeNull()
  })
})

describe("custom reading font family sanitising", () => {
  it("keeps plain installed family names, including CJK ones", () => {
    expect(sanitizeCustomFontFamilyName("Source Han Serif SC")).toBe("Source Han Serif SC")
    expect(sanitizeCustomFontFamilyName("  思源宋体  ")).toBe("思源宋体")
  })

  it("rejects names that could escape the font-family declaration", () => {
    expect(sanitizeCustomFontFamilyName('Arial"; color: red')).toBeNull()
    expect(sanitizeCustomFontFamilyName("Arial\\27 serif")).toBeNull()
    expect(sanitizeCustomFontFamilyName("Arial\nSerif")).toBeNull()
    expect(sanitizeCustomFontFamilyName("Arial;}")).toBeNull()
    expect(sanitizeCustomFontFamilyName("Arial(url)")).toBeNull()
    expect(sanitizeCustomFontFamilyName("Arial<script>")).toBeNull()
  })

  it("rejects empty and over-long names instead of truncating them", () => {
    expect(sanitizeCustomFontFamilyName("")).toBeNull()
    expect(sanitizeCustomFontFamilyName("   ")).toBeNull()
    expect(sanitizeCustomFontFamilyName("A".repeat(MAX_CUSTOM_FONT_FAMILY_LENGTH))).toHaveLength(MAX_CUSTOM_FONT_FAMILY_LENGTH)
    expect(sanitizeCustomFontFamilyName("A".repeat(MAX_CUSTOM_FONT_FAMILY_LENGTH + 1))).toBeNull()
    expect(sanitizeCustomFontFamilyName(null)).toBeNull()
  })
})

describe("custom reading colours", () => {
  it("renders nothing for the preset themes", () => {
    for (const theme of ["paper", "warm", "slate", "dark", "sepia", "eyeCare"] as const) {
      expect(resolveReadingCustomColors(theme, safeAppearance)).toBeNull()
    }
  })

  it("renders the stored colours for the custom theme", () => {
    expect(resolveReadingCustomColors("custom", safeAppearance)).toEqual({
      backgroundColor: "#101418",
      color: "#e8e6e1",
    })
  })

  it("falls back per channel so one bad value cannot drop the other", () => {
    expect(resolveReadingCustomColors("custom", { ...safeAppearance, background: null })).toEqual({
      backgroundColor: READING_CUSTOM_BACKGROUND_FALLBACK,
      color: "#e8e6e1",
    })
    expect(resolveReadingCustomColors("custom", { ...safeAppearance, text: null })).toEqual({
      backgroundColor: "#101418",
      color: READING_CUSTOM_TEXT_FALLBACK,
    })
    expect(resolveReadingCustomColors("custom", { background: null, text: null, fontFamily: null })).toEqual({
      backgroundColor: READING_CUSTOM_BACKGROUND_FALLBACK,
      color: READING_CUSTOM_TEXT_FALLBACK,
    })
  })
})

describe("custom reading font", () => {
  it("only quotes the stored family for the custom font option", () => {
    expect(resolveCustomFontFamilyCss("custom", safeAppearance)).toBe('"思源宋体", sans-serif')
    expect(resolveCustomFontFamilyCss("serif", safeAppearance)).toBeNull()
  })

  it("leaves the font to the preset classes when nothing was stored", () => {
    expect(resolveCustomFontFamilyCss("custom", { background: null, text: null, fontFamily: null })).toBeNull()
  })
})

describe("reading shell brightness", () => {
  it("keeps the fixed dark themes dark and the light themes light", () => {
    expect(isDarkReadingTheme("slate", safeAppearance)).toBe(true)
    expect(isDarkReadingTheme("dark", safeAppearance)).toBe(true)
    for (const theme of ["paper", "warm", "sepia", "eyeCare"] as const) {
      expect(isDarkReadingTheme(theme, safeAppearance)).toBe(false)
    }
  })

  it("judges the custom theme from its background luminance", () => {
    expect(isDarkReadingTheme("custom", { background: "#000000", text: null, fontFamily: null })).toBe(true)
    expect(isDarkReadingTheme("custom", { background: "#0f0f11", text: null, fontFamily: null })).toBe(true)
    expect(isDarkReadingTheme("custom", { background: "#ffffff", text: null, fontFamily: null })).toBe(false)
    expect(isDarkReadingTheme("custom", { background: READING_CUSTOM_BACKGROUND_FALLBACK, text: null, fontFamily: null })).toBe(false)
  })

  it("treats an unset or drifted custom background as the light default", () => {
    expect(isDarkReadingTheme("custom", { background: null, text: null, fontFamily: null })).toBe(false)
    // 未经 sanitize 的值（后端漂移）不得进入亮度计算。
    expect(isDarkReadingTheme("custom", { background: "not-a-colour", text: null, fontFamily: null })).toBe(false)
  })
})

describe("custom reading render", () => {
  it("combines colours, font, and shell brightness for the custom theme", () => {
    expect(resolveReadingCustomRender("custom", "custom", safeAppearance)).toEqual({
      colors: { backgroundColor: "#101418", color: "#e8e6e1" },
      fontFamilyCss: '"思源宋体", sans-serif',
      darkShell: true,
    })
  })

  it("leaves the preset themes on their class-based rendering", () => {
    expect(resolveReadingCustomRender("warm", "serif", safeAppearance)).toEqual({
      colors: null,
      fontFamilyCss: null,
      darkShell: false,
    })
  })
})

describe("resolveReadingPresentation custom fields", () => {
  it("sanitises the stored custom appearance before the readers see it", () => {
    const presentation = resolveReadingPresentation({
      ...base,
      theme: "custom",
      fontFamily: "custom",
      customBackground: "#AABBCC",
      customText: "not-a-colour",
      customFontFamily: 'Bad"; font',
    }, false)
    expect(presentation.theme).toBe("custom")
    expect(presentation.fontFamily).toBe("custom")
    expect(presentation.customBackground).toBe("#aabbcc")
    expect(presentation.customText).toBeNull()
    expect(presentation.customFontFamily).toBeNull()
    // 自定义外观不影响其余排版取值。
    expect(presentation.fontSizePx).toBe(18)
    expect(presentation.lineHeight).toBe(1.85)
    expect(presentation.contentWidthPx).toBe(700)
  })

  it("keeps a preset theme on its own rendering even with custom values stored", () => {
    const settings: ReadingSettingsValue = {
      ...base,
      customBackground: "#000000",
      customText: "#ffffff",
      customFontFamily: "思源宋体",
    }
    const presentation = resolveReadingPresentation(settings, false)
    expect(presentation.theme).toBe("warm")
    expect(presentation.fontFamily).toBe("serif")
    expect(resolveReadingCustomRender(presentation.theme, presentation.fontFamily, resolveReadingCustomAppearance(settings))).toEqual({
      colors: null,
      fontFamilyCss: null,
      darkShell: false,
    })
  })
})
