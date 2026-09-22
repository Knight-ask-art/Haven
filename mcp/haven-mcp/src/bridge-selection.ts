// 桥接选择：显式、fail-closed、测试替身不可用作生产默认。
//
// 规范：docs/architecture/AI_SYSTEM.md §5、§10。
//
// 三条规则：
// 1. **默认不可用。** 不设环境变量 → `UnavailableHavenAgentBridge`。
// 2. **测试替身必须被显式注入，而且是"注入"而不是"配置"。**
//    `fixture` 模式需要一个 `fixtureFactory` 参数；生产入口（`index.ts`）**不传**它，
//    因此生产进程的模块图里根本不存在任何 fixture 实现——它不是"关掉的开关"，
//    而是"没有那段代码"。
// 3. **未实现的传输要报错，不要静默降级。** 要求 `live` 时如果直接退回 unavailable，
//    调用方会以为桥接已配置、只是数据为空。这里返回显式拒绝，由入口打印并以非零码退出。

import { UnavailableHavenAgentBridge, type HavenAgentBridge } from "./bridge.js";
import {
  HAVEN_ENDPOINT_ENV_VAR,
  LiveHavenAgentBridge,
  type EndpointValidationOptions,
} from "./local-broker.js";

export const BRIDGE_ENV_VAR = "HAVEN_MCP_BRIDGE";
export const ALLOW_TEST_BRIDGE_ENV_VAR = "HAVEN_MCP_ALLOW_TEST_BRIDGE";

export type BridgeSelection =
  | { ok: true; bridge: HavenAgentBridge }
  | { ok: false; reason: string };

export interface BridgeSelectionOptions {
  /**
   * 测试专用桥接工厂。
   *
   * **只有测试会传它。** 生产入口不传，因此 `fixture` 在生产里必然被拒绝
   * （见 `selectBridge` 的第一条 fixture 分支）。
   */
  fixtureFactory?: () => HavenAgentBridge;
  /** 仅供测试固定平台；生产默认使用 process.platform。 */
  platform?: EndpointValidationOptions["platform"];
}

/**
 * 依据环境选择桥接。
 *
 * 纯函数：不读全局状态、不打印，因此可以被穷举测试。
 */
export function selectBridge(
  env: Record<string, string | undefined>,
  options: BridgeSelectionOptions = {},
): BridgeSelection {
  const requested = (env[BRIDGE_ENV_VAR] ?? "unavailable").trim().toLowerCase();

  switch (requested) {
    case "":
    case "unavailable":
    case "none":
      return { ok: true, bridge: new UnavailableHavenAgentBridge() };

    case "fixture": {
      // 测试替身需要**双重**显式确认：允许开关 + 实际注入了工厂。
      if (env[ALLOW_TEST_BRIDGE_ENV_VAR] !== "1") {
        return {
          ok: false,
          reason:
            `拒绝以 fixture 桥接启动：${ALLOW_TEST_BRIDGE_ENV_VAR} 未设为 1。` +
            "fixture 只是测试替身，绝不能被当成生产数据源。",
        };
      }
      if (options.fixtureFactory === undefined) {
        return {
          ok: false,
          reason:
            "拒绝以 fixture 桥接启动：本入口没有注入 fixture 工厂。" +
            "测试替身只能由测试代码注入，不能通过环境变量在生产进程里启用。",
        };
      }
      return { ok: true, bridge: options.fixtureFactory() };
    }

    case "live": {
      const endpoint = env[HAVEN_ENDPOINT_ENV_VAR];
      if (endpoint === undefined || endpoint.length === 0) {
        return {
          ok: true,
          bridge: new UnavailableHavenAgentBridge(
            "已要求 live 桥接，但没有设置 HAVEN_MCP_ENDPOINT；不会猜测或扫描本地端点。",
          ),
        };
      }
      return {
        ok: true,
        bridge: new LiveHavenAgentBridge(endpoint, { env, platform: options.platform }),
      };
    }

    default:
      return {
        ok: false,
        reason: `未知的 ${BRIDGE_ENV_VAR} 取值：${JSON.stringify(requested)}。允许值：unavailable | fixture | live。`,
      };
  }
}
