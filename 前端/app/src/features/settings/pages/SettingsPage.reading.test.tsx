// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { useState } from "react"
import { afterEach, describe, expect, it, vi } from "vitest"

import {
  READING_CUSTOM_BACKGROUND_FALLBACK,
  READING_CUSTOM_TEXT_FALLBACK,
} from "@/features/reader/lib/reading-settings-mapping"
import { applySettingsPatch } from "@/lib/ipc/settings-wire"
import type { PreferenceGetResult, PreferenceUpdateResult, ReadingSettingsValue, SettingsPatch } from "@/lib/ipc/settings-wire"
import {
  READING_CUSTOM_BACKGROUND_LABEL,
  READING_CUSTOM_COLOR_DISABLED_HINT,
  READING_CUSTOM_FONT_INVALID_HINT,
  READING_CUSTOM_FONT_LABEL,
  READING_CUSTOM_TEXT_LABEL,
  readingFontLabel,
  readingThemePatch,
} from "../lib/settingsDisplay"
import type { SettingsFormController } from "../lib/useSettingsForm"
import { settingsGateway } from "../ipc/gateway"
import { ReadingSettings } from "./SettingsPage"

// 只挂载阅读分区：不触达任何 IPC，也不需要 Router。
vi.mock("@/lib/ipc/runtime", () => ({
  getHavenClient: () => { throw new Error("组件测试不提供 IPC 客户端") },
  getHavenClientMode: () => "mock",
  isTauriRuntime: () => false,
  resolveTauriRuntime: () => false,
  selectHavenClientMode: () => "mock",
}))

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  Reflect.deleteProperty(window, "matchMedia")
})

function readingValue(overrides: Partial<ReadingSettingsValue> = {}): ReadingSettingsValue {
  return {
    section: "reading",
    fontFamily: "serif",
    customFontFamily: null,
    fontSize: "medium",
    lineHeight: "comfortable",
    contentWidth: "medium",
    theme: "warm",
    customBackground: null,
    customText: null,
    fontWeight: "regular",
    letterSpacing: "normal",
    systemAuto: true,
    pagination: "scroll",
    ...overrides,
  }
}

function resourcePreferenceResult(reading = readingValue()): PreferenceGetResult {
  return {
    schemaVersion: 1,
    mediaItemId: "media-item-1",
    editionId: "edition-1",
    readingPatch: null,
    comicPatch: null,
    editionReadingPatch: null,
    editionComicPatch: null,
    mediaItemReadingPatch: null,
    mediaItemComicPatch: null,
    effectiveReading: { ...reading, pagination: reading.pagination ?? "scroll" },
    effectiveComic: {
      section: "comic",
      viewMode: "single",
      direction: "ltr",
      pageGap: "twelve",
      preloadPages: "three",
    },
    mediaItemRevision: "media-rev-1",
    editionRevision: "edition-rev-1",
  }
}

function controller(value: ReadingSettingsValue, change: (patch: SettingsPatch) => void): SettingsFormController {
  return {
    section: "reading",
    state: { status: "ready", saved: value, revision: null },
    displayValue: value,
    isLoading: false,
    isSaving: false,
    isDirty: false,
    hasError: false,
    errorMessage: null,
    change,
    save: () => undefined,
    retry: () => undefined,
    reload: () => undefined,
    resetToDefaults: () => undefined,
  }
}

function makeChangeSpy() {
  return vi.fn<(patch: SettingsPatch) => void>()
}

/** 只挂载阅读分区；需要跨多次渲染比较时把同一个 spy 传进来复用。 */
function renderReading(
  value: ReadingSettingsValue,
  change = makeChangeSpy(),
): ReturnType<typeof makeChangeSpy> {
  render(<ReadingSettings form={controller(value, change)} resourceContext={null} showNotice={() => undefined} />)
  return change
}

/**
 * 有状态测试宿主：把阅读分区接到一份真实的草稿状态上。
 *
 * `change` 走的是 `useSettingsForm` 同一条落地路径（`applySettingsPatch`），所以
 * `fireEvent` 之后组件会用**新的草稿**重新渲染——测试断言的是「控件写进草稿」并且
 * 「草稿重新渲染出新的预览」，而不是只看 spy 被调用过。
 */
function ReadingStatefulHarness({
  initial,
  onPatch,
  draftRef,
}: {
  initial: ReadingSettingsValue
  onPatch: (patch: SettingsPatch) => void
  draftRef: { current: ReadingSettingsValue }
}) {
  const [draft, setDraft] = useState(initial)
  draftRef.current = draft
  const change = (patch: SettingsPatch) => {
    onPatch(patch)
    setDraft((current) => {
      const next = applySettingsPatch(current, patch)
      return next.section === "reading" ? next : current
    })
  }
  return <ReadingSettings form={controller(draft, change)} resourceContext={null} showNotice={() => undefined} />
}

/** 挂载有状态宿主，返回收到的 patch 序列与「当前草稿」的读取器。 */
function renderStatefulReading(initial: ReadingSettingsValue): {
  patches: SettingsPatch[]
  draft: () => ReadingSettingsValue
} {
  const patches: SettingsPatch[] = []
  const draftRef = { current: initial }
  render(<ReadingStatefulHarness initial={initial} onPatch={(patch) => patches.push(patch)} draftRef={draftRef} />)
  return { patches, draft: () => draftRef.current }
}

// ---- 分区结构 ----

/** 分组控件的便捷取用（aria-label 与页面上的标签一致）。 */
function presetGroup(): HTMLElement {
  return screen.getByRole("group", { name: "主题预设" })
}

function typographyGroup(label: "字号" | "行高" | "正文宽度"): HTMLElement {
  return screen.getByRole("group", { name: label })
}

function paginationModeGroup(): HTMLElement {
  return screen.getByRole("group", { name: "阅读模式" })
}

/** 标题所在的主卡片（`SettingsGroup` 渲染成 section）。 */
function cardOf(title: string): HTMLElement {
  return screen.getByText(title).closest("section") as HTMLElement
}

/**
 * 阅读分区自己的主卡片标题。设计稿里排版、底色和方式只有两张主卡片；
 * 「资源内设」是另一项已登记能力（资源级覆盖），不计入这个数量。
 */
function readingCardTitles(): string[] {
  return Array.from(document.querySelectorAll("section"))
    .map((section) => section.querySelector("h3")?.textContent ?? "")
    .filter((title) => title !== "" && title !== "资源内设")
}

function inputValue(label: string): string {
  return (screen.getByLabelText(label) as HTMLInputElement).value
}

function inputElement(label: string): HTMLInputElement {
  return screen.getByLabelText(label) as HTMLInputElement
}

/**
 * 取色器在 React 里属于 text-like input，onChange 由 input 事件驱动；仓库既有测试用的是
 * fireEvent.change。这里两个事件都发一次，断言就不依赖 React 内部最终听的是哪一个。
 */
function pickColor(label: string, value: string): void {
  const input = screen.getByLabelText(label)
  fireEvent.input(input, { target: { value } })
  fireEvent.change(input, { target: { value } })
}

// ---- 排版预览 ----
// 预览元素把排版参数放在 inline style 上，所以断言直接读 style，不看 Tailwind 类名。

function previewMeasure(): HTMLElement {
  return screen.getByTestId("reading-typography-preview-measure")
}

function previewMeta(): string {
  return screen.getByTestId("reading-typography-preview-meta").textContent ?? ""
}

function sampleElement(): HTMLElement {
  return screen.getByTestId("reading-typography-preview-sample")
}

/** 预览正文栏在预览框里的相对宽度（%）。 */
function measureWidthOf(value: ReadingSettingsValue): number {
  renderReading(value)
  const width = Number.parseFloat(previewMeasure().style.width)
  cleanup()
  return width
}

/** jsdom 会把 inline 颜色规范化成 `rgb(r, g, b)`；hex 写法也一并接受。 */
function colorMatches(actual: string, hex: string): boolean {
  const normalized = actual.replace(/\s+/g, "").toLowerCase()
  const channels = [1, 3, 5].map((offset) => Number.parseInt(hex.slice(offset, offset + 2), 16))
  return normalized === hex.toLowerCase() || normalized === `rgb(${channels.join(",")})`
}

describe("reading theme and custom colour gating", () => {
  it("keeps the custom colour pickers disabled until the theme is custom", () => {
    renderReading(readingValue({ theme: "warm" }))
    expect(inputElement(READING_CUSTOM_BACKGROUND_LABEL).disabled).toBe(true)
    expect(inputElement(READING_CUSTOM_TEXT_LABEL).disabled).toBe(true)
    expect(screen.getByText(READING_CUSTOM_COLOR_DISABLED_HINT)).toBeTruthy()
  })

  it("enables them on the custom theme and drops the explanation", () => {
    renderReading(readingValue({ theme: "custom" }))
    expect(inputElement(READING_CUSTOM_BACKGROUND_LABEL).disabled).toBe(false)
    expect(inputElement(READING_CUSTOM_TEXT_LABEL).disabled).toBe(false)
    expect(screen.queryByText(READING_CUSTOM_COLOR_DISABLED_HINT)).toBeNull()
  })

  it("writes the picked colour without flipping the theme", () => {
    const change = renderReading(readingValue({ theme: "custom" }))
    pickColor(READING_CUSTOM_BACKGROUND_LABEL, "#102030")
    expect(change).toHaveBeenCalledWith({ section: "reading", customBackground: "#102030" })
    pickColor(READING_CUSTOM_TEXT_LABEL, "#f0f0f0")
    expect(change).toHaveBeenCalledWith({ section: "reading", customText: "#f0f0f0" })
  })

  it("maps every theme preset card to its wire value", () => {
    const change = renderReading(readingValue({ theme: "warm" }))
    const presets = screen.getByRole("group", { name: "主题预设" })
    fireEvent.click(within(presets).getByRole("button", { name: "纸张" }))
    expect(change).toHaveBeenCalledWith({ section: "reading", theme: "paper" })
    fireEvent.click(within(presets).getByRole("button", { name: "夜间" }))
    expect(change).toHaveBeenCalledWith({ section: "reading", theme: "dark" })
    fireEvent.click(within(presets).getByRole("button", { name: "护眼" }))
    expect(change).toHaveBeenCalledWith({ section: "reading", theme: "eyeCare" })
    fireEvent.click(within(presets).getByRole("button", { name: "自定义" }))
    expect(change).toHaveBeenCalledWith({ section: "reading", theme: "custom" })
  })

  it("marks exactly the active preset as pressed", () => {
    renderReading(readingValue({ theme: "sepia" }))
    const presets = screen.getByRole("group", { name: "主题预设" })
    expect(within(presets).getByRole("button", { name: "复古" }).getAttribute("aria-pressed")).toBe("true")
    expect(within(presets).getByRole("button", { name: "护眼" }).getAttribute("aria-pressed")).toBe("false")
  })

  it("does not erase the stored custom colours when a preset is chosen", () => {
    const change = renderReading(readingValue({ theme: "custom", customBackground: "#102030", customText: "#f0f0f0" }))
    fireEvent.click(within(screen.getByRole("group", { name: "主题预设" })).getByRole("button", { name: "纸张" }))
    // 只写 theme：自定义颜色仍留在草稿里，取色器也不会把主题改回 custom。
    expect(change).toHaveBeenCalledWith({ section: "reading", theme: "paper" })
  })

  it("shows the built-in fallbacks while nothing is stored", () => {
    renderReading(readingValue())
    expect(inputValue(READING_CUSTOM_BACKGROUND_LABEL)).toBe(READING_CUSTOM_BACKGROUND_FALLBACK)
    expect(inputValue(READING_CUSTOM_TEXT_LABEL)).toBe(READING_CUSTOM_TEXT_FALLBACK)
  })

  it("shows the stored colours instead of the fallbacks", () => {
    renderReading(readingValue({ theme: "custom", customBackground: "#102030", customText: "#f0f0f0" }))
    expect(inputValue(READING_CUSTOM_BACKGROUND_LABEL)).toBe("#102030")
    expect(inputValue(READING_CUSTOM_TEXT_LABEL)).toBe("#f0f0f0")
  })
})

describe("reading custom font family field", () => {
  it("only appears when the font option is custom", () => {
    renderReading(readingValue({ fontFamily: "serif", customFontFamily: "思源宋体" }))
    expect(screen.queryByLabelText(READING_CUSTOM_FONT_LABEL)).toBeNull()
    cleanup()
    renderReading(readingValue({ fontFamily: "custom", customFontFamily: "思源宋体" }))
    expect(inputValue(READING_CUSTOM_FONT_LABEL)).toBe("思源宋体")
  })

  it("commits a safe installed family name and switches the font option to custom", () => {
    const change = renderReading(readingValue({ fontFamily: "custom" }))
    fireEvent.change(screen.getByLabelText(READING_CUSTOM_FONT_LABEL), { target: { value: "思源宋体" } })
    expect(change).toHaveBeenCalledWith({ section: "reading", fontFamily: "custom", customFontFamily: "思源宋体" })
  })

  it("keeps unsafe input out of the draft and explains why", () => {
    const change = renderReading(readingValue({ fontFamily: "custom" }))
    fireEvent.change(screen.getByLabelText(READING_CUSTOM_FONT_LABEL), { target: { value: 'Arial"; color: red' } })
    expect(change).not.toHaveBeenCalled()
    expect(inputValue(READING_CUSTOM_FONT_LABEL)).toBe('Arial"; color: red')
    expect(screen.getByText(READING_CUSTOM_FONT_INVALID_HINT)).toBeTruthy()
  })

  it("clears the stored family name when the field is emptied", () => {
    const change = renderReading(readingValue({ fontFamily: "custom", customFontFamily: "思源宋体" }))
    fireEvent.change(screen.getByLabelText(READING_CUSTOM_FONT_LABEL), { target: { value: "" } })
    expect(change).toHaveBeenCalledWith({ section: "reading", customFontFamily: null })
  })
})

describe("reading typography preview", () => {
  it("follows font, font size, line height and content width", () => {
    renderReading(readingValue({ fontFamily: "kai", fontSize: "small", lineHeight: "compact", contentWidth: "narrow" }))
    const measure = previewMeasure()
    expect(Number.parseFloat(measure.style.fontSize)).toBe(16)
    expect(Number.parseFloat(measure.style.lineHeight)).toBeCloseTo(1.65, 3)
    expect(Number.parseFloat(measure.style.width)).toBeCloseTo(75.6, 1)
    expect(previewMeta()).toContain("620 px")
    // 字体预设用阅读器同一套类名渲染样例。
    expect(sampleElement().className).toContain("font-serif")
    expect(sampleElement().className).toContain("italic")
  })

  it("moves to the large typography when those options change", () => {
    renderReading(readingValue({ fontSize: "large", lineHeight: "airy", contentWidth: "wide" }))
    const measure = previewMeasure()
    expect(Number.parseFloat(measure.style.fontSize)).toBe(21)
    expect(Number.parseFloat(measure.style.lineHeight)).toBeCloseTo(2.05, 3)
    expect(Number.parseFloat(measure.style.width)).toBe(100)
    expect(previewMeta()).toContain("820 px")
  })

  it("renders a visibly wider reading measure for wide than for narrow", () => {
    const narrow = measureWidthOf(readingValue({ contentWidth: "narrow" }))
    const medium = measureWidthOf(readingValue({ contentWidth: "medium" }))
    const wide = measureWidthOf(readingValue({ contentWidth: "wide" }))
    expect(narrow).toBeLessThan(medium)
    expect(medium).toBeLessThan(wide)
    expect(wide).toBeLessThanOrEqual(100)
  })

  it("uses the reader's colours for preset and custom themes", () => {
    renderReading(readingValue({ theme: "sepia" }))
    expect(colorMatches(previewMeasure().style.backgroundColor, "#f4ecd8")).toBe(true)
    cleanup()
    renderReading(readingValue({ theme: "custom", customBackground: "#102030", customText: "#f0f0f0" }))
    expect(colorMatches(previewMeasure().style.backgroundColor, "#102030")).toBe(true)
    expect(colorMatches(previewMeasure().style.color, "#f0f0f0")).toBe(true)
    cleanup()
    renderReading(readingValue({ theme: "custom", customBackground: null, customText: null }))
    expect(colorMatches(previewMeasure().style.backgroundColor, READING_CUSTOM_BACKGROUND_FALLBACK)).toBe(true)
    expect(colorMatches(previewMeasure().style.color, READING_CUSTOM_TEXT_FALLBACK)).toBe(true)
  })

  it("applies the stored custom family to the sample and says so when it is missing", () => {
    renderReading(readingValue({ fontFamily: "custom", customFontFamily: "思源宋体" }))
    expect(previewMeasure().style.fontFamily).toBe('"思源宋体", sans-serif')
    expect(previewMeta()).toContain("思源宋体")
    cleanup()
    renderReading(readingValue({ fontFamily: "custom", customFontFamily: null }))
    expect(previewMeasure().style.fontFamily).toBe("")
    expect(previewMeta()).toContain("未填族名")
  })
})

describe("reading pagination", () => {
  it("states which reader actually consumes the global pagination setting", () => {
    renderReading(readingValue({ pagination: "scroll" }))
    expect(screen.getByText("仅对图书文本阅读器生效；文章、PDF 等格式继续使用各自阅读方式。切回连续滚动后会保存为连续滚动。")).toBeTruthy()
  })

  it("offers only the top-level choice while scrolling", () => {
    renderReading(readingValue({ pagination: "scroll" }))
    expect(screen.getByRole("group", { name: "阅读模式" })).toBeTruthy()
    expect(screen.queryByRole("group", { name: "分页方式" })).toBeNull()
  })

  it("treats a missing stored value as scroll", () => {
    renderReading(readingValue({ pagination: undefined }))
    const mode = screen.getByRole("group", { name: "阅读模式" })
    expect(within(mode).getByRole("button", { name: "连续滚动" }).getAttribute("aria-pressed")).toBe("true")
    expect(screen.queryByRole("group", { name: "分页方式" })).toBeNull()
  })

  it("reveals the page sub-choice only in paginated mode", () => {
    renderReading(readingValue({ pagination: "paginated" }))
    const single = screen.getByRole("group", { name: "分页方式" })
    expect(within(single).getByRole("button", { name: "单页" }).getAttribute("aria-pressed")).toBe("true")
    expect(within(single).getByRole("button", { name: "双页" }).getAttribute("aria-pressed")).toBe("false")
    cleanup()
    renderReading(readingValue({ pagination: "double" }))
    const double = screen.getByRole("group", { name: "分页方式" })
    expect(within(double).getByRole("button", { name: "双页" }).getAttribute("aria-pressed")).toBe("true")
    // 双页仍然属于「分页」这一层，不是第三种顶层模式。
    expect(within(screen.getByRole("group", { name: "阅读模式" })).getByRole("button", { name: "分页" }).getAttribute("aria-pressed")).toBe("true")
  })

  it("writes the wire value behind each level", () => {
    const change = renderReading(readingValue({ pagination: "scroll" }))
    fireEvent.click(within(screen.getByRole("group", { name: "阅读模式" })).getByRole("button", { name: "分页" }))
    expect(change).toHaveBeenLastCalledWith({ section: "reading", pagination: "paginated" })
    cleanup()
    renderReading(readingValue({ pagination: "paginated" }), change)
    fireEvent.click(within(screen.getByRole("group", { name: "分页方式" })).getByRole("button", { name: "双页" }))
    expect(change).toHaveBeenLastCalledWith({ section: "reading", pagination: "double" })
    cleanup()
    renderReading(readingValue({ pagination: "double" }), change)
    fireEvent.click(within(screen.getByRole("group", { name: "阅读模式" })).getByRole("button", { name: "连续滚动" }))
    expect(change).toHaveBeenLastCalledWith({ section: "reading", pagination: "scroll" })
  })

  it("does not write a patch when the already active mode is clicked", () => {
    const change = renderReading(readingValue({ pagination: "scroll" }))
    fireEvent.click(within(screen.getByRole("group", { name: "阅读模式" })).getByRole("button", { name: "连续滚动" }))
    expect(change).not.toHaveBeenCalled()
  })
})

// ---- 设计稿的两张主卡片 ----
// 排版（含预览）、底色、方式各自不再单独成卡：预览与排版控件在第一张，底色与方式在第二张。

describe("reading two-card composition", () => {
  it("keeps typography, base colour and mode in exactly two main cards", () => {
    renderReading(readingValue({ theme: "custom", fontFamily: "custom", customFontFamily: "思源宋体" }))
    expect(readingCardTitles()).toEqual(["排版与预览", "阅读底色与方式"])
  })

  it("puts the preview and the whole typography grid in the first card", () => {
    renderReading(readingValue())
    const typography = cardOf("排版与预览")
    expect(within(typography).getByTestId("reading-typography-preview-measure")).toBeTruthy()
    expect(within(typography).getByRole("button", { name: "默认字体" })).toBeTruthy()
    expect(within(typography).getByRole("group", { name: "正文宽度" })).toBeTruthy()
    expect(within(typography).getByRole("group", { name: "字号" })).toBeTruthy()
    expect(within(typography).getByRole("group", { name: "行高" })).toBeTruthy()
  })

  it("keeps the custom family input in the typography card, not the colour card", () => {
    renderReading(readingValue({ fontFamily: "custom", customFontFamily: "思源宋体" }))
    const typography = cardOf("排版与预览")
    const base = cardOf("阅读底色与方式")
    expect(within(typography).getByLabelText(READING_CUSTOM_FONT_LABEL)).toBeTruthy()
    expect(within(base).queryByLabelText(READING_CUSTOM_FONT_LABEL)).toBeNull()
  })

  it("puts the presets, colour pickers and reading mode in the second card", () => {
    renderReading(readingValue({ theme: "custom" }))
    const base = cardOf("阅读底色与方式")
    expect(within(base).getByRole("group", { name: "主题预设" })).toBeTruthy()
    expect(within(base).getByLabelText(READING_CUSTOM_BACKGROUND_LABEL)).toBeTruthy()
    expect(within(base).getByLabelText(READING_CUSTOM_TEXT_LABEL)).toBeTruthy()
    expect(within(base).getByRole("group", { name: "阅读模式" })).toBeTruthy()
    // 排版控件不再落在底色卡片里。
    expect(within(base).queryByRole("group", { name: "字号" })).toBeNull()
    expect(within(base).queryByRole("group", { name: "行高" })).toBeNull()
    expect(within(base).queryByRole("group", { name: "正文宽度" })).toBeNull()
  })

  it("renders the four-line 山居秋暝 sample inside the preview measure", () => {
    renderReading(readingValue())
    const lines = within(previewMeasure()).getAllByText(/[，。]$/)
    expect(lines).toHaveLength(4)
    expect(lines[0].textContent).toBe("空山新雨后，天气晚来秋。")
    expect(lines[3].textContent).toBe("随意春芳歇，王孙自可留。")
  })
})

// ---- 有状态往返：控件 → 草稿 → 重新渲染的预览 ----

describe("reading controls round-trip through the draft", () => {
  it("enables the colour pickers as soon as the custom theme lands in the draft", () => {
    const harness = renderStatefulReading(readingValue({ theme: "warm" }))
    expect(inputElement(READING_CUSTOM_BACKGROUND_LABEL).disabled).toBe(true)
    expect(inputElement(READING_CUSTOM_TEXT_LABEL).disabled).toBe(true)

    fireEvent.click(within(presetGroup()).getByRole("button", { name: "自定义" }))

    expect(harness.patches).toEqual([{ section: "reading", theme: "custom" }])
    expect(harness.draft().theme).toBe("custom")
    expect(inputElement(READING_CUSTOM_BACKGROUND_LABEL).disabled).toBe(false)
    expect(inputElement(READING_CUSTOM_TEXT_LABEL).disabled).toBe(false)
    expect(screen.queryByText(READING_CUSTOM_COLOR_DISABLED_HINT)).toBeNull()
  })

  it("carries the preset choice into the draft and keeps the stored custom colours", () => {
    const harness = renderStatefulReading(readingValue({ theme: "custom", customBackground: "#102030", customText: "#f0f0f0" }))
    fireEvent.click(within(presetGroup()).getByRole("button", { name: "石板" }))

    expect(harness.patches).toEqual([{ section: "reading", theme: "slate" }])
    expect(harness.draft()).toMatchObject({ theme: "slate", customBackground: "#102030", customText: "#f0f0f0" })
    // 底色切走后取色器重新禁用，但存的值没有被清掉。
    expect(inputElement(READING_CUSTOM_BACKGROUND_LABEL).disabled).toBe(true)
    expect(inputValue(READING_CUSTOM_BACKGROUND_LABEL)).toBe("#102030")
  })

  it("re-renders the preview from the draft after font, size, line height and width change", () => {
    const harness = renderStatefulReading(readingValue())

    fireEvent.click(screen.getByRole("button", { name: "默认字体" }))
    fireEvent.click(screen.getByRole("option", { name: "楷体" }))
    expect(harness.draft().fontFamily).toBe("kai")
    expect(sampleElement().className).toContain("font-serif")
    expect(sampleElement().className).toContain("italic")
    expect(previewMeta()).toContain("楷体")

    fireEvent.click(within(typographyGroup("字号")).getByRole("button", { name: "大" }))
    fireEvent.click(within(typographyGroup("行高")).getByRole("button", { name: "宽松" }))
    fireEvent.click(within(typographyGroup("正文宽度")).getByRole("button", { name: "宽" }))

    expect(harness.draft()).toMatchObject({ fontSize: "large", lineHeight: "airy", contentWidth: "wide" })
    const measure = previewMeasure()
    expect(Number.parseFloat(measure.style.fontSize)).toBe(21)
    expect(Number.parseFloat(measure.style.lineHeight)).toBeCloseTo(2.05, 3)
    expect(Number.parseFloat(measure.style.width)).toBe(100)
    expect(measure.dataset.measurePx).toBe("820")
    expect(previewMeta()).toContain("820 px")
  })

  it("widens the measure step by step instead of overlapping", () => {
    const harness = renderStatefulReading(readingValue({ contentWidth: "narrow" }))
    const widths = [Number.parseFloat(previewMeasure().style.width)]
    for (const label of ["适中", "宽"]) {
      fireEvent.click(within(typographyGroup("正文宽度")).getByRole("button", { name: label }))
      widths.push(Number.parseFloat(previewMeasure().style.width))
    }
    expect(widths[0]).toBeLessThan(widths[1])
    expect(widths[1]).toBeLessThan(widths[2])
    expect(widths[2]).toBeLessThanOrEqual(100)
    expect(harness.draft().contentWidth).toBe("wide")
  })

  it("writes both pagination levels through the draft and hides the sub-choice again", () => {
    const harness = renderStatefulReading(readingValue({ pagination: "scroll" }))
    expect(screen.queryByRole("group", { name: "分页方式" })).toBeNull()

    fireEvent.click(within(paginationModeGroup()).getByRole("button", { name: "分页" }))
    expect(harness.patches).toEqual([{ section: "reading", pagination: "paginated" }])
    expect(harness.draft().pagination).toBe("paginated")
    const sub = screen.getByRole("group", { name: "分页方式" })
    expect(within(sub).getByRole("button", { name: "单页" }).getAttribute("aria-pressed")).toBe("true")

    fireEvent.click(within(sub).getByRole("button", { name: "双页" }))
    expect(harness.draft().pagination).toBe("double")
    expect(within(paginationModeGroup()).getByRole("button", { name: "分页" }).getAttribute("aria-pressed")).toBe("true")

    fireEvent.click(within(paginationModeGroup()).getByRole("button", { name: "连续滚动" }))
    expect(harness.draft().pagination).toBe("scroll")
    expect(screen.queryByRole("group", { name: "分页方式" })).toBeNull()
  })

  it("repairs a stale systemAuto when the already-selected 跟随系统 card is clicked", () => {
    // 历史快照：theme 已经是 system，但 systemAuto=false。契约里 systemAuto 才是那
    // 个开关本身，缺了它阅读器会把「跟随系统」渲染成固定暖光——卡片说的和做的不一致。
    const harness = renderStatefulReading(readingValue({ theme: "system", systemAuto: false }))
    const systemCard = within(presetGroup()).getByRole("button", { name: "跟随系统" })
    // 卡片本来就是选中的，所以下面点的正是「已经选中」的这一项。
    expect(systemCard.getAttribute("aria-pressed")).toBe("true")

    fireEvent.click(systemCard)

    expect(harness.patches).toEqual([readingThemePatch("system")])
    expect(harness.patches[0]).toEqual({ section: "reading", theme: "system", systemAuto: true })
    expect(harness.draft()).toMatchObject({ theme: "system", systemAuto: true })
  })

  it("leaves systemAuto untouched when a fixed theme is chosen", () => {
    const harness = renderStatefulReading(readingValue({ theme: "system", systemAuto: false }))

    fireEvent.click(within(presetGroup()).getByRole("button", { name: "纸张" }))

    // 固定主题不消费 systemAuto，所以不顺手改它：只写 theme。
    expect(harness.patches).toEqual([{ section: "reading", theme: "paper" }])
    expect(harness.draft()).toMatchObject({ theme: "paper", systemAuto: false })
  })

  it("keeps a stored legacy font shown and writes the real wire value of a new choice", () => {
    // 旧快照里的 fangsong 在阅读器里就是「系统衬线」：选项里不再提供它，但必须仍然
    // 回选当前值，否则用户一进设置就看到空选择、一保存就静默改掉旧设置。
    const harness = renderStatefulReading(readingValue({ fontFamily: "fangsong" }))
    const trigger = screen.getByRole("button", { name: "默认字体" })
    expect(trigger.textContent).toContain(readingFontLabel("fangsong"))

    fireEvent.click(trigger)
    expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual([
      "系统无衬线",
      "系统衬线",
      "楷体",
      "黑体",
      "自定义",
      "仿宋（等同系统衬线）",
    ])

    fireEvent.click(screen.getByRole("option", { name: "系统衬线" }))
    expect(harness.patches).toEqual([{ section: "reading", fontFamily: "serif" }])
    expect(harness.draft().fontFamily).toBe("serif")
  })

  it("offers no duplicate-render font entries for a value that is already pickable", () => {
    renderStatefulReading(readingValue({ fontFamily: "serif" }))

    fireEvent.click(screen.getByRole("button", { name: "默认字体" }))
    const labels = screen.getAllByRole("option").map((option) => option.textContent)
    expect(labels).toEqual(["系统无衬线", "系统衬线", "楷体", "黑体", "自定义"])
    // 与「系统衬线」渲染完全相同的「仿宋」「免费字体」不再作为新选项出现。
    expect(labels.some((label) => label?.includes("等同"))).toBe(false)
  })
})

describe("resource-level reading width", () => {
  const context = { workId: "work-1", editionId: "edition-1", mediaItemId: "media-item-1" }

  it("shows the inherited width and persists only an explicitly changed width", async () => {
    const initial = resourcePreferenceResult()
    const updated = resourcePreferenceResult(readingValue({ contentWidth: "wide" }))
    vi.spyOn(settingsGateway, "preferenceGet").mockResolvedValue(initial)
    const update = vi.spyOn(settingsGateway, "preferenceUpdate").mockResolvedValue({
      result: { ...updated, mediaItemReadingPatch: { contentWidth: "wide" } },
      target: "media_item",
      revision: "media-rev-2",
      changed: true,
    } satisfies PreferenceUpdateResult)

    render(
      <ReadingSettings
        form={controller(readingValue(), makeChangeSpy())}
        resourceContext={context}
        showNotice={() => undefined}
      />,
    )

    const width = await screen.findByRole("button", { name: "本资源正文宽度" })
    expect(width.textContent).toContain("适中")
    fireEvent.click(width)
    fireEvent.click(screen.getByRole("option", { name: "宽" }))
    fireEvent.click(screen.getByRole("button", { name: "保存本资源设置" }))

    await waitFor(() => expect(update).toHaveBeenCalledWith(expect.objectContaining({
      mediaItemId: "media-item-1",
      editionId: "edition-1",
      target: "media_item",
      readingPatch: { contentWidth: "wide" },
      expectedRevision: "media-rev-1",
    })))
  })

  it("does not materialize an inherited width when the user leaves it unchanged", async () => {
    const initial = resourcePreferenceResult()
    vi.spyOn(settingsGateway, "preferenceGet").mockResolvedValue(initial)
    const update = vi.spyOn(settingsGateway, "preferenceUpdate").mockResolvedValue({
      result: initial,
      target: "media_item",
      revision: "media-rev-1",
      changed: false,
    } satisfies PreferenceUpdateResult)

    render(
      <ReadingSettings
        form={controller(readingValue(), makeChangeSpy())}
        resourceContext={context}
        showNotice={() => undefined}
      />,
    )

    await screen.findByRole("button", { name: "本资源正文宽度" })
    fireEvent.click(screen.getByRole("button", { name: "保存本资源设置" }))

    await waitFor(() => expect(update).toHaveBeenCalledWith(expect.objectContaining({
      target: "media_item",
      readingPatch: null,
      expectedRevision: "media-rev-1",
    })))
  })
})
