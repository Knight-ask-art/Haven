// 取消（`notifications/cancelled`）必须一路传到传输层。
//
// 为什么单独一个文件：`withBridgeTimeout` 已经把取消做成了稳定错误，但**错误只在
// handler 把 `context` 继续往下传时才有意义**。曾经的 9 个 handler 里有 8 个收下
// `context` 之后就没再用过它——MCP 侧回了 `HAVEN_BRIDGE_CANCELLED`，而底层那条连接
// 照旧跑完，提案照旧落库。这类缺陷不会让任何"返回了什么"的断言失败，只能靠直接
// 断言"传输层看到了取消"来钉住。
//
// 两条用例分工：
// - 端口级：逐个工具断言 handler 真的把上下文交给了桥接层；
// - 端到端：走真实 in-memory transport，断言一次真实取消会 abort 掉那个信号。

import { describe, expect, it } from "vitest";
import { ErrorCode, McpError } from "@modelcontextprotocol/sdk/types.js";

import type {
  BridgeCallContext,
  HavenAgentBridge,
  HavenCapabilityReport,
  LibrarySummaryResult,
  MediaCapabilitiesResult,
  OnboardingStateResult,
  ProposalOutcome,
  ResourcePreferenceProposalRequest,
  ResourcePreferenceSnapshotResult,
  SettingSourcesResult,
  SettingsProposalRequest,
  SettingsSnapshotResult,
} from "../src/bridge.js";
import { FIXTURE_IDS, FixtureHavenAgentBridge } from "./support/fixture-bridge.js";
import { connectHarness } from "./support/harness.js";

const UUID_EDITION = "0196f0d2-0000-7000-8000-0000000000e1";
const UUID_MEDIA_ITEM = "0196f0d2-0000-7000-8000-0000000000d1";

/** 工具 → 它应当触达的桥接操作名。 */
const TOOL_OPERATION: ReadonlyArray<readonly [string, string]> = [
  ["get_system_capabilities", "getCapabilityManifest"],
  ["get_settings_snapshot", "getSettingsSnapshot"],
  ["get_setting_sources", "getSettingSources"],
  ["get_resource_preference_snapshot", "getResourcePreferenceSnapshot"],
  ["get_library_summary", "getLibrarySummary"],
  ["get_media_capabilities", "getMediaCapabilities"],
  ["get_onboarding_state", "getOnboardingState"],
  ["propose_settings_patch", "proposeSettingsPatch"],
  ["propose_resource_preference_patch", "proposeResourcePreferencePatch"],
];

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

/** 一个可由测试放行的闸门。 */
function gate(): { promise: Promise<void>; open: () => void } {
  let open!: () => void;
  const promise = new Promise<void>((resolve) => {
    open = resolve;
  });
  return { promise, open };
}

/**
 * 记录每次端口调用收到的 `BridgeCallContext` 的桥接替身。
 *
 * 断言点不是"handler 返回了什么"，而是"传输层有没有拿到那个信号"。
 */
class ContextRecordingBridge implements HavenAgentBridge {
  readonly kind = "fixture" as const;
  readonly available = true;
  readonly statusDetail = "记录调用上下文的测试替身（仅测试）。";

  readonly inner = new FixtureHavenAgentBridge();
  /** 操作名 → 该次调用收到的上下文。 */
  readonly contexts = new Map<string, BridgeCallContext | undefined>();
  /** 进入被挂起的那个操作时 resolve，供测试同步。 */
  entered: Promise<void> = Promise.resolve();
  /** 命中该操作名时挂住不返回；`null` 表示不挂起。 */
  hangOperation: string | null = null;
  hangGate: { promise: Promise<void>; open: () => void } | null = null;
  private enterOperation: (() => void) | null = null;

  private async record<T>(
    operation: string,
    context: BridgeCallContext | undefined,
    run: () => Promise<T>,
  ): Promise<T> {
    this.contexts.set(operation, context);
    if (this.hangOperation === operation) {
      this.hangGate = gate();
      this.enterOperation?.();
      await this.hangGate.promise;
    }
    return await run();
  }

  /** 让下一次进入 `operation` 时可被等待。 */
  armEnterSignal(operation: string): void {
    this.hangOperation = operation;
    this.entered = new Promise<void>((resolve) => {
      this.enterOperation = resolve;
    });
  }

  signalOf(operation: string): AbortSignal | undefined {
    return this.contexts.get(operation)?.signal;
  }

  async getCapabilityManifest(context?: BridgeCallContext): Promise<HavenCapabilityReport> {
    return await this.record("getCapabilityManifest", context, () => this.inner.getCapabilityManifest());
  }

  async getSettingsSnapshot(context?: BridgeCallContext): Promise<SettingsSnapshotResult> {
    return await this.record("getSettingsSnapshot", context, () => this.inner.getSettingsSnapshot());
  }

  async getSettingSources(context?: BridgeCallContext): Promise<SettingSourcesResult> {
    return await this.record("getSettingSources", context, () => this.inner.getSettingSources());
  }

  async getResourcePreferenceSnapshot(
    _request: {
      target_scope: "edition" | "media_item";
      edition_id: string;
      media_item_id: string | null;
    },
    context?: BridgeCallContext,
  ): Promise<ResourcePreferenceSnapshotResult> {
    return await this.record(
      "getResourcePreferenceSnapshot",
      context,
      () => this.inner.getResourcePreferenceSnapshot(),
    );
  }

  async getLibrarySummary(
    request: { limit: number },
    context?: BridgeCallContext,
  ): Promise<LibrarySummaryResult> {
    return await this.record("getLibrarySummary", context, () => this.inner.getLibrarySummary(request));
  }

  async getMediaCapabilities(
    request: { media_item_id: string | null; limit: number },
    context?: BridgeCallContext,
  ): Promise<MediaCapabilitiesResult> {
    return await this.record(
      "getMediaCapabilities",
      context,
      () => this.inner.getMediaCapabilities(request),
    );
  }

  async getOnboardingState(context?: BridgeCallContext): Promise<OnboardingStateResult> {
    return await this.record("getOnboardingState", context, () => this.inner.getOnboardingState());
  }

  async proposeSettingsPatch(
    request: SettingsProposalRequest,
    context?: BridgeCallContext,
  ): Promise<ProposalOutcome> {
    return await this.record(
      "proposeSettingsPatch",
      context,
      () => this.inner.proposeSettingsPatch(request),
    );
  }

  async proposeResourcePreferencePatch(
    request: ResourcePreferenceProposalRequest,
    context?: BridgeCallContext,
  ): Promise<ProposalOutcome> {
    return await this.record(
      "proposeResourcePreferencePatch",
      context,
      () => this.inner.proposeResourcePreferencePatch(request),
    );
  }
}

async function waitUntil(predicate: () => boolean, timeoutMs = 2_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error("等待条件超时");
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}

describe("MCP 取消传播", () => {
  it("9 个工具的 handler 都把取消上下文交给了桥接层", async () => {
    const bridge = new ContextRecordingBridge();
    const harness = await connectHarness(bridge);
    try {
      for (const [tool, operation] of TOOL_OPERATION) {
        const result = await harness.client.callTool({ name: tool, arguments: minimalArgs(tool) });
        expect(result.isError ?? false, `${tool} 不应失败`).toBe(false);
        const context = bridge.contexts.get(operation);
        expect(context, `${tool} 必须把 BridgeCallContext 传给 ${operation}`).toBeDefined();
        // 真正的回归点：上下文里的信号必须存在，而不是 undefined。
        expect(context?.signal, `${tool} 传给 ${operation} 的信号缺失`).toBeInstanceOf(AbortSignal);
      }
    } finally {
      await harness.close();
    }
  });

  it("客户端取消会 abort 掉传输层信号", async () => {
    const bridge = new ContextRecordingBridge();
    bridge.armEnterSignal("getSettingsSnapshot");
    const harness = await connectHarness(bridge);
    try {
      const controller = new AbortController();
      // **拒绝处理器必须在 abort 之前就挂上。** `callTool` 在 abort 的同一个事件循环里
      // 就以取消错误落定，而下面还有一个 await（等信号真正抵达端口）才轮到断言；那段
      // 间隙里这个 promise 是"已拒绝且没有处理器"，Node 会把它记成一次 unhandled
      // rejection，测试于是以一个与取消毫不相干的原因失败。把它折算成返回值，
      // 断言对象也就从"有没有抛出"变成"取消的结果到底是什么"。
      const call = harness.client
        .callTool(
          { name: "get_settings_snapshot", arguments: { section: "reading" } },
          undefined,
          { signal: controller.signal },
        )
        .then(
          () => null,
          (error: unknown) => error,
        );

      await bridge.entered;
      const signal = bridge.signalOf("getSettingsSnapshot");
      expect(signal, "端口调用必须收到信号").toBeInstanceOf(AbortSignal);
      expect(signal?.aborted).toBe(false);

      controller.abort();

      // 取消必须真的抵达端口，而不是只让调用方"不再等结果"。
      await waitUntil(() => signal?.aborted === true);
      expect(signal?.aborted).toBe(true);

      // 客户端一侧同样收尾：SDK 在 abort 时以取消错误结束这次请求，而不是一直挂着。
      // 这里钉的就是"结束了、且是以取消结束的"——只断言抛了个 Error 的话，
      // 任何其它失败（协议错误、超时）都会冒充取消。
      const error = await call;
      expect(error).toBeInstanceOf(McpError);
      expect((error as McpError).code).toBe(ErrorCode.RequestTimeout);
    } finally {
      bridge.hangGate?.open();
      await harness.close();
    }
  });

  it("提案在途时取消同样抵达传输层，且端口没有多创建一条提案", async () => {
    const bridge = new ContextRecordingBridge();
    bridge.armEnterSignal("proposeSettingsPatch");
    const harness = await connectHarness(bridge);
    try {
      const controller = new AbortController();
      // 同前一条用例：处理器先挂上，abort 之后才断言，避免未处理拒绝掩盖真正的失败。
      const call = harness.client
        .callTool(
          { name: "propose_settings_patch", arguments: minimalArgs("propose_settings_patch") },
          undefined,
          { signal: controller.signal },
        )
        .then(
          () => null,
          (error: unknown) => error,
        );

      await bridge.entered;
      const signal = bridge.signalOf("proposeSettingsPatch");
      expect(signal, "提案端口必须收到信号").toBeInstanceOf(AbortSignal);

      controller.abort();
      await waitUntil(() => signal?.aborted === true);
      // 端口被挂住，因此这次取消没有走到"创建提案"那一步。
      expect(bridge.inner.proposals).toHaveLength(0);
      expect(bridge.inner.mutationCount).toBe(0);
      const error = await call;
      expect(error).toBeInstanceOf(McpError);
      expect((error as McpError).code).toBe(ErrorCode.RequestTimeout);
    } finally {
      bridge.hangGate?.open();
      await harness.close();
    }
  });
});
