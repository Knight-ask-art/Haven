// 自定义主题调色板的默认种子与颜色工具（Appearance Stage 1B）。
//
// 单一事实源仍是 index.css：`--background` 等 19 个 token 在 light / dark 下解析出的
// 最终颜色，就是这里的 DEFAULT_LIGHT_PALETTE / DEFAULT_DARK_PALETTE。用户从
// system/light/dark 切到 custom 时，调色板必须从一个**真实存在过的外观**出发，
// 而不是凭空造一套颜色——否则「切到自定义」会立刻把界面变成另一副样子。
//
// 这份镜像由 appearance-palette.test.ts 从 index.css 里逐 token 解析后对齐：
// index.css 改了 token，这里对不上就会失败。

import type { AppTheme, AppThemePalette } from "@/lib/ipc/settings-wire"

/**
 * 调色板字段的展示顺序与中文标签。
 *
 * 顺序与 label 只影响 UI；字段全集由 `AppThemePalette` 类型与 settings-wire 的
 * `guardAppThemePalette` 分别把关（19 个字段一个不少、一个不多）。
 */
export const PALETTE_FIELDS: ReadonlyArray<readonly [keyof AppThemePalette, string]> = [
  ["background", "背景"],
  ["foreground", "前景文字"],
  ["card", "卡片"],
  ["cardForeground", "卡片文字"],
  ["popover", "浮层"],
  ["popoverForeground", "浮层文字"],
  ["primary", "主色"],
  ["primaryForeground", "主色文字"],
  ["secondary", "次级面"],
  ["secondaryForeground", "次级文字"],
  ["muted", "弱化面"],
  ["mutedForeground", "弱化文字"],
  ["accent", "强调面"],
  ["accentForeground", "强调文字"],
  ["destructive", "危险色"],
  ["destructiveForeground", "危险文字"],
  ["border", "描边"],
  ["input", "输入框描边"],
  ["ring", "焦点环"],
]

/**
 * 运行时由**强调色**接管、因此不作为可编辑项的调色板字段（带展示标签）。
 *
 * 契约在 `appearance-runtime.ts` 的 `customThemeTokenEntries`：`theme=custom` 时
 * `--primary` 与 `--ring` 都被 `accentColor` 覆盖（调色板里这两个字段写什么都不生效）。
 * 界面因此不能把它们做成可编辑控件——一个改了没有任何效果的控件，比没有这个控件更糟。
 *
 * 字段仍然保留在 [`PALETTE_FIELDS`] 与 `AppThemePalette` 里：wire 守卫要求 19 个字段
 * 一个不少，而且「切到自定义」时的默认值仍然取自 index.css 的既有 token。
 */
export const PALETTE_DERIVED_FIELDS: ReadonlyArray<readonly [keyof AppThemePalette, string]> = [
  ["primary", "主色"],
  ["ring", "焦点环"],
]

/**
 * 用户真正能编辑的调色板字段（19 − 2）。取值器与「没有无效果控件」的回归测试共用这一份清单。
 */
export const PALETTE_EDITABLE_FIELDS: ReadonlyArray<readonly [keyof AppThemePalette, string]> =
  PALETTE_FIELDS.filter(
    ([field]) => !PALETTE_DERIVED_FIELDS.some(([derived]) => derived === field),
  )

/**
 * 默认浅色调色板 = index.css `:root` 下 19 个 token 的解析值。
 *
 * `border` / `input` 的既有取值带 alpha（`rgba(60, 60, 67, 0.16)`），因此是 8 位形态。
 */
export const DEFAULT_LIGHT_PALETTE: AppThemePalette = {
  background: "#ffffff",
  foreground: "#1d1d1f",
  card: "#ffffff",
  cardForeground: "#1d1d1f",
  popover: "#ffffff",
  popoverForeground: "#1d1d1f",
  primary: "#007aff",
  primaryForeground: "#ffffff",
  secondary: "#f5f5f7",
  secondaryForeground: "#1d1d1f",
  muted: "#f5f5f7",
  mutedForeground: "#6e6e73",
  accent: "#f5f5f7",
  accentForeground: "#1d1d1f",
  destructive: "#ff3b30",
  destructiveForeground: "#ffffff",
  border: "#3c3c4329",
  input: "#3c3c4329",
  ring: "#007aff",
}

/** 默认深色调色板 = index.css `.dark` 下 19 个 token 的解析值。 */
export const DEFAULT_DARK_PALETTE: AppThemePalette = {
  background: "#000000",
  foreground: "#f5f5f7",
  card: "#242426",
  cardForeground: "#f5f5f7",
  popover: "#242426",
  popoverForeground: "#f5f5f7",
  primary: "#0a84ff",
  primaryForeground: "#000000",
  secondary: "#1c1c1e",
  secondaryForeground: "#f5f5f7",
  muted: "#1c1c1e",
  mutedForeground: "#aeaeb2",
  accent: "#1c1c1e",
  accentForeground: "#f5f5f7",
  destructive: "#ff453a",
  destructiveForeground: "#000000",
  border: "#5454587a",
  input: "#5454587a",
  ring: "#0a84ff",
}

/** 默认强调色 = index.css `--ds-blue-500`（`--ring` 的浅色取值）。 */
export const DEFAULT_ACCENT_COLOR = "#007aff"

export type ThemePresetId = "ink" | "oatmeal" | "daiqing" | "chestnut"
export type PaletteColorRole = "background" | "card" | "foreground"

type ThemePaletteRecipe = {
  background: string
  foreground: string
  card: string
  secondary: string
  mutedForeground: string
  accentSurface: string
  border: string
}

export type ThemePreset = {
  id: ThemePresetId
  label: string
  description: string
  swatches: readonly [string, string, string]
  theme: AppTheme
}

function paletteFromRecipe(
  base: AppThemePalette,
  recipe: ThemePaletteRecipe,
  accentColor: string,
): AppThemePalette {
  return {
    ...base,
    background: recipe.background,
    foreground: recipe.foreground,
    card: recipe.card,
    cardForeground: recipe.foreground,
    popover: recipe.card,
    popoverForeground: recipe.foreground,
    primary: accentColor,
    primaryForeground: "#ffffff",
    secondary: recipe.secondary,
    secondaryForeground: recipe.foreground,
    muted: recipe.secondary,
    mutedForeground: recipe.mutedForeground,
    accent: recipe.accentSurface,
    accentForeground: recipe.foreground,
    border: recipe.border,
    input: recipe.border,
    ring: accentColor,
  }
}

function createThemePreset(
  id: ThemePresetId,
  label: string,
  description: string,
  accentColor: string,
  light: ThemePaletteRecipe,
  dark: ThemePaletteRecipe,
): ThemePreset {
  return {
    id,
    label,
    description,
    swatches: [light.background, light.card, accentColor],
    theme: {
      light: paletteFromRecipe(DEFAULT_LIGHT_PALETTE, light, accentColor),
      dark: paletteFromRecipe(DEFAULT_DARK_PALETTE, dark, accentColor),
      accentColor,
    },
  }
}

/** 面向用户的主题配色预设；每套同时包含浅色与深色取值。 */
export const THEME_PRESETS: readonly ThemePreset[] = [
  createThemePreset(
    "ink",
    "墨黑",
    "安静克制的中性色",
    "#5b6b5e",
    { background: "#f5f5f1", foreground: "#2c2e29", card: "#fcfcf8", secondary: "#e9eae4", mutedForeground: "#72766f", accentSurface: "#e2e6df", border: "#2c2e2926" },
    { background: "#171916", foreground: "#f0f1ec", card: "#222521", secondary: "#2b2f2a", mutedForeground: "#a9ada5", accentSurface: "#353b34", border: "#f0f1ec2b" },
  ),
  createThemePreset(
    "oatmeal",
    "燕麦",
    "柔和温暖的纸感浅色",
    "#8a6d4a",
    { background: "#f4efe6", foreground: "#383229", card: "#fcf8f1", secondary: "#ebe2d3", mutedForeground: "#786e5f", accentSurface: "#e9ddca", border: "#38322929" },
    { background: "#201b16", foreground: "#f1e9de", card: "#2b251e", secondary: "#382f25", mutedForeground: "#b9aa97", accentSurface: "#403729", border: "#f1e9de2b" },
  ),
  createThemePreset(
    "daiqing",
    "黛青",
    "带一点青意的沉静配色",
    "#557b75",
    { background: "#edf2ef", foreground: "#273532", card: "#fafcfb", secondary: "#dde8e3", mutedForeground: "#687a75", accentSurface: "#d9e6e1", border: "#27353224" },
    { background: "#17211f", foreground: "#e9f0ed", card: "#202d2a", secondary: "#293936", mutedForeground: "#a6b8b2", accentSurface: "#344642", border: "#e9f0ed2b" },
  ),
  createThemePreset(
    "chestnut",
    "秋栗",
    "温润沉稳的栗棕色",
    "#8a604e",
    { background: "#f4edea", foreground: "#3c2d28", card: "#fcf8f5", secondary: "#ebdeda", mutedForeground: "#7d6961", accentSurface: "#e7d9d3", border: "#3c2d2826" },
    { background: "#221a18", foreground: "#f2e8e2", card: "#2f2421", secondary: "#3b2d29", mutedForeground: "#b4a29b", accentSurface: "#44332e", border: "#f2e8e22b" },
  ),
]

/** 给设置草稿一份独立副本，避免表单状态与预设常量互相引用。 */
export function themePresetAppTheme(id: ThemePresetId): AppTheme {
  const preset = THEME_PRESETS.find((candidate) => candidate.id === id)
  if (!preset) throw new Error(`未知的主题配色：${id}`)
  return {
    accentColor: preset.theme.accentColor,
    light: { ...preset.theme.light },
    dark: { ...preset.theme.dark },
  }
}

/** 新建自定义主题的起点：两套真实调色板 + 强调色，全部来自 index.css。 */
export function defaultAppTheme(): AppTheme {
  return {
    light: { ...DEFAULT_LIGHT_PALETTE },
    dark: { ...DEFAULT_DARK_PALETTE },
    accentColor: DEFAULT_ACCENT_COLOR,
  }
}

/** 规范十六进制颜色：恰好 `#rrggbb` 或 `#rrggbbaa`（小写）。与 wire 守卫同一份规则。 */
const HEX_COLOR_PATTERN = /^#[0-9a-f]{6}([0-9a-f]{2})?$/

/** 是否为 wire 接受的规范颜色。 */
export function isPaletteHexColor(value: unknown): value is string {
  return typeof value === "string" && HEX_COLOR_PATTERN.test(value)
}

/**
 * 取 `<input type="color">` 能编辑的那 6 位（`#rrggbb`）。
 *
 * 颜色选择器无法表达 alpha，所以带 alpha 的既存取值（border/input）在这里只暴露
 * 它的 RGB 部分；alpha 由 `applyPaletteColorInput` 原样保留。
 */
export function paletteColorInputValue(value: string): string {
  return isPaletteHexColor(value) ? value.slice(0, 7) : "#000000"
}

/**
 * 把颜色选择器的 6 位结果写回字段：既存取值带 alpha 时沿用同一个 alpha 字节。
 *
 * 传进来的值不是规范 `#rrggbb` 时返回 null——宁可让控件不改动草稿，也不写一个
 * wire 守卫会拒绝的颜色进去。
 */
export function applyPaletteColorInput(previous: string, input: string): string | null {
  if (!/^#[0-9a-f]{6}$/.test(input)) return null
  const alpha = isPaletteHexColor(previous) && previous.length === 9 ? previous.slice(7) : ""
  return `${input}${alpha}`
}

/** 复制一份调色板并替换其中一个字段；字段值非法时返回原调色板（不制造坏数据）。 */
export function withPaletteField(
  palette: AppThemePalette,
  field: keyof AppThemePalette,
  value: string,
): AppThemePalette {
  if (!isPaletteHexColor(value)) return palette
  return { ...palette, [field]: value }
}

/** 以用户熟悉的角色编辑颜色，并同步同角色的相关表面，不暴露底层字段清单。 */
export function withPaletteRoleColor(
  palette: AppThemePalette,
  role: PaletteColorRole,
  input: string,
): AppThemePalette {
  const source = palette[role]
  const color = applyPaletteColorInput(source, input)
  if (!color) return palette
  switch (role) {
    case "background":
      return { ...palette, background: color }
    case "card":
      return { ...palette, card: color, popover: color }
    case "foreground":
      return { ...palette, foreground: color, cardForeground: color, popoverForeground: color }
  }
}

/**
 * 写入强调色，并把两套调色板里**由它接管**的两个字段一并改成同一个值。
 *
 * 运行时的契约是「`accentColor` 同时驱动 `--primary` 与 `--ring`」（见
 * `appearance-runtime.ts` 的 `customThemeTokenEntries`）。不把这两个字段一起改掉，
 * 存下来的自定义主题里就会留着一组永远不生效、只会在将来被误读的颜色。
 * 输入不是规范十六进制时返回原主题，调用方应丢弃这次输入。
 */
export function withAccentColor(theme: AppTheme, input: string): AppTheme {
  const accentColor = applyPaletteColorInput(theme.accentColor, input)
  if (!accentColor) return theme
  return {
    accentColor,
    light: withPaletteField(withPaletteField(theme.light, "primary", accentColor), "ring", accentColor),
    dark: withPaletteField(withPaletteField(theme.dark, "primary", accentColor), "ring", accentColor),
  }
}

/**
 * 把一份可能来自后端（也可能缺失/损坏）的自定义主题归一成可编辑的形态。
 *
 * 缺失即用契约默认值补齐，而不是把「还没创建过自定义主题」判成不可编辑——用户
 * 点「自定义」时必须立刻能拿到一套真实存在的调色板。
 */
export function normalizeAppTheme(theme: AppTheme | null | undefined): AppTheme {
  const fallback = defaultAppTheme()
  if (!theme) return fallback
  const normalizePalette = (
    palette: AppThemePalette | undefined,
    preset: AppThemePalette,
  ): AppThemePalette => {
    const source = palette ?? preset
    const result = { ...preset }
    for (const [field] of PALETTE_FIELDS) {
      const value = source[field]
      if (typeof value === "string") result[field] = value
    }
    return result
  }
  return {
    light: normalizePalette(theme.light, DEFAULT_LIGHT_PALETTE),
    dark: normalizePalette(theme.dark, DEFAULT_DARK_PALETTE),
    accentColor: isPaletteHexColor(theme.accentColor) ? theme.accentColor : DEFAULT_ACCENT_COLOR,
  }
}
