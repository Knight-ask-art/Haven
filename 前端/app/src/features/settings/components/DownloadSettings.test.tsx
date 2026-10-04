// @vitest-environment jsdom
//
// 下载设置分区的用例：挂在**真实的** useSettingsForm 上（草稿 / dirty / CAS / 错误全走状态机），
// 只把 SettingsGateway 当作 IO 边界打桩。要钉住的是：
// 1. 控件读的是网关给的真实快照，不是组件自带的默认草稿；
// 2. 保存只提交 downloads 分区，并带上快照里读到的 revision；
// 3. 未保存的选择留在草稿里；「恢复默认」回到契约默认值且不写任何东西；
// 4. 读失败时如实报错，重试重新读取，且不会把一份没读到的草稿提交上去。

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { MemoryRouter } from "react-router"
import { afterEach, describe, expect, it, vi } from "vitest"
import type {
  DownloadSettingsValue,
  SettingsSnapshot,
  SettingsUpdateResult,
} from "@/lib/ipc/settings-wire"
import { settingsGateway } from "../ipc/gateway"
import { useSettingsForm } from "../lib/useSettingsForm"
import { DownloadSettings } from "./DownloadSettings"

// 组件测试不提供 IPC 客户端：任何漏打桩的调用都当场失败，而不是悄悄走 Mock 客户端。
vi.mock("@/lib/ipc/runtime", () => ({
  getHavenClient: () => { throw new Error("下载分区测试不提供 IPC 客户端") },
  getHavenClientMode: () => "mock",
  isTauriRuntime: () => false,
  resolveTauriRuntime: () => false,
  selectHavenClientMode: () => "mock",
}))

afterEach(() => { cleanup(); vi.restoreAllMocks() })

/** 契约默认值：3 个任务 / 不限速 / 自动继续为开（settings-wire 的 defaultSettingsValue）。 */
const SAVED: DownloadSettingsValue = { section: "downloads", concurrentTasks: "three", speedLimit: "unlimited", autoContinue: true }
const STORED: DownloadSettingsValue = { section: "downloads", concurrentTasks: "five", speedLimit: "mbps5", autoContinue: false }
const PICKED: DownloadSettingsValue = { section: "downloads", concurrentTasks: "two", speedLimit: "unlimited", autoContinue: true }

function snapshot(value: DownloadSettingsValue, revision: string | null = "downloads-rev-1"): SettingsSnapshot {
  return { value, revision }
}

function Harness() {
  const form = useSettingsForm("downloads", settingsGateway)
  return <DownloadSettings form={form} />
}

function renderDownloads(): void {
  render(<MemoryRouter><Harness /></MemoryRouter>)
}

function segment(label: string): HTMLElement {
  return screen.getByRole("group", { name: label })
}
function optionPressed(label: string, option: string): string | null {
  return within(segment(label)).getByRole("button", { name: option }).getAttribute("aria-pressed")
}
function saveButton(): HTMLButtonElement {
  return screen.getByRole("button", { name: "保存修改" }) as HTMLButtonElement
}
function autoContinue(): HTMLElement {
  return screen.getByRole("switch", { name: "自动继续中断任务" })
}

describe("下载设置分区走真实的 Settings 表单", () => {
  it("加载中禁用控件，读到快照后显示已存储的下载策略", async () => {
    let release!: (value: SettingsSnapshot) => void
    vi.spyOn(settingsGateway, "settingsGet").mockReturnValue(new Promise<SettingsSnapshot>((resolve) => { release = resolve }))
    renderDownloads()

    expect(screen.getByText("正在加载…")).toBeTruthy()
    expect(saveButton().disabled).toBe(true)

    await act(async () => { release(snapshot(STORED)) })

    await waitFor(() => expect(screen.getByText("设置已保存")).toBeTruthy())
    expect(settingsGateway.settingsGet).toHaveBeenCalledWith("downloads")
    expect(optionPressed("同时下载数量", "5 个任务")).toBe("true")
    expect(optionPressed("下载速度限制", "5 MB/s")).toBe("true")
    expect(autoContinue().getAttribute("aria-checked")).toBe("false")
    expect(saveButton().disabled).toBe(true)
  })

  it("保存只提交 downloads 分区，并带上快照里读到的 revision", async () => {
    vi.spyOn(settingsGateway, "settingsGet").mockResolvedValue(snapshot(SAVED, "downloads-rev-7"))
    const updated: SettingsUpdateResult = { value: PICKED, revision: "downloads-rev-8", changed: true }
    const update = vi.spyOn(settingsGateway, "settingsUpdate").mockResolvedValue(updated)
    renderDownloads()
    await waitFor(() => expect(screen.getByText("设置已保存")).toBeTruthy())

    fireEvent.click(within(segment("同时下载数量")).getByRole("button", { name: "2 个任务" }))
    expect(screen.getByText("有未保存的修改")).toBeTruthy()

    fireEvent.click(saveButton())
    await waitFor(() => expect(update).toHaveBeenCalledTimes(1))
    expect(update).toHaveBeenCalledWith({
      section: "downloads",
      expectedRevision: "downloads-rev-7",
      patch: { section: "downloads", concurrentTasks: "two" },
    })
    await waitFor(() => expect(screen.getByText("设置已保存")).toBeTruthy())
    expect(optionPressed("同时下载数量", "2 个任务")).toBe("true")
  })

  it("未保存的选择留在草稿里，「恢复默认」回到契约默认值且不写任何东西", async () => {
    vi.spyOn(settingsGateway, "settingsGet").mockResolvedValue(snapshot(SAVED))
    const update = vi.spyOn(settingsGateway, "settingsUpdate")
    renderDownloads()
    await waitFor(() => expect(screen.getByText("设置已保存")).toBeTruthy())

    fireEvent.click(within(segment("下载速度限制")).getByRole("button", { name: "10 MB/s" }))
    fireEvent.click(autoContinue())
    expect(screen.getByText("有未保存的修改")).toBeTruthy()
    expect(optionPressed("下载速度限制", "10 MB/s")).toBe("true")
    expect(autoContinue().getAttribute("aria-checked")).toBe("false")

    fireEvent.click(screen.getByRole("button", { name: "恢复默认" }))
    // 契约默认值就是刚读到的快照，所以草稿与已保存值重新一致。
    await waitFor(() => expect(screen.getByText("设置已保存")).toBeTruthy())
    expect(optionPressed("同时下载数量", "3 个任务")).toBe("true")
    expect(optionPressed("下载速度限制", "不限速")).toBe("true")
    expect(autoContinue().getAttribute("aria-checked")).toBe("true")
    expect(update).not.toHaveBeenCalled()
  })

  it("读失败时如实报错，重试重新读取，且不会保存一份没读到的草稿", async () => {
    const get = vi.spyOn(settingsGateway, "settingsGet")
      .mockRejectedValueOnce({ code: "INTERNAL_ERROR", userMessage: "读取下载设置失败", retryable: true })
      .mockResolvedValueOnce(snapshot(SAVED))
    const update = vi.spyOn(settingsGateway, "settingsUpdate")
    renderDownloads()

    await waitFor(() => expect(screen.getByRole("alert")).toBeTruthy())
    expect(screen.getByText("读取下载设置失败")).toBeTruthy()
    expect(saveButton().disabled).toBe(true)

    fireEvent.click(saveButton())
    expect(update, "没读到快照时不得提交任何 patch").not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole("button", { name: "重试" }))
    await waitFor(() => expect(screen.getByText("设置已保存")).toBeTruthy())
    expect(get).toHaveBeenCalledTimes(2)
  })
})
