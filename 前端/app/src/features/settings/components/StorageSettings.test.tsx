// @vitest-environment jsdom
//
// 存储页两个列表的消费者用例：只把云盘 Gateway 与**无关的**本地位置 Hook 打桩。
// 要钉住的是：
// 1. 云盘列表读取失败时必须如实报错，绝不能把"读不到"说成"没有配置授权"；
// 2. 同一目录的「确认移除」重复点击只发出一次请求，成功后才提示并刷新。

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type {
  CloudAccountDto,
  CloudFolderDto,
  CloudStorageListDto,
  StorageLocationDto,
} from "@/lib/ipc/generated/wire"

const gateway = vi.hoisted(() => ({
  list: vi.fn(),
  folderBinding: vi.fn(),
  removeFolder: vi.fn(),
  disconnect: vi.fn(),
}))

// 本页的云盘数据只从 Gateway 进来；本地位置 Hook 与本次回归无关，
// 单独打桩以免为了它拖入整个 storage-gateway / IPC 客户端依赖图。
const local = vi.hoisted(() => ({
  value: {
    locations: [] as StorageLocationDto[],
    scans: {},
    isLoading: false,
    error: null,
    busy: false,
    reload: vi.fn(async () => {}),
    addDirectory: vi.fn(),
    rebindDirectory: vi.fn(),
    removeDirectory: vi.fn(),
    scanDirectory: vi.fn(),
    cancelDirectoryScan: vi.fn(),
  },
}))

vi.mock("../ipc/cloud-storage-gateway", () => ({ cloudStorageGateway: gateway }))
vi.mock("../lib/useStorageLocations", () => ({ useStorageLocations: () => local.value }))

import { StorageSettings } from "./StorageSettings"

const ACCOUNT: CloudAccountDto = {
  id: "5c6d7e8f-9a0b-4c1d-8e2f-3a4b5c6d7e8f",
  displayName: "只读媒体账户",
  connected: true,
  generation: 7,
}
const LOCATION: StorageLocationDto = {
  locationId: "6d7e8f9a-0b1c-4d2e-9f3a-4b5c6d7e8f9a",
  displayName: "漫画目录",
  providerType: "google_drive",
  status: "connected",
}
const BINDING: CloudFolderDto = {
  locationId: LOCATION.locationId,
  accountId: ACCOUNT.id,
  createdAtMs: 1_760_000_000_000,
}
const LIST: CloudStorageListDto = { oauthAvailable: true, accounts: [ACCOUNT] }
const EMPTY_CONFIGURED: CloudStorageListDto = { oauthAvailable: true, accounts: [] }
const EMPTY_UNCONFIGURED: CloudStorageListDto = { oauthAvailable: false, accounts: [] }

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((complete) => { resolve = complete })
  return { promise, resolve }
}

beforeEach(() => {
  for (const method of Object.values(gateway)) method.mockReset()
  local.value.locations = []
  local.value.reload.mockClear()
  local.value.addDirectory.mockClear()
  local.value.rebindDirectory.mockClear()
  local.value.removeDirectory.mockClear()
  local.value.scanDirectory.mockClear()
  local.value.cancelDirectoryScan.mockClear()
})

afterEach(() => { cleanup() })

describe("StorageSettings 的云盘列表消费者", () => {
  it("缺少 OAuth 配置时行级状态与只读连接流程保持一致", async () => {
    gateway.list.mockResolvedValue(EMPTY_UNCONFIGURED)
    render(<StorageSettings showNotice={vi.fn()} />)
    await waitFor(() => expect(screen.getByText("未配置")).toBeTruthy())
    expect(screen.queryByText("未连接")).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "查看连接流程" }))
    expect(await screen.findByText("当前应用未配置 Google OAuth 客户端，暂不能发起授权。")).toBeTruthy()
    expect((screen.getByRole("button", { name: "打开 Google 授权页" }) as HTMLButtonElement).disabled).toBe(true)
  })

  it("已配置但没有账户时展示未连接，不误报为未配置", async () => {
    gateway.list.mockResolvedValue(EMPTY_CONFIGURED)
    render(<StorageSettings showNotice={vi.fn()} />)
    await waitFor(() => expect(screen.getByText("未连接")).toBeTruthy())
    expect(screen.queryByText("未配置")).toBeNull()
    expect(screen.getByRole("button", { name: "连接流程" })).toBeTruthy()
  })

  it("云盘列表读取失败：显示错误与重试，不得把失败说成「未配置授权」", async () => {
    gateway.list.mockRejectedValue({ code: "INTERNAL_ERROR", userMessage: "读取云盘状态失败", retryable: true })
    render(<StorageSettings showNotice={vi.fn()} />)

    await waitFor(() => expect(screen.getByRole("alert")).toBeTruthy())
    expect(screen.getByText("读取云盘状态失败")).toBeTruthy()
    expect(screen.queryByText(/未配置 Google 授权/), "读失败不等于没配置，不能给用户错误结论").toBeNull()
    expect(screen.queryByText("未配置")).toBeNull()
    expect((screen.getByRole("button", { name: "连接 Google Drive" }) as HTMLButtonElement).disabled, "没有快照时不得进入连接流程").toBe(true)
    expect(screen.getByRole("button", { name: "重新读取" })).toBeTruthy()
  })

  it("同一目录的「确认移除」重复点击只发一次请求，成功后才提示并刷新", async () => {
    gateway.list.mockResolvedValue(LIST)
    gateway.folderBinding.mockResolvedValue(BINDING)
    const pending = deferred<boolean>()
    gateway.removeFolder.mockReturnValue(pending.promise)
    const showNotice = vi.fn()
    local.value.locations = [LOCATION]
    render(<StorageSettings showNotice={showNotice} />)

    await waitFor(() => expect(screen.getByRole("button", { name: "移除" })).toBeTruthy())
    fireEvent.click(screen.getByRole("button", { name: "移除" }))
    const confirm = screen.getByRole("button", { name: "确认移除" })
    fireEvent.click(confirm)
    fireEvent.click(confirm)

    expect(gateway.removeFolder, "重复点击必须撞上单飞锁").toHaveBeenCalledTimes(1)
    expect(gateway.removeFolder).toHaveBeenCalledWith(LOCATION.locationId)
    expect(showNotice, "请求还没回来，不得先报成功").not.toHaveBeenCalled()

    await act(async () => { pending.resolve(true); await pending.promise })
    await waitFor(() => expect(showNotice).toHaveBeenCalledWith("已移除云盘目录绑定；远端原文件与账户授权保留"))
    expect(gateway.removeFolder).toHaveBeenCalledTimes(1)
  })
})
