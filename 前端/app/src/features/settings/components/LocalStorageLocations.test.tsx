// @vitest-environment jsdom
// 控件消费真实 Hook 的返回结构，后端生命周期由 SQLite 集成覆盖。
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"
import type { StorageLocationDto } from "@/lib/ipc/generated/wire"
import type { StorageScanState, useStorageLocations } from "../lib/useStorageLocations"
import { LocalStorageLocations } from "./LocalStorageLocations"

type StorageController = ReturnType<typeof useStorageLocations>
const READ_ONLY: StorageLocationDto = { locationId: "1f2a3b4c-5d6e-4f70-8192-a3b4c5d6e7f8", displayName: "只读漫画目录", providerType: "local", status: "read_only" }
const CONNECTED: StorageLocationDto = { locationId: "2a3b4c5d-6e7f-4081-92a3-b4c5d6e7f809", displayName: "本地影视目录", providerType: "local", status: "connected" }
const RUNNING: StorageScanState = { taskId: "3b4c5d6e-7f80-4192-a3b4-c5d6e7f80912", phaseCode: "started", phase: "开始扫描", filesSeen: 12, newItem: 3, message: null, terminal: false }

function controller(overrides: Partial<StorageController> = {}): StorageController {
  return {
    locations: [], scans: {}, isLoading: false, error: null, busy: false,
    reload: vi.fn(async () => {}), addDirectory: vi.fn(async () => {}),
    rebindDirectory: vi.fn(async () => {}), removeDirectory: vi.fn(async () => {}),
    scanDirectory: vi.fn(async () => {}), cancelDirectoryScan: vi.fn(async () => {}), ...overrides,
  }
}
function rowOf(name: string) {
  const row = screen.getByText(name).closest("tr")
  if (!row) throw new Error(`缺少目录行：${name}`)
  return row
}
afterEach(() => { cleanup() })

describe("LocalStorageLocations 的扫描与确认", () => {
  it("非 connected 位置禁用扫描，可用位置向 Hook 发起扫描", () => {
    const storage = controller({ locations: [READ_ONLY, CONNECTED] })
    render(<LocalStorageLocations storage={storage} />)
    expect(within(rowOf(READ_ONLY.displayName)).getByText("只读")).toBeTruthy()
    expect((within(rowOf(READ_ONLY.displayName)).getByRole("button", { name: "扫描" }) as HTMLButtonElement).disabled).toBe(true)
    const scan = within(rowOf(CONNECTED.displayName)).getByRole("button", { name: "扫描" })
    expect((scan as HTMLButtonElement).disabled).toBe(false)
    fireEvent.click(scan)
    expect(storage.scanDirectory).toHaveBeenCalledWith(CONNECTED)
  })

  it("运行中的扫描展示进度和取消，不提供重复扫描入口", () => {
    const storage = controller({ locations: [CONNECTED], scans: { [CONNECTED.locationId]: RUNNING } })
    render(<LocalStorageLocations storage={storage} />)
    const row = rowOf(CONNECTED.displayName)
    expect(within(row).getByText(/开始扫描 · 已见 12 · 新增 3/)).toBeTruthy()
    expect(within(row).queryByRole("button", { name: "扫描" })).toBeNull()
    fireEvent.click(within(row).getByRole("button", { name: "取消扫描" }))
    expect(storage.cancelDirectoryScan).toHaveBeenCalledTimes(1)
    expect(storage.cancelDirectoryScan).toHaveBeenCalledWith(CONNECTED, RUNNING)
  })

  it("移除必须确认，确认条同步收起避免再次提交", () => {
    const storage = controller({ locations: [CONNECTED] })
    render(<LocalStorageLocations storage={storage} />)
    fireEvent.click(screen.getByLabelText("本地影视目录的更多操作"))
    fireEvent.click(within(rowOf(CONNECTED.displayName)).getByText("移除绑定"))
    expect(storage.removeDirectory).not.toHaveBeenCalled()
    const confirm = within(screen.getByRole("group", { name: "确认移除本地目录" })).getByRole("button", { name: "确认移除" })
    fireEvent.click(confirm)
    expect(storage.removeDirectory).toHaveBeenCalledTimes(1)
    expect(storage.removeDirectory).toHaveBeenCalledWith(CONNECTED)
    expect(screen.queryByRole("group", { name: "确认移除本地目录" })).toBeNull()
    fireEvent.click(confirm)
    expect(storage.removeDirectory).toHaveBeenCalledTimes(1)
  })
})
