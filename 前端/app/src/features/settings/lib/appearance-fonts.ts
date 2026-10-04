import { isSafeUiFontFamilyName, type UiFontPresetWire } from "@/lib/ipc/settings-wire"

export const UI_FONT_SYSTEM_TOKEN = "--haven-font-ui-system"
export const UI_FONT_SERIF_TOKEN = "--haven-font-ui-humanist-serif"
export const UI_FONT_MODERN_TOKEN = "--haven-font-ui-modern-sans"

export const UI_FONT_PRESETS: ReadonlyArray<{ id: UiFontPresetWire; label: string; sample: string }> = [
  { id: "system", label: "跟随系统", sample: "系统字形" },
  { id: "humanist_serif", label: "人文衬线", sample: "宋体 · 文楷" },
  { id: "modern_sans", label: "现代黑体", sample: "清晰 · 利落" },
  { id: "custom_system", label: "自定义系统字体", sample: "本机字体" },
]

const PRESET_STACK_TOKENS: Record<UiFontPresetWire, string> = {
  system: `var(${UI_FONT_SYSTEM_TOKEN})`,
  humanist_serif: `var(${UI_FONT_SERIF_TOKEN})`,
  modern_sans: `var(${UI_FONT_MODERN_TOKEN})`,
  custom_system: `var(${UI_FONT_SYSTEM_TOKEN})`,
}

/** 将已校验的系统字体名放在跟随系统字体栈之前；不接受 CSS 片段或路径。 */
export function uiFontCssStack(
  preset: UiFontPresetWire | undefined,
  family: string | null | undefined,
): string {
  const selectedPreset = preset ?? "system"
  if (selectedPreset === "custom_system" && family && isSafeUiFontFamilyName(family)) {
    return `${JSON.stringify(family)}, var(${UI_FONT_SYSTEM_TOKEN})`
  }
  return PRESET_STACK_TOKENS[selectedPreset]
}
