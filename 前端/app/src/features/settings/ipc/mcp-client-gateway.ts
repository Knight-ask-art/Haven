// 外部 MCP 客户端（Codex / Claude Code）自动配置的 Gateway：设置页「外部 Agent 接入」
// 分组的唯一数据通道。
//
// 契约：docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md §9.1.1。
//
// 职责边界：
// - 只调用 Typed HavenClient（Tauri 环境 invoke / Mock 环境 fixture），组件不得直接 invoke。
// - 运行时形状守卫：响应必须精确匹配后端 DTO；未知键、symbol 键、非枚举键一律拒绝
//   （Reflect.ownKeys 而非 Object.keys）。畸形载荷不能变成"未配置、可写"——那会让界面
//   给出一个按钮，而用户按下时我们已经在碰他的配置文件了。
// - **不接受路径、不接受配置片段**。目标只能是闭合枚举里的客户端；写到哪个文件由后端按
//   用户授权范围算出，前端连"想写到哪"都表达不了。
// - 拒绝写入（`blocked` / `malformed`）不是错误而是**状态**：后端如实说明原因，界面照实显示，
//   绝不把"没有写入"渲染成"已配置"。

import { getHavenClient } from "@/lib/ipc/runtime";
import { toHavenError } from "@/lib/ipc/errors";
import {
  guardMcpClientConfigStatus,
  type McpClientConfigStateWire,
  type McpClientConfigStatusWire,
  type McpClientTargetStatusWire,
  type McpClientTargetWire,
} from "@/lib/ipc/mcp-client-wire";

export type { McpClientConfigStateWire, McpClientConfigStatusWire, McpClientTargetStatusWire, McpClientTargetWire };

const INVALID_RESPONSE = "客户端配置接口返回了非法数据";

function invalidResponse(): never {
  throw toHavenError({ code: "INTERNAL_ERROR", userMessage: INVALID_RESPONSE, retryable: false });
}

/** 读取两个客户端的 Haven 配置状态（只读，不写任何文件）。 */
export async function fetchMcpClientConfigStatus(): Promise<McpClientConfigStatusWire> {
  try {
    const value: unknown = await getHavenClient().mcpClientConfigStatus();
    if (!guardMcpClientConfigStatus(value)) invalidResponse();
    return value;
  } catch (error) {
    throw toHavenError(error);
  }
}

/**
 * 把 Haven 这一条写进被点中的客户端配置。
 *
 * 目标参数是闭合枚举而不是路径：这个方法在类型上就无法表达"写到别的地方"。
 */
export async function applyMcpClientConfig(
  target: McpClientTargetWire,
): Promise<McpClientConfigStatusWire> {
  try {
    const value: unknown = await getHavenClient().mcpClientConfigApply({ target });
    if (!guardMcpClientConfigStatus(value)) invalidResponse();
    return value;
  } catch (error) {
    throw toHavenError(error);
  }
}

/** 状态 → 中文标签。五种状态必须互相可区分。 */
export function mcpClientStateLabel(state: McpClientConfigStateWire): string {
  switch (state) {
    case "configured":
      return "已配置"
    case "outdated":
      return "内容已过期"
    case "malformed":
      return "无法安全编辑"
    case "blocked":
      return "当前不可写入"
    default:
      return "未配置"
  }
}

/**
 * 按钮文案。
 *
 * `configured` 时没有要写的东西，因此按钮是"重新写入"（幂等，且用户可能想覆盖被外部改动
 * 过的内容）；其余可写状态都是"写入"。
 */
export function mcpClientActionLabel(state: McpClientConfigStateWire): string {
  return state === "configured" ? "重新写入" : "写入配置"
}

/** 该客户端当前是否允许被写入：只有后端说 `writable` 才算。 */
export function mcpClientCanWrite(entry: McpClientTargetStatusWire): boolean {
  return entry.writable
}

/**
 * 面向用户的结论句。
 *
 * `configured` 必须是"已写入且与当前运行时一致"，不能写成"已生效"——生效与否取决于用户
 * 是否重启客户端并连上 Haven，那件事我们在界面里没有证据。
 */
export function mcpClientStatusNote(entry: McpClientTargetStatusWire): string {
  switch (entry.state) {
    case "configured":
      return "这一条已经指向随包分发的运行时；重启客户端后生效。"
    case "outdated":
      return "已有配置与当前运行时不一致，重新写入只会更新 Haven 这一条。"
    case "malformed":
      return "栖阅不会改写读不懂或结构不符的配置文件，请先手动处理。"
    case "blocked":
      return "当前条件下不会写入任何东西。"
    default:
      return "写入只会新增 Haven 这一条，其它配置保持不变。"
  }
}
