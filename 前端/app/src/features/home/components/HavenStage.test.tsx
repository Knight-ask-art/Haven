// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { MemoryRouter } from "react-router"
import { afterEach, describe, expect, it, vi } from "vitest"
import { HavenStage, type HavenStageProps } from "./HavenStage"

const baseProps: HavenStageProps = {
  id: "work-1",
  title: "测试作品",
  metadata: "图书",
  description: "描述",
  backdropUrl: "",
  primaryActionLabel: "继续阅读",
}

afterEach(cleanup)

function renderStage(props: Partial<HavenStageProps> = {}) {
  return render(
    <MemoryRouter>
      <HavenStage {...baseProps} {...props} />
    </MemoryRouter>,
  )
}

describe("HavenStage controlled actions", () => {
  it.each([
    ["paused", "继续下载"],
    ["interrupted", "重试下载"],
  ] as const)("presents %s as an actionable recovery control", (downloadStatus, label) => {
    const onAction = vi.fn()
    renderStage({ downloadStatus, onAction, isDownloaded: false })
    const button = screen.getByRole("button", { name: label })
    expect(button.hasAttribute("disabled")).toBe(false)
    fireEvent.click(button)
    expect(onAction).toHaveBeenCalledWith("download")
  })

  it("hides offline management actions without a manageable task", () => {
    renderStage({ isDownloaded: false, canManageOffline: false })
    fireEvent.click(screen.getByTitle("更多选项"))
    expect(screen.queryByRole("button", { name: "在文件夹中定位" })).toBeNull()
    expect(screen.queryByRole("button", { name: "删除离线内容" })).toBeNull()
  })

  it("emits offline management intents without mutating persistence locally", () => {
    const onAction = vi.fn()
    renderStage({ isDownloaded: true, canManageOffline: true, onAction })

    fireEvent.click(screen.getByTitle("更多选项"))
    fireEvent.click(screen.getByRole("button", { name: "在文件夹中定位" }))
    expect(onAction).toHaveBeenLastCalledWith("folder")

    fireEvent.click(screen.getByTitle("更多选项"))
    fireEvent.click(screen.getByRole("button", { name: "删除离线内容" }))
    expect(onAction).toHaveBeenLastCalledWith("delete")
  })

  it("disables persistent actions while a request is pending", () => {
    renderStage({ isActionPending: true, isDownloaded: false })
    expect((screen.getByRole("button", { name: "继续阅读" }) as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByTitle("加入收藏") as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByTitle("下载至本地") as HTMLButtonElement).disabled).toBe(true)
  })

  it("says resume / retry for a recoverable download instead of 下载至本地", () => {
    renderStage({ isDownloaded: false, downloadStatus: "paused" })
    expect(screen.getByTitle("下载已暂停，可继续")).toBeTruthy()
    cleanup()
    renderStage({ isDownloaded: false, downloadStatus: "interrupted" })
    expect(screen.getByTitle("下载已中断，可继续或重试")).toBeTruthy()
    expect(screen.queryByTitle("下载至本地")).toBeNull()
  })
})
