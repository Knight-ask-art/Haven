// 内置 Skill wire 守卫与三态投影的测试（原生 Skill 运行时）。
//
// 前半部分只断言纯函数：不读网络、不碰客户端单例、不依赖 React。
// 最后一个 describe 例外：**身份边界**只有走真实的网关函数才成立（守卫本身回答不了
// "这是不是我刚写的那一项"），因此它替换掉 IPC 客户端。
//
// 断言的安全边界：
// - 响应必须精确匹配后端 DTO：未知键（尤其是技能正文）一律拒绝，而不是"忽略"；
// - `stale` 是独立状态：它既不能显示成「已启用」，也不能被折叠成「未启用」；
// - 技能正文**不在 wire 类型里**：任何携带正文的响应都必须被判为非法。

import { beforeEach, describe, expect, it, vi } from "vitest"

import {
  guardAgentSkillListResult,
  guardAgentSkillState,
  type AgentSkillActivationWire,
} from "@/lib/ipc/agent-skill-wire"

// 身份边界用例需要替换 IPC 客户端；其余用例不碰它。
const mocks = vi.hoisted(() => ({ getHavenClient: vi.fn() }))

vi.mock("@/lib/ipc/runtime", () => ({ getHavenClient: mocks.getHavenClient }))

import {
  NO_BUILTIN_SKILLS,
  agentSkillActionLabel,
  agentSkillCharsLabel,
  agentSkillIsEffective,
  agentSkillStateLabel,
  agentSkillToggleTarget,
  setAgentSkillEnabled,
} from "./agent-skill-gateway"

const STATES: AgentSkillActivationWire[] = ["enabled", "disabled", "stale"]

function state(overrides: Record<string, unknown> = {}) {
  return {
    schemaVersion: 1,
    skillId: "haven-agent-proposal",
    description: "读取脱敏上下文并创建待批准提案",
    instructionsChars: 5_800,
    state: "disabled",
    ...overrides,
  }
}

describe("内置 Skill wire 运行时守卫", () => {
  it("接受三态各自自洽的投影", () => {
    for (const value of STATES) {
      expect(guardAgentSkillState(state({ state: value })), value).toBe(true)
    }
    // 0 字是合法事实（本次构建没有可注入正文），不是缺字段。
    expect(guardAgentSkillState(state({ instructionsChars: 0 }))).toBe(true)
  })

  it("拒绝未知键，而不是忽略它们——尤其是技能正文", () => {
    // 正文必须留在 Rust 侧：任何"多带一段正文"的响应都是契约漂移。
    for (const extra of ["instructions", "content", "body", "prompt", "path", "sourceHash"]) {
      expect(guardAgentSkillState(state({ [extra]: "任意正文" })), extra).toBe(false)
    }
    expect(guardAgentSkillState(state({ enabled: true }))).toBe(false)
    expect(guardAgentSkillState(state({ instructionsHash: "a".repeat(64) }))).toBe(false)
  })

  it("拒绝 Object.keys 看不见的 symbol 与非枚举键", () => {
    const withSymbol = state() as Record<string | symbol, unknown>
    withSymbol[Symbol("instructions")] = "隐藏的正文"
    expect(guardAgentSkillState(withSymbol)).toBe(false)

    const hidden = state()
    Object.defineProperty(hidden, "body", { value: "隐藏的正文", enumerable: false })
    expect(guardAgentSkillState(hidden)).toBe(false)
  })

  it("拒绝缺字段、版本不符与闭合集合之外的取值", () => {
    expect(guardAgentSkillState({ ...state(), schemaVersion: 2 })).toBe(false)
    expect(guardAgentSkillState({ ...state(), skillId: "" })).toBe(false)
    expect(guardAgentSkillState({ ...state(), description: "" })).toBe(false)
    // `active` / `pending` 这类"看起来合理"的状态不是本契约的一部分。
    for (const bogus of ["active", "pending", "unknown", "Enabled", true, 1, null]) {
      expect(guardAgentSkillState(state({ state: bogus })), String(bogus)).toBe(false)
    }
    // 字数必须是整数且非负：小数与负数都表示上游算错了。
    for (const bogus of [-1, 1.5, "5800", null, true]) {
      expect(guardAgentSkillState(state({ instructionsChars: bogus })), String(bogus)).toBe(false)
    }
    expect(
      guardAgentSkillState({
        schemaVersion: 1,
        skillId: "haven-agent-proposal",
        instructionsChars: 5_800,
        state: "disabled",
      }),
    ).toBe(false)
    for (const notAnObject of [null, undefined, "enabled", [], 7]) {
      expect(guardAgentSkillState(notAnObject)).toBe(false)
    }
  })

  it("列表守卫逐项校验，并拒绝非数组", () => {
    expect(guardAgentSkillListResult({ schemaVersion: 1, skills: [state()] })).toBe(true)
    // 空目录是合法结果（这个构建没有内置技能），不是错误。
    expect(guardAgentSkillListResult({ schemaVersion: 1, skills: [] })).toBe(true)
    expect(guardAgentSkillListResult({ schemaVersion: 1, skills: [state(), state({ state: "stale" })] })).toBe(true)
    expect(guardAgentSkillListResult({ schemaVersion: 1, skills: [state({ state: "active" })] })).toBe(false)
    expect(guardAgentSkillListResult({ schemaVersion: 1, skills: {} })).toBe(false)
    expect(guardAgentSkillListResult({ schemaVersion: 1, skills: [{ ...state(), instructions: "x" }] })).toBe(false)
    expect(guardAgentSkillListResult({ schemaVersion: 1 })).toBe(false)
    expect(guardAgentSkillListResult({ schemaVersion: 1, skills: [], total: 0 })).toBe(false)
  })
})

describe("内置 Skill 三态投影", () => {
  it("stale 与 disabled 必须给出不同标签", () => {
    expect(agentSkillStateLabel("enabled")).toBe("已启用")
    expect(agentSkillStateLabel("disabled")).toBe("未启用")
    expect(agentSkillStateLabel("stale")).toBe("内容已更新，需重新确认")
    expect(agentSkillStateLabel("stale")).not.toBe(agentSkillStateLabel("disabled"))
    expect(agentSkillStateLabel("stale")).not.toBe(agentSkillStateLabel("enabled"))
  })

  it("只有 enabled 会被送进模型请求", () => {
    expect(agentSkillIsEffective("enabled")).toBe(true)
    expect(agentSkillIsEffective("disabled")).toBe(false)
    // stale 是一条已经失效的同意：运行时已经把它丢掉了。
    expect(agentSkillIsEffective("stale")).toBe(false)
  })

  it("开关动作：enabled 是停用，disabled 与 stale 都是启用（stale 的启用即重新确认）", () => {
    expect(agentSkillToggleTarget("enabled")).toBe(false)
    expect(agentSkillToggleTarget("disabled")).toBe(true)
    expect(agentSkillToggleTarget("stale")).toBe(true)
    expect(agentSkillActionLabel("enabled")).toBe("停用")
    expect(agentSkillActionLabel("disabled")).toBe("启用")
    expect(agentSkillActionLabel("stale")).toBe("启用")
  })

  it("字数为 0 时如实说明本次构建没有可注入正文，不编造一个长度", () => {
    expect(agentSkillCharsLabel(0)).toBe("本次构建没有可注入正文")
    expect(agentSkillCharsLabel(5_800)).toBe("注入约 5800 字")
  })

  it("空目录文案说明的是构建事实，不是「没有技能可用」的营销话术", () => {
    expect(NO_BUILTIN_SKILLS).toContain("内置技能")
  })
})

/**
 * 身份边界：**写回的那一项必须是被写的那一项**。
 *
 * 形状守卫只能回答"这是一份合法的技能状态"，回答不了"这是我刚写的那一项"。少了这条
 * 比对，一次错配的响应会把另一项技能的开关状态覆盖到本项上（`useAgentSkills` 按
 * `skillId` 合并单项结果），而界面上完全看不出哪一项被改错了。
 */
describe("内置 Skill 网关的身份边界", () => {
  const SKILL_ID = "haven-agent-proposal"
  const OTHER_SKILL_ID = "another-skill"

  beforeEach(() => {
    mocks.getHavenClient.mockReset()
  })

  it("写回属于另一项技能的状态时拒绝，而不是当成这一项的结果", async () => {
    mocks.getHavenClient.mockReturnValue({
      agentSkillSetEnabled: vi.fn(async () => state({ skillId: OTHER_SKILL_ID, state: "enabled" })),
    })
    const mismatch = await setAgentSkillEnabled(SKILL_ID, true).then(
      () => null,
      (error: unknown) => error as { code?: string; retryable?: boolean; message?: string },
    )
    expect(mismatch?.code).toBe("INTERNAL_ERROR")
    expect(mismatch?.retryable).toBe(false)
    expect(mismatch?.message ?? "").not.toContain(OTHER_SKILL_ID)

    // 反向证据：同一份载荷在匹配的 skillId 下必须通过，否则上面的拒绝可以靠"一律拒绝"通过。
    mocks.getHavenClient.mockReturnValue({
      agentSkillSetEnabled: vi.fn(async () => state({ state: "enabled" })),
    })
    await expect(setAgentSkillEnabled(SKILL_ID, true)).resolves.toMatchObject({
      skillId: SKILL_ID,
      state: "enabled",
    })
  })
})
