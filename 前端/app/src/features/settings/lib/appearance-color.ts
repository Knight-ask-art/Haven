export type HslColor = {
  hue: number
  saturation: number
  lightness: number
}

export type RgbColor = {
  red: number
  green: number
  blue: number
}

export const RECOMMENDED_READING_COLORS = [
  { label: "羊皮纸", value: "#F5F1E8" },
  { label: "燕麦灰", value: "#E8E2D8" },
  { label: "浅竹青", value: "#DDE8E0" },
  { label: "炭墨", value: "#292B28" },
] as const

/** 接受常见的 #RRGGBB / #RGB 输入，返回契约使用的规范小写六位 Hex。 */
export function parseHexColor(input: string): string | null {
  const value = input.trim().replace(/^#/, "")
  if (/^[\da-f]{3}$/i.test(value)) {
    const expanded = [...value].map((digit) => digit + digit).join("")
    return "#" + expanded.toLowerCase()
  }
  return /^[\da-f]{6}$/i.test(value) ? "#" + value.toLowerCase() : null
}

export function hexToRgb(input: string): RgbColor | null {
  const hex = parseHexColor(input)
  if (!hex) return null
  return {
    red: Number.parseInt(hex.slice(1, 3), 16),
    green: Number.parseInt(hex.slice(3, 5), 16),
    blue: Number.parseInt(hex.slice(5, 7), 16),
  }
}

export function rgbToHex(color: RgbColor): string {
  const channel = (value: number) => Math.round(Math.min(255, Math.max(0, value))).toString(16).padStart(2, "0")
  return "#" + channel(color.red) + channel(color.green) + channel(color.blue)
}

export function rgbToHsl({ red, green, blue }: RgbColor): HslColor {
  const r = red / 255
  const g = green / 255
  const b = blue / 255
  const max = Math.max(r, g, b)
  const min = Math.min(r, g, b)
  const delta = max - min
  const lightness = (max + min) / 2
  let hue = 0
  let saturation = 0

  if (delta !== 0) {
    saturation = delta / (1 - Math.abs(2 * lightness - 1))
    if (max === r) hue = ((g - b) / delta) % 6
    else if (max === g) hue = (b - r) / delta + 2
    else hue = (r - g) / delta + 4
    hue *= 60
  }

  return {
    hue: (hue + 360) % 360,
    saturation: saturation * 100,
    lightness: lightness * 100,
  }
}

export function hslToRgb({ hue, saturation, lightness }: HslColor): RgbColor {
  const h = ((hue % 360) + 360) % 360
  const s = Math.min(100, Math.max(0, saturation)) / 100
  const l = Math.min(100, Math.max(0, lightness)) / 100
  const chroma = (1 - Math.abs(2 * l - 1)) * s
  const secondary = chroma * (1 - Math.abs((h / 60) % 2 - 1))
  const offset = l - chroma / 2
  let channels: readonly [number, number, number]

  if (h < 60) channels = [chroma, secondary, 0]
  else if (h < 120) channels = [secondary, chroma, 0]
  else if (h < 180) channels = [0, chroma, secondary]
  else if (h < 240) channels = [0, secondary, chroma]
  else if (h < 300) channels = [secondary, 0, chroma]
  else channels = [chroma, 0, secondary]

  const [red, green, blue] = channels.map((channel) => Math.round((channel + offset) * 255))
  return { red, green, blue }
}

export function hexToHsl(input: string): HslColor | null {
  const color = hexToRgb(input)
  return color ? rgbToHsl(color) : null
}

export function hslToHex(color: HslColor): string {
  return rgbToHex(hslToRgb(color))
}

/** WCAG 相对亮度对比度，供阅读提示使用；不拦截或限制用户的自选颜色。 */
export function colorContrastRatio(first: string, second: string): number | null {
  const firstRgb = hexToRgb(first)
  const secondRgb = hexToRgb(second)
  if (!firstRgb || !secondRgb) return null

  const luminance = ({ red, green, blue }: RgbColor) => {
    const linear = [red, green, blue].map((channel) => {
      const value = channel / 255
      return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4
    })
    return linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722
  }

  const firstLuminance = luminance(firstRgb)
  const secondLuminance = luminance(secondRgb)
  const lighter = Math.max(firstLuminance, secondLuminance)
  const darker = Math.min(firstLuminance, secondLuminance)
  return (lighter + 0.05) / (darker + 0.05)
}
