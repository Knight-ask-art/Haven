import { useEffect, useId, useLayoutEffect, useRef, useState } from "react"
import type { CSSProperties } from "react"
import { X } from "lucide-react"
import { cn } from "@/lib/utils"
import {
  RECOMMENDED_READING_COLORS,
  colorContrastRatio,
  hexToHsl,
  hexToRgb,
  hslToHex,
  parseHexColor,
  rgbToHex,
  rgbToHsl,
  type HslColor,
  type RgbColor,
} from "../lib/appearance-color"

type ColorFormat = "hex" | "rgb" | "hsl"
type ChannelDraft = { first: string; second: string; third: string }
type ContrastTarget = { label: string; color: string }

type AppearanceColorPickerProps = {
  label: string
  value: string
  contrastTargets: readonly ContrastTarget[]
  recentColors: readonly string[]
  onDraftChange: (color: string | null) => void
  onApply: (color: string) => void
  onRecentColor: (color: string) => void
}

function channelsFromRgb(color: RgbColor): ChannelDraft {
  return { first: String(color.red), second: String(color.green), third: String(color.blue) }
}

function channelsFromHsl(color: HslColor): ChannelDraft {
  return {
    first: String(Math.round(color.hue)),
    second: String(Math.round(color.saturation)),
    third: String(Math.round(color.lightness)),
  }
}

function boundedNumber(value: string, max: number, integerOnly: boolean): number | null {
  if (value.trim() === "") return null
  const parsed = Number(value)
  if (!Number.isFinite(parsed) || parsed < 0 || parsed > max) return null
  if (integerOnly && !Number.isInteger(parsed)) return null
  return parsed
}

function rgbFromDraft(draft: ChannelDraft): RgbColor | null {
  const red = boundedNumber(draft.first, 255, true)
  const green = boundedNumber(draft.second, 255, true)
  const blue = boundedNumber(draft.third, 255, true)
  return red === null || green === null || blue === null ? null : { red, green, blue }
}

function hslFromDraft(draft: ChannelDraft): HslColor | null {
  const hue = boundedNumber(draft.first, 360, false)
  const saturation = boundedNumber(draft.second, 100, false)
  const lightness = boundedNumber(draft.third, 100, false)
  return hue === null || saturation === null || lightness === null
    ? null
    : { hue, saturation, lightness }
}

export function AppearanceColorPicker({
  label,
  value,
  contrastTargets,
  recentColors,
  onDraftChange,
  onApply,
  onRecentColor,
}: AppearanceColorPickerProps) {
  const rootId = useId()
  const headingId = rootId + "-heading"
  const rootRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const panelRef = useRef<HTMLDivElement>(null)
  const [open, setOpen] = useState(false)
  const [format, setFormat] = useState<ColorFormat>("hex")
  const [draftColor, setDraftColor] = useState(parseHexColor(value) ?? "#000000")
  const [hexDraft, setHexDraft] = useState(value.toUpperCase())
  const initialRgb = hexToRgb(value) ?? { red: 0, green: 0, blue: 0 }
  const initialHsl = hexToHsl(value) ?? { hue: 0, saturation: 0, lightness: 0 }
  const [rgbDraft, setRgbDraft] = useState(() => channelsFromRgb(initialRgb))
  const [hslDraft, setHslDraft] = useState(() => channelsFromHsl(initialHsl))
  const [hue, setHue] = useState(initialHsl.hue)
  const [saturation, setSaturation] = useState(initialHsl.saturation)
  const [lightness, setLightness] = useState(initialHsl.lightness)
  const [arrowLeft, setArrowLeft] = useState(24)

  const setColorFromHex = (input: string, source: ColorFormat | "range" | "preset") => {
    const color = parseHexColor(input)
    if (!color) return
    const rgb = hexToRgb(color)
    const hsl = rgb ? rgbToHsl(rgb) : null
    if (!rgb || !hsl) return

    setDraftColor(color)
    setHexDraft(color.toUpperCase())
    setHue(hsl.hue)
    setSaturation(hsl.saturation)
    setLightness(hsl.lightness)
    if (source !== "rgb") setRgbDraft(channelsFromRgb(rgb))
    if (source !== "hsl") setHslDraft(channelsFromHsl(hsl))
    onDraftChange(color)
  }

  const openPicker = () => {
    const initial = parseHexColor(value) ?? "#000000"
    const rgb = hexToRgb(initial) ?? { red: 0, green: 0, blue: 0 }
    const hsl = rgbToHsl(rgb)
    setFormat("hex")
    setDraftColor(initial)
    setHexDraft(initial.toUpperCase())
    setRgbDraft(channelsFromRgb(rgb))
    setHslDraft(channelsFromHsl(hsl))
    setHue(hsl.hue)
    setSaturation(hsl.saturation)
    setLightness(hsl.lightness)
    setOpen(true)
    onDraftChange(initial)
  }

  const dismiss = (restoreFocus: boolean) => {
    setOpen(false)
    onDraftChange(null)
    if (restoreFocus) triggerRef.current?.focus()
  }

  useEffect(() => {
    if (!open) return
    const handleOutsidePointer = (event: MouseEvent) => {
      if (event.target instanceof Node && !rootRef.current?.contains(event.target)) dismiss(false)
    }
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault()
        dismiss(true)
      }
    }
    document.addEventListener("mousedown", handleOutsidePointer)
    window.addEventListener("keydown", handleKeyDown)
    return () => {
      document.removeEventListener("mousedown", handleOutsidePointer)
      window.removeEventListener("keydown", handleKeyDown)
    }
  }, [open, onDraftChange])

  useLayoutEffect(() => {
    if (!open) return
    const measureArrow = () => {
      const root = rootRef.current?.getBoundingClientRect()
      const trigger = triggerRef.current?.getBoundingClientRect()
      if (!root || !trigger) return
      const panelWidth = panelRef.current?.getBoundingClientRect().width ?? root.width
      const center = trigger.left + trigger.width / 2 - root.left
      setArrowLeft(Math.max(12, Math.min(panelWidth - 24, center - 7)))
    }
    measureArrow()
    const panel = panelRef.current
    if (panel && panel.getBoundingClientRect().bottom > window.innerHeight - 16) {
      panel.scrollIntoView({ block: "nearest" })
    }
    window.addEventListener("resize", measureArrow)
    return () => window.removeEventListener("resize", measureArrow)
  }, [open])

  const parsedHexDraft = format === "hex" ? parseHexColor(hexDraft) : null
  const parsedRgbDraft = format === "rgb" ? rgbFromDraft(rgbDraft) : null
  const parsedHslDraft = format === "hsl" ? hslFromDraft(hslDraft) : null
  const validInput = format === "hex"
    ? parsedHexDraft !== null
    : format === "rgb"
      ? parsedRgbDraft !== null
      : parsedHslDraft !== null
  const lowContrastTarget = contrastTargets
    .map((target) => ({ ...target, ratio: colorContrastRatio(draftColor, target.color) }))
    .filter((target) => target.ratio !== null && target.ratio < 4.5)
    .sort((first, second) => (first.ratio ?? 0) - (second.ratio ?? 0))[0]

  const handleHexChange = (input: string) => {
    setHexDraft(input)
    const color = parseHexColor(input)
    if (color) setColorFromHex(color, "hex")
  }

  const handleRgbChange = (channel: keyof ChannelDraft, input: string) => {
    const next = { ...rgbDraft, [channel]: input }
    setRgbDraft(next)
    const parsed = rgbFromDraft(next)
    if (parsed) setColorFromHex(rgbToHex(parsed), "rgb")
  }

  const handleHslChange = (channel: keyof ChannelDraft, input: string) => {
    const next = { ...hslDraft, [channel]: input }
    setHslDraft(next)
    const parsed = hslFromDraft(next)
    if (parsed) setColorFromHex(hslToHex(parsed), "hsl")
  }

  const handleRangeChange = (next: HslColor) => {
    setHue(next.hue)
    setSaturation(next.saturation)
    setLightness(next.lightness)
    setColorFromHex(hslToHex(next), "range")
  }

  const handleFormatChange = (next: ColorFormat) => {
    setFormat(next)
    const rgb = hexToRgb(draftColor)
    const hsl = rgb ? rgbToHsl(rgb) : null
    if (rgb) setRgbDraft(channelsFromRgb(rgb))
    if (hsl) setHslDraft(channelsFromHsl(hsl))
    setHexDraft(draftColor.toUpperCase())
  }

  const applyColor = () => {
    if (!validInput) return
    const color = format === "hex"
      ? parsedHexDraft
      : format === "rgb" && parsedRgbDraft
        ? rgbToHex(parsedRgbDraft)
        : parsedHslDraft
          ? hslToHex(parsedHslDraft)
          : null
    if (!color) return
    onApply(color)
    onRecentColor(color)
    dismiss(false)
  }

  const pickerStyle: CSSProperties & {
    "--color-picker-arrow-left"?: string
    "--color-picker-hue"?: string
  } = {
    "--color-picker-arrow-left": arrowLeft + "px",
    "--color-picker-hue": hue + "deg",
  }

  return (
    <div ref={rootRef} className={cn("min-w-0", open && "col-span-2")} data-color-picker-control="">
      <div className="flex min-h-[48px] items-center justify-between gap-2 rounded-[10px] border border-[var(--haven-settings-border-subtle)] px-3 py-2">
        <span className="truncate text-[12px] text-[var(--haven-settings-foreground)]">{label}</span>
        <button
          ref={triggerRef}
          type="button"
          aria-label={label}
          aria-haspopup="dialog"
          aria-expanded={open}
          aria-controls={open ? rootId : undefined}
          title={"当前颜色 " + value.toUpperCase() + "，点击调整"}
          onClick={() => open ? dismiss(false) : openPicker()}
          className="h-8 w-10 shrink-0 rounded-md border border-black/15 shadow-sm outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary)]"
          style={{ backgroundColor: value }}
        />
      </div>

      {/* Reserve space for the anchored panel so it never covers sibling color controls or the live preview. */}
      {open && (
        <div className="relative mt-3" style={pickerStyle}>
          <span
            aria-hidden="true"
            className="absolute -top-[6px] z-10 h-3 w-3 rotate-45 border-l border-t border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-card)]"
            style={{ left: arrowLeft }}
          />
          <div
            id={rootId}
            ref={panelRef}
            role="dialog"
            aria-labelledby={headingId}
            className="settings-color-picker-dialog rounded-[14px] border border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-card)] p-4 shadow-[0_12px_32px_rgba(0,0,0,0.3)]"
          >
            <div className="flex items-start justify-between gap-3">
            <div className="flex min-w-0 items-center gap-3">
              <span
                aria-hidden="true"
                className="h-9 w-9 shrink-0 rounded-lg border border-black/15"
                style={{ backgroundColor: draftColor }}
              />
              <div className="min-w-0">
                <h3 id={headingId} className="text-[13px] font-semibold text-[var(--haven-settings-foreground)]">调整颜色</h3>
                <p className="truncate text-[11px] text-[var(--haven-settings-muted)]">{label} · {draftColor.toUpperCase()}</p>
              </div>
            </div>
            <button
              type="button"
              aria-label="关闭颜色选择器"
              onClick={() => dismiss(true)}
              className="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-full text-[var(--haven-settings-muted-strong)] transition-colors hover:bg-[var(--haven-settings-card-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary)]"
            >
              <X className="h-4 w-4" aria-hidden="true" />
            </button>
          </div>

          <div className="mt-4">
            <p className="mb-2 text-[11px] font-medium text-[var(--haven-settings-muted-strong)]">推荐色</p>
            <div className="grid grid-cols-4 gap-2">
              {RECOMMENDED_READING_COLORS.map((color) => (
                <button
                  key={color.value}
                  type="button"
                  aria-label={color.label + " " + color.value}
                  title={color.label + " " + color.value}
                  onClick={() => setColorFromHex(color.value, "preset")}
                  className="flex min-h-10 min-w-0 items-center gap-2 rounded-lg border border-[var(--haven-settings-control-border)] px-2 text-left transition-colors hover:bg-[var(--haven-settings-card-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary)]"
                >
                  <span aria-hidden="true" className="h-4 w-4 shrink-0 rounded-full border border-black/15" style={{ backgroundColor: color.value }} />
                  <span className="truncate text-[10px] text-[var(--haven-settings-foreground)]">{color.label}</span>
                </button>
              ))}
            </div>
          </div>

          <div className="mt-4">
            <p className="mb-2 text-[11px] font-medium text-[var(--haven-settings-muted-strong)]">最近使用</p>
            {recentColors.length > 0 ? (
              <div className="flex flex-wrap gap-2">
                {recentColors.map((color) => (
                  <button
                    key={color}
                    type="button"
                    aria-label={"最近使用色 " + color.toUpperCase()}
                    title={color.toUpperCase()}
                    onClick={() => setColorFromHex(color, "preset")}
                    className="h-7 w-7 rounded-full border border-black/15 outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary)]"
                    style={{ backgroundColor: color }}
                  />
                ))}
              </div>
            ) : (
              <p className="text-[11px] text-[var(--haven-settings-muted)]">本次外观页会话中应用过的颜色会显示在这里。</p>
            )}
          </div>

          <div className="mt-4 grid gap-3 sm:grid-cols-3">
            <label className="min-w-0 text-[11px] text-[var(--haven-settings-muted-strong)]">
              色相
              <input
                aria-label="色相"
                type="range"
                min="0"
                max="360"
                step="1"
                value={Math.round(hue)}
                onChange={(event) => handleRangeChange({ hue: Number(event.target.value), saturation, lightness })}
                className="settings-color-picker-range settings-color-picker-range--hue mt-1 block"
              />
            </label>
            <label className="min-w-0 text-[11px] text-[var(--haven-settings-muted-strong)]">
              饱和度
              <input
                aria-label="饱和度"
                type="range"
                min="0"
                max="100"
                step="1"
                value={Math.round(saturation)}
                onChange={(event) => handleRangeChange({ hue, saturation: Number(event.target.value), lightness })}
                className="settings-color-picker-range settings-color-picker-range--saturation mt-1 block"
              />
            </label>
            <label className="min-w-0 text-[11px] text-[var(--haven-settings-muted-strong)]">
              明度
              <input
                aria-label="明度"
                type="range"
                min="0"
                max="100"
                step="1"
                value={Math.round(lightness)}
                onChange={(event) => handleRangeChange({ hue, saturation, lightness: Number(event.target.value) })}
                className="settings-color-picker-range settings-color-picker-range--lightness mt-1 block"
              />
            </label>
          </div>

          <div className="mt-4 flex min-w-0 items-center gap-2">
            {format === "hex" ? (
              <input
                aria-label="Hex 色值"
                type="text"
                autoComplete="off"
                spellCheck={false}
                maxLength={7}
                value={hexDraft}
                onChange={(event) => handleHexChange(event.target.value)}
                className="h-10 min-w-0 flex-1 rounded-lg border border-[var(--haven-settings-control-border)] bg-[var(--haven-settings-control)] px-3 font-mono text-[12px] uppercase text-[var(--haven-settings-foreground)] outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary)]"
              />
            ) : (
              <div className="grid min-w-0 flex-1 grid-cols-3 gap-2">
                {[
                  { key: "first", label: format === "rgb" ? "R" : "H", max: format === "rgb" ? "255" : "360", step: format === "rgb" ? "1" : "any" },
                  { key: "second", label: format === "rgb" ? "G" : "S", max: format === "rgb" ? "255" : "100", step: format === "rgb" ? "1" : "any" },
                  { key: "third", label: format === "rgb" ? "B" : "L", max: format === "rgb" ? "255" : "100", step: format === "rgb" ? "1" : "any" },
                ].map((channel) => (
                  <label key={channel.key} className="min-w-0 text-center text-[10px] text-[var(--haven-settings-muted)]">
                    {channel.label}
                    <input
                      aria-label={channel.label}
                      type="number"
                      min="0"
                      max={channel.max}
                      step={channel.step}
                      value={format === "rgb" ? rgbDraft[channel.key as keyof ChannelDraft] : hslDraft[channel.key as keyof ChannelDraft]}
                      onChange={(event) => format === "rgb"
                        ? handleRgbChange(channel.key as keyof ChannelDraft, event.target.value)
                        : handleHslChange(channel.key as keyof ChannelDraft, event.target.value)}
                      className="mt-1 h-9 w-full min-w-0 rounded-lg border border-[var(--haven-settings-control-border)] bg-[var(--haven-settings-control)] px-1 text-center text-[12px] text-[var(--haven-settings-foreground)] outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary)]"
                    />
                  </label>
                ))}
              </div>
            )}
            <select
              aria-label="颜色格式"
              value={format}
              onChange={(event) => handleFormatChange(event.target.value as ColorFormat)}
              className="h-10 w-[82px] shrink-0 rounded-lg border border-[var(--haven-settings-control-border)] bg-[var(--haven-settings-control)] px-2 text-[11px] text-[var(--haven-settings-foreground)] outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary)]"
            >
              <option value="hex">Hex</option>
              <option value="rgb">RGB</option>
              <option value="hsl">HSL</option>
            </select>
          </div>

          {!validInput && (
            <p className="mt-2 text-[11px] text-[var(--haven-settings-danger)]" role="alert">
              {format === "hex" ? "请输入有效的 3 位或 6 位 Hex 色值。" : "颜色数值超出允许范围，请检查输入。"}
            </p>
          )}
          {lowContrastTarget && (
            <p className="mt-3 rounded-lg bg-[var(--haven-settings-primary-06)] px-3 py-2 text-[11px] leading-5 text-[var(--haven-settings-primary)]" role="status" aria-live="polite">
              与{lowContrastTarget.label}的对比度偏低（{(lowContrastTarget.ratio ?? 0).toFixed(1)}:1），可能影响文字阅读。
            </p>
          )}

          <div className="mt-4 flex justify-end gap-2 border-t border-[var(--haven-settings-border-subtle)] pt-3">
            <button
              type="button"
              onClick={() => dismiss(true)}
              className="h-9 rounded-full border border-[var(--haven-settings-control-border)] px-4 text-[12px] font-medium text-[var(--haven-settings-muted-strong)] transition-colors hover:bg-[var(--haven-settings-card-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary)]"
            >
              取消
            </button>
            <button
              type="button"
              disabled={!validInput}
              onClick={applyColor}
              className="h-9 rounded-full bg-[var(--haven-settings-primary)] px-4 text-[12px] font-semibold text-[var(--haven-settings-primary-foreground)] transition-colors hover:bg-[var(--haven-settings-primary-hover)] disabled:cursor-not-allowed disabled:opacity-45 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary)]"
            >
              应用颜色
            </button>
          </div>
          </div>
        </div>
      )}
    </div>
  )
}
