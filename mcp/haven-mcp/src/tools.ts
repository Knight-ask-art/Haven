// 9 个 MCP 工具的注册。**冻结清单**，见 docs/architecture/AI_SYSTEM.md §5。
//
// 结构：
// - `TOOL_IMPLEMENTATION` 是"本 Haven 版本能做什么"的**唯一**声明处。它同时决定了
//   错误语义：未实现的能力即使桥接接通也返回 `HAVEN_CAPABILITY_UNAVAILABLE`，
//   而不是假装桥接不可用——两者对调用方意味着完全不同的下一步。
// - 七个只读工具走 `readOnly` 包装，两个提案工具走 `propose` 包装；两者都只调用
//   `HavenAgentBridge`，本文件不读 SQLite、不碰文件系统、不 invoke Tauri。
// - 提案工具**只创建 pending 提案**。审批与执行发生在 Haven 自己的 UI 里，
//   本层没有任何可达路径可以 apply/approve。

import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import type { CallToolResult } from "@modelcontextprotocol/sdk/types.js";
import { z } from "zod";

import { SCHEMA_VERSION, TOOL_NAMES, type ResponseFormat, type ToolName } from "./constants.js";
import { capabilityUnavailable, invalidArgument, toErrorPayload } from "./errors.js";
import { renderRecord, toolFailure, toolSuccess } from "./respond.js";
import type { HavenAgentBridge } from "./bridge.js";
import { withBridgeTimeout } from "./bridge.js";
import {
  emptyInputSchema,
  librarySummaryInputSchema,
  mediaCapabilitiesInputSchema,
  preferenceSnapshotInputSchema,
  resourcePreferenceProposalInputSchema,
  sectionInputSchema,
  settingsProposalInputSchema,
} from "./schemas.js";

// ---- "本版本实现了什么"的唯一声明处 ----

interface ImplementedTool {
  implemented: true;
  /** 桥接操作名，只用于超时错误文案（必须是不含用户数据的固定短语）。 */
  operation: string;
}

interface UnimplementedTool {
  implemented: false;
  /** Haven 能力名（`AgentCapability` 的 snake_case 形式）。 */
  capability: string;
  /** 为什么还没实现——会出现在错误文案里，供调用方判断是否是"稍后重试"。 */
  reason: string;
}

type ToolImplementation = ImplementedTool | UnimplementedTool;

/**
 * 工具 → 实现状态。
 *
 * 当前只有 2 个工具在 Haven 后端有真实用例，与
 * `AgentCapabilityManifest::for_current_slice()`（`settings_read` / `settings_proposal`）
 * 完全一致。其余 6 个的能力在 Application 层不存在，`AgentCapabilityManifest::validate`
 * 甚至**拒绝**声明它们。`test/tools.test.ts` 冻结这个集合。
 */
export const TOOL_IMPLEMENTATION: Record<ToolName, ToolImplementation> = {
  get_system_capabilities: { implemented: true, operation: "读取能力清单" },
  get_settings_snapshot: { implemented: true, operation: "读取设置快照" },
  get_setting_sources: {
    implemented: false,
    capability: "setting_sources_read",
    reason: "Haven 后端尚无面向 Agent 的「设置来源分层」只读用例。",
  },
  get_resource_preference_snapshot: {
    implemented: false,
    capability: "resource_preference_read",
    reason: "Haven 后端尚无面向 Agent 的资源偏好上下文快照（现有快照只覆盖 reading 分区）。",
  },
  get_library_summary: {
    implemented: false,
    capability: "library_summary_read",
    reason: "Haven 尚未实现 library_summary_read 能力；能力清单里该位为 false。",
  },
  get_media_capabilities: {
    implemented: false,
    capability: "media_capabilities_read",
    reason: "Haven 后端尚无面向 Agent 的条目能力声明投影。",
  },
  get_onboarding_state: {
    implemented: false,
    capability: "onboarding_read",
    reason: "Haven 后端尚无面向 Agent 的引导状态只读用例。",
  },
  propose_settings_patch: { implemented: true, operation: "创建设置提案" },
  propose_resource_preference_patch: {
    implemented: false,
    capability: "resource_preference_proposal",
    reason: "Haven 后端尚无面向 Agent 的资源偏好提案用例（提案内核支持该操作类型，但缺少 Agent 作用域入口）。",
  },
};

// ---- 工具元数据（名称、标题、说明、注解）----

interface ToolMetadata {
  title: string;
  description: string;
  readOnly: boolean;
}

const READ_ONLY_ANNOTATIONS = {
  readOnlyHint: true,
  destructiveHint: false,
  idempotentHint: true,
  openWorldHint: false,
} as const;

const PROPOSAL_ANNOTATIONS = {
  readOnlyHint: false,
  destructiveHint: false,
  idempotentHint: false,
  openWorldHint: false,
} as const;

export const TOOL_METADATA: Record<ToolName, ToolMetadata> = {
  get_system_capabilities: {
    readOnly: true,
    title: "获取系统能力",
    description: `读取 MCP server 自身的工具面与 Haven 侧的能力清单。

这是**唯一**总是可用的工具：它不需要 Haven 运行时桥接，因此在其它工具全部不可用时，
调用方仍能知道"现在到底能做什么、为什么不能做"。

Args:
  - response_format ('json' | 'markdown'): 输出格式（默认 'json'）

Returns:
  {
    "schema_version": 1,
    "mcp_server": { "name", "version", "tool_count", "tools": [...] },
    "bridge": { "kind": "unavailable" | "fixture" | "live", "available": bool, "detail": string },
    "haven": { "available": bool, "agent_api_version": number|null, "capabilities": {...}|null, "reason": string|null },
    "tool_implementation": { "<tool_name>": "implemented" | "not_implemented" },
    "forbidden_operations": [...]
  }

Examples:
  - Use when: "我能用栖阅做什么？" -> 先调用本工具，再决定后续调用
  - Don't use when: 你已经知道要读设置 -> 直接调用 get_settings_snapshot

Error Handling:
  - 本工具不返回错误：桥接不可用时 haven.available 为 false，并给出 reason。`,
  },
  get_settings_snapshot: {
    readOnly: true,
    title: "获取设置快照",
    description: `读取全局阅读设置的**脱敏**快照，连同它的身份（context id / hash / revision）。

用途：在提议任何设置改动之前了解当前档位，并把返回的 context_id / context_hash /
base_revision 原样回传给 propose_settings_patch。三者缺一，提案会被 Haven 拒绝（零写入）。

Args:
  - section ('reading'): 设置分区；当前闭合集合只有 'reading'
  - response_format ('json' | 'markdown'): 输出格式（默认 'json'）

Returns:
  {
    "schema_version": 1,
    "subject_scope": "global",
    "section": "reading",
    "context_id": string,          // 回传给提案工具
    "context_hash": string,        // 64 位小写十六进制；回传给提案工具
    "revision": string | null,     // 从未保存过时为 null
    "settings": { ... 排版档位 ... },
    "redacted_fields": string[]    // 服务端已脱敏的字段名；这些字段的值不可信
  }

Examples:
  - Use when: "把字号调大一点" -> 先读快照，再用返回的锚点创建提案
  - Don't use when: 只想看能力清单 -> 用 get_system_capabilities

Error Handling:
  - HAVEN_BRIDGE_UNAVAILABLE: 本构建没有接通 Haven 运行时
  - HAVEN_CAPABILITY_UNAVAILABLE: 当前 Haven 版本未实现 settings_read`,
  },
  get_setting_sources: {
    readOnly: true,
    title: "获取设置来源",
    description: `读取某个设置分区在**各层**（默认值 / 全局 / 版本 / 条目）上的来源与版本号。

用途：解释"这个值为什么是这个值"——它来自默认值，还是用户显式设过，还是被更具体的层覆盖。

Args:
  - section ('reading'): 设置分区
  - response_format ('json' | 'markdown'): 输出格式（默认 'json'）

Returns:
  { "schema_version": 1, "section": "reading", "revision": string|null,
    "layers": [ { "layer": "default"|"global"|"edition"|"media_item", "present": bool, "revision": string|null } ] }

Examples:
  - Use when: "为什么我的字号设置没生效？" -> 看哪一层覆盖了它

Error Handling:
  - HAVEN_CAPABILITY_UNAVAILABLE: 当前 Haven 版本尚未实现该只读用例（本工具的**协议**
    已冻结，但**运行时能力**未接通）。`,
  },
  get_resource_preference_snapshot: {
    readOnly: true,
    title: "获取资源偏好快照",
    description: `读取某个版本或条目上的资源级阅读偏好（文本排版与漫画排版）。

用途：在提议资源级改动之前了解当前值，并取得 proposal 所需的 context 锚点。

Args:
  - target_scope ('edition' | 'media_item'): 作用域
  - edition_id (string): 目标版本 id（UUID）
  - media_item_id (string | null): target_scope='media_item' 时必填
  - response_format ('json' | 'markdown'): 输出格式（默认 'json'）

Returns:
  { "schema_version": 1, "target_scope", "edition_id", "media_item_id": string|null,
    "revision": string|null, "reading": {...}|null, "comic": {...}|null }

Examples:
  - Use when: "这本书的排版单独调一下" -> 先读该条目的偏好快照

Error Handling:
  - HAVEN_CAPABILITY_UNAVAILABLE: 当前 Haven 版本尚未实现该只读用例。`,
  },
  get_library_summary: {
    readOnly: true,
    title: "获取媒体库摘要",
    description: `读取媒体库的聚合计数、分类分布与最近在看的条目。

只返回计数与稳定标识，不返回本地路径、文件名或封面二进制。

Args:
  - limit (number): 返回条数上限（1–50，默认 20）
  - response_format ('json' | 'markdown'): 输出格式（默认 'json'）

Returns:
  { "schema_version": 1,
    "counts": { "works", "editions", "media_items", "favorites", "in_progress" },
    "categories": [ { "category", "work_count" } ],
    "recent": [ { "work_id", "title", "category", "progress_ratio": number|null } ],
    "truncated": bool }

Examples:
  - Use when: "我的库里都有什么？" -> 先看摘要，再决定是否需要更细的读取

Error Handling:
  - HAVEN_CAPABILITY_UNAVAILABLE: Haven 的 library_summary_read 能力尚未实现
    （能力清单里该位为 false，且 Haven 拒绝声明未实现的能力）。`,
  },
  get_media_capabilities: {
    readOnly: true,
    title: "获取条目能力",
    description: `读取媒体条目的**声明式**能力：能否开启阅读会话、能否抽取正文、能否渲染页面。

这是能力声明（declared），不是运行时探测结果；Haven 不会为了回答本问题去打开文件。

Args:
  - media_item_id (string | null): 只查询单个条目时填写；null 表示按 limit 返回一批
  - limit (number): 返回条数上限（1–50，默认 20）
  - response_format ('json' | 'markdown'): 输出格式（默认 'json'）

Returns:
  { "schema_version": 1,
    "items": [ { "media_item_id", "media_type", "availability",
                 "can_open_session", "can_extract_text", "can_render_pages",
                 "declared_capabilities": string[] } ],
    "truncated": bool }

Examples:
  - Use when: "这个条目能不能直接读正文？" -> 查它的 declared_capabilities

Error Handling:
  - HAVEN_CAPABILITY_UNAVAILABLE: 当前 Haven 版本尚未实现该只读用例。`,
  },
  get_onboarding_state: {
    readOnly: true,
    title: "获取引导状态",
    description: `读取本机是否已完成关键配置：是否有媒体库位置、是否有内容、是否配置了 AI Provider。

Args:
  - response_format ('json' | 'markdown'): 输出格式（默认 'json'）

Returns:
  { "schema_version": 1, "completed_steps": string[], "next_step": string|null,
    "has_storage_location": bool, "has_library_content": bool, "has_ai_provider": bool }

Examples:
  - Use when: "我还没设置过栖阅，下一步该做什么？"

Error Handling:
  - HAVEN_CAPABILITY_UNAVAILABLE: 当前 Haven 版本尚未实现该只读用例。`,
  },
  propose_settings_patch: {
    readOnly: false,
    title: "提议全局设置改动",
    description: `为全局设置创建一个**待批准**提案。本工具**不会**修改任何设置。

调用链：本工具 -> Typed Haven Agent Bridge -> Haven 提案内核（canonical JSON + SHA-256 digest）
-> 返回 pending 提案。真正的写入只能由用户在 Haven 界面里逐项审查 digest 后批准，
届时由 Rust 做 CAS 校验并写入回执。

Args:
  - section ('reading'): 设置分区
  - context_id (string): get_settings_snapshot 返回的 context id
  - context_hash (string): get_settings_snapshot 返回的 64 位小写十六进制 hash
  - base_revision (string | null): get_settings_snapshot 返回的 revision
  - patch (object): 要改的档位；至少一个非 null 字段。字段与取值见下：
      font_family: 'sans'|'serif'|'kai'|'heiti'|'fangsong'|'mianfei'|'custom'
      custom_font_family: string
      font_size: 'small'|'medium'|'large'
      line_height: 'compact'|'comfortable'|'airy'
      content_width: 'narrow'|'medium'|'wide'
      theme: 'system'|'paper'|'warm'|'slate'|'dark'|'sepia'|'eyeCare'|'custom'
      custom_background / custom_text: string
      font_weight: 'light'|'regular'|'medium'|'semibold'|'bold'
      letter_spacing: 'tight'|'normal'|'relaxed'|'loose'
      pagination: 'scroll'|'paginated'|'double'
      system_auto: boolean
  - response_format ('json' | 'markdown'): 输出格式（默认 'json'）

Returns:
  { "schema_version": 1, "proposal_id", "status": "pending", "digest",
    "target_label", "subject_scope": "global", "section": "reading",
    "base_revision", "created_at", "expires_at",
    "changes": [ { "key", "before", "after" } ] }

  返回里**没有** approval token、secret、credentialRef、绝对路径或 SQL：
  这些既不在 Haven 的提案投影里，也不会经过桥接端口。

Examples:
  - Use when: "帮我把阅读字号调大" -> 先 get_settings_snapshot 取锚点，再调用本工具，
    然后告诉用户去 Haven 界面批准
  - Don't use when: 你想直接改设置 —— 本工具做不到，这是刻意的

Error Handling:
  - INVALID_ARGUMENT: 输入不合法（字段名拼写、取值、锚点缺失）
  - HAVEN_BRIDGE_UNAVAILABLE: 本构建没有接通 Haven 运行时
  - SETTING_PROPOSAL_*: Haven 提案内核的稳定拒绝码（例如上下文过期、digest 不匹配）`,
  },
  propose_resource_preference_patch: {
    readOnly: false,
    title: "提议资源偏好改动",
    description: `为某个版本或条目创建**待批准**的阅读偏好提案。本工具**不会**修改任何设置。

与 propose_settings_patch 同一条链路，作用域更窄：只影响指定的 edition / media_item。

Args:
  - target_scope ('edition' | 'media_item'): 作用域
  - edition_id (string): 目标版本 id（UUID）
  - media_item_id (string | null): target_scope='media_item' 时必填，否则必须为 null
  - context_id / context_hash / base_revision: 来自最近一次读取工具
  - patch (object): { reading: {...}, comic: {...} } 至少一项；字段同 propose_settings_patch，
    comic 支持 view_mode / direction / page_gap / preload_pages
  - response_format ('json' | 'markdown'): 输出格式（默认 'json'）

Returns:
  { "schema_version": 1, "proposal_id", "status": "pending", "digest",
    "target_label", "subject_scope": "edition"|"media_item",
    "edition_id", "media_item_id", "base_revision", "created_at", "expires_at",
    "changes": [ { "key", "before", "after" } ] }

Examples:
  - Use when: "这本漫画想用双页显示" -> 先读该条目偏好，再提议 comic.view_mode

Error Handling:
  - INVALID_ARGUMENT: 输入不合法
  - HAVEN_CAPABILITY_UNAVAILABLE: Haven 尚无面向 Agent 的资源偏好提案用例
    （提案内核支持该操作类型，但缺少 Agent 作用域入口）。`,
  },
};

// ---- 输出 schema（由 SDK 强制校验，保证 structuredContent 与声明一致）----

const envelope = { schema_version: z.literal(SCHEMA_VERSION) };

const capabilitySetSchema = z.strictObject({
  settings_read: z.boolean(),
  settings_proposal: z.boolean(),
  library_summary_read: z.boolean(),
  metadata_proposal: z.boolean(),
  rename_proposal: z.boolean(),
  secret_read: z.boolean(),
  filesystem_write: z.boolean(),
});

export const systemCapabilitiesOutputSchema = z.strictObject({
  ...envelope,
  mcp_server: z.strictObject({
    name: z.string(),
    version: z.string(),
    tool_count: z.number().int(),
    tools: z.array(z.string()),
  }),
  bridge: z.strictObject({
    kind: z.enum(["unavailable", "fixture", "live"]),
    available: z.boolean(),
    detail: z.string(),
  }),
  haven: z.strictObject({
    available: z.boolean(),
    agent_api_version: z.number().int().nullable(),
    capabilities: capabilitySetSchema.nullable(),
    reason: z.string().nullable(),
  }),
  tool_implementation: z.record(z.string(), z.enum(["implemented", "not_implemented"])),
  forbidden_operations: z.array(z.string()),
});

const changeSchema = z.strictObject({
  key: z.string(),
  before: z.string(),
  after: z.string(),
});

const proposalOutputSchema = z.strictObject({
  ...envelope,
  proposal_id: z.string(),
  status: z.literal("pending"),
  digest: z.string(),
  target_label: z.string(),
  subject_scope: z.enum(["global", "edition", "media_item"]),
  section: z.enum(["reading"]).nullable(),
  edition_id: z.string().nullable(),
  media_item_id: z.string().nullable(),
  base_revision: z.string().nullable(),
  created_at: z.string(),
  expires_at: z.string(),
  changes: z.array(changeSchema),
});

const settingsSnapshotOutputSchema = z.strictObject({
  ...envelope,
  subject_scope: z.literal("global"),
  section: z.literal("reading"),
  context_id: z.string(),
  context_hash: z.string(),
  revision: z.string().nullable(),
  settings: z.record(z.string(), z.unknown()),
  redacted_fields: z.array(z.string()),
});

const settingSourcesOutputSchema = z.strictObject({
  ...envelope,
  section: z.literal("reading"),
  revision: z.string().nullable(),
  layers: z.array(
    z.strictObject({
      layer: z.enum(["default", "global", "edition", "media_item"]),
      present: z.boolean(),
      revision: z.string().nullable(),
    }),
  ),
});

const resourcePreferenceSnapshotOutputSchema = z.strictObject({
  ...envelope,
  target_scope: z.enum(["edition", "media_item"]),
  edition_id: z.string(),
  media_item_id: z.string().nullable(),
  revision: z.string().nullable(),
  reading: z.record(z.string(), z.unknown()).nullable(),
  comic: z.record(z.string(), z.unknown()).nullable(),
});

const librarySummaryOutputSchema = z.strictObject({
  ...envelope,
  counts: z.strictObject({
    works: z.number().int(),
    editions: z.number().int(),
    media_items: z.number().int(),
    favorites: z.number().int(),
    in_progress: z.number().int(),
  }),
  categories: z.array(z.strictObject({ category: z.string(), work_count: z.number().int() })),
  recent: z.array(
    z.strictObject({
      work_id: z.string(),
      title: z.string(),
      category: z.string(),
      progress_ratio: z.number().nullable(),
    }),
  ),
  truncated: z.boolean(),
});

const mediaCapabilitiesOutputSchema = z.strictObject({
  ...envelope,
  items: z.array(
    z.strictObject({
      media_item_id: z.string(),
      media_type: z.string(),
      availability: z.string(),
      can_open_session: z.boolean(),
      can_extract_text: z.boolean(),
      can_render_pages: z.boolean(),
      declared_capabilities: z.array(z.string()),
    }),
  ),
  truncated: z.boolean(),
});

const onboardingOutputSchema = z.strictObject({
  ...envelope,
  completed_steps: z.array(z.string()),
  next_step: z.string().nullable(),
  has_storage_location: z.boolean(),
  has_library_content: z.boolean(),
  has_ai_provider: z.boolean(),
});

// ---- 注册 ----

/** 未实现能力的显式拒绝。即使桥接接通，这些工具也不得编造结果。 */
function refuseUnimplemented(name: ToolName, implementation: UnimplementedTool): never {
  throw capabilityUnavailable(`${implementation.capability}（工具 ${name}：${implementation.reason}）`);
}

/**
 * 注册全部 9 个工具。
 *
 * 注册顺序与 `TOOL_NAMES` 一致；`test/tools.test.ts` 断言注册结果**恰好等于**该集合，
 * 因此这个函数不可能悄悄多注册一个工具。
 */
export function registerHavenTools(server: McpServer, bridge: HavenAgentBridge): void {
  const readOnly = (
    name: ToolName,
    config: { inputSchema: z.ZodType; outputSchema: z.ZodType },
    handler: (args: Record<string, unknown>, bridge: HavenAgentBridge) => Promise<Record<string, unknown>>,
  ): void => {
    const implementation = TOOL_IMPLEMENTATION[name];
    server.registerTool(
      name,
      {
        title: TOOL_METADATA[name].title,
        description: TOOL_METADATA[name].description,
        inputSchema: config.inputSchema,
        outputSchema: config.outputSchema,
        annotations: READ_ONLY_ANNOTATIONS,
      },
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (async (args: any): Promise<CallToolResult> => {
        try {
          if (!implementation.implemented) refuseUnimplemented(name, implementation);
          const structured = await withBridgeTimeout(implementation.operation, () =>
            handler(args, bridge),
          );
          return toolSuccess({
            toolName: name,
            structured,
            format: args.response_format as ResponseFormat,
            render: (value) => renderRecord(TOOL_METADATA[name].title, value),
          });
        } catch (error) {
          return toolFailure(toErrorPayload(normalizeToolError(error)));
        }
      }) as never,
    );
  };

  const propose = (
    name: ToolName,
    config: { inputSchema: z.ZodType; outputSchema: z.ZodType },
    handler: (args: Record<string, unknown>, bridge: HavenAgentBridge) => Promise<Record<string, unknown>>,
  ): void => {
    const implementation = TOOL_IMPLEMENTATION[name];
    server.registerTool(
      name,
      {
        title: TOOL_METADATA[name].title,
        description: TOOL_METADATA[name].description,
        inputSchema: config.inputSchema,
        outputSchema: config.outputSchema,
        annotations: PROPOSAL_ANNOTATIONS,
      },
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (async (args: any): Promise<CallToolResult> => {
        try {
          if (!implementation.implemented) refuseUnimplemented(name, implementation);
          const structured = await withBridgeTimeout(implementation.operation, () =>
            handler(args, bridge),
          );
          return toolSuccess({
            toolName: name,
            structured,
            format: args.response_format as ResponseFormat,
            render: (value) => renderRecord(TOOL_METADATA[name].title, value),
          });
        } catch (error) {
          return toolFailure(toErrorPayload(normalizeToolError(error)));
        }
      }) as never,
    );
  };

  readOnly(
    "get_system_capabilities",
    { inputSchema: emptyInputSchema, outputSchema: systemCapabilitiesOutputSchema },
    async (_args, activeBridge) => buildCapabilities(activeBridge),
  );

  readOnly(
    "get_settings_snapshot",
    { inputSchema: sectionInputSchema, outputSchema: settingsSnapshotOutputSchema },
    async (_args, activeBridge) => {
      const snapshot = await activeBridge.getSettingsSnapshot();
      return {
        schema_version: SCHEMA_VERSION,
        subject_scope: snapshot.subject_scope,
        section: snapshot.section,
        context_id: snapshot.context_id,
        context_hash: snapshot.context_hash,
        revision: snapshot.revision,
        settings: snapshot.settings,
        redacted_fields: snapshot.redacted_fields,
      };
    },
  );

  readOnly(
    "get_setting_sources",
    { inputSchema: sectionInputSchema, outputSchema: settingSourcesOutputSchema },
    async (_args, activeBridge) => {
      const sources = await activeBridge.getSettingSources();
      return { schema_version: SCHEMA_VERSION, ...sources };
    },
  );

  readOnly(
    "get_resource_preference_snapshot",
    { inputSchema: preferenceSnapshotInputSchema, outputSchema: resourcePreferenceSnapshotOutputSchema },
    async (args: Record<string, unknown>, activeBridge) => {
      const input = args as unknown as z.infer<typeof preferenceSnapshotInputSchema>;
      if (input.target_scope === "media_item" && input.media_item_id === null) {
        throw invalidArgument("target_scope='media_item' 时必须提供 media_item_id。");
      }
      const snapshot = await activeBridge.getResourcePreferenceSnapshot({
        target_scope: input.target_scope,
        edition_id: input.edition_id,
        media_item_id: input.media_item_id,
      });
      return { schema_version: SCHEMA_VERSION, ...snapshot };
    },
  );

  readOnly(
    "get_library_summary",
    { inputSchema: librarySummaryInputSchema, outputSchema: librarySummaryOutputSchema },
    async (args: Record<string, unknown>, activeBridge) => {
      const input = args as unknown as z.infer<typeof librarySummaryInputSchema>;
      const summary = await activeBridge.getLibrarySummary({ limit: input.limit });
      return { schema_version: SCHEMA_VERSION, ...summary };
    },
  );

  readOnly(
    "get_media_capabilities",
    { inputSchema: mediaCapabilitiesInputSchema, outputSchema: mediaCapabilitiesOutputSchema },
    async (args: Record<string, unknown>, activeBridge) => {
      const input = args as unknown as z.infer<typeof mediaCapabilitiesInputSchema>;
      const capabilities = await activeBridge.getMediaCapabilities({
        media_item_id: input.media_item_id,
        limit: input.limit,
      });
      return { schema_version: SCHEMA_VERSION, ...capabilities };
    },
  );

  readOnly(
    "get_onboarding_state",
    { inputSchema: emptyInputSchema, outputSchema: onboardingOutputSchema },
    async (_args, activeBridge) => {
      const state = await activeBridge.getOnboardingState();
      return { schema_version: SCHEMA_VERSION, ...state };
    },
  );

  propose(
    "propose_settings_patch",
    { inputSchema: settingsProposalInputSchema, outputSchema: proposalOutputSchema },
    async (args: Record<string, unknown>, activeBridge) => {
      const input = args as unknown as z.infer<typeof settingsProposalInputSchema>;
      const outcome = await activeBridge.proposeSettingsPatch({
        section: input.section,
        context_id: input.context_id,
        context_hash: input.context_hash,
        base_revision: input.base_revision,
        patch: input.patch,
      });
      return { schema_version: SCHEMA_VERSION, ...outcome };
    },
  );

  propose(
    "propose_resource_preference_patch",
    {
      inputSchema: resourcePreferenceProposalInputSchema,
      outputSchema: proposalOutputSchema,
    },
    async (args: Record<string, unknown>, activeBridge) => {
      const input = args as unknown as z.infer<typeof resourcePreferenceProposalInputSchema>;
      const outcome = await activeBridge.proposeResourcePreferencePatch({
        target_scope: input.target_scope,
        edition_id: input.edition_id,
        media_item_id: input.media_item_id,
        context_id: input.context_id,
        context_hash: input.context_hash,
        base_revision: input.base_revision,
        patch: input.patch,
      });
      return { schema_version: SCHEMA_VERSION, ...outcome };
    },
  );
}

/** 本工具永不出错：桥接不可用只体现为 `haven.available = false`。 */
async function buildCapabilities(bridge: HavenAgentBridge): Promise<Record<string, unknown>> {
  let havenAvailable = false;
  let agentApiVersion: number | null = null;
  let capabilities: Record<string, boolean> | null = null;
  let reason: string | null = null;

  try {
    const manifest = await withBridgeTimeout("读取能力清单", () => bridge.getCapabilityManifest());
    havenAvailable = true;
    agentApiVersion = manifest.agent_api_version;
    capabilities = { ...manifest.capabilities };
  } catch (error) {
    // 这里刻意吞掉错误并把状态**如实**报告出来：本工具的用途正是回答
    // "为什么其它工具不能用"，它自己失败会让调用方彻底失去诊断手段。
    reason = toErrorPayload(normalizeToolError(error)).code;
  }

  const toolImplementation: Record<string, "implemented" | "not_implemented"> = {};
  for (const name of TOOL_NAMES) {
    toolImplementation[name] = TOOL_IMPLEMENTATION[name].implemented
      ? "implemented"
      : "not_implemented";
  }

  return {
    schema_version: SCHEMA_VERSION,
    mcp_server: {
      name: "haven-mcp-server",
      version: "0.1.0-beta.1",
      tool_count: TOOL_NAMES.length,
      tools: [...TOOL_NAMES],
    },
    bridge: { kind: bridge.kind, available: bridge.available, detail: bridge.statusDetail },
    haven: {
      available: havenAvailable,
      agent_api_version: agentApiVersion,
      capabilities,
      reason,
    },
    tool_implementation: toolImplementation,
    forbidden_operations: [
      "apply",
      "approve",
      "reject",
      "write",
      "delete",
      "rename",
      "filesystem",
      "shell",
      "sql",
      "secret_read",
      "arbitrary_prompt",
      "arbitrary_command",
    ],
  };
}

/**
 * 把 Zod 校验失败转成**只含字段路径**的稳定错误。
 *
 * 为什么不用 `issue.message`：Zod 的枚举错误消息里会带上"收到的值"，而调用方完全可能
 * 把 API key 误填进某个字段——那样密钥就会出现在错误文本里，并被写进外部模型的上下文。
 * 字段路径同样可能含用户数据（例如一个叫 `sk-...` 的未知键），所以路径也要过脱敏。
 */
function normalizeToolError(error: unknown): unknown {
  if (error instanceof z.ZodError) {
    const paths = [...new Set(error.issues.map((issue) => issue.path.join(".") || "(root)"))]
      .slice(0, 12)
      .join(", ");
    return invalidArgument(
      `输入不合法；请检查这些字段的名称与取值：${paths}。未知字段会被拒绝（schema 是 strict 的）。`,
    );
  }
  return error;
}
