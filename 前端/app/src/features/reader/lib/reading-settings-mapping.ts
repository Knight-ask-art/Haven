import type { ReadingPaginationWire, ReadingSettingsValue } from "@/lib/ipc/settings-wire"

export type ReaderTheme = "paper" | "warm" | "slate" | "dark" | "sepia" | "eyeCare" | "custom"
export type ReaderFontFamily = "sans" | "serif" | "kai" | "heiti" | "fangsong" | "mianfei" | "custom"
export type ReaderPaginationMode = ReadingPaginationWire

export interface ReadingPresentation {
  theme: ReaderTheme
  fontFamily: ReaderFontFamily
  customFontFamily: string | null
  fontSizePx: number
  lineHeight: number
  contentWidthPx: number
  fontWeight: number
  letterSpacing: number
  customBackground: string | null
  customText: string | null
  systemAuto: boolean
  pagination: ReaderPaginationMode
}

/**
 * Map the persisted global Reading preference to the two text-reader renderers.
 * `prefersDark` is sampled by the caller so this pure function stays DOM-free and
 * can be reused by EPUB/TXT/Markdown/article readers and tested in isolation.
 */
export function resolveReadingPresentation(
  settings: ReadingSettingsValue,
  prefersDark: boolean,
): ReadingPresentation {
  return {
    theme: resolveReadingTheme(settings.theme, prefersDark, settings.systemAuto),
    fontFamily: settings.fontFamily,
    customFontFamily: settings.customFontFamily ?? null,
    fontSizePx: resolveReadingFontSize(settings.fontSize),
    lineHeight: resolveReadingLineHeight(settings.lineHeight),
    contentWidthPx: resolveReadingContentWidth(settings.contentWidth),
    fontWeight: resolveReadingFontWeight(settings.fontWeight),
    letterSpacing: resolveReadingLetterSpacing(settings.letterSpacing),
    customBackground: settings.customBackground ?? null,
    customText: settings.customText ?? null,
    systemAuto: settings.systemAuto,
    pagination: settings.pagination ?? "scroll",
  }
}

function resolveReadingTheme(
  theme: ReadingSettingsValue["theme"],
  prefersDark: boolean,
  systemAuto: boolean,
): ReaderTheme {
  if (theme === "custom") return "custom"
  if (theme === "sepia") return "sepia"
  if (theme === "eyeCare") return "eyeCare"
  if (theme === "slate") return "slate"
  if (theme === "paper") return "paper"
  if (theme === "dark") return "dark"
  if (theme === "system") {
    if (!systemAuto) return "warm"
    return prefersDark ? "dark" : "warm"
  }
  return "warm"
}

function resolveReadingFontWeight(weight: ReadingSettingsValue["fontWeight"]): number {
  if (weight === "light") return 300
  if (weight === "medium") return 500
  if (weight === "semibold") return 600
  if (weight === "bold") return 700
  return 400
}

function resolveReadingLetterSpacing(spacing: ReadingSettingsValue["letterSpacing"]): number {
  if (spacing === "tight") return -0.02
  if (spacing === "relaxed") return 0.06
  if (spacing === "loose") return 0.12
  return 0
}

function resolveReadingFontSize(size: ReadingSettingsValue["fontSize"]): number {
  if (size === "small") return 16
  if (size === "large") return 21
  return 18
}

function resolveReadingLineHeight(lineHeight: ReadingSettingsValue["lineHeight"]): number {
  if (lineHeight === "compact") return 1.65
  if (lineHeight === "airy") return 2.05
  return 1.85
}

function resolveReadingContentWidth(width: ReadingSettingsValue["contentWidth"]): number {
  if (width === "narrow") return 620
  if (width === "wide") return 820
  return 700
}
