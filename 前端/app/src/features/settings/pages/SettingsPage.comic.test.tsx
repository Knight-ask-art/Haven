// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react"
import { useState } from "react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { applySettingsPatch } from "@/lib/ipc/settings-wire"
import type { ComicSettingsValue, SettingsPatch } from "@/lib/ipc/settings-wire"
import type { SettingsFormController } from "../lib/useSettingsForm"
import { ComicSettings } from "./SettingsPage"

vi.mock("@/lib/ipc/runtime", () => ({
  getHavenClient: () => { throw new Error("组件测试不提供 IPC 客户端") },
  getHavenClientMode: () => "mock",
  isTauriRuntime: () => false,
  resolveTauriRuntime: () => false,
  selectHavenClientMode: () => "mock",
}))

afterEach(() => cleanup())

function comicValue(overrides: Partial<ComicSettingsValue> = {}): ComicSettingsValue {
  return {
    section: "comic",
    viewMode: "single",
    direction: "rtl",
    pageGap: "twelve",
    preloadPages: "three",
    ...overrides,
  }
}

function comicForm(
  value: ComicSettingsValue,
  change: (patch: SettingsPatch) => void,
  isDirty: boolean,
  actions: { save?: () => void; resetToDefaults?: () => void } = {},
): SettingsFormController {
  return {
    section: "comic",
    state: { status: "ready", saved: value, revision: null },
    displayValue: value,
    isLoading: false,
    isSaving: false,
    isDirty,
    hasError: false,
    errorMessage: null,
    change,
    save: actions.save ?? (() => undefined),
    retry: () => undefined,
    reload: () => undefined,
    resetToDefaults: actions.resetToDefaults ?? (() => undefined),
  }
}

function ComicStatefulHarness({
  onPatch,
}: {
  onPatch: (patch: SettingsPatch) => void
}) {
  const [draft, setDraft] = useState(comicValue())
  const [dirty, setDirty] = useState(false)
  const change = (patch: SettingsPatch) => {
    onPatch(patch)
    const next = applySettingsPatch(draft, patch)
    if (next.section === "comic") setDraft(next)
    setDirty(true)
  }

  return <ComicSettings form={comicForm(draft, change, dirty)} resourceContext={null} showNotice={() => undefined} />
}

describe("ComicSettings", () => {
  it("matches the approved page structure without a summary rail or extra panels", () => {
    render(<ComicSettings form={comicForm(comicValue(), () => undefined, false)} resourceContext={null} showNotice={() => undefined} />)

    expect(screen.getByRole("heading", { name: "漫画" })).toBeTruthy()
    expect(screen.getByRole("heading", { name: "阅读方式" })).toBeTruthy()
    expect(screen.getByRole("heading", { name: "页面间距与预加载" })).toBeTruthy()
    expect(screen.getByTestId("comic-resource-preferences-empty")).toBeTruthy()
    expect(screen.getByText("所有设置已保存")).toBeTruthy()
    expect(screen.getByText("恢复默认")).toBeTruthy()
    expect(screen.queryByText("当前配置")).toBeNull()
    expect(screen.queryByText("快速操作")).toBeNull()

    const modeGroup = within(screen.getByRole("group", { name: "阅读模式" }))
    expect(modeGroup.getByRole("button", { name: "单页" }).getAttribute("aria-pressed")).toBe("true")
    expect(modeGroup.getByRole("button", { name: "双页" }).getAttribute("aria-pressed")).toBe("false")
    expect(screen.getByRole("group", { name: "翻页方向" })).toBeTruthy()
    expect(screen.getByRole("group", { name: "页面间距" })).toBeTruthy()
    expect(screen.getByRole("group", { name: "预加载页数" })).toBeTruthy()
  })

  it("writes each selection to the existing comic form draft and reflects the selected state", () => {
    const patches: SettingsPatch[] = []
    render(<ComicStatefulHarness onPatch={(patch) => patches.push(patch)} />)

    fireEvent.click(screen.getByRole("button", { name: "双页" }))
    fireEvent.click(screen.getByRole("button", { name: "从左向右" }))
    fireEvent.click(screen.getByRole("button", { name: "24 px" }))
    fireEvent.click(screen.getByRole("button", { name: "5 页" }))

    expect(patches).toEqual([
      { section: "comic", viewMode: "double" },
      { section: "comic", direction: "ltr" },
      { section: "comic", pageGap: "twenty_four" },
      { section: "comic", preloadPages: "five" },
    ])
    expect(screen.getByRole("button", { name: "双页" }).getAttribute("aria-pressed")).toBe("true")
    expect(screen.getByRole("button", { name: "从左向右" }).getAttribute("aria-pressed")).toBe("true")
    expect(screen.getByRole("button", { name: "24 px" }).getAttribute("aria-pressed")).toBe("true")
    expect(screen.getByRole("button", { name: "5 页" }).getAttribute("aria-pressed")).toBe("true")
    expect(screen.getByText("有未保存的修改")).toBeTruthy()
    expect(screen.getByRole("button", { name: "保存修改" })).toBeTruthy()
  })

  it("keeps the existing save and reset actions available from the page status row", () => {
    const save = vi.fn()
    const resetToDefaults = vi.fn()
    render(
      <ComicSettings
        form={comicForm(comicValue(), () => undefined, true, { save, resetToDefaults })}
        resourceContext={null}
        showNotice={() => undefined}
      />,
    )

    fireEvent.click(screen.getByRole("button", { name: "保存修改" }))
    fireEvent.click(screen.getByRole("button", { name: "恢复默认" }))

    expect(save).toHaveBeenCalledTimes(1)
    expect(resetToDefaults).toHaveBeenCalledTimes(1)
  })
})
