// MCP 工具契约测试。
//
// 走真实 in-memory transport，因此覆盖的是协议层行为（输入校验、outputSchema 校验、
// 错误传递、工具清单），而不只是"handler 返回了什么"。
//
// 每条断言对应 docs/architecture/AI_SYSTEM.md §5 的一条边界。

import { describe, expect, it } from "vitest";

import {
  FORBIDDEN_TOOL_NAME_FRAGMENTS,
  PROPOSAL_TOOL_NAMES,
  READ_ONLY_TOOL_NAMES,
  RESPONSE_FORMATS,
  TOOL_NAMES,
  TOOL_NAME_PATTERN,
} from "../src/constants.js";
import { UnavailableHavenAgentBridge, type HavenAgentBridge } from "../src/bridge.js";
import { HavenMcpError } from "../src/errors.js";
import { TOOL_IMPLEMENTATION } from "../src/tools.js";
import { containsForbiddenMaterial } from "../src/redact.js";
import {
  FIXTURE_IDS,
  FixtureHavenAgentBridge,
  LeakyHavenAgentBridge,
} from "./support/fixture-bridge.js";
import { connectHarness, errorPayloadOf, textOf, type Harness } from "./support/harness.js";

const UUID_EDITION = "0196f0d2-0000-7000-8000-0000000000e1";
const UUID_MEDIA_ITEM = "0196f0d2-0000-7000-8000-0000000000d1";

/** 每个工具的合法最小入参（用于"能调通"的路径）。 */
function minimalArgs(name: string): Record<string, unknown> {
  switch (name) {
    case "get_settings_snapshot":
    case "get_setting_sources":
      return { section: "reading" };
    case "get_resource_preference_snapshot":
      return { target_scope: "media_item", edition_id: UUID_EDITION, media_item_id: UUID_MEDIA_ITEM };
    case "get_media_capabilities":
      return { media_item_id: null };
    case "propose_settings_patch":
      return {
        section: "reading",
        context_id: FIXTURE_IDS.context,
        context_hash: FIXTURE_IDS.digest,
        base_revision: "rev-1",
        patch: { font_size: "large" },
      };
    case "propose_resource_preference_patch":
      return {
        target_scope: "media_item",
        edition_id: UUID_EDITION,
        media_item_id: UUID_MEDIA_ITEM,
        context_id: FIXTURE_IDS.context,
        context_hash: FIXTURE_IDS.digest,
        base_revision: "rev-1",
        patch: { comic: { view_mode: "double" } },
      };
    default:
      return {};
  }
}

async function withHarness<T>(bridge: HavenAgentBridge, run: (harness: Harness) => Promise<T>): Promise<T> {
  const harness = await connectHarness(bridge);
  try {
    return await run(harness);
  } finally {
    await harness.close();
  }
}

describe("工具清单（冻结）", () => {
  it("恰好注册 9 个工具，且名称与冻结清单完全一致", async () => {
    await withHarness(new FixtureHavenAgentBridge(), async ({ client }) => {
      const { tools } = await client.listTools();
      expect(tools.map((tool) => tool.name)).toEqual([...TOOL_NAMES]);
      expect(tools).toHaveLength(9);
    });
  });

  it("不存在 apply / approve / write / delete 等禁用工具", async () => {
    await withHarness(new FixtureHavenAgentBridge(), async ({ client }) => {
      const { tools } = await client.listTools();
      for (const tool of tools) {
        for (const fragment of FORBIDDEN_TOOL_NAME_FRAGMENTS) {
          expect(
            tool.name.includes(fragment),
            `工具 ${tool.name} 命中禁用词根 ${fragment}`,
          ).toBe(false);
        }
      }
      // 显式点名：这些名字绝不允许出现。
      const names = tools.map((tool) => tool.name);
      for (const forbidden of [
        "apply",
        "approve",
        "reject",
        "write",
        "delete",
        "metadata_patch",
        "file_renames",
        "run_shell",
        "exec_sql",
        "read_secret",
        "invoke_tauri",
        "raw_prompt",
      ]) {
        expect(names).not.toContain(forbidden);
      }
    });
  });

  it("工具名是 snake_case，工具面 = 7 只读 + 2 提案，没有第三类", async () => {
    await withHarness(new FixtureHavenAgentBridge(), async ({ client }) => {
      const { tools } = await client.listTools();
      for (const tool of tools) {
        expect(tool.name).toMatch(TOOL_NAME_PATTERN);
      }
      expect([...READ_ONLY_TOOL_NAMES, ...PROPOSAL_TOOL_NAMES].sort()).toEqual([...TOOL_NAMES].sort());
    });
  });

  it("每个工具都声明了 title / description / inputSchema / outputSchema / 注解", async () => {
    await withHarness(new FixtureHavenAgentBridge(), async ({ client }) => {
      const { tools } = await client.listTools();
      for (const tool of tools) {
        expect(tool.title, tool.name).toBeTruthy();
        expect(tool.description, tool.name).toContain("Args:");
        expect(tool.inputSchema, tool.name).toBeTruthy();
        expect(tool.outputSchema, tool.name).toBeTruthy();
        expect(tool.annotations, tool.name).toBeTruthy();
      }
    });
  });

  it("7 个只读工具标记 readOnlyHint=true，2 个提案工具标记 readOnlyHint=false 且非 destructive", async () => {
    await withHarness(new FixtureHavenAgentBridge(), async ({ client }) => {
      const { tools } = await client.listTools();
      for (const tool of tools) {
        const readOnly = (READ_ONLY_TOOL_NAMES as readonly string[]).includes(tool.name);
        expect(tool.annotations?.readOnlyHint, tool.name).toBe(readOnly);
        expect(tool.annotations?.destructiveHint, tool.name).toBe(false);
      }
    });
  });

  it("inputSchema 对客户端宣告 additionalProperties:false（未知字段会被拒绝）", async () => {
    await withHarness(new FixtureHavenAgentBridge(), async ({ client }) => {
      const { tools } = await client.listTools();
      for (const tool of tools) {
        const schema = tool.inputSchema as { additionalProperties?: unknown };
        expect(schema.additionalProperties, tool.name).toBe(false);
      }
    });
  });

  it("inputSchema 的字段名一律 snake_case（对外协议命名约定）", async () => {
    await withHarness(new FixtureHavenAgentBridge(), async ({ client }) => {
      const { tools } = await client.listTools();
      const offenders: string[] = [];
      const collect = (node: unknown, toolName: string, depth = 0): void => {
        if (depth > 6 || node === null || typeof node !== "object") return;
        const record = node as Record<string, unknown>;
        for (const key of Object.keys(record.properties ?? {})) {
          if (!/^[a-z][a-z0-9_]*$/.test(key)) offenders.push(`${toolName}.${key}`);
        }
        for (const child of Object.values(record.properties ?? {})) {
          collect(child, toolName, depth + 1);
        }
        for (const child of Object.values(record.$defs ?? {})) {
          collect(child, toolName, depth + 1);
        }
      };
      for (const tool of tools) collect(tool.inputSchema, tool.name);
      expect(offenders).toEqual([]);
    });
  });
});

describe("输入校验（strict Zod）", () => {
  it("拒绝未知输入字段", async () => {
    await withHarness(new FixtureHavenAgentBridge(), async ({ client }) => {
      const result = await client.callTool({
        name: "get_settings_snapshot",
        arguments: { section: "reading", apply: true },
      });
      // SDK 把输入校验失败归一成 tool error（isError），不是 JSON-RPC 拒绝。
      // 关键契约是：**绝不**返回成功的结构化结果。
      expect(result.isError).toBe(true);
      expect(result.structuredContent).toBeUndefined();
    });
  });

  it("提案工具拒绝 apply / approve 之类的越权字段，且不产生任何提案", async () => {
    const bridge = new FixtureHavenAgentBridge();
    await withHarness(bridge, async ({ client }) => {
      for (const extra of [{ apply: true }, { approve: true }, { auto_apply: true }, { token: "x" }]) {
        const result = await client.callTool({
          name: "propose_settings_patch",
          arguments: { ...minimalArgs("propose_settings_patch"), ...extra },
        });
        expect(result.isError, JSON.stringify(extra)).toBe(true);
        expect(result.structuredContent).toBeUndefined();
      }
      expect(bridge.mutationCount).toBe(0);
      expect(bridge.proposals).toHaveLength(0);
    });
  });

  it("拒绝非法枚举值与错误类型的字段", async () => {
    await withHarness(new FixtureHavenAgentBridge(), async ({ client }) => {
      const cases = [
        { name: "get_settings_snapshot", arguments: { section: "general" } },
        { name: "get_library_summary", arguments: { limit: 0 } },
        { name: "get_library_summary", arguments: { limit: 999 } },
        {
          name: "propose_settings_patch",
          arguments: { ...minimalArgs("propose_settings_patch"), patch: { font_size: "gigantic" } },
        },
        {
          name: "propose_settings_patch",
          arguments: { ...minimalArgs("propose_settings_patch"), patch: {} },
        },
        {
          name: "propose_settings_patch",
          arguments: { ...minimalArgs("propose_settings_patch"), patch: { font_size: "large", unknown_key: 1 } },
        },
        {
          name: "propose_settings_patch",
          arguments: {
            ...minimalArgs("propose_settings_patch"),
            context_hash: "NOT-A-DIGEST",
          },
        },
        {
          name: "propose_settings_patch",
          arguments: { ...minimalArgs("propose_settings_patch"), section: "reading", context_id: "not-a-uuid" },
        },
      ];
      for (const call of cases) {
        const result = await client.callTool(call);
        expect(result.isError, JSON.stringify(call.arguments)).toBe(true);
      }
    });
  });

  it("target_scope 与 media_item_id 必须一致", async () => {
    await withHarness(new FixtureHavenAgentBridge(), async ({ client }) => {
      const result = await client.callTool({
        name: "propose_resource_preference_patch",
        arguments: {
          ...minimalArgs("propose_resource_preference_patch"),
          target_scope: "edition",
          media_item_id: UUID_MEDIA_ITEM,
        },
      });
      expect(result.isError).toBe(true);
      expect(textOf(result)).toContain("media_item_id 与 target_scope 必须一致");
    });
  });

  it("输入校验错误的文本不回显 Haven 侧的任何东西", async () => {
    await withHarness(new FixtureHavenAgentBridge(), async ({ client }) => {
      const result = await client.callTool({
        name: "get_settings_snapshot",
        arguments: { section: "not-a-section" },
      });
      expect(result.isError).toBe(true);
      // 说明：校验错误由 SDK 生成，它会回显调用方**自己**传入的值。那不是跨方泄漏
      // （同一方的输入回到同一方），因此这里断言的是真正要紧的那一半：
      // 错误文本里不允许出现任何 Haven 侧材料。
      const text = textOf(result);
      expect(text).not.toContain("haven:");
      expect(text).not.toContain("C:\\");
      expect(text).not.toContain("credentialRef");
    });
  });
});

describe("只读工具零写入", () => {
  it("调用全部 7 个只读工具后，桥接的写入计数仍为 0", async () => {
    const bridge = new FixtureHavenAgentBridge();
    await withHarness(bridge, async ({ client }) => {
      for (const name of READ_ONLY_TOOL_NAMES) {
        await client.callTool({ name, arguments: minimalArgs(name) });
      }
      expect(bridge.mutationCount).toBe(0);
      expect(bridge.calls).not.toContain("proposeSettingsPatch");
      expect(bridge.calls).not.toContain("proposeResourcePreferencePatch");
    });
  });

  it("桥接端口自身的只读方法不改变状态", async () => {
    const bridge = new FixtureHavenAgentBridge();
    await bridge.getCapabilityManifest();
    await bridge.getSettingsSnapshot();
    await bridge.getSettingSources();
    await bridge.getResourcePreferenceSnapshot({
      target_scope: "media_item",
      edition_id: UUID_EDITION,
      media_item_id: UUID_MEDIA_ITEM,
    });
    await bridge.getLibrarySummary({ limit: 5 });
    await bridge.getMediaCapabilities({ media_item_id: null, limit: 5 });
    await bridge.getOnboardingState();
    expect(bridge.mutationCount).toBe(0);
  });

  it("只读工具在桥接不可用时返回稳定错误，而不是空数据", async () => {
    await withHarness(new UnavailableHavenAgentBridge(), async ({ client }) => {
      const result = await client.callTool({
        name: "get_settings_snapshot",
        arguments: { section: "reading" },
      });
      expect(result.isError).toBe(true);
      const payload = errorPayloadOf(result);
      expect(payload.code).toBe("HAVEN_BRIDGE_UNAVAILABLE");
      expect(payload.retryable).toBe(false);
      // 绝不能用空快照冒充"读取成功"。
      expect(result.structuredContent).toBeUndefined();
    });
  });
});

describe("提案工具只创建 pending 提案", () => {
  it("propose_settings_patch 返回 pending 提案，且不携带 token / secret / 路径", async () => {
    const bridge = new FixtureHavenAgentBridge();
    await withHarness(bridge, async ({ client }) => {
      const result = await client.callTool({
        name: "propose_settings_patch",
        arguments: minimalArgs("propose_settings_patch"),
      });
      expect(result.isError).toBeFalsy();
      const structured = result.structuredContent as Record<string, unknown>;
      expect(structured.status).toBe("pending");
      expect(structured.digest).toBe(FIXTURE_IDS.digest);
      expect(bridge.mutationCount).toBe(1);
      expect(bridge.proposals).toHaveLength(1);
      expect(bridge.proposals[0]?.status).toBe("pending");

      const text = textOf(result);
      for (const forbidden of ["approval_token", "approvalToken", "secret", "credentialRef", "C:\\", "haven:"]) {
        expect(text).not.toContain(forbidden);
      }
      expect(containsForbiddenMaterial(structured)).toBe(false);
    });
  });

  it("资源偏好提案走真实桥接方法，并且仍只返回 pending", async () => {
    await withHarness(new FixtureHavenAgentBridge(), async ({ client }) => {
      const result = await client.callTool({
        name: "propose_resource_preference_patch",
        arguments: minimalArgs("propose_resource_preference_patch"),
      });
      expect(result.isError).toBeFalsy();
      expect((result.structuredContent as Record<string, unknown>).status).toBe("pending");
    });
  });

  it("Haven 提案内核拒绝时，稳定错误码被透传且不产生提案", async () => {
    // 例如上下文过期 / digest 不匹配：Haven 会 fail-closed（零写入）。
    // MCP 侧必须如实转达这个拒绝，而不是把它变成"提案已创建"。
    const bridge = new FixtureHavenAgentBridge();
    bridge.failure = new HavenMcpError(
      "SETTING_PROPOSAL_CONTEXT_MISMATCH",
      "设置上下文已过期，请重新读取快照后再提议。",
      false,
    );
    await withHarness(bridge, async ({ client }) => {
      const result = await client.callTool({
        name: "propose_settings_patch",
        arguments: minimalArgs("propose_settings_patch"),
      });
      expect(result.isError).toBe(true);
      const payload = errorPayloadOf(result);
      expect(payload.code).toBe("SETTING_PROPOSAL_CONTEXT_MISMATCH");
      expect(payload.retryable).toBe(false);
      expect(bridge.proposals).toHaveLength(0);
    });
  });

  it("提案结果的结构里没有 approval token / secret / 路径的位置", async () => {
    const bridge = new FixtureHavenAgentBridge();
    await withHarness(bridge, async ({ client }) => {
      const result = await client.callTool({
        name: "propose_settings_patch",
        arguments: minimalArgs("propose_settings_patch"),
      });
      const keys = Object.keys(result.structuredContent as Record<string, unknown>);
      expect(keys).toEqual([
        "base_revision",
        "changes",
        "created_at",
        "digest",
        "edition_id",
        "expires_at",
        "media_item_id",
        "proposal_id",
        "schema_version",
        "section",
        "status",
        "subject_scope",
        "target_label",
      ]);
    });
  });

  it("9 个冻结工具均有对应的真实实现声明", () => {
    const implemented = TOOL_NAMES.filter((name) => TOOL_IMPLEMENTATION[name].implemented);
    expect(implemented).toEqual([...TOOL_NAMES]);
    expect(implemented).toHaveLength(9);
  });
});

describe("get_system_capabilities：永远可用且如实", () => {
  it("桥接可用时报告 Haven 能力清单与工具实现状态", async () => {
    await withHarness(new FixtureHavenAgentBridge(), async ({ client }) => {
      const result = await client.callTool({ name: "get_system_capabilities", arguments: {} });
      expect(result.isError).toBeFalsy();
      const structured = result.structuredContent as Record<string, any>;
      expect(structured.schema_version).toBe(1);
      expect(structured.mcp_server.tool_count).toBe(9);
      expect(structured.bridge).toMatchObject({ kind: "fixture", available: true });
      expect(structured.haven.available).toBe(true);
      expect(structured.haven.capabilities.settings_read).toBe(true);
      expect(structured.haven.capabilities.library_summary_read).toBe(true);
      expect(structured.tool_implementation.get_settings_snapshot).toBe("implemented");
      expect(structured.tool_implementation.get_library_summary).toBe("implemented");
      expect(structured.forbidden_operations).toContain("apply");
    });
  });

  it("桥接不可用时依然成功，并给出 reason（否则调用方彻底失去诊断手段）", async () => {
    await withHarness(new UnavailableHavenAgentBridge(), async ({ client }) => {
      const result = await client.callTool({ name: "get_system_capabilities", arguments: {} });
      expect(result.isError).toBeFalsy();
      const structured = result.structuredContent as Record<string, any>;
      expect(structured.bridge).toMatchObject({ kind: "unavailable", available: false });
      expect(structured.haven.available).toBe(false);
      expect(structured.haven.reason).toBe("HAVEN_BRIDGE_UNAVAILABLE");
      expect(structured.haven.capabilities).toBeNull();
    });
  });

  it("已接通的只读能力均返回真实结构化结果", async () => {
    await withHarness(new FixtureHavenAgentBridge(), async ({ client }) => {
      for (const name of ["get_library_summary", "get_media_capabilities", "get_onboarding_state", "get_setting_sources", "get_resource_preference_snapshot"]) {
        const result = await client.callTool({ name, arguments: minimalArgs(name) });
        expect(result.isError, name).toBeFalsy();
        expect(result.structuredContent, name).toBeDefined();
      }
    });
  });
});

describe("脱敏", () => {
  it("桥接返回的凭据 target / 绝对路径 / 密钥 / denylist 键全部被剥离", async () => {
    await withHarness(new LeakyHavenAgentBridge(), async ({ client }) => {
      const result = await client.callTool({
        name: "get_settings_snapshot",
        arguments: { section: "reading" },
      });
      expect(result.isError).toBeFalsy();
      const structured = result.structuredContent as Record<string, unknown>;
      const serialized = JSON.stringify(structured);

      for (const leaked of [
        "sk-leaked-value-1234567890",
        "haven:ai:gw-main",
        "C:\\Users\\someone",
        "Bearer abcdefghijklmnop",
        "SELECT data_json FROM settings",
        "SELECT * FROM setting_proposals",
        '{"fontSize":"large"}',
        '{"data":[{"id":"gpt-4o"}]}',
        "at Object.<anonymous>",
      ]) {
        expect(serialized, `不得泄漏 ${leaked}`).not.toContain(leaked);
        expect(textOf(result)).not.toContain(leaked);
      }
      // denylist 键名整体丢弃（含数据库原始列名与 Provider 原始响应）。
      for (const dropped of [
        "secret",
        "credential_ref",
        "sql",
        "sql_statement",
        "db_row",
        "data_json",
        "payload_json",
        "raw_provider_response",
        "stack_trace",
      ]) {
        expect(Object.keys(structured), `不得保留键 ${dropped}`).not.toContain(dropped);
      }
      expect(containsForbiddenMaterial(structured)).toBe(false);
    });
  });

  it("markdown 文本同样不携带被剥离的材料", async () => {
    await withHarness(new LeakyHavenAgentBridge(), async ({ client }) => {
      const result = await client.callTool({
        name: "get_settings_snapshot",
        arguments: { section: "reading", response_format: "markdown" },
      });
      const text = textOf(result);
      expect(text).not.toContain("sk-");
      expect(text).not.toContain("C:\\Users");
      expect(text).not.toContain("haven:ai:");
    });
  });
});

describe("响应有界", () => {
  it("超大载荷返回稳定的 RESPONSE_TOO_LARGE，而不是截断成不一致的两份", async () => {
    const bridge = new BulkySettingsBridge();
    await withHarness(bridge, async ({ client }) => {
      const result = await client.callTool({
        name: "get_settings_snapshot",
        arguments: { section: "reading" },
      });
      expect(result.isError).toBe(true);
      expect(errorPayloadOf(result).code).toBe("HAVEN_RESPONSE_TOO_LARGE");
      expect(result.structuredContent).toBeUndefined();
    });
  });
});

describe("structuredContent 与文本内容一致", () => {
  it("json 格式下 JSON.parse(text) 与 structuredContent 深度相等", async () => {
    const bridge = new FixtureHavenAgentBridge();
    await withHarness(bridge, async ({ client }) => {
      for (const name of ["get_system_capabilities", "get_settings_snapshot", "propose_settings_patch"]) {
        const result = await client.callTool({
          name,
          arguments: { ...minimalArgs(name), response_format: "json" },
        });
        expect(result.isError, name).toBeFalsy();
        expect(JSON.parse(textOf(result)), name).toEqual(result.structuredContent);
      }
    });
  });

  it("markdown 格式下结构化值里的每个标量都出现在文本中（同一次调用内核对）", async () => {
    const bridge = new FixtureHavenAgentBridge();
    await withHarness(bridge, async ({ client }) => {
      for (const name of ["get_system_capabilities", "get_settings_snapshot", "propose_settings_patch"]) {
        const result = await client.callTool({
          name,
          arguments: { ...minimalArgs(name), response_format: "markdown" },
        });
        expect(result.isError, name).toBeFalsy();
        const text = textOf(result);
        for (const scalar of scalarLeaves(result.structuredContent)) {
          expect(text, `${name} 的 markdown 缺少标量 ${scalar}`).toContain(scalar);
        }
      }
    });
  });

  it("两种格式描述同一份事实（对确定性只读工具交叉核对）", async () => {
    const bridge = new FixtureHavenAgentBridge();
    await withHarness(bridge, async ({ client }) => {
      // 只对**确定性**工具做跨格式核对：提案工具每次调用都会产生新的 proposal id，
      // 跨调用比对会变成 flaky。提案工具的同调用一致性由上一个用例覆盖。
      for (const name of ["get_system_capabilities", "get_settings_snapshot"]) {
        const jsonResult = await client.callTool({
          name,
          arguments: { ...minimalArgs(name), response_format: "json" },
        });
        const markdownResult = await client.callTool({
          name,
          arguments: { ...minimalArgs(name), response_format: "markdown" },
        });
        const markdownText = textOf(markdownResult);
        expect(markdownText, name).not.toBe(textOf(jsonResult));
        for (const scalar of scalarLeaves(jsonResult.structuredContent)) {
          expect(markdownText, `${name} 的 markdown 缺少标量 ${scalar}`).toContain(scalar);
        }
      }
    });
  });

  it("response_format 只在两个合法取值间切换", () => {
    expect([...RESPONSE_FORMATS]).toEqual(["json", "markdown"]);
  });
});

describe("错误结果不携带 structuredContent", () => {
  it("桥接失败时只返回稳定的 JSON 错误载荷", async () => {
    const bridge = new FixtureHavenAgentBridge();
    bridge.failure = new Error("boom: raw failure text must not reach the client");
    await withHarness(bridge, async ({ client }) => {
      const result = await client.callTool({
        name: "get_settings_snapshot",
        arguments: { section: "reading" },
      });
      expect(result.isError).toBe(true);
      expect(result.structuredContent).toBeUndefined();
      // 非 HavenMcpError 的异常一律归一为 INTERNAL_ERROR，不回显原始消息。
      expect(errorPayloadOf(result).code).toBe("INTERNAL_ERROR");
    });
  });

  it("桥接抛出的稳定错误码被原样透传（带 retryable 语义）", async () => {
    const bridge = new FixtureHavenAgentBridge();
    bridge.failure = new HavenMcpError("HAVEN_BRIDGE_TIMEOUT", "桥接超时。", true);
    await withHarness(bridge, async ({ client }) => {
      const result = await client.callTool({
        name: "get_settings_snapshot",
        arguments: { section: "reading" },
      });
      const payload = errorPayloadOf(result);
      expect(payload.code).toBe("HAVEN_BRIDGE_TIMEOUT");
      expect(payload.retryable).toBe(true);
    });
  });
});

/** 收集对象图里的全部标量叶子（字符串化），用于 markdown 一致性核对。 */
function scalarLeaves(value: unknown, into: string[] = []): string[] {
  if (value === null || value === undefined) return into;
  if (typeof value === "string") {
    if (value.length > 0) into.push(value);
    return into;
  }
  if (typeof value === "number" || typeof value === "boolean") {
    into.push(String(value));
    return into;
  }
  if (Array.isArray(value)) {
    for (const item of value) scalarLeaves(item, into);
    return into;
  }
  if (typeof value === "object") {
    for (const child of Object.values(value as Record<string, unknown>)) scalarLeaves(child, into);
  }
  return into;
}

/** 返回一个巨大的 settings 记录，用来触发字符上限。 */
class BulkySettingsBridge extends FixtureHavenAgentBridge {
  override async getSettingsSnapshot() {
    const base = await super.getSettingsSnapshot();
    const settings: Record<string, unknown> = {};
    for (let index = 0; index < 2_000; index += 1) {
      settings[`key_${index}`] = "v".repeat(300);
    }
    return { ...base, settings };
  }
}
