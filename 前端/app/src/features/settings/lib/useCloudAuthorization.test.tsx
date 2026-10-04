// @vitest-environment jsdom
//
// 授权 Hook 的时序用例。
//
// 这里要钉的缺陷不会改变"界面上显示了什么"，只会改变**谁先谁后**，所以用可控 promise
// 安排顺序，逐条锁住四次"不得发生"：
// 1. 双击只能发起一次授权尝试；
// 2. begin 迟到前取消 → 注销刚拿到的尝试，且不得开始轮询；
// 3. 面板卸载后迟到的 begin / poll 不得推进状态，更不得完成授权；
// 4. 轮询失败必须注销尝试，不能把它留成下一次授权的前置。

import { act, cleanup, renderHook, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type {
  CloudAccountDto,
  CloudConnectAttemptDto,
  CloudConnectPollDto,
} from "@/lib/ipc/generated/wire"

const gateway = vi.hoisted(() => ({
  begin: vi.fn(),
  poll: vi.fn(),
  complete: vi.fn(),
  cancel: vi.fn(),
}))

// Hook 只依赖这个 IO 边界；不要加载测试不使用的整个生产客户端依赖图。
vi.mock("../ipc/cloud-storage-gateway", () => ({ cloudStorageGateway: gateway }))

import { useCloudAuthorization } from "./useCloudAuthorization"

const ATTEMPT: CloudConnectAttemptDto = {
  attemptId: "3f1c0b2e-1a2b-4c3d-9e4f-0a1b2c3d4e5f",
  expiresAtMs: 1_760_000_000_000,
}
const ACCOUNT: CloudAccountDto = {
  id: "5b7d9c11-2222-4333-8444-555566667777",
  displayName: "只读媒体账户",
  connected: true,
  generation: 1,
}
const AUTHORIZED: CloudConnectPollDto = { status: "authorized", expiresAtMs: ATTEMPT.expiresAtMs }

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((complete) => { resolve = complete })
  return { promise, resolve }
}

beforeEach(() => {
  for (const method of Object.values(gateway)) method.mockReset()
  gateway.cancel.mockResolvedValue("cancelled")
  gateway.complete.mockResolvedValue(ACCOUNT)
})

// 本文件没开 vitest globals，RTL 的自动清理不生效；显式卸载，免得上一个用例的 Hook 活到下一个。
afterEach(() => { cleanup() })

describe("useCloudAuthorization 的授权尝试时序", () => {
  it("双击「打开授权页」只发起一次授权，并且只用一个尝试句柄轮询", async () => {
    const begin = deferred<CloudConnectAttemptDto>()
    gateway.begin.mockReturnValue(begin.promise)
    gateway.poll.mockReturnValue(new Promise(() => {})) // 停在"等待回调"，不排下一轮定时器
    const onAuthorized = vi.fn()
    const { result } = renderHook(() => useCloudAuthorization(undefined, onAuthorized))

    await act(async () => { void result.current.begin(); void result.current.begin() })
    expect(gateway.begin, "第二次点击必须撞上 beginLock").toHaveBeenCalledTimes(1)
    expect(result.current.status).toBe("starting")

    await act(async () => { begin.resolve(ATTEMPT); await begin.promise })
    await waitFor(() => expect(gateway.poll).toHaveBeenCalledTimes(1))
    expect(gateway.poll).toHaveBeenCalledWith(ATTEMPT.attemptId)
    expect(result.current.status).toBe("pending")
    expect(onAuthorized).not.toHaveBeenCalled()
  })

  it("begin 迟到前取消：注销刚拿到的尝试，且不再轮询", async () => {
    const begin = deferred<CloudConnectAttemptDto>()
    gateway.begin.mockReturnValue(begin.promise)
    const onAuthorized = vi.fn()
    const { result } = renderHook(() => useCloudAuthorization(undefined, onAuthorized))

    await act(async () => { void result.current.begin() })
    await act(async () => { await result.current.cancel() })
    expect(result.current.status, "还没有句柄时只能如实报告已取消").toBe("cancelled")

    await act(async () => { begin.resolve(ATTEMPT); await begin.promise })
    await waitFor(() => expect(gateway.cancel).toHaveBeenCalledWith(ATTEMPT.attemptId))
    expect(gateway.poll, "已被取消的尝试不得进入轮询").not.toHaveBeenCalled()
    expect(result.current.status).toBe("cancelled")
    expect(onAuthorized).not.toHaveBeenCalled()
  })

  it("面板在 begin 在途时卸载：迟到的响应被丢弃并注销尝试", async () => {
    const begin = deferred<CloudConnectAttemptDto>()
    gateway.begin.mockReturnValue(begin.promise)
    const onAuthorized = vi.fn()
    const { result, unmount } = renderHook(() => useCloudAuthorization(undefined, onAuthorized))

    await act(async () => { void result.current.begin() })
    unmount()
    await act(async () => { begin.resolve(ATTEMPT); await begin.promise })

    await waitFor(() => expect(gateway.cancel).toHaveBeenCalledTimes(1))
    expect(gateway.cancel).toHaveBeenCalledWith(ATTEMPT.attemptId)
    expect(gateway.poll).not.toHaveBeenCalled()
    expect(onAuthorized).not.toHaveBeenCalled()
  })

  it("面板卸载后迟到的 authorized 轮询不得完成授权", async () => {
    gateway.begin.mockResolvedValue(ATTEMPT)
    const poll = deferred<CloudConnectPollDto>()
    gateway.poll.mockReturnValue(poll.promise)
    const onAuthorized = vi.fn()
    const { result, unmount } = renderHook(() => useCloudAuthorization(undefined, onAuthorized))

    await act(async () => { await result.current.begin() })
    expect(result.current.status).toBe("pending")

    unmount()
    await act(async () => { poll.resolve(AUTHORIZED); await poll.promise })

    expect(gateway.complete, "面板已经不在了，这份授权结果不属于任何人").not.toHaveBeenCalled()
    expect(onAuthorized).not.toHaveBeenCalled()
    await waitFor(() => expect(gateway.cancel).toHaveBeenCalledWith(ATTEMPT.attemptId))
  })

  it("轮询失败：如实报错并注销尝试，不留成下一次授权的前置", async () => {
    gateway.begin.mockResolvedValue(ATTEMPT)
    gateway.poll.mockRejectedValue({ code: "CLOUD_OAUTH_FAILED", userMessage: "授权轮询失败", retryable: true })
    const onAuthorized = vi.fn()
    const { result } = renderHook(() => useCloudAuthorization(undefined, onAuthorized))

    await act(async () => { await result.current.begin() })
    await waitFor(() => expect(result.current.status).toBe("failed"))
    expect(result.current.error?.code).toBe("CLOUD_OAUTH_FAILED")
    expect(result.current.error?.message).toBe("授权轮询失败")
    expect(gateway.cancel).toHaveBeenCalledWith(ATTEMPT.attemptId)
    expect(gateway.complete).not.toHaveBeenCalled()
    expect(onAuthorized).not.toHaveBeenCalled()
  })

  it("authorized 只完成一次，并把账户投影交给调用方", async () => {
    gateway.begin.mockResolvedValue(ATTEMPT)
    gateway.poll.mockResolvedValue(AUTHORIZED)
    const onAuthorized = vi.fn()
    const { result } = renderHook(() => useCloudAuthorization(ACCOUNT.id, onAuthorized))

    await act(async () => { await result.current.begin() })

    await waitFor(() => expect(onAuthorized).toHaveBeenCalledTimes(1))
    expect(gateway.begin).toHaveBeenCalledWith({ accountId: ACCOUNT.id })
    expect(gateway.complete).toHaveBeenCalledTimes(1)
    expect(gateway.complete).toHaveBeenCalledWith(ATTEMPT.attemptId)
    expect(onAuthorized).toHaveBeenCalledWith(ACCOUNT)
    await waitFor(() => expect(result.current.status).toBe("completed"))
    expect(result.current.pending).toBe(false)
    expect(gateway.cancel).not.toHaveBeenCalled()
  })

  it("complete 在途时卸载：迟到的账户投影不得调用 onAuthorized", async () => {
    gateway.begin.mockResolvedValue(ATTEMPT)
    gateway.poll.mockResolvedValue(AUTHORIZED)
    const complete = deferred<CloudAccountDto>()
    gateway.complete.mockReturnValue(complete.promise)
    const onAuthorized = vi.fn()
    const { result, unmount } = renderHook(() => useCloudAuthorization(undefined, onAuthorized))

    await act(async () => { await result.current.begin() })
    await waitFor(() => expect(result.current.status).toBe("completing"))
    expect(gateway.complete).toHaveBeenCalledWith(ATTEMPT.attemptId)

    unmount()
    await act(async () => { complete.resolve(ACCOUNT); await complete.promise })

    expect(onAuthorized, "面板已经不在了，这份账户投影不属于任何人").not.toHaveBeenCalled()
    expect(result.current.status, "不得被迟到结果改写成已完成").not.toBe("completed")
    await waitFor(() => expect(gateway.cancel).toHaveBeenCalledWith(ATTEMPT.attemptId))
  })

  it("complete 在途时取消：迟到的账户投影不得调用 onAuthorized，也不得改写成已完成", async () => {
    gateway.begin.mockResolvedValue(ATTEMPT)
    gateway.poll.mockResolvedValue(AUTHORIZED)
    const complete = deferred<CloudAccountDto>()
    gateway.complete.mockReturnValue(complete.promise)
    const onAuthorized = vi.fn()
    const { result } = renderHook(() => useCloudAuthorization(undefined, onAuthorized))

    await act(async () => { await result.current.begin() })
    await waitFor(() => expect(result.current.status).toBe("completing"))

    await act(async () => { await result.current.cancel() })
    expect(gateway.cancel).toHaveBeenCalledWith(ATTEMPT.attemptId)
    expect(result.current.status).toBe("cancelled")

    await act(async () => { complete.resolve(ACCOUNT); await complete.promise })
    expect(onAuthorized, "取消后迟到的完成结果不得再交付").not.toHaveBeenCalled()
    expect(result.current.status).toBe("cancelled")
  })
})
