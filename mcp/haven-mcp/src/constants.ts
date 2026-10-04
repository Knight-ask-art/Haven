// 冻结常量：工具清单、上限、错误码。
//
// 契约来源：docs/architecture/AI_SYSTEM.md §5（MCP 边界）。
// 这里的工具清单是**冻结集合**：第一版只有 9 个工具，任何新增都必须先改架构文档。
// `test/tools.test.ts` 断言注册结果与 `TOOL_NAMES` 完全一致，因此"顺手多注册一个
// write 工具"会在测试里立刻失败。

export const SERVER_NAME = "haven-mcp-server";
export const SERVER_VERSION = "0.1.0-beta.1";

/** 所有 MCP 响应载荷共用的 schema 版本（与 Haven Wire 的 schemaVersion 语义一致）。 */
export const SCHEMA_VERSION = 1 as const;

/** 第一版冻结的 9 个工具（顺序即注册顺序，便于测试与文档对齐）。 */
export const TOOL_NAMES = [
  "get_system_capabilities",
  "get_settings_snapshot",
  "get_setting_sources",
  "get_resource_preference_snapshot",
  "get_library_summary",
  "get_media_capabilities",
  "get_onboarding_state",
  "propose_settings_patch",
  "propose_resource_preference_patch",
] as const;

export type ToolName = (typeof TOOL_NAMES)[number];

/** 7 个只读工具：必须证明零写入。 */
export const READ_ONLY_TOOL_NAMES = [
  "get_system_capabilities",
  "get_settings_snapshot",
  "get_setting_sources",
  "get_resource_preference_snapshot",
  "get_library_summary",
  "get_media_capabilities",
  "get_onboarding_state",
] as const satisfies readonly ToolName[];

/** 2 个提案工具：只创建 pending 提案，永不 Apply。 */
export const PROPOSAL_TOOL_NAMES = [
  "propose_settings_patch",
  "propose_resource_preference_patch",
] as const satisfies readonly ToolName[];

/**
 * 禁止出现在**任何**工具名里的词根。
 *
 * 这是"没有自由写入入口"的可执行证明：审批、执行、删除、文件系统、shell、SQL、
 * 凭据读取都不允许有自己的工具。`test/tools.test.ts` 对真实注册结果跑这条规则。
 */
export const FORBIDDEN_TOOL_NAME_FRAGMENTS = [
  "apply",
  "approve",
  "reject",
  "write",
  "delete",
  "remove",
  "rename",
  "move",
  "create_file",
  "filesystem",
  "fs_",
  "_fs",
  "shell",
  "exec",
  "spawn",
  "sql",
  "query_db",
  "secret",
  "credential",
  "invoke",
  "raw",
  "metadata_patch",
  "file_renames",
  "auto_apply",
] as const;

/** 工具名允许的字符：snake_case 小写。 */
export const TOOL_NAME_PATTERN = /^[a-z][a-z0-9_]{2,63}$/;

/** 单个响应的文本上限（字符）。超出即按工具声明的 shrink 规则收缩，仍超出则稳定报错。 */
export const CHARACTER_LIMIT = 24_000;

/** 列表型数据的条数上限与默认条数。 */
export const MAX_LIST_ITEMS = 50;
export const DEFAULT_LIST_ITEMS = 20;

/** 单次提案允许的最大改动键数（防"一次提案改一整段设置"）。 */
export const MAX_PATCH_KEYS = 12;

/** 桥接层返回的字符串字段上限（Provider/存储侧文本一律限长后才进响应）。 */
export const MAX_TEXT_FIELD_CHARS = 512;

/** 桥接调用的超时上界（毫秒）。超时是稳定错误，不是无限等待。 */
export const BRIDGE_TIMEOUT_MS = 15_000;

/** 稳定错误码（本地产生）。桥接层可以回传自己的稳定码，但必须匹配 STABLE_CODE_PATTERN。 */
export const ERROR_CODES = {
  /** 没有可用的 Haven 桥接：本构建尚未接入运行时传输通道。 */
  BRIDGE_UNAVAILABLE: "HAVEN_BRIDGE_UNAVAILABLE",
  /** `HAVEN_MCP_ENDPOINT` 不是当前平台允许的本地端点。 */
  ENDPOINT_INVALID: "HAVEN_MCP_ENDPOINT_INVALID",
  /** 桥接被显式要求，但该传输在当前构建里不存在。 */
  BRIDGE_NOT_IMPLEMENTED: "HAVEN_BRIDGE_NOT_IMPLEMENTED",
  /** 桥接调用超时。 */
  BRIDGE_TIMEOUT: "HAVEN_BRIDGE_TIMEOUT",
  /** 桥接返回了不符合契约的载荷。 */
  BRIDGE_PROTOCOL_ERROR: "HAVEN_BRIDGE_PROTOCOL_ERROR",
  /** 请求的 Haven 能力在本版本未实现（例如 library_summary_read）。 */
  CAPABILITY_UNAVAILABLE: "HAVEN_CAPABILITY_UNAVAILABLE",
  /** 输入非法（本地 Zod 校验或桥接侧校验）。 */
  INVALID_ARGUMENT: "INVALID_ARGUMENT",
  /** 响应在收缩后仍超限。 */
  RESPONSE_TOO_LARGE: "HAVEN_RESPONSE_TOO_LARGE",
  /** 兜底：未知失败。绝不携带原始堆栈或参数。 */
  INTERNAL: "INTERNAL_ERROR",
} as const;

/** 稳定错误码的形状：SCREAMING_SNAKE_CASE，3–64 字符。 */
export const STABLE_CODE_PATTERN = /^[A-Z][A-Z0-9_]{2,63}$/;

/** 响应格式（与 mcp-builder 规范的 response_format 一致）。 */
export const RESPONSE_FORMATS = ["json", "markdown"] as const;
export type ResponseFormat = (typeof RESPONSE_FORMATS)[number];
