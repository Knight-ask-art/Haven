// @vitest-environment jsdom

import { act, renderHook, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { AgentSkillListResultWire, AgentSkillStateWire } from "@/lib/ipc/agent-skill-wire"

const { listAgentSkills, setAgentSkillEnabled } = vi.hoisted(() => ({
  listAgentSkills: vi.fn(),
  setAgentSkillEnabled: vi.fn(),
}))

vi.mock("../ipc/agent-skill-gateway", () => ({ listAgentSkills, setAgentSkillEnabled }))

import { useAgentSkills } from "./useAgentSkills"

const DISABLED: AgentSkillStateWire = {
  schemaVersion: 1,
  skillId: "haven-agent-proposal",
  description: "读取脱敏上下文并创建待批准提案",
  instructionsChars: 5_800,
  state: "disabled",
}

/**
 * 两类失败都写成**真实契约形状**（`ErrorDto`）。
 *
 * 网关抛出的就是它；用一个裸 `Error` 代替会顺带改变断言的含义——裸异常会被
 * `toHavenError` 归一成通用文案，于是"错误文案是否正确"这件事根本没被验证。
 * 裸异常的脱敏由本文件最后一条用例单独负责。
 */
const WRITE_FAILED = {
  code: "AGENT_SKILL_SET_ENABLED_FAILED",
  userMessage: "技能启停写入失败",
  retryable: true,
} as const

const READ_FAILED = {
  code: "AGENT_SKILL_LIST_FAILED",
  userMessage: "内置技能列表读取失败",
  retryable: true,
} as const

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((complete) => { resolve = complete })
  return { promise, resolve }
}

beforeEach(() => {
  listAgentSkills.mockReset()
  setAgentSkillEnabled.mockReset()
  listAgentSkills.mockResolvedValue({ schemaVersion: 1, skills: [DISABLED] } satisfies AgentSkillListResultWire)
})

describe("useAgentSkills 并发读写", () => {
  it("启停完成后使在途旧列表失效，不让它覆盖权威写入结果", async () => {
    const write = deferred<AgentSkillStateWire>()
    const staleRead = deferred<AgentSkillListResultWire>()
    setAgentSkillEnabled.mockReturnValue(write.promise)
    listAgentSkills.mockResolvedValueOnce({ schemaVersion: 1, skills: [DISABLED] })

    const { result } = renderHook(() => useAgentSkills())
    await waitFor(() => expect(result.current.skills?.[0]?.state).toBe("disabled"))

    let togglePromise!: Promise<boolean>
    act(() => { togglePromise = result.current.setEnabled(DISABLED.skillId, true) })

    listAgentSkills.mockReturnValueOnce(staleRead.promise)
    let reloadPromise!: Promise<boolean>
    act(() => { reloadPromise = result.current.reload() })

    const enabled: AgentSkillStateWire = { ...DISABLED, state: "enabled" }
    await act(async () => {
      write.resolve(enabled)
      await togglePromise
    })
    await act(async () => {
      staleRead.resolve({ schemaVersion: 1, skills: [DISABLED] })
      await reloadPromise
    })

    expect(result.current.skills?.[0]?.state).toBe("enabled")
    expect(result.current.loading).toBe(false)
    expect(result.current.pendingSkillId).toBeNull()
  })

  it("拒绝并发启停命令，避免重复写入同一授权", async () => {
    const write = deferred<AgentSkillStateWire>()
    setAgentSkillEnabled.mockReturnValue(write.promise)
    const { result } = renderHook(() => useAgentSkills())
    await waitFor(() => expect(result.current.skills?.length).toBe(1))

    let first!: Promise<boolean>
    let second!: Promise<boolean>
    act(() => {
      first = result.current.setEnabled(DISABLED.skillId, true)
      second = result.current.setEnabled(DISABLED.skillId, false)
    })
    await expect(second).resolves.toBe(false)
    expect(setAgentSkillEnabled).toHaveBeenCalledTimes(1)

    await act(async () => {
      write.resolve({ ...DISABLED, state: "enabled" })
      await first
    })
    expect(result.current.skills?.[0]?.state).toBe("enabled")
  })

  it("写入失败后回读成功仍保留写入错误，并保持权威状态不变", async () => {
    setAgentSkillEnabled.mockRejectedValueOnce(WRITE_FAILED)
    const { result } = renderHook(() => useAgentSkills())
    await waitFor(() => expect(result.current.skills?.[0]?.state).toBe("disabled"))

    await act(async () => {
      await expect(result.current.setEnabled(DISABLED.skillId, true)).resolves.toBe(false)
    })

    expect(listAgentSkills).toHaveBeenCalledTimes(2)
    expect(result.current.skills?.[0]?.state).toBe("disabled")
    expect(result.current.error?.code).toBe(WRITE_FAILED.code)
    expect(result.current.error?.message).toBe(WRITE_FAILED.userMessage)
  })

  it("写入失败之后手动「重新读取」成功，也不会把那次失败洗掉", async () => {
    // 回归点：一次成功的列表读取只证明"现在的状态是什么"，不证明"刚才那次启停为什么
    // 没生效"。若重读成功就清空 error，用户点完「重新读取」会看到界面一切正常，
    // 而他那次没生效的启停再没有任何解释。
    setAgentSkillEnabled.mockRejectedValueOnce(WRITE_FAILED)
    const { result } = renderHook(() => useAgentSkills())
    await waitFor(() => expect(result.current.skills?.[0]?.state).toBe("disabled"))

    await act(async () => {
      await expect(result.current.setEnabled(DISABLED.skillId, true)).resolves.toBe(false)
    })
    expect(result.current.error?.code).toBe(WRITE_FAILED.code)

    // 用户手动重读：这一次读取成功（默认 mock 返回原样列表）。
    await act(async () => {
      await expect(result.current.reload()).resolves.toBe(true)
    })

    expect(result.current.loading).toBe(false)
    expect(result.current.skills?.[0]?.state).toBe("disabled")
    expect(result.current.error?.code).toBe(WRITE_FAILED.code)
    expect(result.current.error?.message).toBe(WRITE_FAILED.userMessage)
  })

  it("下一次写入成功会清掉挂起的写入错误", async () => {
    setAgentSkillEnabled.mockRejectedValueOnce(WRITE_FAILED)
    const { result } = renderHook(() => useAgentSkills())
    await waitFor(() => expect(result.current.skills?.[0]?.state).toBe("disabled"))

    await act(async () => {
      await expect(result.current.setEnabled(DISABLED.skillId, true)).resolves.toBe(false)
    })
    expect(result.current.error?.code).toBe(WRITE_FAILED.code)

    setAgentSkillEnabled.mockResolvedValueOnce({ ...DISABLED, state: "enabled" })
    await act(async () => {
      await expect(result.current.setEnabled(DISABLED.skillId, true)).resolves.toBe(true)
    })

    expect(result.current.error).toBeNull()
    expect(result.current.skills?.[0]?.state).toBe("enabled")
  })

  it("显式关闭错误会连挂起的写入失败一起清掉", async () => {
    setAgentSkillEnabled.mockRejectedValueOnce(WRITE_FAILED)
    const { result } = renderHook(() => useAgentSkills())
    await waitFor(() => expect(result.current.skills?.[0]?.state).toBe("disabled"))

    await act(async () => {
      await expect(result.current.setEnabled(DISABLED.skillId, true)).resolves.toBe(false)
    })
    act(() => result.current.dismissError())
    expect(result.current.error).toBeNull()

    await act(async () => {
      await expect(result.current.reload()).resolves.toBe(true)
    })
    expect(result.current.error).toBeNull()
  })

  it("重新读取失败会丢弃旧快照，避免以过期状态继续操作", async () => {
    const { result } = renderHook(() => useAgentSkills())
    await waitFor(() => expect(result.current.skills?.[0]?.state).toBe("disabled"))
    listAgentSkills.mockRejectedValueOnce(READ_FAILED)

    await act(async () => {
      await expect(result.current.reload()).resolves.toBe(false)
    })

    expect(result.current.skills).toBeNull()
    expect(result.current.error?.code).toBe(READ_FAILED.code)
    expect(result.current.error?.message).toBe(READ_FAILED.userMessage)
  })

  it("写入与回读都失败时报告两项错误且不保留旧状态", async () => {
    setAgentSkillEnabled.mockRejectedValueOnce(WRITE_FAILED)
    const { result } = renderHook(() => useAgentSkills())
    await waitFor(() => expect(result.current.skills?.[0]?.state).toBe("disabled"))
    listAgentSkills.mockRejectedValueOnce(READ_FAILED)

    await act(async () => {
      await expect(result.current.setEnabled(DISABLED.skillId, true)).resolves.toBe(false)
    })

    expect(result.current.skills).toBeNull()
    expect(result.current.error?.message).toContain(WRITE_FAILED.userMessage)
    expect(result.current.error?.message).toContain(READ_FAILED.userMessage)
    expect(result.current.error?.message).toContain("最新技能状态也读取失败")
  })

  it("非契约异常只给出通用文案，不把内部错误文本带进界面", async () => {
    // 后端 IPC 之外的异常（打包错误、第三方库错误）可能带路径或参数。它们不是 ErrorDto，
    // 因此必须被归一为固定文案——这条断言就是"脱敏没有被改成透传"的守卫。
    setAgentSkillEnabled.mockRejectedValueOnce(
      new Error("ENOENT: C:\\Users\\someone\\secret.json"),
    )
    const { result } = renderHook(() => useAgentSkills())
    await waitFor(() => expect(result.current.skills?.[0]?.state).toBe("disabled"))

    await act(async () => {
      await expect(result.current.setEnabled(DISABLED.skillId, true)).resolves.toBe(false)
    })

    expect(result.current.error?.code).toBe("INTERNAL_ERROR")
    expect(result.current.error?.message).toBe("操作失败，请稍后重试")
    expect(result.current.error?.message).not.toContain("secret.json")
  })
})
