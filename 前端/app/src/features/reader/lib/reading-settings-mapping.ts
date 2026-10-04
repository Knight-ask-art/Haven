import type { ReadingPaginationWire, ReadingSettingsValue } from "@/lib/ipc/settings-wire"

export type ReaderTheme = "paper" | "warm" | "slate" | "dark" | "sepia" | "eyeCare" | "custom"
export type ReaderFontFamily = "sans" | "serif" | "kai" | "heiti" | "fangsong" | "mianfei" | "custom"
export type ReaderPaginationMode = ReadingPaginationWire

export interface ReadingPresentation {
  theme: ReaderTheme
  fontFamily: ReaderFontFamily
  /** 已通过安全校验的字体族名；仅 `fontFamily=custom` 时有意义。 */
  customFontFamily: string | null
  fontSizePx: number
  lineHeight: number
  contentWidthPx: number
  fontWeight: number
  letterSpacing: number
  /** 已通过 `#rrggbb` 校验的背景色；仅 `theme=custom` 时有意义。 */
  customBackground: string | null
  /** 已通过 `#rrggbb` 校验的文字色；仅 `theme=custom` 时有意义。 */
  customText: string | null
  systemAuto: boolean
  pagination: ReaderPaginationMode
}

/**
 * 自定义阅读外观的安全取值。
 *
 * Settings 的 reading 分区把自定义配色和自定义字体族保存为自由文本，只有通过
 * 这里校验的值才允许进入 Reader 渲染路径。非法值一律按「未设置」处理，由调用方
 * 回退到该主题的安全默认，而不是把原始字符串直接拼进 CSS。
 */
export interface ReadingCustomAppearance {
  background: string | null
  text: string | null
  fontFamily: string | null
}

/** 自定义字体族名的长度上限；超长输入按无效处理，不截断成另一个字体名。 */
export const MAX_CUSTOM_FONT_FAMILY_LENGTH = 64

/** 自定义主题未设置或取值非法时的安全默认（与既有 custom 主题观感一致）。 */
export const READING_CUSTOM_BACKGROUND_FALLBACK = "#f5efe3"
export const READING_CUSTOM_TEXT_FALLBACK = "#3c332b"

/** 两个文本阅读器共用的主题样式，供设置预览也从同一事实源读取。 */
export const READING_THEME_PRESENTATION: Record<ReaderTheme, { className: string; background: string; color: string }> = {
  paper: { className: "bg-[#fcfcfc] text-[#1d1d1f]", background: "#fcfcfc", color: "#1d1d1f" },
  warm: { className: "bg-[#f5efe3] text-[#3c332b]", background: "#f5efe3", color: "#3c332b" },
  slate: { className: "bg-[#292a2d] text-[#e3e3e8]", background: "#292a2d", color: "#e3e3e8" },
  dark: { className: "bg-[#0f0f11] text-[#d4d4d8]", background: "#0f0f11", color: "#d4d4d8" },
  sepia: { className: "bg-[#f4ecd8] text-[#5b4636]", background: "#f4ecd8", color: "#5b4636" },
  eyeCare: { className: "bg-[#cce8cc] text-[#2e4a2e]", background: "#cce8cc", color: "#2e4a2e" },
  custom: { className: "bg-[#f5efe3] text-[#3c332b]", background: READING_CUSTOM_BACKGROUND_FALLBACK, color: READING_CUSTOM_TEXT_FALLBACK },
}

/** 全局字体设置在图书、文章与设置预览中的统一呈现。 */
export const READING_FONT_CLASSES: Record<ReaderFontFamily, string> = {
  sans: "font-sans",
  serif: "font-serif",
  kai: "font-serif italic",
  heiti: "font-sans font-bold",
  fangsong: "font-serif",
  mianfei: "font-sans",
  custom: "font-sans",
}

const HEX_COLOR_PATTERN = /^#[0-9a-fA-F]{6}$/
// 控制字符、引号、反斜杠、括号和 CSS 语句分隔符都可能让字体名逃出 font-family
// 声明；这些字符在真实字体族名里不会出现，因此直接判为无效。
const UNSAFE_FONT_FAMILY_PATTERN = /[\u0000-\u001F\u007F-\u009F"'\\;{}<>()\u2028\u2029]/

// 与白色文字对比更强的临界相对亮度：(L+0.05)^2 = 1.05 × 0.05 → L ≈ 0.1791。
// 低于该值时自定义背景按深色阅读外壳处理，高于该值按浅色处理。
const DARK_BACKGROUND_LUMINANCE = 0.1791

/** 校验并规范化自定义色值；只接受契约允许的 `#rrggbb`，其余一律视为未设置。 */
export function sanitizeCustomReadingColor(raw: string | null | undefined): string | null {
  if (typeof raw !== "string") return null
  const value = raw.trim()
  return HEX_COLOR_PATTERN.test(value) ? value.toLowerCase() : null
}

/**
 * 校验自定义字体族名。这里只接受**本机已安装字体族名**，不涉及字体文件导入：
 * 名称最终作为 CSS `font-family` 的单个族名使用，因此必须排除能逃出该声明的字符。
 */
export function sanitizeCustomFontFamilyName(raw: string | null | undefined): string | null {
  if (typeof raw !== "string") return null
  const value = raw.trim()
  if (value.length === 0 || value.length > MAX_CUSTOM_FONT_FAMILY_LENGTH) return null
  return UNSAFE_FONT_FAMILY_PATTERN.test(value) ? null : value
}

/** 提取 persisted Reading 设置里可安全使用的自定义外观值。 */
export function resolveReadingCustomAppearance(settings: ReadingSettingsValue): ReadingCustomAppearance {
  return {
    background: sanitizeCustomReadingColor(settings.customBackground),
    text: sanitizeCustomReadingColor(settings.customText),
    fontFamily: sanitizeCustomFontFamilyName(settings.customFontFamily),
  }
}

export interface ReadingCustomColors {
  backgroundColor: string
  color: string
}

/**
 * 自定义配色的实际渲染值：只有 `theme=custom` 才生效。两个颜色各自回退，
 * 一边缺失或非法不会连带丢掉另一边的有效值；其余主题返回 null，继续由主题类名渲染。
 */
export function resolveReadingCustomColors(
  theme: ReaderTheme,
  appearance: ReadingCustomAppearance,
): ReadingCustomColors | null {
  if (theme !== "custom") return null
  return {
    backgroundColor: appearance.background ?? READING_CUSTOM_BACKGROUND_FALLBACK,
    color: appearance.text ?? READING_CUSTOM_TEXT_FALLBACK,
  }
}

/**
 * 自定义字体族的 CSS 值：只有 `fontFamily=custom` 且字体名通过校验时返回。
 * 名称用双引号包成单个族名（校验已排除引号），并保留通用兜底，避免未安装时
 * 退回浏览器默认字体。
 */
export function resolveCustomFontFamilyCss(
  fontFamily: ReaderFontFamily,
  appearance: ReadingCustomAppearance,
): string | null {
  if (fontFamily !== "custom" || !appearance.fontFamily) return null
  return `"${appearance.fontFamily}", sans-serif`
}

/** 相对亮度（WCAG 2.x 定义），用于判断自定义背景应配深色还是浅色阅读外壳。 */
function relativeLuminance(hexColor: string): number {
  const channels = [1, 3, 5].map((offset) => {
    const srgb = Number.parseInt(hexColor.slice(offset, offset + 2), 16) / 255
    return srgb <= 0.04045 ? srgb / 12.92 : ((srgb + 0.055) / 1.055) ** 2.4
  })
  return 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2]
}

/**
 * 阅读外壳（页头、抽屉、工具面板）是否使用深色样式。固定深色主题恒为真；
 * 自定义主题按背景色亮度判断，未设置或非法时按浅色安全默认处理。
 */
export function isDarkReadingTheme(theme: ReaderTheme, appearance: ReadingCustomAppearance): boolean {
  if (theme === "slate" || theme === "dark") return true
  if (theme !== "custom") return false
  const background = sanitizeCustomReadingColor(appearance.background)
  if (!background) return false
  return relativeLuminance(background) < DARK_BACKGROUND_LUMINANCE
}

/** Reader 真正需要的自定义外观渲染结果（根容器配色 + 正文字体 + 外壳明暗）。 */
export interface ReadingCustomRender {
  /** 根容器的实际配色；仅 `theme=custom` 时非 null，其余主题继续用主题类名。 */
  colors: ReadingCustomColors | null
  /** 正文元素的 `font-family` 值；仅 `fontFamily=custom` 且字体名有效时非 null。 */
  fontFamilyCss: string | null
  /** 阅读外壳（页头/抽屉/工具面板）是否使用深色样式。 */
  darkShell: boolean
}

/**
 * 单一入口：把 Reader 当前状态里的 theme/fontFamily 与设置里的自定义取值组合成
 * 可直接落到 DOM 的渲染值。两个文本阅读器共用它，避免各自组装时对回退取值和
 * 外壳明暗的判断出现分歧。
 */
export function resolveReadingCustomRender(
  theme: ReaderTheme,
  fontFamily: ReaderFontFamily,
  appearance: ReadingCustomAppearance,
): ReadingCustomRender {
  return {
    colors: resolveReadingCustomColors(theme, appearance),
    fontFamilyCss: resolveCustomFontFamilyCss(fontFamily, appearance),
    darkShell: isDarkReadingTheme(theme, appearance),
  }
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
  const custom = resolveReadingCustomAppearance(settings)
  return {
    theme: resolveReadingTheme(settings.theme, prefersDark, settings.systemAuto),
    fontFamily: settings.fontFamily,
    customFontFamily: custom.fontFamily,
    fontSizePx: resolveReadingFontSize(settings.fontSize),
    lineHeight: resolveReadingLineHeight(settings.lineHeight),
    contentWidthPx: resolveReadingContentWidth(settings.contentWidth),
    fontWeight: resolveReadingFontWeight(settings.fontWeight),
    letterSpacing: resolveReadingLetterSpacing(settings.letterSpacing),
    customBackground: custom.background,
    customText: custom.text,
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
