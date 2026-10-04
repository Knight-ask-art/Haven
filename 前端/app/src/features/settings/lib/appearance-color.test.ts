import { describe, expect, it } from "vitest"

import {
  colorContrastRatio,
  hexToHsl,
  hexToRgb,
  hslToHex,
  parseHexColor,
  rgbToHex,
  rgbToHsl,
} from "./appearance-color"

describe("appearance color editing", () => {
  it("accepts pasted six-digit and shorthand Hex values without persisting invalid text", () => {
    expect(parseHexColor(" #F5F5F1 ")).toBe("#f5f5f1")
    expect(parseHexColor("abc")).toBe("#aabbcc")
    expect(parseHexColor("#12xz45")).toBeNull()
    expect(parseHexColor("#12345678")).toBeNull()
    expect(parseHexColor("##123456")).toBeNull()
  })

  it("converts between Hex and RGB without channel drift", () => {
    expect(hexToRgb("#F5F5F1")).toEqual({ red: 245, green: 245, blue: 241 })
    expect(rgbToHex({ red: 245, green: 245, blue: 241 })).toBe("#f5f5f1")
    expect(rgbToHex({ red: -1, green: 256, blue: 15.7 })).toBe("#00ff10")
  })

  it("converts primary colors to HSL and back", () => {
    expect(hexToHsl("#ff0000")).toEqual({ hue: 0, saturation: 100, lightness: 50 })
    expect(rgbToHsl({ red: 0, green: 255, blue: 0 })).toEqual({ hue: 120, saturation: 100, lightness: 50 })
    expect(hslToHex({ hue: 240, saturation: 100, lightness: 50 })).toBe("#0000ff")
    expect(hslToHex({ hue: 360, saturation: 100, lightness: 50 })).toBe("#ff0000")
  })

  it("measures text contrast symmetrically using WCAG relative luminance", () => {
    expect(colorContrastRatio("#ffffff", "#000000")).toBe(21)
    expect(colorContrastRatio("#000000", "#ffffff")).toBe(21)
    expect(colorContrastRatio("#ffffff", "#777777")).toBeLessThan(4.5)
    expect(colorContrastRatio("#ffffff", "#767676")).toBeGreaterThan(4.5)
    expect(colorContrastRatio("invalid", "#000000")).toBeNull()
  })
})
