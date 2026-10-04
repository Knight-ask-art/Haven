// 外部 MCP 客户端「一键配置」的最小 IPC 类型消费层。
//
// 单一事实源是后端 Rust 类型，而不是本文件：
//   - 后端/crates/haven-application/src/services/mcp_client_config.rs
//     （`McpClientConfigStatusDto` / `McpClientTargetStatusDto` / `McpClientConfigureRequest`，
//      serde `rename_all = "camelCase"` + `deny_unknown_fields`）
//   - src-tauri/src/commands/mcp_client.rs（命令名与参数形状）
//
// 这些类型**刻意不放进 `generated/wire.ts`**：那是 ts-rs 生成物，只能由
// `cargo run -p haven-application --example gen_wire_bindings` 重写，手改会与
// `wire_bindings_consistency` 冲突。本文件沿用 `agent-skill-wire.ts` 的既有做法——
// 后端契约未进入 ts-rs 生成清单时，由前端手写镜像 + 运行时守卫。
//
// 边界：
// - 类型里**没有**文件内容、没有路径输入。目标只能来自闭合枚举（两个客户端），
//   写入的具体路径由后端按用户授权范围算出，前端连"想写到哪"都表达不了。
// - `configPath` 是后端算出的**将要写入**的位置，只用于显示，不接受前端回传。
// - `state` 是闭合五态。`malformed` / `blocked` 都是"没有写入"，不能被折叠成"未配置"
//   （前者要用户先修文件，后者要用户先满足前提）。

/** 被支持的外部客户端。与 Rust `McpClientTargetDto` 同值。 */
export type McpClientTargetWire = "codex" | "claude_code"

/** 单个客户端的配置状态。与 Rust `McpClientConfigStateDto` 同值。 */
export type McpClientConfigStateWire =
  | "configured"
  | "not_configured"
  | "outdated"
  | "malformed"
  | "blocked"

export type McpClientTargetStatusWire = {
  target: McpClientTargetWire
  label: string
  /** 将要写入的配置文件绝对路径；被环境变量重定向时为 null。仅用于显示。 */
  configPath: string | null
  state: McpClientConfigStateWire
  /** 现在按下按钮会不会真的写。 */
  writable: boolean
  /** 面向用户的安全说明：只有原因，不含文件内容或凭据。 */
  detail: string
}

export type McpClientConfigStatusWire = {
  schemaVersion: 1
  /** 随包分发的 MCP 运行时是否就绪。 */
  runtimeReady: boolean
  runtimeDetail: string
  targets: McpClientTargetStatusWire[]
}

export type McpClientConfigureRequestWire = {
  target: McpClientTargetWire
}

const TARGETS: readonly McpClientTargetWire[] = ["codex", "claude_code"]
const STATES: readonly McpClientConfigStateWire[] = [
  "configured",
  "not_configured",
  "outdated",
  "malformed",
  "blocked",
]
const TARGET_STATUS_FIELDS = [
  "target",
  "label",
  "configPath",
  "state",
  "writable",
  "detail",
] as const
const STATUS_FIELDS = ["schemaVersion", "runtimeReady", "runtimeDetail", "targets"] as const

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

function isText(value: unknown): value is string {
  return typeof value === "string" && value.length > 0
}

function isOptionalText(value: unknown): value is string | null {
  return value === null || isText(value)
}

export function guardMcpClientTargetStatus(
  value: unknown,
): value is McpClientTargetStatusWire {
  if (!isRecord(value) || !hasExactFields(value, TARGET_STATUS_FIELDS)) return false
  return typeof value.target === "string"
    && (TARGETS as readonly string[]).includes(value.target)
    && isText(value.label)
    && isOptionalText(value.configPath)
    && typeof value.state === "string"
    && (STATES as readonly string[]).includes(value.state)
    && typeof value.writable === "boolean"
    && isText(value.detail)
}

/**
 * 状态响应守卫。
 *
 * 额外钉住两条**跨字段**事实，它们都不是"形状"而是"语义"：
 * - `state === "blocked" | "malformed"` 时不得说成可写（否则界面会给出一个必然失败的按钮）；
 * - `configured` 时也不得说成可写（已经一致就没有要写的东西）。
 */
export function guardMcpClientConfigStatus(
  value: unknown,
): value is McpClientConfigStatusWire {
  if (!isRecord(value) || !hasExactFields(value, STATUS_FIELDS)) return false
  if (value.schemaVersion !== 1) return false
  if (typeof value.runtimeReady !== "boolean") return false
  if (typeof value.runtimeDetail !== "string") return false
  if (!Array.isArray(value.targets) || !value.targets.every(guardMcpClientTargetStatus)) {
    return false
  }
  // 两个客户端各一条，且不重复：少一条意味着界面会少显示一个客户端，
  // 而用户看不出少了什么。
  const seen = new Set(value.targets.map((entry) => entry.target))
  if (seen.size !== TARGETS.length || value.targets.length !== TARGETS.length) return false
  return value.targets.every((entry) => {
    if (entry.state === "blocked" || entry.state === "malformed") return !entry.writable
    if (entry.state === "configured") return !entry.writable
    return true
  })
}
