// 桥接选择测试：默认不可用、测试替身不可作生产默认、未实现传输显式拒绝。

import { describe, expect, it } from "vitest";

import {
  ALLOW_TEST_BRIDGE_ENV_VAR,
  BRIDGE_ENV_VAR,
  selectBridge,
} from "../src/bridge-selection.js";
import { UnavailableHavenAgentBridge } from "../src/bridge.js";
import { HAVEN_ENDPOINT_ENV_VAR, LiveHavenAgentBridge } from "../src/local-broker.js";
import { FixtureHavenAgentBridge } from "./support/fixture-bridge.js";

describe("selectBridge", () => {
  it("默认（未设环境变量）返回显式不可用桥接", () => {
    const selection = selectBridge({});
    expect(selection.ok).toBe(true);
    if (!selection.ok) return;
    expect(selection.bridge).toBeInstanceOf(UnavailableHavenAgentBridge);
    expect(selection.bridge.kind).toBe("unavailable");
    expect(selection.bridge.available).toBe(false);
  });

  it("显式 unavailable / none 同样返回不可用桥接", () => {
    for (const value of ["unavailable", "none", "  UNAVAILABLE  "]) {
      const selection = selectBridge({ [BRIDGE_ENV_VAR]: value });
      expect(selection.ok, value).toBe(true);
      if (selection.ok) expect(selection.bridge.kind).toBe("unavailable");
    }
  });

  it("要求 live 但没有端点时保持诊断可用，并明确报告不可用", () => {
    const selection = selectBridge({ [BRIDGE_ENV_VAR]: "live" });
    expect(selection.ok).toBe(true);
    if (!selection.ok) return;
    expect(selection.bridge).toBeInstanceOf(UnavailableHavenAgentBridge);
    expect(selection.bridge.statusDetail).toContain(HAVEN_ENDPOINT_ENV_VAR);
  });

  it("live 的非法端点不尝试连接，而是由桥接返回 endpoint invalid", async () => {
    const selection = selectBridge({ [BRIDGE_ENV_VAR]: "live", [HAVEN_ENDPOINT_ENV_VAR]: "tcp://127.0.0.1:1" });
    expect(selection.ok).toBe(true);
    if (!selection.ok) return;
    expect(selection.bridge).toBeInstanceOf(LiveHavenAgentBridge);
    await expect(selection.bridge.getCapabilityManifest()).rejects.toMatchObject({
      code: "HAVEN_MCP_ENDPOINT_INVALID",
      retryable: false,
    });
  });

  it("live 的 Windows 合法端点选择真实桥接", () => {
    const selection = selectBridge(
      {
        [BRIDGE_ENV_VAR]: "live",
        [HAVEN_ENDPOINT_ENV_VAR]: "\\\\.\\pipe\\haven-agent-v1-0123abcd",
      },
      { platform: "win32" },
    );
    expect(selection.ok).toBe(true);
    if (selection.ok) {
      expect(selection.bridge).toBeInstanceOf(LiveHavenAgentBridge);
      expect(selection.bridge.kind).toBe("live");
      expect(selection.bridge.available).toBe(true);
    }
  });

  it("fixture 在没有允许开关时被拒绝", () => {
    const selection = selectBridge({ [BRIDGE_ENV_VAR]: "fixture" });
    expect(selection.ok).toBe(false);
    if (selection.ok) return;
    expect(selection.reason).toContain(ALLOW_TEST_BRIDGE_ENV_VAR);
  });

  it("fixture 即使开关打开，没有注入工厂也仍被拒绝", () => {
    // 这是"测试 fake 不能成为生产默认"的核心：生产入口不注入工厂，
    // 因此环境变量本身永远不足以启用一个假桥接。
    const selection = selectBridge({
      [BRIDGE_ENV_VAR]: "fixture",
      [ALLOW_TEST_BRIDGE_ENV_VAR]: "1",
    });
    expect(selection.ok).toBe(false);
    if (selection.ok) return;
    expect(selection.reason).toContain("没有注入 fixture 工厂");
  });

  it("fixture 只有在开关打开且注入了工厂时才成立（测试路径）", () => {
    const selection = selectBridge(
      { [BRIDGE_ENV_VAR]: "fixture", [ALLOW_TEST_BRIDGE_ENV_VAR]: "1" },
      { fixtureFactory: () => new FixtureHavenAgentBridge() },
    );
    expect(selection.ok).toBe(true);
    if (!selection.ok) return;
    expect(selection.bridge.kind).toBe("fixture");
  });

  it("未知取值被拒绝并列出允许值", () => {
    const selection = selectBridge({ [BRIDGE_ENV_VAR]: "production" });
    expect(selection.ok).toBe(false);
    if (selection.ok) return;
    expect(selection.reason).toContain("unavailable");
    expect(selection.reason).toContain("fixture");
    expect(selection.reason).toContain("live");
  });
});

describe("UnavailableHavenAgentBridge", () => {
  it("每个端口方法都返回稳定的 HAVEN_BRIDGE_UNAVAILABLE", async () => {
    const bridge = new UnavailableHavenAgentBridge();
    const calls: (() => Promise<unknown>)[] = [
      () => bridge.getCapabilityManifest(),
      () => bridge.getSettingsSnapshot(),
      () => bridge.getSettingSources(),
      () => bridge.getResourcePreferenceSnapshot({ target_scope: "edition", edition_id: "x", media_item_id: null }),
      () => bridge.getLibrarySummary({ limit: 1 }),
      () => bridge.getMediaCapabilities({ media_item_id: null, limit: 1 }),
      () => bridge.getOnboardingState(),
      () =>
        bridge.proposeSettingsPatch({
          section: "reading",
          context_id: "x",
          context_hash: "y",
          base_revision: null,
          patch: {},
        }),
      () =>
        bridge.proposeResourcePreferencePatch({
          target_scope: "edition",
          edition_id: "x",
          media_item_id: null,
          context_id: "x",
          context_hash: "y",
          base_revision: null,
          patch: {},
        }),
    ];

    for (const call of calls) {
      await expect(call()).rejects.toMatchObject({
        code: "HAVEN_BRIDGE_UNAVAILABLE",
        retryable: false,
      });
    }
  });

  it("不返回任何伪造数据（没有空列表、没有零计数）", async () => {
    const bridge = new UnavailableHavenAgentBridge();
    // 抛错而不是返回 0：返回 0 计数器会被调用方当成"库里确实没有作品"。
    await expect(bridge.getLibrarySummary({ limit: 10 })).rejects.toThrow();
  });
});
