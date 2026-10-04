import { describe, expect, it } from "vitest"
// 与现有 CSS 源文本测试一致：Vitest 将 CSS ?raw 替换为空串；app tsconfig 不加载 Node 类型。
// @ts-expect-error node:fs 的类型不在 app tsconfig 中，实际读盘只由 Vitest 在 Node 下执行。
import { readFileSync } from "node:fs"
import { DEFAULT_DARK_PALETTE, DEFAULT_LIGHT_PALETTE, THEME_PRESETS } from "./appearance-palette"

const css = readFileSync(new URL("../../../index.css", import.meta.url), "utf8")
const navigationCss = readFileSync(new URL("../components/settings-navigation.css", import.meta.url), "utf8")
const sourcesUi = readFileSync(new URL("../components/SourcesSettings.tsx", import.meta.url), "utf8")
const aiCss = readFileSync(new URL("../components/ai-assistant/ai-assistant.css", import.meta.url), "utf8")

// jsdom 不负责颜色合成；这里保护语义配色的下限，实际 hover/focus 仍由浏览器验收。
function settingsTokens(selector: string): Record<string, string> {
  const blocks = [...css.matchAll(/(?:^|\n)  (:root|\.dark|\[data-haven-theme="custom"\])\s*\{([^}]+)\}/g)]
  const block = blocks.find((match) => match[1] === selector && match[2].includes("--haven-settings-background:"))
  if (!block) throw new Error(`Missing settings palette: ${selector}`)
  return Object.fromEntries([...block[2].matchAll(/(--haven-settings-[\w-]+):\s*([^;]+);/g)].map((match) => [match[1], match[2].trim()]))
}

function luminance(hex: string): number {
  if (!/^#[0-9a-f]{6}$/i.test(hex)) throw new Error(`Expected hex palette color: ${hex}`)
  const rgb = [1, 3, 5].map((offset) => {
    const channel = Number.parseInt(hex.slice(offset, offset + 2), 16) / 255
    return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4
  })
  return rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722
}

function contrast(a: string, b: string): number {
  const [bright, dark] = [luminance(a), luminance(b)].sort((x, y) => y - x)
  return (bright + 0.05) / (dark + 0.05)
}

function customControlColor(token: string, foreground: string, card: string): string {
  const match = token.match(/^color-mix\(in srgb, var\(--foreground\) (\d+)%, var\(--card\)\)$/)
  if (!match) throw new Error(`Expected foreground/card control mix: ${token}`)
  const weight = Number(match[1]) / 100
  return "#" + [1, 3, 5].map((offset) => Math.round(
    Number.parseInt(foreground.slice(offset, offset + 2), 16) * weight
    + Number.parseInt(card.slice(offset, offset + 2), 16) * (1 - weight),
  ).toString(16).padStart(2, "0")).join("")
}

const light = settingsTokens(":root")
const surfaces = ["background", "sidebar", "card", "card-hover", "control"]

describe("设置浅色配色回归", () => {
  it("辅助文字角色保持可读而不折叠为同一个灰度层级", () => {
    const tones = ["muted-strong", "muted", "muted-subtle", "muted-faint"].map((role) => light[`--haven-settings-${role}`])
    expect(new Set(tones).size).toBe(4)
    for (let index = 1; index < tones.length; index += 1) {
      expect(luminance(tones[index])).toBeGreaterThan(luminance(tones[index - 1]))
    }
    expect(contrast(tones[0], tones[3])).toBeGreaterThanOrEqual(1.4)
  })
  it.each(["muted", "muted-strong", "muted-subtle", "muted-faint", "warning"])("%s 在浅色语义面上的文字对比度至少 4.5:1", (role) => {
    for (const surface of surfaces) {
      expect(contrast(light[`--haven-settings-${role}`], light[`--haven-settings-${surface}`]), `${role} on ${surface}`).toBeGreaterThanOrEqual(4.5)
    }
  })

  it("控件轮廓与关闭轨道具有独立配色，不加重装饰分隔线", () => {
    for (const role of ["control-border", "toggle-off", "toggle-off-hover"]) {
      for (const surface of surfaces) {
        expect(contrast(light[`--haven-settings-${role}`], light[`--haven-settings-${surface}`]), `${role} on ${surface}`).toBeGreaterThanOrEqual(3)
      }
    }
    expect(contrast(light["--haven-settings-toggle-thumb"], light["--haven-settings-toggle-off"])).toBeGreaterThanOrEqual(3)
    expect(contrast(light["--haven-settings-primary-foreground"], light["--haven-settings-primary"])).toBeGreaterThanOrEqual(3)
    expect(light["--haven-settings-border"]).toBe("#deddd6")
    expect(light["--haven-settings-border-subtle"]).toBe("rgba(0, 0, 0, 0.04)")
  })

  it("深色与自定义主题不继承浅色关闭轨道和控件边框", () => {
    const dark = settingsTokens(".dark")
    const custom = settingsTokens('[data-haven-theme="custom"]')
    expect(dark["--haven-settings-control-border"]).toBe("var(--haven-settings-border)")
    expect(dark["--haven-settings-toggle-off"]).toBe("var(--haven-settings-control)")
    expect(contrast(dark["--haven-settings-warning"], dark["--haven-settings-background"])).toBeGreaterThanOrEqual(4.5)
    expect(light["--haven-settings-toggle-thumb-on"]).toBe("var(--haven-settings-primary-foreground)")
    expect(custom["--haven-settings-toggle-thumb"]).toBe("var(--card)")
    expect(custom["--haven-settings-warning"]).toBe("var(--foreground)")
    expect(contrast(dark["--haven-settings-primary-foreground"], dark["--haven-settings-primary"])).toBeGreaterThanOrEqual(3)
  })

  it("自定义种子与全部预设的浅深色控件仍可识别，不依赖近乎同色的 secondary", () => {
    const custom = settingsTokens('[data-haven-theme="custom"]')
    const palettes = [DEFAULT_LIGHT_PALETTE, DEFAULT_DARK_PALETTE, ...THEME_PRESETS.flatMap((preset) => [preset.theme.light, preset.theme.dark])]
    for (const palette of palettes) {
      for (const role of ["control-border", "toggle-off", "toggle-off-hover"]) {
        const value = customControlColor(custom[`--haven-settings-${role}`], palette.foreground, palette.card)
        for (const surface of [palette.background, palette.card, palette.secondary]) {
          expect(contrast(value, surface), `${role} on ${surface}`).toBeGreaterThanOrEqual(3)
        }
      }
      expect(contrast(palette.primaryForeground, palette.primary)).toBeGreaterThanOrEqual(3)
    }
  })

  it("添加来源的悬停由按钮角色负责，不把强调色文字留在浅色卡底上", () => {
    expect(sourcesUi).toContain("enabled:hover:bg-[var(--haven-settings-primary-hover)]")
  })

  it("AI 工作台窄窗为顶部导航额外留高，不侵占底部 Dock 安全区", () => {
    expect(aiCss).toMatch(/@media \(max-width: 767px\)\s*\{\s*\.haven-ai-workbench\s*\{ height: min\(849px, calc\(100dvh - 222px\)\); \}/)
    expect(aiCss).toMatch(/@media \(max-width: 420px\)\s*\{\s*\.haven-ai-workbench\s*\{ height: min\(849px, calc\(100dvh - 270px\)\); \}/)
  })

  it("导航的普通 hover 规则排除当前页，保留深底浅字的选中态", () => {
    expect(navigationCss).toContain('.settings-navigation__item:hover:not(:disabled):not([aria-current="page"])')
    expect(navigationCss).not.toMatch(/\.settings-navigation__item:hover:not\(:disabled\)\s*\{/)
    expect(contrast(light["--haven-settings-strong-foreground"], light["--haven-settings-strong-surface"])).toBeGreaterThanOrEqual(4.5)
  })

  it("导航只有文字槽伸展，图标保留自身宽度，不使用匹配所有 span 的布局规则", () => {
    expect(navigationCss).toMatch(/\.settings-navigation__icon\s*\{\s*flex:\s*none;/)
    expect(navigationCss).toMatch(/\.settings-navigation__text\s*\{\s*flex:\s*1;\s*min-width:\s*0;/)
    expect(navigationCss).not.toMatch(/\.settings-navigation__item\s*>\s*span\s*\{/)
  })
})
