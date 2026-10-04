// @vitest-environment jsdom
//
// 云盘登记表 Hook 的时序用例。
//
// 这里要钉的不是"页面显示了什么"，而是三类只有**谁先谁后**才能成立的只读不变量：
// 1. 位置集合变化后，先发出的读取迟到返回，不得覆盖后发出那次的快照与绑定投影；
// 2. 断开账户必须带上调用方读到的代际；CAS 失败要如实报错，且不得当成成功去重读；
// 3. 面板卸载后，在途请求迟到返回，不得再触发一次列表重读。

import { act, cleanup, renderHook, waitFor } from "@testing-library/react"
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
  disconnect: vi.fn(),
}))

// Hook 只依赖这个 IO 边界；不要加载测试不使用的整个生产客户端依赖图。
vi.mock("../ipc/cloud-storage-gateway", () => ({ cloudStorageGateway: gateway }))

import { useCloudStorageRegistry } from "./useCloudStorageRegistry"

const ACCOUNT_A: CloudAccountDto = {
  id: "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d",
  displayName: "只读媒体账户",
  connected: true,
  generation: 7,
}
const ACCOUNT_B: CloudAccountDto = {
  id: "2b3c4d5e-6f7a-4b8c-9d0e-1f2a3b4c5d6e",
  displayName: "第二个只读账户",
  connected: true,
  generation: 3,
}
const LOCATION_A: StorageLocationDto = {
  locationId: "3c4d5e6f-7a8b-4c9d-8e0f-1a2b3c4d5e6f",
  displayName: "漫画目录",
  providerType: "google_drive",
  status: "connected",
}
const LOCATION_B: StorageLocationDto = {
  locationId: "4d5e6f7a-8b9c-4d0e-9f1a-2b3c4d5e6f7a",
  displayName: "资料目录",
  providerType: "google_drive",
  status: "connected",
}
const BINDING_A: CloudFolderDto = {
  locationId: LOCATION_A.locationId,
  accountId: ACCOUNT_A.id,
  createdAtMs: 1_760_000_000_000,
}
const BINDING_B: CloudFolderDto = {
  locationId: LOCATION_B.locationId,
  accountId: ACCOUNT_B.id,
  createdAtMs: 1_760_000_001_000,
}
const LIST_A_ONLY: CloudStorageListDto = { oauthAvailable: true, accounts: [ACCOUNT_A] }
const LIST_BOTH: CloudStorageListDto = { oauthAvailable: true, accounts: [ACCOUNT_A, ACCOUNT_B] }

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((complete) => { resolve = complete })
  return { promise, resolve }
}

beforeEach(() => {
  for (const method of Object.values(gateway)) method.mockReset()
})

// 本文件没开 vitest globals，RTL 的自动清理不生效；显式卸载，免得上一个用例的 Hook 活到下一个。
afterEach(() => { cleanup() })

describe("useCloudStorageRegistry 的只读投影时序", () => {
  it("位置集合变化后，先发出的读取迟到返回也不得覆盖后发出的快照与绑定", async () => {
    const stale = deferred<CloudStorageListDto>()
    gateway.list.mockReturnValueOnce(stale.promise).mockResolvedValue(LIST_BOTH)
    gateway.folderBinding.mockImplementation(async (id: string) => (id === LOCATION_A.locationId ? BINDING_A : BINDING_B))
    const { result, rerender } = renderHook(
      ({ locations }: { locations: StorageLocationDto[] }) => useCloudStorageRegistry(locations),
      { initialProps: { locations: [LOCATION_A] } },
    )
    expect(gateway.list, "挂载时就发出第一次读取").toHaveBeenCalledTimes(1)

    rerender({ locations: [LOCATION_A, LOCATION_B] })
    await waitFor(() => expect(result.current.snapshot).toEqual(LIST_BOTH))
    expect(result.current.folders).toEqual([BINDING_A, BINDING_B])

    await act(async () => { stale.resolve(LIST_A_ONLY); await stale.promise })
    expect(result.current.snapshot, "旧位置集合的读取结果不得覆盖新快照").toEqual(LIST_BOTH)
    expect(result.current.folders).toEqual([BINDING_A, BINDING_B])
    expect(result.current.loading).toBe(false)
  })

  it("断开账户带上读到的代际；CAS 失败如实报错，且不得走成功路径重读", async () => {
    gateway.list.mockResolvedValue(LIST_A_ONLY)
    gateway.folderBinding.mockResolvedValue(BINDING_A)
    const { result } = renderHook(() => useCloudStorageRegistry([LOCATION_A]))
    await waitFor(() => expect(result.current.snapshot).toEqual(LIST_A_ONLY))

    // 契约里的真实代际冲突：CLOUD_ACCOUNT_STALE（Application `account_stale()`）。
    gateway.disconnect.mockRejectedValue({ code: "CLOUD_ACCOUNT_STALE", userMessage: "云盘账户状态已变化，请刷新后重试", retryable: true })
    let outcome = true
    await act(async () => { outcome = await result.current.disconnect(ACCOUNT_A) })

    expect(gateway.disconnect, "CAS 必须用调用方读到的代际，不能自增或用常量").toHaveBeenCalledWith({
      accountId: ACCOUNT_A.id,
      expectedGeneration: ACCOUNT_A.generation,
    })
    expect(outcome, "CAS 失败不是成功").toBe(false)
    expect(result.current.error?.code).toBe("CLOUD_ACCOUNT_STALE")
    expect(result.current.error?.message).toBe("云盘账户状态已变化，请刷新后重试")
    expect(gateway.list, "失败不得触发成功路径的列表重读").toHaveBeenCalledTimes(1)
    expect(result.current.busy).toBe(false)
  })

  it("卸载后在途的断开迟到返回：不得再触发列表重读", async () => {
    gateway.list.mockResolvedValue(LIST_A_ONLY)
    gateway.folderBinding.mockResolvedValue(BINDING_A)
    const { result, unmount } = renderHook(() => useCloudStorageRegistry([LOCATION_A]))
    await waitFor(() => expect(result.current.snapshot).toEqual(LIST_A_ONLY))

    const pending = deferred<CloudAccountDto>()
    gateway.disconnect.mockReturnValue(pending.promise)
    let outcome: Promise<boolean> | undefined
    act(() => { outcome = result.current.disconnect(ACCOUNT_A) })
    unmount()
    await act(async () => { pending.resolve({ ...ACCOUNT_A, connected: false, generation: ACCOUNT_A.generation + 1 }); await pending.promise })

    expect(gateway.list, "面板已卸载，迟到的断开成功不得再读一次列表").toHaveBeenCalledTimes(1)
    expect(await outcome, "断开本身成功；是否刷新由调用方按挂载状态决定").toBe(true)
  })
})
