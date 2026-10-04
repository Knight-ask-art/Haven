// @vitest-environment jsdom

// 应用工程只引入 vite/client 类型（tsconfig.app.json 的 types 白名单），程序里没有
// @types/node，node:fs 因此不在可解析的模块里。
// @ts-expect-error node:fs 不在本工程可解析的模块里（types 里补上 "node" 后即可删除本行）
import { readFileSync } from "node:fs"
import { afterEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, renderHook, waitFor } from "@testing-library/react"
import type {
  AppearanceAssetKindWire,
  AppearanceAssetsWire,
  AppearanceSettingsValue,
  AppTheme,
  AppThemePalette,
  WallpaperSelection,
} from "@/lib/ipc/settings-wire"
import type { FontEnvironment, FontFaceLike } from "./appearance-runtime"
import {
  CUSTOM_FONT_FALLBACK,
  CUSTOM_FONT_FAMILY,
  CUSTOM_FONT_VARIABLE,
  CUSTOM_THEME_TOKEN_NAMES,
  HOME_WALLPAPER_SURFACE_CLASS,
  applyUiFontSelection,
  applyCustomFont,
  applyCustomThemeTokens,
  appearanceAssetRequestUri,
  clearCustomThemeTokens,
  currentHomeWallpaperFailure,
  currentWallpaperRetryEpoch,
  customThemeTokenEntries,
  installAppearanceProjection,
  installCustomFont,
  isAppearanceAssetValidated,
  isControlledAppearanceUri,
  reportHomeWallpaperUnavailable,
  requestWallpaperRetry,
  resolveWallpaperLayer,
  subscribeWallpaperRetry,
  subscribeHomeWallpaperFailure,
  systemPrefersDark,
  useAppearanceWallpaper,
  useHomeWallpaperFailure,
  useWallpaperRetryEpoch,
  wallpaperAssetId,
  wallpaperAssetKind,
  writeCustomThemeTokens,
} from "./appearance-runtime"

// 直接读构建时用的那两份源文件：token 名与字体变量的消费者必须和它们对齐，
// 而不是和测试里另抄的一份清单对齐。
//
// 读盘而不是走 `?raw` 导入：测试运行器不把这两份文件的源文本交出来（拿到的是空串），
// 对齐断言就变成恒真了。地址一律由 import.meta.url 解析，与进程工作目录无关；
// 文件读不到时 readFileSync 直接抛错，不会静默退化成「空文件也算过」。
/** 相对本测试文件解析源文件地址；readFileSync 直接接受 file: URL。 */
function sourceUrl(relativePath: string): URL {
  return new URL(relativePath, import.meta.url)
}

const indexCss: string = readFileSync(sourceUrl("../../../index.css"), "utf8")
const tailwindConfigSource: string = readFileSync(sourceUrl("../../../../tailwind.config.ts"), "utf8")

const FONT_ASSET_ID = "0196f0d2-0000-7000-8000-00000000f001"
const SECOND_FONT_ASSET_ID = "0196f0d2-0000-7000-8000-00000000f002"
const STATIC_ASSET_ID = "0196f0d2-0000-7000-8000-00000000a001"
const DYNAMIC_ASSET_ID = "0196f0d2-0000-7000-8000-00000000a002"
/** 强调色刻意与调色板里的 primary / ring 不同值：否则「谁覆盖谁」根本测不出来。 */
const ACCENT_COLOR = "#c2410c"
const PALETTE_PRIMARY = "#3366ff"
const CONTROLLED_URI = `haven-resource://appearance/${STATIC_ASSET_ID}`
const FONT_URI = `haven-resource://appearance/${FONT_ASSET_ID}`
const SECOND_FONT_URI = `haven-resource://appearance/${SECOND_FONT_ASSET_ID}`
const FONT_VARIABLE_VALUE = `"${CUSTOM_FONT_FAMILY}", ${CUSTOM_FONT_FALLBACK}`

afterEach(() => {
  cleanup()
  clearCustomThemeTokens(document.documentElement)
  document.documentElement.style.removeProperty(CUSTOM_FONT_VARIABLE)
  Reflect.deleteProperty(window, "matchMedia")
})

// ---------- 夹具 ----------

function appearanceValue(overrides: {
  theme?: AppearanceSettingsValue["theme"]
  reduceMotion?: boolean
  customTheme?: AppTheme | null
  wallpaper?: WallpaperSelection
  customFontAssetId?: string | null
  uiFontPreset?: AppearanceSettingsValue["uiFontPreset"]
  uiFontFamily?: string | null
} = {}): AppearanceSettingsValue {
  return {
    section: "appearance",
    theme: overrides.theme ?? "system",
    density: "comfortable",
    sidebar: "auto",
    reduceMotion: overrides.reduceMotion ?? false,
    interfaceFontMode: "system",
    customTheme: overrides.customTheme ?? null,
    wallpaper: overrides.wallpaper ?? { kind: "none" },
    customFontAssetId: overrides.customFontAssetId ?? null,
    uiFontPreset: overrides.uiFontPreset ?? "system",
    uiFontFamily: overrides.uiFontFamily ?? null,
  }
}

/** 明暗两套取值可区分的调色板；`border`/`input` 用带 alpha 的规范形态。 */
function palettePreset(seed: "light" | "dark"): AppThemePalette {
  const isLight = seed === "light"
  return {
    background: isLight ? "#ffffff" : "#101010",
    foreground: isLight ? "#1d1d1f" : "#f5f5f7",
    card: isLight ? "#ffffff" : "#1a1a1a",
    cardForeground: isLight ? "#1d1d1f" : "#f5f5f7",
    popover: isLight ? "#ffffff" : "#1a1a1a",
    popoverForeground: isLight ? "#1d1d1f" : "#f5f5f7",
    primary: PALETTE_PRIMARY,
    primaryForeground: "#ffffff",
    secondary: isLight ? "#f5f5f7" : "#202024",
    secondaryForeground: isLight ? "#1d1d1f" : "#f5f5f7",
    muted: isLight ? "#f5f5f7" : "#202024",
    mutedForeground: isLight ? "#6e6e73" : "#aeaeb2",
    accent: isLight ? "#f5f5f7" : "#26262b",
    accentForeground: isLight ? "#1d1d1f" : "#f5f5f7",
    destructive: isLight ? "#ff3b30" : "#ff453a",
    destructiveForeground: "#ffffff",
    border: isLight ? "#3c3c4329" : "#54545852",
    input: isLight ? "#3c3c4329" : "#54545852",
    ring: PALETTE_PRIMARY,
  }
}

const CUSTOM_THEME: AppTheme = {
  light: palettePreset("light"),
  dark: palettePreset("dark"),
  accentColor: ACCENT_COLOR,
}

function customThemeValue(theme: AppTheme = CUSTOM_THEME): AppearanceSettingsValue {
  return appearanceValue({ theme: "custom", customTheme: theme })
}

function assetsWire(
  ...entries: ReadonlyArray<{
    assetId: string
    kind: AppearanceAssetKindWire
    state: "pending" | "validated" | "rejected"
    displayName?: string | null
  }>
): AppearanceAssetsWire {
  return {
    schemaVersion: 1,
    assets: entries.map((entry) => ({
      assetId: entry.assetId,
      kind: entry.kind,
      state: entry.state,
      byteSize: 4096,
      displayName: entry.displayName ?? null,
    })),
  }
}

function validatedFont(displayName: string | null = null): AppearanceAssetsWire {
  return assetsWire({ assetId: FONT_ASSET_ID, kind: "font", state: "validated", displayName })
}

function fontEnvironment(options: { loadRejects?: boolean } = {}) {
  const created: Array<{ family: string; source: string; face: FontFaceLike }> = []
  const added: FontFaceLike[] = []
  const deleted: FontFaceLike[] = []
  const environment: FontEnvironment = {
    createFontFace(family, source) {
      const face: FontFaceLike = {
        load: options.loadRejects
          ? async () => {
              throw new Error("字体解码失败")
            }
          : async () => undefined,
      }
      created.push({ family, source, face })
      return face
    },
    fonts: {
      add(face) {
        added.push(face)
      },
      delete(face) {
        deleted.push(face)
        return true
      },
    },
  }
  return { environment, created, added, deleted }
}

/** 手动闸门：把一次异步流程停在指定位置，让两次安装按测试要求的顺序落地。 */
function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise
  })
  return { promise, resolve }
}

/** 冲刷已排队的微任务链：字体安装全程是 Promise，没有定时器参与。 */
async function settleAsyncWork(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0))
}

/** 资产列表的手动闸门：断言只在这批资产真的被消费之后才求值。 */
function gatedAssetList() {
  let settle: (assets: AppearanceAssetsWire) => void = () => undefined
  let fail: (error: unknown) => void = () => undefined
  const calls: Array<{ kind: AppearanceAssetKindWire | null }> = []
  const listAssets = vi.fn((request: { kind: AppearanceAssetKindWire | null }) => {
    calls.push(request)
    return new Promise<AppearanceAssetsWire>((resolve, reject) => {
      settle = resolve
      fail = reject
    })
  })
  return { listAssets, calls, settle: (assets: AppearanceAssetsWire) => settle(assets), fail: (error: unknown) => fail(error) }
}

/** 只回答减少动效查询的 matchMedia 替身（jsdom 默认没有 matchMedia）。 */
function stubReducedMotionPreference(matches: boolean): void {
  const listeners = new Set<() => void>()
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    writable: true,
    value: (query: string) => ({
      matches: query.includes("prefers-reduced-motion") ? matches : false,
      media: query,
      onchange: null,
      addEventListener: (_type: string, listener: () => void) => {
        listeners.add(listener)
      },
      removeEventListener: (_type: string, listener: () => void) => {
        listeners.delete(listener)
      },
      addListener: () => undefined,
      removeListener: () => undefined,
      dispatchEvent: () => false,
    }),
  })
}

// ---------- 调色板投影 ----------

describe("custom palette projection", () => {
  it("maps every declared token from the matching light/dark palette", () => {
    const light = customThemeTokenEntries(customThemeValue(), false)
    const dark = customThemeTokenEntries(customThemeValue(), true)

    expect(Object.keys(light).sort()).toEqual([...CUSTOM_THEME_TOKEN_NAMES].sort())
    expect(Object.keys(dark).sort()).toEqual([...CUSTOM_THEME_TOKEN_NAMES].sort())

    expect(light["--background"]).toBe("#ffffff")
    expect(dark["--background"]).toBe("#101010")
    expect(light["--foreground"]).toBe("#1d1d1f")
    expect(dark["--foreground"]).toBe("#f5f5f7")
    // 带 alpha 的既有 token 保持 8 位形态，不被压成不透明色。
    expect(light["--border"]).toBe("#3c3c4329")
    expect(light["--input"]).toBe("#3c3c4329")
  })

  it("drives --primary and --ring from the accent color, overriding the palette entries", () => {
    const light = customThemeTokenEntries(customThemeValue(), false)
    const dark = customThemeTokenEntries(customThemeValue(), true)
    expect(light["--primary"]).toBe(ACCENT_COLOR)
    expect(light["--ring"]).toBe(ACCENT_COLOR)
    expect(dark["--primary"]).toBe(ACCENT_COLOR)
    expect(dark["--ring"]).toBe(ACCENT_COLOR)

    // 夹具里 accentColor 与调色板 primary / ring 是不同取值：只有当这两个 token 真的
    // 被 accentColor 覆盖时才会与调色板不同，否则「本来就同值」会让断言恒真。
    const palette = palettePreset("light")
    expect(ACCENT_COLOR).not.toBe(palette.primary)
    expect(ACCENT_COLOR).not.toBe(palette.ring)
    expect(light["--primary"]).not.toBe(palette.primary)
    expect(light["--ring"]).not.toBe(palette.ring)
    // 调色板里的 accent 是另一件事：它照常由调色板决定，不被 accentColor 牵连。
    expect(light["--accent"]).toBe(palette.accent)
  })

  it("projects the light palette when the environment cannot report a dark preference", () => {
    // jsdom 没有 matchMedia：安全默认是浅色，而不是「猜一个深色」。
    expect(systemPrefersDark()).toBe(false)
    expect(customThemeTokenEntries(customThemeValue(), systemPrefersDark())["--background"]).toBe("#ffffff")
  })

  it("projects nothing for other themes, a missing theme, or a rejected palette", () => {
    expect(customThemeTokenEntries(appearanceValue({ theme: "light" }), false)).toEqual({})
    expect(customThemeTokenEntries(appearanceValue({ theme: "system" }), false)).toEqual({})
    expect(customThemeTokenEntries(appearanceValue({ theme: "custom", customTheme: null }), false)).toEqual({})

    // 多写一个 token：闭合形状在守卫处被拒绝，不做「尽力而为」的部分投影。
    const withExtraToken = { ...palettePreset("light"), shadow: "#000000" } as unknown as AppThemePalette
    expect(
      customThemeTokenEntries(customThemeValue({ ...CUSTOM_THEME, light: withExtraToken }), false),
    ).toEqual({})

    // 非规范颜色（路径、URL、任意 CSS 文本）不是颜色，不得进入 CSS 变量。
    for (const invalid of [
      "url(https://evil.example/a.png)",
      "C:/wallpapers/a.png",
      "var(--primary)",
      "rgb(1,2,3)",
      "#FFF",
    ]) {
      const withInvalidColor = { ...palettePreset("light"), background: invalid } as unknown as AppThemePalette
      expect(
        customThemeTokenEntries(customThemeValue({ ...CUSTOM_THEME, light: withInvalidColor }), false),
        `${invalid} 不得成为 CSS token`,
      ).toEqual({})
    }
  })
})

// ---------- 与 index.css 的对接（token 名 + 字体变量消费者） ----------

/**
 * 运行时能接管的 19 个 token 名，在这里字面列出，而不是从 PALETTE_TOKEN_MAP 推导。
 *
 * 写进 documentElement 的 inline token 只有 index.css 里有同名声明时才可能生效，
 * 所以这份清单必须两边对齐：运行时少写一个，那个颜色用户就改不动；多写一个，写进去
 * 也没人读，清理时还会误删别的层。
 */
const INDEX_CSS_THEME_TOKENS = [
  "--background",
  "--foreground",
  "--card",
  "--card-foreground",
  "--popover",
  "--popover-foreground",
  "--primary",
  "--primary-foreground",
  "--secondary",
  "--secondary-foreground",
  "--muted",
  "--muted-foreground",
  "--accent",
  "--accent-foreground",
  "--destructive",
  "--destructive-foreground",
  "--border",
  "--input",
  "--ring",
] as const

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
}

/** 取 index.css 里某个选择器的声明块；找不到就失败，绝不「读不到就算过」。 */
function cssDeclarationBlock(selector: string): string {
  const match = new RegExp(`${escapeRegExp(selector)}\\s*\\{([^}]*)\\}`).exec(indexCss)
  if (!match) throw new Error(`index.css 里找不到 ${selector} 的声明块`)
  return match[1]
}

describe("index.css docking", () => {
  it("declares exactly the 19 tokens the runtime projects", () => {
    expect(INDEX_CSS_THEME_TOKENS).toHaveLength(19)
    expect([...CUSTOM_THEME_TOKEN_NAMES]).toEqual([...INDEX_CSS_THEME_TOKENS])

    for (const token of INDEX_CSS_THEME_TOKENS) {
      // 默认主题（:root）与 .dark 都要有自己的声明：两套调色板都得能回到 index.css。
      const declared = indexCss.match(new RegExp(`^\\s*${escapeRegExp(token)}:`, "gm")) ?? []
      expect(declared.length, `${token} 必须在默认主题与 .dark 里各声明一次`).toBeGreaterThanOrEqual(2)
    }
  })

  it("consumes the custom font variable in every general UI sans declaration", () => {
    // 三处通用 UI 无衬线声明都必须读变量，否则自定义字体在设置页 / 该类作用范围内不生效。
    for (const selector of ["body", ".font-haven-sans", ".settings-page"]) {
      expect(
        cssDeclarationBlock(selector),
        `${selector} 没有读 ${CUSTOM_FONT_VARIABLE}`,
      ).toContain(`font-family: var(${CUSTOM_FONT_VARIABLE},`)
    }

    // 装饰性手写体刻意不跟随：它们表达的是那款字体本身，不是 UI 默认字体。
    for (const selector of [".font-haven-marker", ".font-haven-salt"]) {
      expect(cssDeclarationBlock(selector)).not.toContain(CUSTOM_FONT_VARIABLE)
    }

    // 一份回退栈：变量值自带 tailwind `fontFamily.sans`，也就是 body 的 `inherit`
    // 落到的那份；三处声明因此只有一个覆盖点，不会出现第二套默认字体。
    expect(FONT_VARIABLE_VALUE).toBe(`"${CUSTOM_FONT_FAMILY}", ${CUSTOM_FONT_FALLBACK}`)
    const bodyFallback = /font-family:\s*var\(--haven-ui-font-family,\s*([^)]+)\)/
      .exec(cssDeclarationBlock("body"))
    expect(bodyFallback?.[1].trim()).toBe("inherit")

    const tailwindSans = /fontFamily:\s*\{\s*sans:\s*\[([^\]]*)\]/.exec(tailwindConfigSource)
    const families = (tailwindSans?.[1] ?? "")
      .split(",")
      .map((entry) => entry.trim().replace(/["']/g, ""))
      .join(", ")
    expect(families).toBe(CUSTOM_FONT_FALLBACK.replace(/["']/g, ""))
  })

  it("binds the home surface transparency to the wallpaper presence attribute", () => {
    // 这三个字符串必须严丝合缝地对上，否则「选了壁纸却看不见」会原样重现：
    // AppShell 写 data-haven-wallpaper="true"，HomePage 根元素带这个类名，
    // index.css 的那条规则是两者之间唯一的连接点（见 AppShell 的图层顺序注释）。
    expect(HOME_WALLPAPER_SURFACE_CLASS).toBe("haven-wallpaper-surface")
    const block = cssDeclarationBlock(
      `[data-haven-wallpaper="true"] .${HOME_WALLPAPER_SURFACE_CLASS}`,
    )
    expect(block).toContain("background-color: transparent")
    // 类名在 index.css 里只有这一处定义：不存在第二条规则抢同一块背景，
    // 也不存在「没有壁纸时也让开背景」的分支（AppShell 那时写的是 "false"）。
    expect(indexCss.split(HOME_WALLPAPER_SURFACE_CLASS)).toHaveLength(2)
  })

  it("ships a system prefers-reduced-motion fallback that does not read the app setting", () => {
    const media = /@media\s*\(prefers-reduced-motion:\s*reduce\)\s*\{([\s\S]*?)\n\}/.exec(indexCss)
    expect(media, "index.css 必须有一条 prefers-reduced-motion 的 CSS 兜底").not.toBeNull()
    const block = media?.[1] ?? ""

    expect(block).toContain("[data-haven-reduce-motion]")
    // 只按属性存在匹配、不读取值：设置项关闭（属性为 "false"）而系统要求减少动效时
    // 兜底同样成立；带 ="true" 的选择器做不到这一点。
    expect(block).not.toContain('[data-haven-reduce-motion="true"]')
    expect(block).toContain("transition: none")
    expect(block).toContain("animation: none")
  })
})

// ---------- 运行时 token 清理 ----------

describe("runtime token cleanup", () => {
  it("writes the palette on documentElement and removes every token on cleanup", () => {
    const root = document.documentElement
    const cleanupTokens = applyCustomThemeTokens(customThemeValue())

    for (const token of CUSTOM_THEME_TOKEN_NAMES) {
      expect(root.style.getPropertyValue(token)).not.toBe("")
    }

    cleanupTokens()
    for (const token of CUSTOM_THEME_TOKEN_NAMES) {
      expect(root.style.getPropertyValue(token)).toBe("")
    }
  })

  it("clears the previous palette before writing the next one", () => {
    const root = document.documentElement
    writeCustomThemeTokens({ "--background": "#123456", "--card": "#654321" })
    writeCustomThemeTokens({ "--background": "#ffffff" })

    expect(root.style.getPropertyValue("--background")).toBe("#ffffff")
    expect(
      root.style.getPropertyValue("--card"),
      "上一套调色板不得残留",
    ).toBe("")
  })

  it("leaves nothing behind when the value is not a custom theme", () => {
    const root = document.documentElement
    writeCustomThemeTokens({ "--background": "#123456" })

    const cleanupTokens = applyCustomThemeTokens(appearanceValue({ theme: "dark" }))
    expect(root.style.getPropertyValue("--background")).toBe("")

    cleanupTokens()
    for (const token of CUSTOM_THEME_TOKEN_NAMES) {
      expect(root.style.getPropertyValue(token)).toBe("")
    }
  })

  it("removes palette tokens and the font variable when the whole projection is cleaned up", () => {
    const root = document.documentElement
    const dispose = installAppearanceProjection(
      appearanceValue({ theme: "custom", customTheme: CUSTOM_THEME, customFontAssetId: null }),
    )

    expect(root.style.getPropertyValue("--background")).toBe("#ffffff")

    dispose()
    for (const token of CUSTOM_THEME_TOKEN_NAMES) {
      expect(root.style.getPropertyValue(token)).toBe("")
    }
    expect(root.style.getPropertyValue(CUSTOM_FONT_VARIABLE)).toBe("")
  })
})

// ---------- 自定义字体资产 ----------

describe("UI font projection", () => {
  it("projects each persisted preset and a validated system family, then cleans up", () => {
    const root = document.createElement("div")
    const cases: Array<{
      preset: NonNullable<AppearanceSettingsValue["uiFontPreset"]>
      family: string | null
      expected: string
    }> = [
      { preset: "system", family: null, expected: "var(--haven-font-ui-system)" },
      { preset: "humanist_serif", family: null, expected: "var(--haven-font-ui-humanist-serif)" },
      { preset: "modern_sans", family: null, expected: "var(--haven-font-ui-modern-sans)" },
      { preset: "custom_system", family: "霞鹜文楷", expected: '"霞鹜文楷", var(--haven-font-ui-system)' },
      { preset: "custom_system", family: ");color:red", expected: "var(--haven-font-ui-system)" },
    ]

    for (const { preset, family, expected } of cases) {
      const dispose = applyUiFontSelection(appearanceValue({ uiFontPreset: preset, uiFontFamily: family }), { root })
      expect(root.style.getPropertyValue(CUSTOM_FONT_VARIABLE)).toBe(expected)
      dispose()
      expect(root.style.getPropertyValue(CUSTOM_FONT_VARIABLE)).toBe("")
    }
  })
})

describe("custom font asset", () => {
  it("installs the validated asset behind the internal family, never the display name", async () => {
    const { environment, created, added, deleted } = fontEnvironment()
    const cleanupFont = await installCustomFont(FONT_ASSET_ID, {
      listAssets: async () => validatedFont("思源宋体 <script>"),
      resourceUri: () => FONT_URI,
      environment,
      root: document.documentElement,
    })

    expect(created).toHaveLength(1)
    expect(created[0].family).toBe(CUSTOM_FONT_FAMILY)
    expect(created[0].source).toBe(`url("${FONT_URI}")`)
    expect(created[0].source).not.toContain("思源宋体")
    expect(added).toEqual([created[0].face])
    expect(document.documentElement.style.getPropertyValue(CUSTOM_FONT_VARIABLE)).toBe(
      `"${CUSTOM_FONT_FAMILY}", ${CUSTOM_FONT_FALLBACK}`,
    )

    expect(cleanupFont).not.toBeNull()
    if (!cleanupFont) throw new Error("已校验的字体资产必须装载成功")
    cleanupFont()
    expect(deleted).toEqual([created[0].face])
    expect(document.documentElement.style.getPropertyValue(CUSTOM_FONT_VARIABLE)).toBe("")
  })

  it("refuses a missing, mismatched, unvalidated or unreachable asset", async () => {
    const { environment, added } = fontEnvironment()
    const root = document.documentElement
    const install = (overrides: {
      listAssets?: () => Promise<AppearanceAssetsWire>
      resourceUri?: () => string | null
    } = {}) => installCustomFont(FONT_ASSET_ID, { environment, root, ...overrides })

    // 没有选择资产。
    expect(await installCustomFont(null, { environment, root })).toBeNull()
    expect(await installCustomFont(undefined, { environment, root })).toBeNull()
    // 资产不存在 / 种类不匹配 / 未通过校验。
    expect(await install({ listAssets: async () => assetsWire(), resourceUri: () => FONT_URI })).toBeNull()
    expect(
      await install({
        listAssets: async () => assetsWire({ assetId: FONT_ASSET_ID, kind: "static_wallpaper", state: "validated" }),
        resourceUri: () => FONT_URI,
      }),
    ).toBeNull()
    for (const state of ["pending", "rejected"] as const) {
      expect(
        await install({
          listAssets: async () => assetsWire({ assetId: FONT_ASSET_ID, kind: "font", state }),
          resourceUri: () => FONT_URI,
        }),
        `${state} 的资产不得进入运行时投影`,
      ).toBeNull()
    }
    // 资产列表读不到。
    expect(
      await install({
        listAssets: async () => {
          throw new Error("IPC 不可用")
        },
        resourceUri: () => FONT_URI,
      }),
    ).toBeNull()
    // 拿不到受控 URI：非 Tauri、路径、任意 URL、别的资源域都不行。
    for (const uri of [null, "", "C:/Users/me/font.ttf", "https://example.com/font.woff2", `haven-resource://artwork/${FONT_ASSET_ID}`]) {
      expect(
        await install({ listAssets: async () => validatedFont(), resourceUri: () => uri }),
        `${String(uri)} 不得被接受`,
      ).toBeNull()
    }
    // 运行时没有 FontFace 实现。
    expect(
      await installCustomFont(FONT_ASSET_ID, {
        listAssets: async () => validatedFont(),
        resourceUri: () => FONT_URI,
        environment: null,
        root,
      }),
    ).toBeNull()

    expect(added).toEqual([])
    expect(root.style.getPropertyValue(CUSTOM_FONT_VARIABLE)).toBe("")
  })

  it("installs nothing when the font bytes cannot be decoded", async () => {
    const { environment, created, added } = fontEnvironment({ loadRejects: true })
    const root = document.documentElement

    expect(
      await installCustomFont(FONT_ASSET_ID, {
        listAssets: async () => validatedFont(),
        resourceUri: () => FONT_URI,
        environment,
        root,
      }),
    ).toBeNull()

    expect(created).toHaveLength(1)
    expect(added).toEqual([])
    expect(root.style.getPropertyValue(CUSTOM_FONT_VARIABLE)).toBe("")
  })

  it("revokes an installation that lands after the settings already changed", async () => {
    const { environment, added, deleted } = fontEnvironment()
    const root = document.documentElement
    const dispose = applyCustomFont(FONT_ASSET_ID, {
      listAssets: async () => validatedFont(),
      resourceUri: () => FONT_URI,
      environment,
      root,
    })

    // 资产切换发生在 FontFace 加载完成之前：过期的安装必须被立刻撤销。
    dispose()

    await waitFor(() => expect(deleted.length).toBeGreaterThan(0))
    expect(deleted).toEqual([added[0]])
    expect(root.style.getPropertyValue(CUSTOM_FONT_VARIABLE)).toBe("")
  })

  it("never manufactures a resource URI for an opaque asset id outside Tauri", () => {
    expect(appearanceAssetRequestUri(FONT_ASSET_ID)).toBeNull()
    expect(appearanceAssetRequestUri("C:/Users/secret/font.ttf")).toBeNull()
    expect(appearanceAssetRequestUri("../../etc/passwd")).toBeNull()
  })

  it("never lets an obsolete overlapping install tear down the font installed after it", async () => {
    const { environment, created, added, deleted } = fontEnvironment()
    const root = document.documentElement
    const firstGate = deferred<AppearanceAssetsWire>()
    const secondGate = deferred<AppearanceAssetsWire>()
    const gates = [firstGate, secondGate]
    let call = 0
    const deps = {
      listAssets: () => gates[call++].promise,
      resourceUri: (assetId: string) => (assetId === FONT_ASSET_ID ? FONT_URI : SECOND_FONT_URI),
      environment,
      root,
    }

    // 两次安装重叠：第二次先落地，变量与 face 都属于它。
    const disposeFirst = applyCustomFont(FONT_ASSET_ID, deps)
    const disposeSecond = applyCustomFont(SECOND_FONT_ASSET_ID, deps)
    secondGate.resolve(assetsWire({ assetId: SECOND_FONT_ASSET_ID, kind: "font", state: "validated" }))
    await waitFor(() => expect(added).toHaveLength(1))
    const newerFace = added[0]
    expect(root.style.getPropertyValue(CUSTOM_FONT_VARIABLE)).toBe(FONT_VARIABLE_VALUE)

    // 第一次安装现在才过期落地，而且它自己已经被 dispose 过：它必须什么都不做。
    disposeFirst()
    firstGate.resolve(validatedFont())
    await settleAsyncWork()

    expect(created, "过期安装不得再去读那份已被替换的字体字节").toHaveLength(1)
    expect(added, "过期安装不得把自己加入 document.fonts").toHaveLength(1)
    expect(added[0]).toBe(newerFace)
    expect(
      root.style.getPropertyValue(CUSTOM_FONT_VARIABLE),
      "过期安装不得删掉新安装写好的 CSS 变量",
    ).toBe(FONT_VARIABLE_VALUE)
    expect(deleted, "过期安装不得删掉新安装的 FontFace").toEqual([])

    // 新安装自己的清理照常生效。
    await settleAsyncWork()
    disposeSecond()
    expect(root.style.getPropertyValue(CUSTOM_FONT_VARIABLE)).toBe("")
    expect(deleted).toEqual([newerFace])
  })

  it("keeps exactly one face and leaves the variable to the newest install", async () => {
    const { environment, added, deleted } = fontEnvironment()
    const root = document.documentElement
    const firstGate = deferred<AppearanceAssetsWire>()
    const secondGate = deferred<AppearanceAssetsWire>()
    const gates = [firstGate, secondGate]
    let call = 0
    const deps = {
      listAssets: () => gates[call++].promise,
      resourceUri: (assetId: string) => (assetId === FONT_ASSET_ID ? FONT_URI : SECOND_FONT_URI),
      environment,
      root,
    }

    // 反向顺序：旧的先落地并成了当前安装，新的随后接管。
    const disposeFirst = applyCustomFont(FONT_ASSET_ID, deps)
    firstGate.resolve(validatedFont())
    await waitFor(() => expect(added).toHaveLength(1))
    const olderFace = added[0]

    const disposeSecond = applyCustomFont(SECOND_FONT_ASSET_ID, deps)
    secondGate.resolve(assetsWire({ assetId: SECOND_FONT_ASSET_ID, kind: "font", state: "validated" }))
    await waitFor(() => expect(added).toHaveLength(2))
    const newerFace = added[1]
    expect(deleted, "新安装接管时撤掉旧 face：同一族名下不留两份").toEqual([olderFace])

    // 被顶掉的安装再调用自己的清理：不得删变量，也不得碰新安装的 face。
    disposeFirst()
    expect(root.style.getPropertyValue(CUSTOM_FONT_VARIABLE)).toBe(FONT_VARIABLE_VALUE)
    expect(deleted).toEqual([olderFace])

    disposeSecond()
    expect(root.style.getPropertyValue(CUSTOM_FONT_VARIABLE)).toBe("")
    expect(deleted).toEqual([olderFace, newerFace])
  })

  it("rolls the loaded face back when the CSS variable cannot be written", async () => {
    const { environment, created, added, deleted } = fontEnvironment()
    // 只让变量写入失败：face 已经加入 document.fonts 之后才轮到它。
    const root = {
      style: {
        setProperty() {
          throw new Error("inline 样式写入被拒绝")
        },
      },
    } as unknown as HTMLElement

    const cleanup = await installCustomFont(FONT_ASSET_ID, {
      listAssets: async () => validatedFont(),
      resourceUri: () => FONT_URI,
      environment,
      root,
    })

    expect(cleanup).toBeNull()
    expect(created).toHaveLength(1)
    expect(added).toEqual([created[0].face])
    expect(deleted, "变量写不进去就不能留下没有变量指向的 face").toEqual([created[0].face])
  })
})

// ---------- 壁纸投影（纯决策） ----------

describe("wallpaper projection", () => {
  it("accepts only controlled appearance resource URIs", () => {
    expect(isControlledAppearanceUri(CONTROLLED_URI)).toBe(true)
    expect(isControlledAppearanceUri(`http://haven-resource.appearance/${STATIC_ASSET_ID}`)).toBe(true)

    for (const rejected of [
      "C:/Users/me/wallpaper.png",
      "file:///wallpapers/a.png",
      "https://example.com/a.mp4",
      `haven-resource://artwork/${STATIC_ASSET_ID}`,
      `haven-resource://appearance/${STATIC_ASSET_ID}?path=/etc/passwd`,
      `haven-resource://appearance/${STATIC_ASSET_ID.toUpperCase()}`,
      `/appearance/${STATIC_ASSET_ID}`,
      "",
      null,
      undefined,
    ]) {
      expect(isControlledAppearanceUri(rejected), `${String(rejected)} 不得被当成受控地址`).toBe(false)
    }
  })

  it("returns a layer only for a validated asset behind a controlled URI", () => {
    expect(
      resolveWallpaperLayer({
        selection: { kind: "static", assetId: STATIC_ASSET_ID },
        reducedMotion: false,
        assetValidated: true,
        resourceUri: CONTROLLED_URI,
      }),
    ).toEqual({ kind: "static", uri: CONTROLLED_URI })

    expect(
      resolveWallpaperLayer({
        selection: { kind: "dynamic", assetId: DYNAMIC_ASSET_ID },
        reducedMotion: false,
        assetValidated: true,
        resourceUri: `haven-resource://appearance/${DYNAMIC_ASSET_ID}`,
      }),
    ).toEqual({ kind: "dynamic", uri: `haven-resource://appearance/${DYNAMIC_ASSET_ID}` })
  })

  it("falls back to no wallpaper for none, unvalidated, malformed or hostile selections", () => {
    const base = { reducedMotion: false, assetValidated: true, resourceUri: CONTROLLED_URI }

    expect(resolveWallpaperLayer({ ...base, selection: { kind: "none" } })).toBeNull()
    expect(resolveWallpaperLayer({ ...base, selection: undefined })).toBeNull()
    expect(resolveWallpaperLayer({ ...base, selection: null })).toBeNull()
    expect(
      resolveWallpaperLayer({ ...base, assetValidated: false, selection: { kind: "static", assetId: STATIC_ASSET_ID } }),
    ).toBeNull()
    expect(
      resolveWallpaperLayer({ ...base, resourceUri: "C:/Users/me/a.png", selection: { kind: "static", assetId: STATIC_ASSET_ID } }),
    ).toBeNull()
    expect(
      resolveWallpaperLayer({ ...base, resourceUri: null, selection: { kind: "static", assetId: STATIC_ASSET_ID } }),
    ).toBeNull()

    // 形状自洽：none 不得携带 assetId，static/dynamic 必须携带，且不得多写 path/url。
    for (const selection of [
      { kind: "none", assetId: STATIC_ASSET_ID },
      { kind: "static" },
      { kind: "video", assetId: STATIC_ASSET_ID },
      { kind: "static", assetId: STATIC_ASSET_ID, path: "C:/a.png" },
      { kind: "static", assetId: STATIC_ASSET_ID, url: "https://example.com/a.png" },
      { kind: "static", assetId: "C:/a.png" },
    ]) {
      const malformed = selection as unknown as WallpaperSelection
      expect(resolveWallpaperLayer({ ...base, selection: malformed })).toBeNull()
      expect(wallpaperAssetKind(malformed)).toBeNull()
      expect(wallpaperAssetId(malformed)).toBeNull()
    }
  })

  it("derives the asset kind and id from a well-formed selection", () => {
    expect(wallpaperAssetKind({ kind: "static", assetId: STATIC_ASSET_ID })).toBe("static_wallpaper")
    expect(wallpaperAssetKind({ kind: "dynamic", assetId: DYNAMIC_ASSET_ID })).toBe("dynamic_wallpaper")
    expect(wallpaperAssetKind({ kind: "none" })).toBeNull()
    expect(wallpaperAssetId({ kind: "static", assetId: STATIC_ASSET_ID })).toBe(STATIC_ASSET_ID)
    expect(wallpaperAssetId({ kind: "none" })).toBeNull()
  })

  it("keeps a static wallpaper under reduced motion but drops a dynamic one", () => {
    expect(
      resolveWallpaperLayer({
        selection: { kind: "static", assetId: STATIC_ASSET_ID },
        reducedMotion: true,
        assetValidated: true,
        resourceUri: CONTROLLED_URI,
      }),
    ).toEqual({ kind: "static", uri: CONTROLLED_URI })

    // 减少动效下动态壁纸一律不渲染：本轮不伪造静帧，也不留一个永远不播放的元素。
    expect(
      resolveWallpaperLayer({
        selection: { kind: "dynamic", assetId: DYNAMIC_ASSET_ID },
        reducedMotion: true,
        assetValidated: true,
        resourceUri: `haven-resource://appearance/${DYNAMIC_ASSET_ID}`,
      }),
    ).toBeNull()
  })

  it("rejects unknown, mismatched or unvalidated assets in the list projection", () => {
    const listed = assetsWire(
      { assetId: FONT_ASSET_ID, kind: "font", state: "validated" },
      { assetId: STATIC_ASSET_ID, kind: "static_wallpaper", state: "pending" },
    )

    expect(isAppearanceAssetValidated(listed, FONT_ASSET_ID, "font")).toBe(true)
    expect(isAppearanceAssetValidated(listed, STATIC_ASSET_ID, "static_wallpaper")).toBe(false)
    expect(isAppearanceAssetValidated(listed, STATIC_ASSET_ID, "font")).toBe(false)
    expect(isAppearanceAssetValidated(listed, DYNAMIC_ASSET_ID, "dynamic_wallpaper")).toBe(false)
    expect(isAppearanceAssetValidated(undefined, FONT_ASSET_ID, "font")).toBe(false)
    expect(
      isAppearanceAssetValidated(
        { schemaVersion: 1, assets: null } as unknown as AppearanceAssetsWire,
        FONT_ASSET_ID,
        "font",
      ),
    ).toBe(false)
  })
})

// ---------- 壁纸投影（React 侧） ----------

describe("wallpaper runtime", () => {
  const controlledUri = (assetId: string) => `haven-resource://appearance/${assetId}`

  it("publishes the layer only after the backend confirms the asset", async () => {
    const gate = gatedAssetList()
    const { result } = renderHook(() =>
      useAppearanceWallpaper(
        appearanceValue({ wallpaper: { kind: "static", assetId: STATIC_ASSET_ID } }),
        { listAssets: gate.listAssets, resourceUri: controlledUri },
      ),
    )

    expect(gate.calls, "只按壁纸种类查询资产").toEqual([{ kind: "static_wallpaper" }])
    expect(result.current, "未经确认的资产不放行").toBeNull()

    await act(async () =>
      gate.settle(assetsWire({ assetId: STATIC_ASSET_ID, kind: "static_wallpaper", state: "validated" })),
    )
    expect(result.current).toEqual({ kind: "static", uri: controlledUri(STATIC_ASSET_ID) })
  })

  it("degrades to no wallpaper when the asset list is unreachable", async () => {
    const gate = gatedAssetList()
    const { result } = renderHook(() =>
      useAppearanceWallpaper(
        appearanceValue({ wallpaper: { kind: "dynamic", assetId: DYNAMIC_ASSET_ID } }),
        { listAssets: gate.listAssets, resourceUri: controlledUri },
      ),
    )

    await act(async () => gate.fail(new Error("IPC 不可用")))
    expect(result.current).toBeNull()
  })

  it("never reuses a previous selection's validation for a new one", async () => {
    const gate = gatedAssetList()
    const { result, rerender } = renderHook(
      ({ value }: { value: AppearanceSettingsValue }) =>
        useAppearanceWallpaper(value, { listAssets: gate.listAssets, resourceUri: controlledUri }),
      { initialProps: { value: appearanceValue({ wallpaper: { kind: "static", assetId: STATIC_ASSET_ID } }) } },
    )

    await act(async () =>
      gate.settle(assetsWire({ assetId: STATIC_ASSET_ID, kind: "static_wallpaper", state: "validated" })),
    )
    expect(result.current).toEqual({ kind: "static", uri: controlledUri(STATIC_ASSET_ID) })

    // 换成另一个资产：新的资产尚未确认，上一次的 validated 不得被复用。
    rerender({ value: appearanceValue({ wallpaper: { kind: "dynamic", assetId: DYNAMIC_ASSET_ID } }) })
    expect(result.current).toBeNull()
    expect(gate.calls).toEqual([{ kind: "static_wallpaper" }, { kind: "dynamic_wallpaper" }])

    await act(async () => gate.settle(assetsWire()))
    expect(result.current).toBeNull()
  })

  it("re-verifies the asset when the selection leaves and comes back to the same one", async () => {
    const gate = gatedAssetList()
    const { result, rerender } = renderHook(
      ({ value }: { value: AppearanceSettingsValue }) =>
        useAppearanceWallpaper(value, { listAssets: gate.listAssets, resourceUri: controlledUri }),
      { initialProps: { value: appearanceValue({ wallpaper: { kind: "static", assetId: STATIC_ASSET_ID } }) } },
    )

    await act(async () =>
      gate.settle(assetsWire({ assetId: STATIC_ASSET_ID, kind: "static_wallpaper", state: "validated" })),
    )
    expect(result.current).toEqual({ kind: "static", uri: controlledUri(STATIC_ASSET_ID) })

    // 切到「无壁纸」：上一份校验结论必须当场作废，而不是留着等下一次选择复用。
    rerender({ value: appearanceValue({ wallpaper: { kind: "none" } }) })
    expect(result.current).toBeNull()

    // 再切回同一个资产：这是一次新的选择（assetId 相同不等于还是刚才那次选择），
    // 在它自己的资产列表请求落地之前，上一份 validated 不得放行任何一层。
    rerender({ value: appearanceValue({ wallpaper: { kind: "static", assetId: STATIC_ASSET_ID } }) })
    expect(result.current, "旧结论不得用于新的选择").toBeNull()
    expect(gate.calls).toEqual([{ kind: "static_wallpaper" }, { kind: "static_wallpaper" }])

    await act(async () =>
      gate.settle(assetsWire({ assetId: STATIC_ASSET_ID, kind: "static_wallpaper", state: "validated" })),
    )
    expect(result.current).toEqual({ kind: "static", uri: controlledUri(STATIC_ASSET_ID) })
  })

  it("drops a dynamic wallpaper under reduced motion, from the setting or the system", async () => {
    const dynamicSelection = appearanceValue({ wallpaper: { kind: "dynamic", assetId: DYNAMIC_ASSET_ID } })
    const validated = assetsWire({ assetId: DYNAMIC_ASSET_ID, kind: "dynamic_wallpaper", state: "validated" })

    stubReducedMotionPreference(true)

    const bySystem = gatedAssetList()
    const systemHook = renderHook(() =>
      useAppearanceWallpaper(dynamicSelection, {
        listAssets: bySystem.listAssets,
        resourceUri: controlledUri,
      }),
    )
    await act(async () => bySystem.settle(validated))
    expect(systemHook.result.current, "系统 prefers-reduced-motion 必须兜底").toBeNull()

    const bySetting = gatedAssetList()
    const settingHook = renderHook(() =>
      useAppearanceWallpaper(dynamicSelection, {
        listAssets: bySetting.listAssets,
        resourceUri: controlledUri,
        reducedMotion: true,
      }),
    )
    await act(async () => bySetting.settle(validated))
    expect(settingHook.result.current).toBeNull()

    // 对照：显式注入 reducedMotion=false 时同一份资产照常投影——既证明上面两条不是
    // 「本来就投影不出来」，也钉住注入值优先于系统偏好这一层 deps 语义。
    const allowed = gatedAssetList()
    const allowedHook = renderHook(() =>
      useAppearanceWallpaper(dynamicSelection, {
        listAssets: allowed.listAssets,
        resourceUri: controlledUri,
        reducedMotion: false,
      }),
    )
    await act(async () => allowed.settle(validated))
    expect(allowedHook.result.current).toEqual({ kind: "dynamic", uri: controlledUri(DYNAMIC_ASSET_ID) })
  })

  it("projects nothing when no wallpaper is selected", () => {
    const gate = gatedAssetList()
    const { result } = renderHook(() =>
      useAppearanceWallpaper(appearanceValue(), {
        listAssets: gate.listAssets,
        resourceUri: controlledUri,
      }),
    )

    expect(result.current).toBeNull()
    expect(gate.calls, "无壁纸时不查询资产").toEqual([])
  })
})

// ---------- 壁纸重试信号 ----------

describe("wallpaper retry signal", () => {
  it("carries no address: the retry entry point cannot name a path or a URL", () => {
    // 重试的形状就是「不接受任何参数」。它重新请求的永远是当前设置里那份受控 URI，
    // 因此不存在「重试到另一个资源上」的入口，也没有可注入的第二个地址。
    expect(requestWallpaperRetry.length, "重试不得接受任何参数").toBe(0)
  })

  it("bumps a monotonic epoch and notifies every subscriber exactly once", () => {
    const before = currentWallpaperRetryEpoch()
    const first = vi.fn()
    const second = vi.fn()
    const unsubscribeFirst = subscribeWallpaperRetry(first)
    const unsubscribeSecond = subscribeWallpaperRetry(second)

    requestWallpaperRetry()

    expect(currentWallpaperRetryEpoch()).toBe(before + 1)
    expect(first).toHaveBeenCalledTimes(1)
    expect(second).toHaveBeenCalledTimes(1)

    // 取消订阅之后不再收到通知：卸载的消费者不会留下悬挂监听。
    unsubscribeFirst()
    requestWallpaperRetry()
    expect(first).toHaveBeenCalledTimes(1)
    expect(second).toHaveBeenCalledTimes(2)
    unsubscribeSecond()
  })

  it("hands one retry to both consumers through the same subscription", async () => {
    // 设置页的预览与 AppShell 的首页层读的是同一个代数：一次重试两边都看到新的一代，
    // 因此「预览恢复了、首页还是空的」这种一半状态不会出现。
    const preview = renderHook(() => useWallpaperRetryEpoch())
    const shell = renderHook(() => useWallpaperRetryEpoch())
    const before = preview.result.current
    expect(shell.result.current).toBe(before)

    await act(async () => {
      requestWallpaperRetry()
    })

    expect(preview.result.current).toBe(before + 1)
    expect(shell.result.current).toBe(before + 1)
  })

  it("keeps the retry in the render process only: nothing is persisted and a fresh module starts at zero", async () => {
    const storedBefore =
      Object.keys(localStorage).length + Object.keys(sessionStorage).length

    requestWallpaperRetry()

    expect(currentWallpaperRetryEpoch()).toBeGreaterThan(0)
    expect(
      Object.keys(localStorage).length + Object.keys(sessionStorage).length,
      "重试不写任何存储",
    ).toBe(storedBefore)

    // 重试不是一份设置：重新装载模块（等价于下一次启动）不会记得它。
    vi.resetModules()
    const fresh = await import("./appearance-runtime")
    expect(fresh.currentWallpaperRetryEpoch(), "重试状态不得跨模块实例存活").toBe(0)
  })
})

describe("homepage wallpaper failure signal", () => {
  it("shares only a current controlled failure and ignores duplicate or stale reports", () => {
    const epoch = currentWallpaperRetryEpoch()
    const uri = "haven-resource://appearance/" + STATIC_ASSET_ID
    const previous = currentHomeWallpaperFailure()
    const listener = vi.fn()
    const unsubscribe = subscribeHomeWallpaperFailure(listener)
    const view = renderHook(() => useHomeWallpaperFailure())

    act(() => {
      reportHomeWallpaperUnavailable("https://example.invalid/wallpaper.png", epoch)
    })
    expect(currentHomeWallpaperFailure()).toBe(previous)

    act(() => {
      reportHomeWallpaperUnavailable(uri, epoch)
      reportHomeWallpaperUnavailable(uri, epoch)
    })
    const failure = currentHomeWallpaperFailure()
    expect(failure).toEqual({ uri, epoch })
    expect(view.result.current).toBe(failure)
    expect(listener).toHaveBeenCalledTimes(1)

    act(() => requestWallpaperRetry())
    act(() => reportHomeWallpaperUnavailable(uri, epoch))
    expect(currentWallpaperRetryEpoch()).toBe(epoch + 1)
    expect(currentHomeWallpaperFailure()).toBe(failure)
    expect(listener).toHaveBeenCalledTimes(1)
    unsubscribe()
  })
})
