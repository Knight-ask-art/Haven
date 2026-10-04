// @vitest-environment jsdom
//
// 外部 Agent 接入 Hook 的竞态用例。
//
// 为什么单独一个文件：这个 Hook 的缺陷都不会让"返回了什么"的断言失败——错误码、状态文案
// 骨架都照旧，坏掉的只有**时序**。因此这里用可控的 promise 精确安排"谁后回来"，逐条钉住
// 三条不变量：
// 1. 一次重读不能作废一条在途的开关（结果与 `action` 都不能被它按回去）；
// 2. 开关落地之后，一条更早发出、更晚回来的读取不得覆盖它；
// 3. 读取失败保留上一次真实投影，不把"读不到"降级成"已关闭"。
//
// 页面级用例（`pages/SettingsPage.ai-provider.test.tsx`）用的是同步 Mock 客户端，安排不出
// 这些顺序，所以它们只能住在这里。

import { act, cleanup, renderHook, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const gateway = vi.hoisted(() => ({
  readAgentBrokerStatus: vi.fn(),
  enableAgentBroker: vi.fn(),
  disableAgentBroker: vi.fn(),
}))

vi.mock("../ipc/agent-broker-gateway", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc/agent-broker-gateway")>()
  return { ...actual, ...gateway }
})

import { useAgentBrokerSettings } from "./useAgentBrokerSettings"

/** 与 Mock / Rust 两侧同值的端点形态（这里只用于让"监听中"有一个真实端点）。 */
const PIPE_ENDPOINT = "\\\\.\\pipe\\haven-agent-v1-3f2a91c4"

const DISABLED = { schemaVersion: 1, status: "disabled", endpoint: null, reason: null }
const LISTENING = { schemaVersion: 1, status: "listening", endpoint: PIPE_ENDPOINT, reason: null }

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((complete, fail) => {
    resolve = complete
    reject = fail
  })
  return { promise, resolve, reject }
}

beforeEach(() => {
  for (const method of Object.values(gateway)) method.mockReset()
  gateway.readAgentBrokerStatus.mockResolvedValue(DISABLED)
})

// 这个文件没有开 vitest globals，React Testing Library 的自动清理不会生效；显式卸载，
// 否则上一个用例挂着的 Hook 会活到下一个用例里。
afterEach(() => {
  cleanup()
})

async function mounted() {
  const rendered = renderHook(() => useAgentBrokerSettings())
  await waitFor(() => expect(rendered.result.current.loading).toBe(false))
  return rendered
}

describe("useAgentBrokerSettings 读取与开关的时序", () => {
  it("一次重读不会作废在途的开关，也不会把按钮放回可点", async () => {
    const { result } = await mounted()

    const enable = deferred<unknown>()
    gateway.enableAgentBroker.mockReturnValue(enable.promise)
    let enabling!: Promise<void>
    act(() => {
      enabling = result.current.enable()
    })
    expect(result.current.action).toBe("enabling")
    expect(result.current.busy).toBe(true)

    // 重读先回来：它作废的是比它更早的读取，绝不能动这次开关。
    // （设置页的「重新加载配置」同时驱动读取与该分组的 reload，这就是真实触发路径。）
    await act(async () => {
      await result.current.reload()
    })
    expect(result.current.action, "读取不得把在途的开关按回 idle").toBe("enabling")
    expect(result.current.busy, "命令还没回来，按钮不能重新可点").toBe(true)

    await act(async () => {
      enable.resolve(LISTENING)
      await enabling
    })
    expect(result.current.action).toBe("idle")
    // 开关的结果没有被那次重读吞掉。
    expect(result.current.status).toBe("listening")
    expect(result.current.endpoint).toBe(PIPE_ENDPOINT)
  })

  it("开关落地后，一条更早发出、更晚回来的读取不得覆盖它", async () => {
    const { result } = await mounted()

    const enable = deferred<unknown>()
    gateway.enableAgentBroker.mockReturnValue(enable.promise)
    let enabling!: Promise<void>
    act(() => {
      enabling = result.current.enable()
    })

    // 开关在途期间又发一次读取：它读到的是**变更之前**的快照（disabled）。
    const staleRead = deferred<unknown>()
    gateway.readAgentBrokerStatus.mockReturnValue(staleRead.promise)
    let reloading!: Promise<void>
    act(() => {
      reloading = result.current.reload()
    })

    // 开关先落地：界面必须显示「监听中」。
    await act(async () => {
      enable.resolve(LISTENING)
      await enabling
    })
    expect(result.current.status).toBe("listening")

    // 旧读取的响应**后**到，且带着变更前的快照：它绝不能把刚写下的事实改回去。
    await act(async () => {
      staleRead.resolve(DISABLED)
      await reloading
    })
    expect(
      result.current.status,
      "变更之前发出的读取不得覆盖开关的结果",
    ).toBe("listening")
    expect(result.current.endpoint).toBe(PIPE_ENDPOINT)
  })

  it("读取失败保留上一次真实投影，不把它降级成「已关闭」", async () => {
    const { result } = await mounted()
    gateway.readAgentBrokerStatus.mockResolvedValueOnce(LISTENING)
    await act(async () => {
      await result.current.reload()
    })
    expect(result.current.status).toBe("listening")

    gateway.readAgentBrokerStatus.mockRejectedValueOnce({
      code: "HAVEN_BROKER_UNAVAILABLE",
      userMessage: "读取失败",
      retryable: true,
    })
    await act(async () => {
      await result.current.reload()
    })

    expect(result.current.error?.message).toBe("读取失败")
    // 上一次的真实投影必须留着：把"读不到"显示成"已关闭"是在谎报监听状态。
    expect(result.current.status).toBe("listening")
  })

  it("默认事实来自客户端：读取成功前不预设任何状态", async () => {
    const pending = deferred<unknown>()
    gateway.readAgentBrokerStatus.mockReturnValue(pending.promise)
    const { result } = renderHook(() => useAgentBrokerSettings())

    expect(result.current.status, "还没有读到客户端事实时不得预设 disabled").toBeNull()
    expect(result.current.loading).toBe(true)

    await act(async () => {
      pending.resolve(DISABLED)
      await pending.promise
    })
    await waitFor(() => expect(result.current.status).toBe("disabled"))
  })
})
