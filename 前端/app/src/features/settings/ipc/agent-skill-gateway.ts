// 内置 Skill Gateway（原生 Skill 运行时）：设置页「内置技能」分组的唯一数据通道。
// 契约：docs/architecture/AI_SYSTEM.md §6。
//
// 职责边界：
// - 只调用 Typed HavenClient（Tauri 环境 invoke / Mock 环境 fixture），组件不得直接 invoke。
// - 运行时形状守卫：响应必须精确匹配后端 DTO；未知键、symbol 键、非枚举键一律拒绝
//   （Reflect.ownKeys 而非 Object.keys）。
// - **技能正文不经过本模块**。网关只能读到状态元数据与逐项启停；正文由 Rust 侧
//   编译期嵌入并只在模型请求内部拼装，前端既读不到也提交不了。
// - `stale` 是独立状态，不能被折叠成「已启用」或「未启用」。

import { getHavenClient } from "@/lib/ipc/runtime";
import { toHavenError } from "@/lib/ipc/errors";
import {
  guardAgentSkillListResult,
  guardAgentSkillState,
  type AgentSkillActivationWire,
  type AgentSkillListResultWire,
  type AgentSkillStateWire,
} from "@/lib/ipc/agent-skill-wire";

export type { AgentSkillActivationWire, AgentSkillStateWire };

/** 空目录的提示文案。技能目录是构建产物，为空意味着这个构建没有内置技能。 */
export const NO_BUILTIN_SKILLS = "当前构建没有内置技能";

const INVALID_RESPONSE = "内置技能接口返回了非法数据";

function invalidResponse(): never {
  throw toHavenError({ code: "INTERNAL_ERROR", userMessage: INVALID_RESPONSE, retryable: false });
}

/** 列出内置技能及其权威启用状态（无内置技能时为空数组，不是错误）。 */
export async function listAgentSkills(): Promise<AgentSkillListResultWire> {
  try {
    const value: unknown = await getHavenClient().agentSkillList();
    if (!guardAgentSkillListResult(value)) invalidResponse();
    return value;
  } catch (error) {
    throw toHavenError(error);
  }
}

/**
 * 启用/停用一项技能（幂等）。
 *
 * 对 `stale` 的技能再次启用就是**对当前内容的重新确认**：后端按当前分发内容
 * 重算摘要，因此这里不需要第二个"确认"动作，也不应该替用户自动确认。
 *
 * 响应必须**属于被写入的那一项**：少了这条比对，一次错配的响应会让另一项技能的开关
 * 状态被本项的结果覆盖（`useAgentSkills` 按 `skillId` 合并单项结果），而界面上完全
 * 看不出哪一项被改错了。
 */
export async function setAgentSkillEnabled(
  skillId: string,
  enabled: boolean,
): Promise<AgentSkillStateWire> {
  try {
    const value: unknown = await getHavenClient().agentSkillSetEnabled({ skillId, enabled });
    if (!guardAgentSkillState(value) || value.skillId !== skillId) invalidResponse();
    return value;
  } catch (error) {
    throw toHavenError(error);
  }
}

/** 状态 → 中文标签。`stale` 与 `disabled` 必须可区分。 */
export function agentSkillStateLabel(state: AgentSkillActivationWire): string {
  switch (state) {
    case "enabled":
      return "已启用";
    case "stale":
      return "内容已更新，需重新确认";
    default:
      return "未启用";
  }
}

/**
 * 该技能当前是否真的会被送进模型请求。
 *
 * 只有 `enabled` 为真；`stale` 是一条已失效的同意，运行时已经把它丢掉了。
 */
export function agentSkillIsEffective(state: AgentSkillActivationWire): boolean {
  return state === "enabled";
}

/**
 * 开关的下一步动作。
 *
 * 对 `enabled` 是"停用"，其余（`disabled` / `stale`）都是"启用"——
 * `stale` 的启用即重新确认。
 */
export function agentSkillToggleTarget(state: AgentSkillActivationWire): boolean {
  return !agentSkillIsEffective(state);
}

/** 开关按钮文案。 */
export function agentSkillActionLabel(state: AgentSkillActivationWire): string {
  return agentSkillToggleTarget(state) ? "启用" : "停用";
}

/** 正文长度的人类可读投影。0 表示该条目没有可注入正文（预览环境等）。 */
export function agentSkillCharsLabel(chars: number): string {
  return chars > 0 ? `注入约 ${chars} 字` : "本次构建没有可注入正文";
}
