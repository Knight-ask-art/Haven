// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import type { PlaybackSettingsValue, SettingsPatch } from "@/lib/ipc/settings-wire"
import { PLAYBACK_RATE_OPTIONS } from "../lib/settingsDisplay"
import type { SettingsFormController } from "../lib/useSettingsForm"
import { PlaybackSettings } from "./SettingsPage"

// 只挂载播放分区：不触达任何 IPC，也不需要 Router。
vi.mock("@/lib/ipc/runtime", () => ({
  getHavenClient: () => { throw new Error("组件测试不提供 IPC 客户端") },
  getHavenClientMode: () => "mock",
  isTauriRuntime: () => false,
  resolveTauriRuntime: () => false,
  selectHavenClientMode: () => "mock",
}))

afterEach(cleanup)

function playbackValue(overrides: Partial<PlaybackSettingsValue> = {}): PlaybackSettingsValue {
  return {
    section: "playback",
    defaultPlaybackRate: "one",
    autoResume: true,
    autoNext: true,
    ...overrides,
  }
}

function controller(value: PlaybackSettingsValue, change: (patch: SettingsPatch) => void): SettingsFormController {
  return {
    section: "playback",
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

function renderPlayback(value: PlaybackSettingsValue): (patch: SettingsPatch) => void {
  const change = vi.fn<(patch: SettingsPatch) => void>()
  render(<PlaybackSettings form={controller(value, change)} />)
  return change
}

function rateGroup(): HTMLElement {
  return screen.getByRole("group", { name: "默认倍速" })
}

describe("playback default rate", () => {
  it("offers exactly the five contract options and nothing else", () => {
    renderPlayback(playbackValue())
    const labels = within(rateGroup()).getAllByRole("button").map((button) => button.textContent)
    expect(labels).toEqual(PLAYBACK_RATE_OPTIONS.map((option) => option.label))
  })

  it("writes the wire value of every option into the draft", () => {
    const change = renderPlayback(playbackValue())
    for (const option of PLAYBACK_RATE_OPTIONS) {
      fireEvent.click(within(rateGroup()).getByRole("button", { name: option.label }))
      expect(change).toHaveBeenLastCalledWith({ section: "playback", defaultPlaybackRate: option.value })
    }
  })

  it("marks the stored rate as pressed and the others as not pressed", () => {
    renderPlayback(playbackValue({ defaultPlaybackRate: "one_point_five" }))
    expect(within(rateGroup()).getByRole("button", { name: "1.5x" }).getAttribute("aria-pressed")).toBe("true")
    expect(within(rateGroup()).getByRole("button", { name: "1.0x" }).getAttribute("aria-pressed")).toBe("false")
  })
})

describe("playback toggles", () => {
  it("writes the flipped auto-next value", () => {
    const change = renderPlayback(playbackValue({ autoNext: true }))
    fireEvent.click(screen.getByRole("switch", { name: "自动下一集" }))
    expect(change).toHaveBeenCalledWith({ section: "playback", autoNext: false })
  })

  it("writes the flipped auto-resume value", () => {
    const change = renderPlayback(playbackValue({ autoResume: false }))
    fireEvent.click(screen.getByRole("switch", { name: "自动继续" }))
    expect(change).toHaveBeenCalledWith({ section: "playback", autoResume: true })
  })

  it("reflects the stored values on the switches themselves", () => {
    renderPlayback(playbackValue({ autoResume: false, autoNext: true }))
    expect(screen.getByRole("switch", { name: "自动继续" }).getAttribute("aria-checked")).toBe("false")
    expect(screen.getByRole("switch", { name: "自动下一集" }).getAttribute("aria-checked")).toBe("true")
  })

  it("exposes exactly the three registered playback settings", () => {
    renderPlayback(playbackValue())
    // 契约里只有 defaultPlaybackRate / autoResume / autoNext：五个倍速按钮 + 两个开关，
    // 其余播放行为（字幕、音轨、画质……）不在本页，也不该在这里出现。
    expect(within(rateGroup()).getAllByRole("button")).toHaveLength(PLAYBACK_RATE_OPTIONS.length)
    expect(screen.getAllByRole("switch")).toHaveLength(2)
  })
})

describe("playback screenshot information", () => {
  it("states the real player shortcut and the real dialog directory", () => {
    renderPlayback(playbackValue())
    expect(screen.getByText("Ctrl+Shift+S")).toBeTruthy()
    expect(screen.getByText("下载 / 栖阅 / 截图")).toBeTruthy()
  })

  it("keeps the screenshot group free of controls, because it is not a setting", () => {
    renderPlayback(playbackValue())
    const group = screen.getByText("Ctrl+Shift+S").closest("section")
    expect(group).not.toBeNull()
    expect(within(group as HTMLElement).queryAllByRole("button")).toHaveLength(0)
    expect(within(group as HTMLElement).queryAllByRole("switch")).toHaveLength(0)
  })
})