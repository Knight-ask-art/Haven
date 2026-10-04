// @vitest-environment jsdom
//
// 挂载**整页**（/settings/overview），把总览布局 hook 固定成一份已知事实，断言页面真的
// 按它渲染。
//
// 与 SettingsPage.overview.test.tsx 的分工：那一条把投影当作入参直接交给渲染器，证明的
// 是「投影 + 渲染器」这一对；这一条走真实路由，证明 `SettingsContent` 确实把
// `useOverviewLayout` 的状态接到了 `SettingsOverview` 上——两者之间的接线错了，
// 组件级用例是发现不了的。

import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { MemoryRouter, Route, Routes } from "react-router"
import { afterEach, describe, expect, it, vi } from "vitest"

import type { OverviewLayoutEditorState } from "../lib/useOverviewLayout"
import {
  defaultOverviewModuleSettings,
  overviewModuleSettingsToLayout,
  setOverviewModuleVisible,
} from "../lib/overview-layout"
// `vi.mock` 会被提升到所有 import 之上，因此这里可以静态导入页面（与同目录的
// SettingsPage.appearance.test.tsx 同一套写法）。
import { SettingsPage } from "./SettingsPage"

/** 页面看到的那份布局编辑器状态（用例在渲染前写入）。 */
const editorState = vi.hoisted(() => ({ current: null as unknown }))
const reloads = vi.hoisted(() => ({ count: 0 }))

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

// 只替换 hook：投影函数、模型与渲染器全部走真实实现，接错线就会在这里暴露。
vi.mock("@/features/settings/lib/useOverviewLayout", async (importOriginal) => {
  const original = await importOriginal<typeof import("@/features/settings/lib/useOverviewLayout")>()
  return {
    ...original,
    useOverviewLayout: () => ({
      state: editorState.current,
      isDirty: false,
      isSaving: false,
      move: () => undefined,
      setSize: () => undefined,
      setVisible: () => undefined,
      save: () => undefined,
      reset: () => undefined,
      reload: () => { reloads.count += 1 },
    }),
  }
})

afterEach(() => {
  cleanup()
  editorState.current = null
  reloads.count = 0
})

/**
 * 一份编辑器状态：`savedLayout` 由 `savedModules` 打包得出，也就是编辑器真的保存过之后
 * 后端会返回的那一份（打包是编辑器与后端之间唯一的不动点）。
 */
function stateOf(overrides: Partial<OverviewLayoutEditorState> = {}): OverviewLayoutEditorState {
  const modules = defaultOverviewModuleSettings()
  const base: OverviewLayoutEditorState = {
    status: "ready",
    modules,
    savedModules: modules,
    savedLayout: overviewModuleSettingsToLayout(modules),
    revision: null,
    message: null,
    retryable: false,
  }
  const merged = { ...base, ...overrides }
  // `savedModules` 被单独改动时，`savedLayout` 必须跟着走——它们描述的是同一份已保存事实。
  return overrides.savedModules && !overrides.savedLayout
    ? { ...merged, savedLayout: overviewModuleSettingsToLayout(overrides.savedModules) }
    : merged
}

function renderOverviewRoute() {
  return render(
    <MemoryRouter initialEntries={["/settings/overview"]}>
      <Routes>
        <Route path="/settings/:section" element={<SettingsPage />} />
      </Routes>
    </MemoryRouter>,
  )
}

/** 页面上真实的模块排列（DOM 顺序）。 */
function moduleOrder(container: HTMLElement): string[] {
  return [...container.querySelectorAll("[data-overview-module]")]
    .map((element) => element.getAttribute("data-overview-module") ?? "")
}

/** 页面给某个模块的档位。 */
function moduleSize(container: HTMLElement, module: string): string | null {
  return container
    .querySelector(`[data-overview-module="${module}"]`)
    ?.getAttribute("data-overview-module-size") ?? null
}

describe("overview route renders the saved arrangement", () => {
  it("projects the editor's saved modules into the page", () => {
    const modules = defaultOverviewModuleSettings()
    editorState.current = stateOf({ savedModules: modules })

    const { container } = renderOverviewRoute()

    expect(moduleOrder(container)).toEqual([
      "preferences",
      "metrics",
      "reading-minutes",
      "type-share",
      "reading-heatmap",
    ])
    expect(moduleSize(container, "preferences")).toBe("large")
    expect(moduleSize(container, "type-share")).toBe("small")
    // 页面标题与统计范围不是模块：它们没有 data-overview-module，也不会被布局藏掉。
    expect(document.body.textContent).toContain("READING OVERVIEW / PERSONAL SIGNAL")
  })

  it("drops a hidden module from the page entirely", () => {
    const modules = setOverviewModuleVisible(defaultOverviewModuleSettings(), "metrics", false)
    editorState.current = stateOf({ modules, savedModules: modules })

    const { container } = renderOverviewRoute()

    expect(moduleOrder(container)).not.toContain("metrics")
    expect(container.querySelector('[aria-label="阅读指标"]')).toBeNull()
    // 其余模块不受影响。
    expect(moduleOrder(container)).toContain("preferences")
    expect(moduleOrder(container)).toContain("reading-heatmap")
  })

  it("follows an unsaved draft's order only after it is saved", () => {
    const saved = defaultOverviewModuleSettings()
    const draft = [saved[1]!, saved[0]!, ...saved.slice(2)]
    editorState.current = stateOf({ modules: draft, savedModules: saved })

    const { container } = renderOverviewRoute()

    // 草稿还没落盘：页面必须显示**已保存**的排列，而不是编辑器里那个还没保存的顺序。
    expect(moduleOrder(container).slice(0, 2)).toEqual(["preferences", "metrics"])
  })

  it("falls back to the default layout with a visible explanation when the read failed", () => {
    editorState.current = stateOf({
      status: "error",
      revision: null,
      message: "数据库暂时不可用",
      retryable: true,
    })

    const { container } = renderOverviewRoute()

    expect(moduleOrder(container)).toEqual([
      "preferences",
      "metrics",
      "reading-minutes",
      "type-share",
      "reading-heatmap",
    ])
    expect(document.body.textContent).toContain("总览布局暂时不可用")
    expect(document.body.textContent).toContain("以下按默认布局显示")

    // 横幅上的入口必须是真实动作：点了就重新读取布局，而不是一个装饰按钮。
    expect(reloads.count).toBe(0)
    fireEvent.click(screen.getByText("重新读取布局"))
    expect(reloads.count).toBe(1)
  })

  it("shows the layout loading state instead of guessing an arrangement", () => {
    editorState.current = stateOf({ status: "loading" })

    const { container } = renderOverviewRoute()

    expect(moduleOrder(container)).toEqual([])
    expect(document.body.textContent).toContain("正在读取总览布局…")
    expect(document.body.textContent).not.toContain("已隐藏全部总览模块")
  })

  it("says the modules were deliberately hidden when the saved layout is empty", () => {
    editorState.current = stateOf({
      savedModules: defaultOverviewModuleSettings().map((entry) => ({ ...entry, visible: false })),
      revision: "overview-1",
    })

    const { container } = renderOverviewRoute()

    expect(moduleOrder(container)).toEqual([])
    expect(document.body.textContent).toContain("已隐藏全部总览模块")
    expect(document.body.textContent).toContain("这是你保存过的排列，不是读取失败")
  })
})
