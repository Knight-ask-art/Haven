// 外部 Agent 接入 Gateway（A5 接线切片）：设置页「外部 Agent 接入」分组的唯一数据通道。
// 契约：docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md §2、§4.2、§4.5、§9。
//
// 职责边界：
// - 只调用 Typed HavenClient 的 agentBrokerStatus / agentBrokerEnable / agentBrokerDisable；
//   组件不得直接 invoke，也不得自己拼端点。
// - 运行时形状守卫：字段必须**正好**是 schemaVersion / status / endpoint / reason
//   （Reflect.ownKeys，symbol 与非枚举键同样被拒），且状态与端点、理由三者互相自洽。
// - **只有真实监听端点**才允许被复制或写进模板。浏览器 Mock 返回的 `mock://…` 既不是
//   Named Pipe 也不是 Unix socket，必须与真实端点区分：它是预览标识，不是可以交给
//   Codex / Claude Code 的地址。
// - 这里没有 approve / apply / reject：Broker 的请求集只有 context / create_proposal，
//   提案的批准与写入只能发生在栖阅界面。

import { getHavenClient } from "@/lib/ipc/runtime";
import { toHavenError } from "@/lib/ipc/errors";
import type {
  AgentBrokerStatusDto,
  AgentBrokerStatusResultDto,
} from "@/lib/ipc/generated/wire";

const BROKER_STATUSES = new Set<string>(["disabled", "listening", "busy", "unavailable"]);

const STATUS_RESULT_FIELDS = ["schemaVersion", "status", "endpoint", "reason"] as const;

/** Windows Named Pipe 前缀（与 Rust `agent_broker::endpoint::PIPE_PREFIX` 同值）。 */
const PIPE_PREFIX = "\\\\.\\pipe\\haven-agent-v1-";
/** Unix socket 固定文件名（与 Rust `agent_broker::endpoint::SOCKET_FILE_NAME` 同值）。 */
const SOCKET_SUFFIX = "/haven/agent-v1.sock";
/** 浏览器 Mock 的端点是演示标识，永远不是可连接的本地端点。 */
const BROWSER_PREVIEW_SCHEME = "mock:";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasExactFields(value: Record<string, unknown>, allowed: readonly string[]): boolean {
  const permitted = new Set<string>(allowed);
  for (const field of Reflect.ownKeys(value)) {
    if (typeof field !== "string") return false;
    if (!permitted.has(field)) return false;
  }
  for (const field of allowed) {
    if (!Object.prototype.hasOwnProperty.call(value, field)) return false;
  }
  return true;
}

/** 可选文本字段：null 或非空字符串。空字符串不是「没有值」，一律拒绝。 */
function isOptionalText(value: unknown): value is string | null {
  return value === null || (typeof value === "string" && value.length > 0);
}

/**
 * `AgentBrokerStatusResultDto` 的严格守卫。
 *
 * 除了字段集合与类型，还要求三件事实互相印证——否则投影本身就是自相矛盾的：
 * - `listening` 必须带非空端点，其余状态必须不带端点；
 * - `reason` 只允许出现在 `unavailable`；
 * - `busy` / `unavailable` 是 fail-closed 结论，不允许被写成 `disabled`。
 */
export function guardAgentBrokerStatusResult(
  value: unknown,
): value is AgentBrokerStatusResultDto {
  if (!isRecord(value)) return false;
  if (!hasExactFields(value, STATUS_RESULT_FIELDS)) return false;
  if (value.schemaVersion !== 1) return false;
  if (typeof value.status !== "string" || !BROKER_STATUSES.has(value.status)) return false;
  if (!isOptionalText(value.endpoint) || !isOptionalText(value.reason)) return false;
  if (value.status !== "unavailable" && value.reason !== null) return false;
  if (value.status === "listening") return value.endpoint !== null;
  return value.endpoint === null;
}

/** 浏览器 Mock 的演示端点：只能标记为预览，不能复制、不能进模板。 */
export function isBrowserPreviewAgentBrokerEndpoint(endpoint: string | null): boolean {
  return endpoint !== null && endpoint.toLowerCase().startsWith(BROWSER_PREVIEW_SCHEME);
}

/**
 * 是否为真实监听端点。
 *
 * Windows 形态按契约 §4.3 精确匹配（前缀 + 恰好 8 位小写十六进制）。Unix 侧这里是
 * **形态检查**（绝对路径 + 固定父目录名与文件名）：界面拿不到 `$XDG_RUNTIME_DIR` /
 * `$HOME`，因此闭合集合的权威判定仍在 Rust，本函数只负责回答"这个值像不像真端点"。
 *
 * 形态判定刻意从严：`mock://…`、`https://…`、相对路径、以及任何不符合上述形态的
 * 字符串都不是可复制端点。宁可少给一个复制按钮，也不要让用户把一段连不上的地址
 * 粘进 Agent 客户端。
 */
export function isRealAgentBrokerEndpoint(endpoint: string | null): boolean {
  if (endpoint === null || endpoint.length === 0) return false;
  if (isBrowserPreviewAgentBrokerEndpoint(endpoint)) return false;
  if (endpoint.startsWith(PIPE_PREFIX)) {
    return /^[0-9a-f]{8}$/.test(endpoint.slice(PIPE_PREFIX.length));
  }
  return endpoint.length > SOCKET_SUFFIX.length
    && endpoint.startsWith("/")
    && endpoint.endsWith(SOCKET_SUFFIX);
}

export type AgentBrokerEndpointKind = "none" | "preview" | "live" | "unknown";

/**
 * 端点三态之外还要留一个 `unknown`：`listening` 却带着无法识别的端点形态属于契约
 * 违约，此时既不能复制（可能是假地址），也不该假装成浏览器预览。
 */
export function agentBrokerEndpointKind(
  status: AgentBrokerStatusDto | null,
  endpoint: string | null,
): AgentBrokerEndpointKind {
  if (status !== "listening" || endpoint === null) return "none";
  if (isBrowserPreviewAgentBrokerEndpoint(endpoint)) return "preview";
  return isRealAgentBrokerEndpoint(endpoint) ? "live" : "unknown";
}

const INVALID_RESPONSE = "外部 Agent 接入接口返回了非法数据";

function invalidResponse(): never {
  throw toHavenError({ code: "INTERNAL_ERROR", userMessage: INVALID_RESPONSE, retryable: false });
}

type AgentBrokerAction = "status" | "enable" | "disable";

/** 三个命令共用一条路径：客户端调用 → 严格守卫 → 归一错误。 */
async function callAgentBroker(action: AgentBrokerAction): Promise<AgentBrokerStatusResultDto> {
  try {
    const client = getHavenClient();
    const value: unknown = action === "status"
      ? await client.agentBrokerStatus()
      : action === "enable"
        ? await client.agentBrokerEnable()
        : await client.agentBrokerDisable();
    if (!guardAgentBrokerStatusResult(value)) invalidResponse();
    return value;
  } catch (error) {
    throw toHavenError(error);
  }
}

/** 读取当前状态。**不**开启、不探测端点；默认事实只能来自客户端。 */
export function readAgentBrokerStatus(): Promise<AgentBrokerStatusResultDto> {
  return callAgentBroker("status");
}

/** 用户显式开启外部 Agent 接入（幂等）。 */
export function enableAgentBroker(): Promise<AgentBrokerStatusResultDto> {
  return callAgentBroker("enable");
}

/** 用户显式关闭外部 Agent 接入（幂等）。 */
export function disableAgentBroker(): Promise<AgentBrokerStatusResultDto> {
  return callAgentBroker("disable");
}

export type AgentBrokerClientTemplateId = "codex" | "claude-code" | "dsh" | "pi";

export interface AgentBrokerClientTemplate {
  id: AgentBrokerClientTemplateId;
  label: string;
  /** 放去哪里的说明；四个客户端的 JSON 本身完全相同（契约 §2、§9）。 */
  hint: string;
}

/**
 * 四个客户端的选择项。
 *
 * JSON 内容刻意**不分叉**：契约 §2 钉死了"四个客户端共用同一份配置模板"，差异只在
 * 配置写在哪、字段叫什么，而那属于各客户端自己的文档，不在本仓库的契约里。
 */
export const AGENT_BROKER_CLIENT_TEMPLATES: readonly AgentBrokerClientTemplate[] = [
  { id: "codex", label: "Codex", hint: "放进 Codex 的 MCP 服务器配置（具体位置见 Codex 自己的文档）。" },
  { id: "claude-code", label: "Claude Code", hint: "放进 Claude Code 的 MCP 服务器配置（具体位置见 Claude Code 自己的文档）。" },
  { id: "dsh", label: "DSH", hint: "放进 DSH 的 MCP 服务器配置（具体位置见 DSH 自己的文档）。" },
  { id: "pi", label: "Pi", hint: "放进 Pi 的 MCP 服务器配置（具体位置见 Pi 自己的文档）。" },
];

/** 模板旁的固定说明：它只是连接配置，不构成任何写入授权。 */
export const AGENT_BROKER_TEMPLATE_NOTE =
  "这只是 MCP 连接配置，不授予批准或应用权限（approve / apply）：提案仍必须回到栖阅界面由你审批。";

export const AGENT_BROKER_TEMPLATE_PATH = "<path-to-haven>/mcp/haven-mcp/dist/index.js";

/**
 * 生成 stdio 启动配置。
 *
 * 端点由调用方动态注入，并且**必须**经 `JSON.stringify` 转义：Windows 的
 * `\\.\pipe\…` 在 JSON 里要写成 `\\\\.\\pipe\\…`，手写拼接会产出无法解析的配置。
 * 这里不写任何模型名：模板只描述怎么把 MCP server 拉起来。
 */
export function buildAgentBrokerMcpTemplate(endpoint: string): string {
  return JSON.stringify(
    {
      command: "node",
      args: [AGENT_BROKER_TEMPLATE_PATH],
      env: {
        HAVEN_MCP_BRIDGE: "live",
        HAVEN_MCP_ENDPOINT: endpoint,
      },
    },
    null,
    2,
  );
}
