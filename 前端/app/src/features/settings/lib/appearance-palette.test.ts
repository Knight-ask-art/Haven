// @ts-expect-error node:fs 不在本工程可解析的模块里（types 里补上 "node" 后即可删除本行）
import { readFileSync } from "node:fs"
import { describe, expect, it } from "vitest"
import { guardAppTheme } from "@/lib/ipc/settings-wire"
import type { AppearanceSettingsValue, AppThemePalette } from "@/lib/ipc/settings-wire"
import {
  DEFAULT_ACCENT_COLOR,
  DEFAULT_DARK_PALETTE,
  DEFAULT_LIGHT_PALETTE,
  PALETTE_DERIVED_FIELDS,
  PALETTE_EDITABLE_FIELDS,
  PALETTE_FIELDS,
  THEME_PRESETS,
  applyPaletteColorInput,
  defaultAppTheme,
  isPaletteHexColor,
  normalizeAppTheme,
  paletteColorInputValue,
  themePresetAppTheme,
  withAccentColor,
  withPaletteField,
  withPaletteRoleColor,
} from "./appearance-palette"
import { customThemeTokenEntries } from "./appearance-runtime"

/**
 * 与 index.css 的对接测试。
 *
 * 这份默认调色板不是「随手挑的颜色」，而是 index.css 里 19 个 token 在 light / dark
 * 下的解析结果。所以这里真的把 index.css 读进来、按 var() 链解析出最终颜色再比对：
 * index.css 改了 token 而这里没跟上，测试就会失败——而不是让「切到自定义主题」静默
 * 把界面换成另一套颜色。
 *
 * 地址由 import.meta.url 解析，与进程工作目录无关；读不到文件时 readFileSync 直接抛错，
 * 不会静默退化成「空文件也算过」。
 */
const indexCss: string = readFileSync(new URL("../../../index.css", import.meta.url), "utf8")

/** 取 CSS 文本里某个选择器的所有声明块，合并成一张变量表（后者覆盖前者）。 */
function collectDeclarations(selector: string): Map<string, string> {
  const declarations = new Map<string, string>()
  const blockPattern = new RegExp(`${selector.replace(/[.[\]*^$+?()|{}\\]/g, "\\$&")}\\s*\\{([^}]*)\\}`, "g")
  const matches = indexCss.matchAll(blockPattern)
  let found = false
  for (const match of matches) {
    found = true
    const body = match[1] ?? ""
    // 逐个匹配自定义属性，而不是 split(";")：一个 block 的第一项前面通常有
    // 注释，直接 split 会把注释和第一个 token 粘在一起，漏掉例如
    // --ds-color-bg-primary 这种恰好位于分组首项的变量。
    for (const declaration of body.matchAll(/(--[\w-]+)\s*:\s*([^;}]*)/g)) {
      declarations.set(declaration[1] as string, (declaration[2] ?? "").trim())
    }
  }
  if (!found) throw new Error(`index.css 里找不到 ${selector} 的声明块`)
  return declarations
}

const rootDeclarations = collectDeclarations(":root")
const darkDeclarations = new Map([...rootDeclarations, ...collectDeclarations(".dark")])

/** 沿 var() 链解析一个变量的最终取值；链断掉时抛错（不静默返回空串）。 */
function resolveVariable(name: string, scope: Map<string, string>): string {
  const seen = new Set<string>()
  let current = name
  for (;;) {
    if (seen.has(current)) throw new Error(`变量 ${name} 出现循环引用`)
    seen.add(current)
    const raw = scope.get(current)
    if (raw === undefined) throw new Error(`index.css 里没有定义 ${current}`)
    const reference = /^var\(\s*(--[\w-]+)\s*\)$/.exec(raw)
    if (!reference) return raw
    current = reference[1] as string
  }
}

/** 把 `#rrggbb` / `rgb()` / `rgba()` 归一成调色板约定的 8 位或 6 位小写十六进制。 */
function toPaletteHex(raw: string): string {
  if (/^#[0-9a-fA-F]{6}$/.test(raw)) return raw.toLowerCase()
  if (/^#[0-9a-fA-F]{8}$/.test(raw)) return raw.toLowerCase()
  const functional = /^rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)(?:[,/\s]+([\d.]+))?\s*\)$/.exec(raw)
  if (!functional) throw new Error(`无法归一成调色板颜色的取值：${raw}`)
  const [, red, green, blue, alpha] = functional
  const channel = (value: string) => Number.parseInt(value, 10).toString(16).padStart(2, "0")
  const hex = `#${channel(red as string)}${channel(green as string)}${channel(blue as string)}`
  if (alpha === undefined) return hex
  return `${hex}${Math.round(Number.parseFloat(alpha) * 255).toString(16).padStart(2, "0")}`
}

/** index.css 的 token 名与调色板字段名一一对应（`cardForeground` → `--card-foreground`）。 */
function tokenNameFor(field: keyof AppThemePalette): string {
  return `--${field.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`)}`
}

describe("appearance palette seeds", () => {
  it("mirrors every light token that index.css resolves under :root", () => {
    for (const [field] of PALETTE_FIELDS) {
      const expected = toPaletteHex(resolveVariable(tokenNameFor(field), rootDeclarations))
      expect(DEFAULT_LIGHT_PALETTE[field], `light.${field}`).toBe(expected)
    }
  })

  it("mirrors every dark token that index.css resolves under .dark", () => {
    for (const [field] of PALETTE_FIELDS) {
      const expected = toPaletteHex(resolveVariable(tokenNameFor(field), darkDeclarations))
      expect(DEFAULT_DARK_PALETTE[field], `dark.${field}`).toBe(expected)
    }
  })

  it("seeds the accent from the same token that drives the light focus ring", () => {
    expect(DEFAULT_ACCENT_COLOR).toBe(DEFAULT_LIGHT_PALETTE.ring)
    expect(defaultAppTheme().accentColor).toBe(DEFAULT_ACCENT_COLOR)
  })

  it("covers all 19 palette fields exactly once", () => {
    const fields = PALETTE_FIELDS.map(([field]) => field)
    expect(new Set(fields).size).toBe(fields.length)
    expect(fields.length).toBe(19)
    expect([...fields].sort()).toEqual(
      [...Object.keys(DEFAULT_LIGHT_PALETTE)].sort(),
    )
    expect([...Object.keys(DEFAULT_DARK_PALETTE)].sort()).toEqual([...fields].sort())
  })
})

describe("appearance palette editing helpers", () => {
  it("only accepts the two canonical hex shapes the wire guard accepts", () => {
    expect(isPaletteHexColor("#3c3c4329")).toBe(true)
    expect(isPaletteHexColor("#007aff")).toBe(true)
    expect(isPaletteHexColor("#007AFF")).toBe(false)
    expect(isPaletteHexColor("#fff")).toBe(false)
    expect(isPaletteHexColor("rgba(60, 60, 67, 0.16)")).toBe(false)
    expect(isPaletteHexColor(null)).toBe(false)
  })

  it("exposes the RGB part to the colour input and keeps the alpha byte on write-back", () => {
    expect(paletteColorInputValue("#3c3c4329")).toBe("#3c3c43")
    expect(paletteColorInputValue("#007aff")).toBe("#007aff")
    expect(applyPaletteColorInput("#3c3c4329", "#112233")).toBe("#11223329")
    expect(applyPaletteColorInput("#007aff", "#112233")).toBe("#112233")
  })

  it("refuses to write a colour the wire guard would reject", () => {
    expect(applyPaletteColorInput("#007aff", "#FFF")).toBeNull()
    expect(applyPaletteColorInput("#007aff", "red")).toBeNull()
    const palette = DEFAULT_LIGHT_PALETTE
    expect(withPaletteField(palette, "background", "not-a-colour")).toBe(palette)
    expect(withPaletteField(palette, "background", "#101010").background).toBe("#101010")
  })

  it("normalises a missing or partial stored theme into an editable one", () => {
    expect(normalizeAppTheme(null)).toEqual(defaultAppTheme())
    const partial = normalizeAppTheme({
      light: { ...DEFAULT_LIGHT_PALETTE, background: "#101010" },
      dark: DEFAULT_DARK_PALETTE,
      accentColor: "#ff0000",
    })
    expect(partial.light.background).toBe("#101010")
    expect(partial.accentColor).toBe("#ff0000")
    expect(partial.dark).toEqual(DEFAULT_DARK_PALETTE)

    // 损坏的强调色回落到默认，而不是把非法值透传给 writeCustomThemeTokens。
    const corrupt = normalizeAppTheme({
      light: DEFAULT_LIGHT_PALETTE,
      dark: DEFAULT_DARK_PALETTE,
      accentColor: "not-a-colour",
    })
    expect(corrupt.accentColor).toBe(DEFAULT_ACCENT_COLOR)
  })

  it("does not let a normalised theme alias the module-level seeds", () => {
    const theme = defaultAppTheme()
    theme.light.background = "#123456"
    theme.dark.background = "#654321"
    expect(DEFAULT_LIGHT_PALETTE.background).toBe("#ffffff")
    expect(DEFAULT_DARK_PALETTE.background).toBe("#000000")
  })

  it("provides the four user-facing themes as valid, independent light/dark palettes", () => {
    expect(THEME_PRESETS.map(({ label }) => label)).toEqual(["墨黑", "燕麦", "黛青", "秋栗"])
    for (const preset of THEME_PRESETS) {
      expect(guardAppTheme(preset.theme), preset.label).toBe(true)
      expect(preset.theme.light.primary).toBe(preset.theme.accentColor)
      expect(preset.theme.light.ring).toBe(preset.theme.accentColor)
      expect(preset.theme.dark.primary).toBe(preset.theme.accentColor)
      expect(preset.theme.dark.ring).toBe(preset.theme.accentColor)
      expect(preset.theme.accentColor).not.toBe("#007aff")
    }

    const copy = themePresetAppTheme("oatmeal")
    copy.light.background = "#123456"
    expect(THEME_PRESETS.find(({ id }) => id === "oatmeal")?.theme.light.background).toBe("#f4efe6")
  })

  it("edits familiar color roles and keeps related card and text surfaces together", () => {
    const base = DEFAULT_LIGHT_PALETTE
    const card = withPaletteRoleColor(base, "card", "#123456")
    expect(card.card).toBe("#123456")
    expect(card.popover).toBe("#123456")

    const text = withPaletteRoleColor(base, "foreground", "#234567")
    expect(text.foreground).toBe("#234567")
    expect(text.cardForeground).toBe("#234567")
    expect(text.popoverForeground).toBe("#234567")

    expect(withPaletteRoleColor(base, "background", "red")).toBe(base)
  })
})

/**
 * 「可编辑但改了没有任何效果」是这次要消灭的东西。
 *
 * 运行时的契约（`customThemeTokenEntries`）会把 `--primary` 与 `--ring` 覆盖成强调色，
 * 因此调色板里这两个字段**不能**做成可编辑控件；其余 17 个字段则必须每一个都真的改变
 * 投影出来的 token。这两件事一起才有意义：只钉住「有 17 个输入框」会漏掉一个不生效的
 * 控件，只钉住「强调色驱动主色」又会漏掉别的字段被覆盖。
 */
describe("自定义调色板的可编辑面", () => {
  const accentDriven = PALETTE_DERIVED_FIELDS.map(([field]) => field)
  const editable = PALETTE_EDITABLE_FIELDS.map(([field]) => field)

  function appearanceValue(
    palette: AppThemePalette,
    accentColor = defaultAppTheme().accentColor,
  ): AppearanceSettingsValue {
    return {
      section: "appearance",
      theme: "custom",
      density: "comfortable",
      sidebar: "auto",
      reduceMotion: false,
      interfaceFontMode: "system",
      customTheme: { ...defaultAppTheme(), light: palette, accentColor },
      wallpaper: { kind: "none" },
      customFontAssetId: null,
    }
  }

  it("splits the 19 palette fields into exactly the editable ones and the accent-driven ones", () => {
    expect(editable).toHaveLength(17)
    expect(accentDriven).toEqual(["primary", "ring"])
    expect(editable.some((field) => accentDriven.includes(field))).toBe(false)
    expect([...editable, ...accentDriven].sort()).toEqual(
      PALETTE_FIELDS.map(([field]) => field).sort(),
    )
  })

  it("makes every editable control change the token it claims to change", () => {
    const base = defaultAppTheme()
    for (const [field] of PALETTE_EDITABLE_FIELDS) {
      const tokens = customThemeTokenEntries(
        appearanceValue(withPaletteField(base.light, field, "#123456")),
        false,
      )
      expect(tokens[tokenNameFor(field)], `${field} 的控件必须真的改变它的 token`).toBe("#123456")
    }
  })

  it("keeps the accent-driven fields out of the editor instead of leaving them editable", () => {
    const base = defaultAppTheme()
    for (const [field] of PALETTE_DERIVED_FIELDS) {
      const tokens = customThemeTokenEntries(
        appearanceValue(withPaletteField(base.light, field, "#123456")),
        false,
      )
      expect(
        tokens[tokenNameFor(field)],
        `${field} 由强调色接管：改调色板里的值不得生效，因此界面上不能给它可编辑控件`,
      ).toBe(base.accentColor)
    }
  })

  it("writes the accent into the two palette fields it drives", () => {
    const next = withAccentColor(defaultAppTheme(), "#ff2d55")
    expect(next.accentColor).toBe("#ff2d55")
    for (const palette of [next.light, next.dark]) {
      expect(palette.primary).toBe("#ff2d55")
      expect(palette.ring).toBe("#ff2d55")
    }
    // 投影出来的主色与焦点环与之一致：存下来的形态和运行时的结论不会各说各话。
    const tokens = customThemeTokenEntries(appearanceValue(next.light, next.accentColor), false)
    expect(tokens["--primary"]).toBe("#ff2d55")
    expect(tokens["--ring"]).toBe("#ff2d55")

    // 输入不是规范十六进制时原样返回，不制造坏颜色。
    const theme = defaultAppTheme()
    expect(withAccentColor(theme, "red")).toBe(theme)
    expect(withAccentColor(theme, "#FFF")).toBe(theme)
  })
})
