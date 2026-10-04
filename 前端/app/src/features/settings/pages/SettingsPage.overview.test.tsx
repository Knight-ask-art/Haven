// @vitest-environment jsdom
//
// 总览必须把「读不到」与「没有记录」当成两件事。
//
// 错误态显示「统计暂时不可用 / 读取失败，请重试」与「不可用」的指标，绝不借用空态那句
// 「暂无本机阅读记录 / 暂无阅读数据」——后者是在替用户断言「你确实什么都没读」，而一次
// 失败的读取根本无从知道这件事。

import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it, vi } from "vitest"
// @ts-expect-error node:fs 不在本工程可解析的模块里（types 里补上 "node" 后即可删除本行）
import { readFileSync } from "node:fs"

import type { SettingsFormController } from "@/features/settings/lib/useSettingsForm"
import type { OverviewLayoutWire, SettingsSectionWire } from "@/lib/ipc/settings-wire"
import { defaultOverviewLayout } from "@/lib/ipc/settings-wire"
import { formDisplayValue, initialFormState } from "../lib/settingsForm"
import {
  EMPTY_READING_OVERVIEW_STATS,
  type ReadingOverviewState,
  type ReadingOverviewStats,
} from "../lib/reading-overview"
import {
  defaultOverviewModuleSettings,
  layoutToOverviewModulePlacements,
  layoutToOverviewModuleSettings,
  overviewModuleSettingsToLayout,
} from "../lib/overview-layout"
import {
  overviewLayoutProjection,
  type OverviewLayoutProjection,
} from "../lib/overview-layout-projection"
import type { OverviewLayoutEditorState } from "../lib/useOverviewLayout"
import { SettingsOverview } from "./SettingsPage"

vi.mock("@/lib/ipc/runtime", () => ({
  getHavenClient: () => { throw new Error("组件测试不提供 IPC 客户端") },
  getHavenClientMode: () => "mock",
  isTauriRuntime: () => false,
  resolveTauriRuntime: () => false,
  selectHavenClientMode: () => "mock",
}))

vi.mock("@/app/notice-center/notice-context", () => ({
  useNotice: () => ({ push: () => undefined }),
}))

/**
 * 一个真实的表单控制器（契约默认值兜底），只为满足总览读取偏好摘要的那几个字段。
 * 这里不 mock Hook：控制器是纯数据，直接按状态机的初始状态构造即可。
 */
function formController(section: SettingsSectionWire): SettingsFormController {
  const state = initialFormState(section)
  return {
    section,
    state,
    displayValue: formDisplayValue(state, section),
    isLoading: true,
    isSaving: false,
    isDirty: false,
    hasError: false,
    errorMessage: null,
    change: () => undefined,
    save: () => undefined,
    retry: () => undefined,
    reload: () => undefined,
    resetToDefaults: () => undefined,
  }
}

type OverviewForms = Parameters<typeof SettingsOverview>[0]["forms"]

function overviewForms(): OverviewForms {
  return {
    general: formController("general"),
    appearance: formController("appearance"),
    playback: formController("playback"),
    reading: formController("reading"),
    comic: formController("comic"),
    downloads: formController("downloads"),
    privacy: formController("privacy"),
  }
}

/** 默认（未自定义过）的总览布局投影：五个模块都可见，排列就是领域默认布局。 */
function readyLayout(): OverviewLayoutProjection {
  return overviewLayoutProjection({
    status: "ready",
    modules: defaultOverviewModuleSettings(),
    savedModules: defaultOverviewModuleSettings(),
    savedLayout: defaultOverviewLayout(),
    revision: null,
    message: null,
    retryable: false,
  })
}

function renderOverview(
  state: ReadingOverviewState,
  layout: OverviewLayoutProjection = readyLayout(),
): string {
  return renderToStaticMarkup(
    <SettingsOverview
      state={state}
      forms={overviewForms()}
      layout={layout}
      onSelectSection={() => undefined}
      onRetryOverview={() => undefined}
      onRetryLayout={() => undefined}
    />,
  )
}

/** 一份被聚合证实的统计：ready 态用它确认「有数据」的语义没有被改坏。 */
const READY_STATS: ReadingOverviewStats = {
  range: { startLocalDate: "2026-09-17", endLocalDate: "2026-09-23", timezone: "UTC+08:00" },
  summary: { preferredType: "book", longestStreakDays: 3, averageDailyMinutes: 42, recentSevenDayMinutes: 294 },
  dailyMinutes: [
    { localDate: "2026-09-17", label: "四", minutes: 12 },
    { localDate: "2026-09-18", label: "五", minutes: 0 },
    { localDate: "2026-09-19", label: "六", minutes: 90 },
    { localDate: "2026-09-20", label: "日", minutes: 32 },
    { localDate: "2026-09-21", label: "一", minutes: 60 },
    { localDate: "2026-09-22", label: "二", minutes: 40 },
    { localDate: "2026-09-23", label: "三", minutes: 60 },
  ],
  typeShares: [{ category: "book", minutes: 294, percentage: 100 }],
  heatmap: {
    peakStartHour: 21,
    peakEndHour: 22,
    cells: [{ dayOfWeek: 1, hour: 21, minutes: 60 }],
  },
}

const NO_RECORD_COPY = "暂无本机阅读记录"
const NO_DATA_COPY = "暂无阅读数据"
const UNAVAILABLE_TITLE = "统计暂时不可用"

describe("reading overview states", () => {
  it("never claims there are no records when the statistics could not be read", () => {
    const html = renderOverview({ status: "error", message: "数据库暂时不可用" })

    // 指标：数值与脚注都说「不可用 / 统计暂时不可用」，而不是空态那句话。
    expect(html).toContain(">不可用<")
    expect(html).toContain(`>${UNAVAILABLE_TITLE}</div>`)
    expect(html).not.toContain(NO_RECORD_COPY)
    expect(html).not.toContain(NO_DATA_COPY)
    expect(html).not.toContain("暂无数据")
  })

  it("shows an error empty state in all three charts instead of a no-data state", () => {
    const html = renderOverview({ status: "error", message: "数据库暂时不可用" })

    // 三块图各自的错误空态都带标题（<p>），空态用的是「暂无…」那两句。
    expect(html).toContain(`>${UNAVAILABLE_TITLE}</p>`)
    expect(html).toContain("读取失败，请重试。")
    // 圆环中心也不再伪装成「无 / 暂无数据」。
    expect(html).not.toContain("暂无数据")

    // 顶端错误横幅与重试入口必须保留。
    expect(html).toContain('role="alert"')
    expect(html).toContain("重试")
  })

  it("keeps the no-record copy for a genuinely empty window", () => {
    const html = renderOverview({ status: "empty", stats: EMPTY_READING_OVERVIEW_STATS })

    expect(html).toContain(NO_RECORD_COPY)
    expect(html).toContain(NO_DATA_COPY)
    // 空态不是错误态：不可用文案一个都不该出现。
    expect(html).not.toContain(UNAVAILABLE_TITLE)
    expect(html).not.toContain(">不可用<")
    expect(html).not.toContain('role="alert"')
  })

  it("keeps the ready state free of both the empty copy and the unavailable copy", () => {
    const html = renderOverview({ status: "ready", stats: READY_STATS })

    expect(html).toContain("小说")
    expect(html).toContain("3 天")
    expect(html).not.toContain(NO_RECORD_COPY)
    expect(html).not.toContain(NO_DATA_COPY)
    expect(html).not.toContain(UNAVAILABLE_TITLE)
  })

  it("keeps the loading state reading rather than concluding anything", () => {
    const html = renderOverview({ status: "loading" })

    expect(html).toContain("正在读取")
    expect(html).not.toContain(UNAVAILABLE_TITLE)
  })
})

const MODULE_ORDER = ["preferences", "metrics", "reading-minutes", "type-share", "reading-heatmap"]

/** 从渲染出的标记里按 DOM 顺序读出模块 ID（这就是屏幕上自上而下的排列）。 */
function renderedModules(html: string): string[] {
  return [...html.matchAll(/data-overview-module="([a-z-]+)"/g)].map((match) => match[1])
}

/** 某个模块外层元素的**开标签原文**：坐标、类名这些真实摆放依据都在它上面。 */
function moduleTag(html: string, module: string): string {
  const match = new RegExp(`<div[^>]*data-overview-module="${module}"[^>]*>`).exec(html)
  if (!match) throw new Error(`标记里找不到模块 ${module}`)
  return match[0]
}

const indexCss: string = readFileSync("src/index.css", "utf8")

/**
 * index.css 里 `@media (min-width: <minWidth>px)` 那个块的**内容**。
 *
 * 用花括号配对切块而不是正则：媒体查询里嵌着规则块，非贪婪正则会在第一个 `}` 就收口，
 * 于是「这条规则到底在不在这个断点里」根本测不出来。找不到直接抛错，不静默算过。
 */
function mediaBlock(minWidth: number): string {
  const start = indexCss.indexOf(`@media (min-width: ${minWidth}px)`)
  if (start < 0) throw new Error(`index.css 里找不到 min-width: ${minWidth}px 的媒体查询`)
  const open = indexCss.indexOf("{", start)
  let depth = 0
  for (let index = open; index < indexCss.length; index += 1) {
    const character = indexCss[index]
    if (character === "{") depth += 1
    else if (character === "}") {
      depth -= 1
      if (depth === 0) return indexCss.slice(open + 1, index)
    }
  }
  throw new Error(`min-width: ${minWidth}px 的媒体查询块没有闭合`)
}

describe("overview modules follow the saved layout", () => {
  it("renders every default module in the domain default order and size", () => {
    const layout = readyLayout()
    expect(layout.status).toBe("ready")
    if (layout.status !== "ready") throw new Error("默认布局必须可渲染")

    expect(layout.placements.map((placement) => placement.module)).toEqual(MODULE_ORDER)
    // 默认版式就是升级前那一屏：两行整宽、两张图 2:1、一行整宽热力图。
    expect(layout.placements.map((placement) => placement.size)).toEqual([
      "large",
      "large",
      "medium",
      "small",
      "large",
    ])

    const html = renderOverview({ status: "empty", stats: EMPTY_READING_OVERVIEW_STATS }, layout)
    expect(renderedModules(html)).toEqual(MODULE_ORDER)
  })

  it("hides a module the user turned off without touching the others", () => {
    const layout: OverviewLayoutProjection = {
      status: "ready",
      stale: false,
      placements: layoutToOverviewModulePlacements({
        schemaVersion: 1,
        modules: [
          { module: "metrics", size: "large", row: 0, column: 0, order: 0 },
          { module: "reading-heatmap", size: "large", row: 1, column: 0, order: 1 },
        ],
      }),
    }
    const html = renderOverview({ status: "empty", stats: EMPTY_READING_OVERVIEW_STATS }, layout)

    expect(renderedModules(html)).toEqual(["metrics", "reading-heatmap"])
    // 被隐藏的模块不只是「看不见」：它的内容根本不进 DOM。
    expect(html).not.toContain("data-overview-module=\"preferences\"")
    expect(html).not.toContain("当前主题")
    expect(html).not.toContain("每日阅读时长")
    // 仍然可见的模块照常渲染真实内容。
    expect(html).toContain("阅读指标")
    expect(html).toContain("阅读时段")
  })

  it("applies the saved order and size instead of the built-in arrangement", () => {
    const layout: OverviewLayoutProjection = {
      status: "ready",
      stale: false,
      placements: layoutToOverviewModulePlacements({
        schemaVersion: 1,
        modules: [
          { module: "type-share", size: "small", row: 0, column: 0, order: 0 },
          { module: "preferences", size: "medium", row: 0, column: 1, order: 1 },
          { module: "metrics", size: "large", row: 1, column: 0, order: 2 },
        ],
      }),
    }
    const html = renderOverview({ status: "empty", stats: EMPTY_READING_OVERVIEW_STATS }, layout)

    expect(renderedModules(html)).toEqual(["type-share", "preferences", "metrics"])
    // 档位确实落到了渲染上（占几列 + 模块内部的卡片列数）。
    expect(html).toContain('data-overview-module="preferences"')
    expect(html).toMatch(/data-overview-module="preferences" data-overview-module-size="medium"/)
    expect(html).toMatch(/data-overview-module="metrics" data-overview-module-size="large"/)
    expect(html).toMatch(/data-overview-module="type-share" data-overview-module-size="small"/)
    // 交给渲染器的坐标被原样带进标记（这一条只证明「渲染器不改写拿到的坐标」；
    // 坐标真的参与摆放由下面 "places every module at its saved coordinates" 证明）。
    expect(html).toMatch(/data-overview-module="preferences"[^>]*data-overview-module-row="0"[^>]*data-overview-module-column="1"/)
  })

  it("treats an explicitly empty layout as the user's choice, not as a failure", () => {
    const layout: OverviewLayoutProjection = { status: "ready", stale: false, placements: [] }
    const html = renderOverview({ status: "ready", stats: READY_STATS }, layout)

    expect(renderedModules(html)).toEqual([])
    expect(html).toContain("已隐藏全部总览模块")
    expect(html).toContain("这是你保存过的排列，不是读取失败")
    // 空布局不是统计失败：统计本身仍然照常渲染在页头里。
    expect(html).toContain("READING OVERVIEW / PERSONAL SIGNAL")
    expect(html).not.toContain(UNAVAILABLE_TITLE)
  })

  it("says the arrangement is a fallback when the layout could not be read", () => {
    const layout = overviewLayoutProjection({
      status: "error",
      modules: defaultOverviewModuleSettings(),
      savedModules: defaultOverviewModuleSettings(),
      savedLayout: defaultOverviewLayout(),
      revision: null,
      message: "数据库暂时不可用",
      retryable: true,
    })
    const html = renderOverview({ status: "ready", stats: READY_STATS }, layout)

    // 读不到布局时**不**空屏：按默认布局显示，并且必须说清这是默认布局。
    expect(renderedModules(html)).toEqual(MODULE_ORDER)
    expect(html).toContain("总览布局暂时不可用")
    expect(html).toContain("以下按默认布局显示，不会覆盖你保存过的排列")
    expect(html).toContain("重新读取布局")
    // 布局横幅与统计横幅是两件独立的事实，不能共用一个 alert。
    expect(html.match(/role="alert"/g)?.length).toBe(1)
  })

  it("says nothing at all while the layout is still loading", () => {
    const html = renderOverview({ status: "ready", stats: READY_STATS }, { status: "loading" })

    expect(renderedModules(html)).toEqual([])
    expect(html).toContain("正在读取总览布局…")
    expect(html).not.toContain("已隐藏全部总览模块")
  })

  it("flags a stale arrangement instead of claiming it is current", () => {
    const stale = overviewLayoutProjection({
      status: "conflict",
      modules: defaultOverviewModuleSettings(),
      savedModules: defaultOverviewModuleSettings(),
      savedLayout: defaultOverviewLayout(),
      revision: "overview-1",
      message: "外观设置已被其他窗口更新",
      retryable: false,
    })
    const html = renderOverview({ status: "ready", stats: READY_STATS }, stale)

    expect(html).toContain("总览布局可能已被其他窗口更新")
    expect(renderedModules(html)).toEqual(MODULE_ORDER)
  })

  it("reads the arrangement from the saved layout, never from the unsaved draft", () => {
    const saved = defaultOverviewModuleSettings()
    const draft = saved.map((entry) => ({ ...entry, visible: false }))
    const state: OverviewLayoutEditorState = {
      status: "ready",
      modules: draft,
      savedModules: saved,
      savedLayout: defaultOverviewLayout(),
      revision: "overview-1",
      message: null,
      retryable: false,
    }
    const layout = overviewLayoutProjection(state)

    expect(layout.status).toBe("ready")
    if (layout.status !== "ready") throw new Error("已保存状态必须可渲染")
    expect(layout.placements.map((placement) => placement.module)).toEqual(MODULE_ORDER)
    expect(layout.stale).toBe(false)
  })

  it("places every module at its saved coordinates instead of re-packing the list", () => {
    // 一份前端打包算法**产不出来**的合法布局：行与行之间有空档。它只可能来自别的写入方，
    // 因此正好用来区分「按存储的坐标摆放」与「把模块清单重新打包一遍」——后者会把
    // metrics 挪到第 1 行，前者保持第 4 行。
    const stored: OverviewLayoutWire = {
      schemaVersion: 1,
      modules: [
        { module: "metrics", size: "large", row: 3, column: 0, order: 0 },
        { module: "preferences", size: "large", row: 0, column: 0, order: 1 },
      ],
    }
    const modules = layoutToOverviewModuleSettings(stored)
    expect(
      overviewModuleSettingsToLayout(modules),
      "这份布局必须不是打包算法的输出，否则本用例区分不出两种实现",
    ).not.toEqual(stored)

    const layout = overviewLayoutProjection({
      status: "ready",
      modules,
      savedModules: modules,
      savedLayout: stored,
      revision: "overview-1",
      message: null,
      retryable: false,
    })
    const html = renderOverview({ status: "empty", stats: EMPTY_READING_OVERVIEW_STATS }, layout)

    // 先后由 order 决定（metrics 的 order 更小），位置由存储的坐标决定。
    expect(renderedModules(html)).toEqual(["metrics", "preferences"])

    // 断言的是**真实摆放**，不是「渲染器把坐标抄进了 data 属性」：
    // 元素带的是 index.css 在 xl 断点上消费的两个自定义属性（1 基网格线），以及那条
    // 规则的类名。data 属性单独存在时，把坐标删掉渲染结果一个像素都不会变。
    const metrics = moduleTag(html, "metrics")
    expect(metrics).toContain("--haven-overview-module-row:4")
    expect(metrics).toContain("--haven-overview-module-column:1")
    expect(metrics).toContain("haven-overview-module-positioned")
    // 第 4 行是存储里的 row=3；重新打包会得到第 1 行——两者在这里必须可区分。
    expect(metrics).not.toContain("--haven-overview-module-row:1")

    const preferences = moduleTag(html, "preferences")
    expect(preferences).toContain("--haven-overview-module-row:1")
    expect(preferences).toContain("--haven-overview-module-column:1")
    expect(preferences).toContain("haven-overview-module-positioned")
  })
})

describe("overview coordinates are applied at the three-column breakpoint", () => {
  it("binds the saved coordinates to the grid only where the grid really has three columns", () => {
    // 坐标走 CSS 自定义属性、断点交给 CSS：内联样式没有断点，而三列网格只在 xl 成立。
    // 这条对接测试钉住的是另一半事实——上面那条证明渲染器发出了坐标，这条证明
    // index.css 真的在 xl 断点上把它们用起来（并且窄屏不套坐标，保持自然单列流）。
    const xl = mediaBlock(1280)
    expect(xl).toContain(".haven-overview-module-positioned")
    expect(xl).toContain("grid-row-start: var(--haven-overview-module-row)")
    expect(xl).toContain("grid-column-start: var(--haven-overview-module-column)")
    expect(mediaBlock(1024)).not.toContain("haven-overview-module-positioned")
  })
})
