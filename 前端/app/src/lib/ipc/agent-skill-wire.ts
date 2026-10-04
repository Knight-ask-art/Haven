// 内置 Skill 的最小 IPC 类型消费层（原生 Skill 运行时）。
//
// 单一事实源是后端 Rust 类型，而不是本文件：
//   - 后端/crates/haven-application/src/services/agent_skill.rs
//     （`AgentSkillStateDto` / `AgentSkillListResultDto` / `AgentSkillSetEnabledRequest`，
//      serde `rename_all = "camelCase"` + `deny_unknown_fields`）
//   - 后端/crates/haven-domain/src/agent_skill.rs（`enabled` / `disabled` / `stale` 三态）
//   - src-tauri/src/commands/agent_skill.rs（命令名与参数形状）
//
// 这些类型**刻意不放进 `generated/wire.ts`**：那是 ts-rs 生成物，只能由
// `cargo run -p haven-application --example gen_wire_bindings` 重写，手改会与
// `wire_bindings_consistency` 冲突。本文件沿用 `settings-wire.ts` 的既有做法——
// 后端契约未进入 ts-rs 生成清单时，由前端手写镜像 + 运行时守卫。
//
// 边界：
// - 类型里**没有**技能正文。正文随二进制分发，只在 Rust 侧拼进模型请求；
//   前端既读不到它，也不能提交它。
// - `state` 是闭合三态。`stale` 表示"启用过，但分发内容变了"——不生效，
//   需要用户重新确认。它不能被显示成"已启用"，也不能被折叠成"未启用"。

/** 单项技能对用户的可见状态。与 Rust `AgentSkillActivationDto` 同值。 */
export type AgentSkillActivationWire = "enabled" | "disabled" | "stale"

export type AgentSkillStateWire = {
  schemaVersion: 1
  skillId: string
  description: string
  instructionsChars: number
  state: AgentSkillActivationWire
}

export type AgentSkillListResultWire = {
  schemaVersion: 1
  skills: AgentSkillStateWire[]
}

export type AgentSkillSetEnabledRequestWire = {
  skillId: string
  enabled: boolean
}

const ACTIVATION_STATES: readonly AgentSkillActivationWire[] = ["enabled", "disabled", "stale"]
const SKILL_STATE_FIELDS = [
  "schemaVersion",
  "skillId",
  "description",
  "instructionsChars",
  "state",
] as const
const SKILL_LIST_FIELDS = ["schemaVersion", "skills"] as const

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

/** 精确字段守卫：未知键、symbol 键、非枚举键一律拒绝（Reflect.ownKeys，不是 Object.keys）。 */
function hasExactFields(value: Record<string, unknown>, allowed: readonly string[]): boolean {
  const permitted = new Set<string>(allowed)
  for (const field of Reflect.ownKeys(value)) {
    if (typeof field !== "string") return false
    if (!permitted.has(field)) return false
  }
  for (const field of allowed) {
    if (!Object.prototype.hasOwnProperty.call(value, field)) return false
  }
  return true
}

export function guardAgentSkillState(value: unknown): value is AgentSkillStateWire {
  if (!isRecord(value) || !hasExactFields(value, SKILL_STATE_FIELDS)) return false
  return value.schemaVersion === 1
    && typeof value.skillId === "string"
    && value.skillId.length > 0
    && typeof value.description === "string"
    && value.description.length > 0
    && typeof value.instructionsChars === "number"
    && Number.isInteger(value.instructionsChars)
    && value.instructionsChars >= 0
    && typeof value.state === "string"
    && (ACTIVATION_STATES as readonly string[]).includes(value.state)
}

export function guardAgentSkillListResult(value: unknown): value is AgentSkillListResultWire {
  if (!isRecord(value) || !hasExactFields(value, SKILL_LIST_FIELDS)) return false
  return value.schemaVersion === 1
    && Array.isArray(value.skills)
    && value.skills.every(guardAgentSkillState)
}
