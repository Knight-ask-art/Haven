// 外观运行时投影（Appearance Stage 1B）。
//
// 事实边界：Rust + SQLite 仍是外观设置的唯一事实源。本模块只读取 settings runtime
// snapshot 里已经过 guard 的外观值，并把它投影到当前 WebView：
//   - 自定义调色板 → documentElement 上的 inline CSS token；
//   - 自定义字体 → FontFace + document.fonts + 一个 CSS 变量；
//   - 壁纸 → 供 AppShell 渲染非交互背景层的受控 URI。
//
// 三条硬边界，越界即回退到「无」而不是猜测：
//   1. 不写 localStorage，也不持有第二份持久化状态，投影只活在当前渲染进程里；
//   2. 只接受 appearanceAssetResourceUri 产出的受控 URI（不透明资产 ID → 后端受控
//      存储），绝不拼接任意 path/url；
//   3. 资产必须由 appearanceAssetsList 报告为 `kind` 匹配且 `state=validated`。
//      displayName 是用户可见的展示名，永远不参与 CSS 标识。
//
// 清理契约：本模块导出的 apply* 都返回清理函数，调用后必须回到 index.css 的默认
// 主题——切回 system/light/dark、切换资产或卸载 App Shell 时都不留残留。

import { useCallback, useEffect, useMemo, useState, useSyncExternalStore } from "react"
import { controlledResourceUri } from "@/lib/artwork-url"
import type {
  AppearanceAssetKindWire,
  AppearanceAssetsListRequestWire,
  AppearanceAssetsWire,
  AppearanceSettingsValue,
  AppThemePalette,
  WallpaperSelection,
} from "@/lib/ipc/settings-wire"
import { guardAppTheme, guardWallpaperSelection } from "@/lib/ipc/settings-wire"
import { appearanceAssetResourceUri, appearanceGateway } from "../ipc/appearance-gateway"
import { uiFontCssStack } from "./appearance-fonts"
import { isPaletteHexColor } from "./appearance-palette"

/** 切换外观或卸载时撤销本次投影。 */
export type AppearanceCleanup = () => void

// ---------- 系统偏好（明暗 / 减少动效） ----------

function systemMediaQuery(query: string): MediaQueryList | null {
  return typeof window !== "undefined" && typeof window.matchMedia === "function"
    ? window.matchMedia(query)
    : null
}

/** 系统当前的深色偏好；没有 matchMedia 的环境（Node / 旧 WebView）按浅色处理。 */
export function systemPrefersDark(): boolean {
  return systemMediaQuery("(prefers-color-scheme: dark)")?.matches === true
}

/** 系统当前的减少动效偏好。 */
export function systemPrefersReducedMotion(): boolean {
  return systemMediaQuery("(prefers-reduced-motion: reduce)")?.matches === true
}

/** 订阅系统偏好变化；返回取消订阅函数（环境不支持订阅时是空操作）。 */
export function subscribeSystemPreference(query: string, listener: () => void): AppearanceCleanup {
  const media = systemMediaQuery(query)
  if (!media?.addEventListener) return () => undefined
  media.addEventListener("change", listener)
  return () => media.removeEventListener("change", listener)
}

/** React 侧读取系统偏好；服务端/无 matchMedia 时恒定 false（安全默认）。 */
export function useSystemPreference(query: string): boolean {
  const subscribe = useCallback(
    (listener: () => void) => subscribeSystemPreference(query, listener),
    [query],
  )
  const read = useCallback(() => systemMediaQuery(query)?.matches === true, [query])
  return useSyncExternalStore(subscribe, read, () => false)
}

// ---------- 自定义调色板 → 既有 CSS token ----------

/**
 * 19 个调色板字段与 index.css 既有 token 的一一对应。
 *
 * 这份清单就是自定义主题能接管的 token 全集：写入按它遍历，清理也按它遍历，
 * 因此不存在「写进去却清不掉」的 token。
 */
const PALETTE_TOKEN_MAP: ReadonlyArray<readonly [keyof AppThemePalette, string]> = [
  ["background", "--background"],
  ["foreground", "--foreground"],
  ["card", "--card"],
  ["cardForeground", "--card-foreground"],
  ["popover", "--popover"],
  ["popoverForeground", "--popover-foreground"],
  ["primary", "--primary"],
  ["primaryForeground", "--primary-foreground"],
  ["secondary", "--secondary"],
  ["secondaryForeground", "--secondary-foreground"],
  ["muted", "--muted"],
  ["mutedForeground", "--muted-foreground"],
  ["accent", "--accent"],
  ["accentForeground", "--accent-foreground"],
  ["destructive", "--destructive"],
  ["destructiveForeground", "--destructive-foreground"],
  ["border", "--border"],
  ["input", "--input"],
  ["ring", "--ring"],
]

/** 全部可被自定义主题接管的 token 名；清理时逐条 removeProperty。 */
export const CUSTOM_THEME_TOKEN_NAMES: readonly string[] = PALETTE_TOKEN_MAP.map(
  ([, token]) => token,
)

/**
 * 计算本次应写入的 inline token。
 *
 * `theme !== "custom"`、调色板缺失或未通过 guard 时返回空表——调用方据此清空全部
 * inline token 回到默认主题，不会残留上一套自定义颜色。
 *
 * `accentColor` 是独立于调色板的强调色（与调色板里的 `accent` 不是同一件事），
 * 它同时驱动 `--primary` 与焦点环 `--ring`。
 */
export function customThemeTokenEntries(
  value: AppearanceSettingsValue,
  prefersDark: boolean,
): Record<string, string> {
  if (value.theme !== "custom" || !guardAppTheme(value.customTheme)) return {}
  const theme = value.customTheme
  const palette = prefersDark ? theme.dark : theme.light
  const entries: Record<string, string> = {}
  for (const [field, token] of PALETTE_TOKEN_MAP) entries[token] = palette[field]
  entries["--primary"] = theme.accentColor
  entries["--ring"] = theme.accentColor
  return entries
}

function themeRoot(): HTMLElement | null {
  return typeof document === "undefined" ? null : document.documentElement
}

/**
 * 第二组 token：浮动 Dock 与首页面板的**材质面**。
 *
 * 它们不在 `PALETTE_TOKEN_MAP` 里，因为调色板的 19 个字段是「语义色」，而这一组是
 * 「在这套语义色之上再叠一层固定 alpha 的材质」。Tailwind 对值是裸 `var(...)` 的颜色
 * 会整条丢弃带 `/NN` 修饰符的工具类，所以 alpha 只能写进取值本身（与 `--border` 同一
 * 个惯例），这组 token 就是那些取值的落点（见 index.css 的「外观运行时面」）。
 *
 * 每一个都取自调色板里语义最接近的字段：浮动底 ← background、抬升面 ← popover、
 * 描边 ← border、静止文字 ← mutedForeground、首页面板底 ← background。
 *
 * 与 19 个 token 共享同一条清理契约：`clearCustomThemeTokens` 同时清掉两组，因此
 * 不存在「写进去却清不掉」的 token。
 */
export const APPEARANCE_SURFACE_TOKEN_NAMES: readonly string[] = [
  "--haven-floating-surface",
  "--haven-floating-surface-raised",
  "--haven-floating-border",
  "--haven-floating-idle",
  "--haven-floating-idle-hover",
  "--haven-home-surface",
]

/**
 * 浮动 Dock 材质的 alpha 字节。
 *
 * 取自 index.css 里这几套默认材质的实测 alpha：浅色底的 86%、深色底的 85%，
 * 以及静止项悬停蒙版的 5% / 10%。自定义主题沿用同一份 alpha：用户换的是颜色，
 * 不是材质的通透度。
 */
const FLOATING_SURFACE_ALPHA = { light: "db", dark: "d9" } as const
/** 抬升材质（「更多」菜单）的 alpha 字节：两套主题都是 90%（`bg-white/90` / `zinc-900/90`）。 */
const FLOATING_SURFACE_RAISED_ALPHA = "e6"
/** 悬停蒙版：浅色 `black/5`、深色 `white/10`，都是「当前主题前景色压一层」。 */
const FLOATING_IDLE_HOVER_ALPHA = { light: "0d", dark: "1a" } as const

/** 换掉颜色自带的 alpha（没有就补一个）；非规范十六进制返回 null，绝不拼一个坏颜色出去。 */
function withAlphaByte(color: string, alphaByte: string): string | null {
  if (!isPaletteHexColor(color)) return null
  return `${color.slice(0, 7)}${alphaByte}`
}

/**
 * 材质面 token 的取值：全部来自当前生效的那套调色板。
 *
 * 与 19 个 token 一样，`theme !== "custom"` 或调色板未通过 guard 时返回空表——调用方
 * 据此清空，让 index.css 里那两套默认材质接管，默认外观一个像素都不变。
 *
 * 只有浮动材质、抬升材质与悬停蒙版需要额外叠 alpha（它们本来就是半透明的），其余
 * （描边、静止文字、首页面板底）直接沿用调色板里规范取值。
 *
 * 悬停蒙版取的是 `foreground` 而不是某个「面」色：它是一个压在材质之上的中性蒙版，
 * 默认主题里那两个取值（`black/5` / `white/10`）正是前景色压一层，因此自定义主题
 * 沿用同一条规则。
 */
export function customThemeSurfaceTokenEntries(
  value: AppearanceSettingsValue,
  prefersDark: boolean,
): Record<string, string> {
  if (value.theme !== "custom" || !guardAppTheme(value.customTheme)) return {}
  const theme = value.customTheme
  const palette = prefersDark ? theme.dark : theme.light
  const entries: Record<string, string> = {
    "--haven-floating-border": palette.border,
    "--haven-floating-idle": palette.mutedForeground,
    "--haven-home-surface": palette.background,
  }
  const derived: ReadonlyArray<readonly [string, string | null]> = [
    [
      "--haven-floating-surface",
      withAlphaByte(
        palette.background,
        prefersDark ? FLOATING_SURFACE_ALPHA.dark : FLOATING_SURFACE_ALPHA.light,
      ),
    ],
    [
      "--haven-floating-surface-raised",
      withAlphaByte(palette.popover, FLOATING_SURFACE_RAISED_ALPHA),
    ],
    [
      "--haven-floating-idle-hover",
      withAlphaByte(
        palette.foreground,
        prefersDark ? FLOATING_IDLE_HOVER_ALPHA.dark : FLOATING_IDLE_HOVER_ALPHA.light,
      ),
    ],
  ]
  for (const [token, derivedValue] of derived) {
    if (derivedValue !== null) entries[token] = derivedValue
  }
  return entries
}

/** 先清空再写入，保证任何时刻 inline token 只对应一份调色板。 */
export function writeCustomThemeTokens(
  entries: Record<string, string>,
  root: HTMLElement | null = themeRoot(),
): void {
  if (!root) return
  clearCustomThemeTokens(root)
  for (const token of CUSTOM_THEME_TOKEN_NAMES) {
    const value = entries[token]
    if (typeof value === "string") root.style.setProperty(token, value)
  }
}

/** 写入材质面 token；由 `writeCustomThemeTokens` 刚刚清空过，因此这里只负责写。 */
export function writeAppearanceSurfaceTokens(
  entries: Record<string, string>,
  root: HTMLElement | null = themeRoot(),
): void {
  if (!root) return
  for (const token of APPEARANCE_SURFACE_TOKEN_NAMES) {
    const value = entries[token]
    if (typeof value === "string") root.style.setProperty(token, value)
  }
}

/** 移除全部外观 token（19 个语义 token + 材质面 token），回到 index.css 的默认主题。 */
export function clearCustomThemeTokens(root: HTMLElement | null = themeRoot()): void {
  if (!root) return
  for (const token of [...CUSTOM_THEME_TOKEN_NAMES, ...APPEARANCE_SURFACE_TOKEN_NAMES]) {
    root.style.removeProperty(token)
  }
}

/**
 * 把自定义调色板投影到 documentElement，并跟随系统明暗重算
 * （自定义主题自带 light/dark 两套调色板，需要一个来源决定用哪一套）。
 *
 * 返回的清理函数移除全部 inline token；非 custom 主题下同样返回清理函数，
 * 调用方无需区分分支。
 */
export function applyCustomThemeTokens(value: AppearanceSettingsValue): AppearanceCleanup {
  const root = themeRoot()
  if (!root) return () => undefined
  const apply = () => {
    const prefersDark = systemPrefersDark()
    // 顺序有意义：writeCustomThemeTokens 先清掉两组 token，再写回 19 个语义 token；
    // 材质面随后补上，因此不存在「清空之后忘了写回」的中间态。
    writeCustomThemeTokens(customThemeTokenEntries(value, prefersDark), root)
    writeAppearanceSurfaceTokens(customThemeSurfaceTokenEntries(value, prefersDark), root)
  }
  apply()
  if (value.theme !== "custom") return () => clearCustomThemeTokens(root)
  const unsubscribe = subscribeSystemPreference("(prefers-color-scheme: dark)", apply)
  return () => {
    unsubscribe()
    clearCustomThemeTokens(root)
  }
}

// ---------- 自定义字体资产 ----------

/**
 * 内部字体族标识：固定常量，与资产 displayName / 原始文件名无关。
 *
 * 把用户可见的展示名拼进 CSS 会让一个纯展示字段变成样式标识，也会让后端文案
 * 直接决定前端样式，因此这里刻意不读取 displayName。
 */
export const CUSTOM_FONT_FAMILY = "HavenUiCustom"
/** 字体加载成功后才写入的 CSS 变量；卸载时删除。 */
export const CUSTOM_FONT_VARIABLE = "--haven-ui-font-family"
/**
 * 未安装自定义字体时的字体栈；与 tailwind `fontFamily.sans` 保持一致
 * （也就是 `html` 上的那份，`body` 的 `inherit` 回退落到它）。
 *
 * 变量本身自带这份回退栈，因此 index.css 里三处通用 UI 字体声明——`body`、
 * `.font-haven-sans`、`.settings-page`——一旦读到变量就收敛到同一份栈；变量缺失时
 * 它们各用自己的既有栈，与安装自定义字体之前完全一致。
 *
 * 刻意不跟随自定义字体的：`.font-haven-marker` / `.font-haven-salt`（表达的是那款
 * 手写体本身，不是 UI 默认字体），以及阅读页由 reading 设置独立接管的字体族。
 */
export const CUSTOM_FONT_FALLBACK = "var(--haven-font-ui-system)"

/** FontFace 的最小投影；浏览器用真实实现，Node / 测试注入替身。 */
export interface FontFaceLike {
  load(): Promise<unknown>
}

export interface FontFaceSetLike {
  add(font: FontFaceLike): void
  delete(font: FontFaceLike): boolean
}

export interface FontEnvironment {
  createFontFace(family: string, source: string): FontFaceLike
  fonts: FontFaceSetLike
}

/**
 * 读取真实的 FontFace / document.fonts。
 *
 * 缺少任何一项（Node、jsdom、未实现 FontFace 的 WebView）都返回 null，让字体投影
 * 安全回退到默认字体栈，而不是在启动路径上抛错。
 */
export function resolveFontEnvironment(): FontEnvironment | null {
  const FontFaceCtor = (globalThis as { FontFace?: unknown }).FontFace
  const fonts = typeof document === "undefined" ? undefined : document.fonts
  if (typeof FontFaceCtor !== "function" || !fonts) return null
  return {
    createFontFace: (family, source) =>
      new (FontFaceCtor as new (family: string, source: string) => FontFaceLike)(family, source),
    fonts: fonts as unknown as FontFaceSetLike,
  }
}

/** 资产必须存在、`kind` 匹配且已通过后端校验，才允许进入运行时投影。 */
export function isAppearanceAssetValidated(
  assets: AppearanceAssetsWire | null | undefined,
  assetId: string,
  kind: AppearanceAssetKindWire,
): boolean {
  if (!assets || !Array.isArray(assets.assets)) return false
  const asset = assets.assets.find((candidate) => candidate.assetId === assetId)
  return asset !== undefined && asset.kind === kind && asset.state === "validated"
}

/**
 * 只认受控外观资源地址（原生 scheme 形态与 Windows WebView 的 http 形态）。
 * 任何其它形状——包括看起来像路径或远端 URL 的字符串——都在这里被拒绝。
 */
const CONTROLLED_APPEARANCE_URI =
  /^(?:haven-resource:\/\/appearance\/|http:\/\/haven-resource\.appearance\/)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/

export function isControlledAppearanceUri(uri: string | null | undefined): uri is string {
  return typeof uri === "string" && CONTROLLED_APPEARANCE_URI.test(uri)
}

/** 不透明资产 ID → 当前 WebView 可请求的受控地址；非 Tauri 运行时返回 null。 */
export function appearanceAssetRequestUri(assetId: string): string | null {
  const uri = controlledResourceUri(appearanceAssetResourceUri(assetId))
  return isControlledAppearanceUri(uri) ? uri : null
}

export interface CustomFontDeps {
  /** 资产校验入口；默认走 appearanceGateway（浏览器生产环境在 client 边界 fail closed）。 */
  listAssets?: (request: AppearanceAssetsListRequestWire) => Promise<AppearanceAssetsWire>
  resourceUri?: (assetId: string) => string | null
  environment?: FontEnvironment | null
  root?: HTMLElement | null
}

function defaultListAssets(
  request: AppearanceAssetsListRequestWire,
): Promise<AppearanceAssetsWire> {
  return appearanceGateway.appearanceAssetsList(request)
}

/** 任何字体选择（包含无需异步加载的预设）都会推进代数，使旧的加载结果失效。 */
let fontInstallSequence = 0
const fontProjectionSequences = new WeakMap<HTMLElement, number>()

function claimFontProjection(root: HTMLElement): number {
  const sequence = (fontInstallSequence += 1)
  fontProjectionSequences.set(root, sequence)
  return sequence
}

function isCurrentFontProjection(root: HTMLElement, sequence: number): boolean {
  return fontProjectionSequences.get(root) === sequence
}

interface FontInstallRecord {
  sequence: number
  /** 只撤销本次安装加入的 FontFace（幂等），绝不碰 CSS 变量。 */
  disposeFace: () => void
}

/**
 * 每个 root 上「当前生效的字体安装」。
 *
 * 不变量：同一个 root 上至多只有一份被安装的 face，`--haven-ui-font-family` 也只
 * 指向这份安装；新安装落地时先接管记录，再撤掉旧 face。
 */
const fontInstallRecords = new WeakMap<HTMLElement, FontInstallRecord>()

/** 校验、加载并安装字体资产；selection sequence 让预设切换也能使过期加载失效。 */
async function installCustomFontAtSequence(
  assetId: string | null | undefined,
  deps: CustomFontDeps,
  sequence: number,
): Promise<AppearanceCleanup | null> {
  if (!assetId) return null
  const root = deps.root ?? themeRoot()
  const environment = deps.environment ?? resolveFontEnvironment()
  if (!root || !environment) return null
  if (!isCurrentFontProjection(root, sequence)) return null
  const listAssets = deps.listAssets ?? defaultListAssets
  const resourceUri = deps.resourceUri ?? appearanceAssetRequestUri

  let assets: AppearanceAssetsWire
  try {
    assets = await listAssets({ kind: "font" })
  } catch {
    return null
  }
  if (!isCurrentFontProjection(root, sequence)) return null
  if (!isAppearanceAssetValidated(assets, assetId, "font")) return null

  const requestUri = resourceUri(assetId)
  if (!isControlledAppearanceUri(requestUri)) return null

  // 校验期间可能已经切换到另一个字体（包含系统预设）。
  const committed = fontInstallRecords.get(root)
  if (!isCurrentFontProjection(root, sequence) || (committed && committed.sequence > sequence)) return null

  let face: FontFaceLike
  try {
    face = environment.createFontFace(CUSTOM_FONT_FAMILY, `url("${requestUri}")`)
    await face.load()
  } catch {
    return null
  }

  // 字体解码期间可能已经切换到另一个字体；旧 face 不得覆盖新选择。
  const landed = fontInstallRecords.get(root)
  if (!isCurrentFontProjection(root, sequence) || (landed && landed.sequence > sequence)) return null

  let added = false
  const disposeFace = () => {
    if (!added) return
    added = false
    environment.fonts.delete(face)
  }

  try {
    environment.fonts.add(face)
    added = true
    root.style.setProperty(
      CUSTOM_FONT_VARIABLE,
      `"${CUSTOM_FONT_FAMILY}", ${CUSTOM_FONT_FALLBACK}`,
    )
  } catch {
    // face 已加入但变量写不进去：本次安装整体回退，不留一份没有变量指向的字体。
    disposeFace()
    return null
  }

  // 新安装接管之后才撤掉旧 face：同一族名下不同时留着两份。
  landed?.disposeFace()
  const record: FontInstallRecord = { sequence, disposeFace }
  fontInstallRecords.set(root, record)

  return () => {
    if (fontInstallRecords.get(root) === record) {
      fontInstallRecords.delete(root)
      if (isCurrentFontProjection(root, sequence)) root.style.removeProperty(CUSTOM_FONT_VARIABLE)
    }
    disposeFace()
  }
}

/** 校验并通过 FontFace 安装一个导入字体；返回清理函数，不触碰其它投影。 */
export async function installCustomFont(
  assetId: string | null | undefined,
  deps: CustomFontDeps = {},
): Promise<AppearanceCleanup | null> {
  const root = deps.root ?? themeRoot()
  if (!root) return null
  return installCustomFontAtSequence(assetId, { ...deps, root }, claimFontProjection(root))
}

/**
 * 幂等的字体投影：同一时刻至多保留一次安装。
 *
 * FontFace 加载是异步的，因此在资产切换时先标记 disposed；过期的加载结果会被立即
 * 撤销，不会盖掉新资产或留下已卸载的字体。
 *
 * 两次投影重叠时（先 dispose 再重新投影，或新旧投影交错）以「后发起者为准」：
 * 先发起的那次即便晚落地，也既不会删掉新安装写好的 CSS 变量，也不会删掉新安装的
 * FontFace；它只会撤销自己，见 installCustomFont 的序号判定。
 */
export function applyCustomFont(
  assetId: string | null | undefined,
  deps: CustomFontDeps = {},
): AppearanceCleanup {
  const root = deps.root ?? themeRoot()
  if (!assetId || !root) return () => undefined
  const sequence = claimFontProjection(root)
  return applyCustomFontAtSequence(assetId, { ...deps, root }, sequence)
}

function applyCustomFontAtSequence(
  assetId: string,
  deps: CustomFontDeps,
  sequence: number,
): AppearanceCleanup {
  let disposed = false
  let installed: AppearanceCleanup | null = null
  void installCustomFontAtSequence(assetId, deps, sequence)
    .then((cleanup) => {
      if (disposed) {
        cleanup?.()
        return
      }
      installed = cleanup
    })
    .catch(() => undefined)
  return () => {
    disposed = true
    installed?.()
    installed = null
  }
}

/** Apply an ephemeral imported font to the specimen only; never changes the app-wide font token. */
export async function installAppearanceFontPreview(
  assetId: string,
  deps: CustomFontDeps = {},
): Promise<{ family: string; cleanup: AppearanceCleanup } | null> {
  const environment = deps.environment ?? resolveFontEnvironment()
  if (!environment) return null
  const listAssets = deps.listAssets ?? defaultListAssets
  const resourceUri = deps.resourceUri ?? appearanceAssetRequestUri
  let assets: AppearanceAssetsWire
  try {
    assets = await listAssets({ kind: "font" })
  } catch {
    return null
  }
  if (!isAppearanceAssetValidated(assets, assetId, "font")) return null
  const requestUri = resourceUri(assetId)
  if (!isControlledAppearanceUri(requestUri)) return null

  const suffix = assetId.replace(/-/g, "")
  const family = `HavenUiPreview${suffix}`
  let face: FontFaceLike
  try {
    face = environment.createFontFace(family, `url("${requestUri}")`)
    await face.load()
    environment.fonts.add(face)
  } catch {
    return null
  }
  let active = true
  return {
    family,
    cleanup: () => {
      if (!active) return
      active = false
      environment.fonts.delete(face)
    },
  }
}

/** Project the selected preset/name immediately and an imported face once it finishes loading. */
export function applyUiFontSelection(
  value: AppearanceSettingsValue,
  deps: CustomFontDeps = {},
): AppearanceCleanup {
  const root = deps.root ?? themeRoot()
  if (!root) return () => undefined

  const sequence = claimFontProjection(root)
  const current = fontInstallRecords.get(root)
  if (current) {
    fontInstallRecords.delete(root)
    current.disposeFace()
  }
  root.style.setProperty(
    CUSTOM_FONT_VARIABLE,
    uiFontCssStack(value.uiFontPreset, value.uiFontFamily),
  )

  const cleanupAsset = value.customFontAssetId
    ? applyCustomFontAtSequence(value.customFontAssetId, { ...deps, root }, sequence)
    : null

  return () => {
    cleanupAsset?.()
    if (isCurrentFontProjection(root, sequence)) root.style.removeProperty(CUSTOM_FONT_VARIABLE)
  }
}

/**
 * 把一份外观值整体投影到当前 WebView：自定义调色板 token + 自定义字体。
 * 返回的清理函数移除全部 inline token 与本次安装的 FontFace，默认主题不受污染。
 */
export function installAppearanceProjection(value: AppearanceSettingsValue): AppearanceCleanup {
  const cleanupTokens = applyCustomThemeTokens(value)
  const cleanupFont = applyUiFontSelection(value)
  return () => {
    cleanupFont()
    cleanupTokens()
  }
}

// ---------- 壁纸背景层 ----------

/** 壁纸层：只有受控 URI，没有 path/url 字段；AppShell 据此渲染非交互背景。 */
export type WallpaperLayer = { kind: "static" | "dynamic"; uri: string }

/**
 * 首页面板的类名——壁纸可见性契约的一半。
 *
 * AppShell 把壁纸媒体层挂在 shell 的 `-z-10` 上，因此它只从「页面自身透明」的地方
 * 透出来；而首页面板自带一整套不透明背景（明暗两套纯色），不处理就永远盖着壁纸。
 * 于是约定：AppShell 在真的渲染出壁纸层时把 shell 的 `data-haven-wallpaper` 置为
 * `"true"`，index.css 里那条
 *
 *   [data-haven-wallpaper="true"] .haven-wallpaper-surface { background-color: transparent }
 *
 * 规则才把这块背景让开。没有壁纸、或选了壁纸但字节读不出来（属性为 `"false"`）时
 * 规则不生效，首页维持原有的不透明外观。
 *
 * 契约的三个端点——本常量、AppShell 写的属性、index.css 的规则——由测试钉在一起，
 * 任何一处漂移都会让「选了壁纸却看不见」重新出现。
 */
export const HOME_WALLPAPER_SURFACE_CLASS = "haven-wallpaper-surface"

/** 壁纸选择 → 需要校验的资产种类；`none` 与非法形状都不需要资产。 */
export function wallpaperAssetKind(
  selection: WallpaperSelection | null | undefined,
): AppearanceAssetKindWire | null {
  if (!guardWallpaperSelection(selection) || selection.kind === "none") return null
  return selection.kind === "static" ? "static_wallpaper" : "dynamic_wallpaper"
}

/** 壁纸选择 → 不透明资产 ID；`none` 与非法形状返回 null。 */
export function wallpaperAssetId(
  selection: WallpaperSelection | null | undefined,
): string | null {
  if (!guardWallpaperSelection(selection) || selection.kind === "none") return null
  return selection.assetId
}

/**
 * 纯投影决策：这一帧应该渲染什么壁纸层。
 *
 * - `none`、形状非法、资产未被验证为 validated、拿不到受控 URI → null（无壁纸）；
 * - 减少动效（设置项或系统偏好）时 dynamic 一律降级为无壁纸：同一个资产没有可用的
 *   静态帧，本轮不做静帧伪造，也不留一个永远不播放的 video 占位。
 */
export function resolveWallpaperLayer(input: {
  selection: WallpaperSelection | null | undefined
  reducedMotion: boolean
  assetValidated: boolean
  resourceUri: string | null | undefined
}): WallpaperLayer | null {
  const { selection } = input
  if (!guardWallpaperSelection(selection) || selection.kind === "none") return null
  if (input.reducedMotion && selection.kind === "dynamic") return null
  if (!input.assetValidated) return null
  if (!isControlledAppearanceUri(input.resourceUri)) return null
  return { kind: selection.kind, uri: input.resourceUri }
}

// ---------- 壁纸重试信号 ----------

/**
 * 一次「重新请求当前壁纸字节」的信号：只增不减的代数，**没有任何载荷**。
 *
 * 为什么是代数而不是一个带地址的入口：重试不接受地址——它重新请求的永远是当前设置里
 * 那份受控 URI（由 `appearanceAssetRequestUri` 从不透明资产 ID 推出）。因此这里没有
 * path / url 参数，形状上就不存在「重试到另一个资源上」的可能。
 *
 * 它也不持久化：代数只活在本渲染进程里，刷新或重启就回到 0。重试不是一份设置，也
 * 不写入设置——它只让已经失败的那次投影作废，重新投影同一份选择。
 */
let wallpaperRetryEpoch = 0
const wallpaperRetryListeners = new Set<() => void>()

/**
 * 请求重新投影当前壁纸。
 *
 * 设置页的预览与 AppShell 的首页层订阅的是同一个代数，因此一次重试同时作废两边的
 * 失败记账：用户在设置页点一下，回到首页看到的还是同一份选择，而不是一个空背景。
 */
export function requestWallpaperRetry(): void {
  wallpaperRetryEpoch += 1
  for (const listener of [...wallpaperRetryListeners]) listener()
}

/** 当前重试代数；订阅者据此判断手上的失败记账是否属于上一代。 */
export function currentWallpaperRetryEpoch(): number {
  return wallpaperRetryEpoch
}

/** 订阅重试代数变化，返回取消订阅函数。 */
export function subscribeWallpaperRetry(listener: () => void): AppearanceCleanup {
  wallpaperRetryListeners.add(listener)
  return () => {
    wallpaperRetryListeners.delete(listener)
  }
}

/**
 * React 侧读取重试代数。
 *
 * 失败记账记的是「哪一份受控 URI **+ 哪一代重试**」：换一份 URI 或用户点了一次重试，
 * 上一次失败就作废。两处消费者（设置页预览、AppShell 首页层）读同一份代数，不会各
 * 记各的。
 */
export function useWallpaperRetryEpoch(): number {
  return useSyncExternalStore(subscribeWallpaperRetry, currentWallpaperRetryEpoch, () => 0)
}

/**
 * AppShell 壁纸字节读取失败的运行时信号。
 *
 * 设置预览与首页背景是两次独立读取：其中一边成功不能替另一边证明可用。保留失败的
 * 受控 URI 与重试代数，让设置页能提供真正重新请求首页背景的入口，不写入设置或磁盘。
 */
export interface HomeWallpaperFailure {
  uri: string
  epoch: number
}

let homeWallpaperFailure: HomeWallpaperFailure | null = null
const homeWallpaperFailureListeners = new Set<() => void>()

export function reportHomeWallpaperUnavailable(uri: string, epoch: number): void {
  if (!isControlledAppearanceUri(uri) || epoch !== currentWallpaperRetryEpoch()) return
  if (homeWallpaperFailure?.uri === uri && homeWallpaperFailure.epoch === epoch) return
  homeWallpaperFailure = { uri, epoch }
  for (const listener of [...homeWallpaperFailureListeners]) listener()
}

export function currentHomeWallpaperFailure(): HomeWallpaperFailure | null {
  return homeWallpaperFailure
}

export function subscribeHomeWallpaperFailure(listener: () => void): AppearanceCleanup {
  homeWallpaperFailureListeners.add(listener)
  return () => {
    homeWallpaperFailureListeners.delete(listener)
  }
}

export function useHomeWallpaperFailure(): HomeWallpaperFailure | null {
  return useSyncExternalStore(
    subscribeHomeWallpaperFailure,
    currentHomeWallpaperFailure,
    () => null,
  )
}

export interface AppearanceWallpaperDeps extends CustomFontDeps {
  /**
   * 已经合成好的减少动效结论（设置项或系统偏好）。
   *
   * 缺省时本模块自己按「设置项 reduceMotion 或系统 prefers-reduced-motion」算一份，
   * 结果与显式传入一致；AppShell 之所以传，是为了让动态壁纸的抑制和 shell 的动效
   * 类名 / data 状态读的是同一个值，而不是各算各的。测试也用它固定这一路取值。
   */
  reducedMotion?: boolean
}

/**
 * 一次资产校验请求的身份：种类 + 不透明资产 ID。
 *
 * `kind` 也进身份，因为同一个资产 ID 换个 kind 是另一次请求（静态壁纸与动态壁纸的
 * 校验结论不能互相顶替）。无壁纸时身份是 `null`——它同样占一个位置，
 * 所以「切到无壁纸再切回同一个资产」也是一次身份变化。
 */
function wallpaperRequestKey(
  kind: AppearanceAssetKindWire | null,
  assetId: string | null,
): string | null {
  return kind && assetId ? `${kind}:${assetId}` : null
}

/** 一份校验结论属于哪次请求；`key` 与当前请求不一致即视为「还没有结论」。 */
interface WallpaperVerification {
  key: string | null
  validated: boolean
}

const NO_WALLPAPER_VERIFICATION: WallpaperVerification = { key: null, validated: false }

/**
 * AppShell 使用的壁纸投影。
 *
 * 资产校验、IPC 失败、运行时拿不到受控 URI、减少动效都收敛为 null（无壁纸层），
 * 页面因此永远不需要处理「半个壁纸」。渲染期读取失败由调用方用 resolveWallpaperLayer
 * 的同一份事实重新收敛（见 AppShell 的 onError 回退）。
 *
 * 校验结论始终绑在「当前这次请求」上：选择一变（换资产、换 kind、切回无壁纸）上一份
 * 结论立刻作废，新的资产列表请求落地之前不会放行任何一层——包括「本来是同一个资产」
 * 的切回，那次同样要重新查一遍后端。
 */
export function useAppearanceWallpaper(
  appearance: AppearanceSettingsValue,
  deps: AppearanceWallpaperDeps = {},
): WallpaperLayer | null {
  const systemReducedMotion = useSystemPreference("(prefers-reduced-motion: reduce)")
  const reducedMotion = deps.reducedMotion ?? (appearance.reduceMotion || systemReducedMotion)
  const selection = appearance.wallpaper
  const kind = wallpaperAssetKind(selection)
  const assetId = wallpaperAssetId(selection)
  const requestKey = wallpaperRequestKey(kind, assetId)
  const [verification, setVerification] = useState<WallpaperVerification>(NO_WALLPAPER_VERIFICATION)
  const listAssets = deps.listAssets

  useEffect(() => {
    // 选择一变，先把上一份结论换成「这次还没有结论」，再去问后端：不做这一步，
    // 「切到 B（请求还在飞）再切回 A」会重新捡起 A 的旧结论立刻放行。
    setVerification({ key: requestKey, validated: false })
    if (!requestKey || !kind || !assetId) return
    let active = true
    const request = listAssets ?? defaultListAssets
    void request({ kind })
      .then((assets) => {
        if (!active) return
        setVerification({
          key: requestKey,
          validated: isAppearanceAssetValidated(assets, assetId, kind),
        })
      })
      .catch(() => {
        if (active) setVerification({ key: requestKey, validated: false })
      })
    return () => {
      active = false
    }
  }, [assetId, kind, requestKey, listAssets])

  const resolveResourceUri = deps.resourceUri ?? appearanceAssetRequestUri
  const resourceUri = assetId ? resolveResourceUri(assetId) : null
  // 结论必须属于「当前这次请求」：换了资产/种类之后，上一份结论在它自己的请求
  // 落地之前一律不算数（包括本次 effect 还没跑到的那一帧）。
  const assetValidated =
    requestKey !== null && verification.key === requestKey && verification.validated
  return useMemo(
    () => resolveWallpaperLayer({ selection, reducedMotion, assetValidated, resourceUri }),
    [selection, reducedMotion, assetValidated, resourceUri],
  )
}
