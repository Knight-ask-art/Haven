// 超时 / 取消之后的**重试语义**。
//
// 为什么单独一个文件：`HAVEN_BRIDGE_TIMEOUT` 与 `HAVEN_BRIDGE_CANCELLED` 都携带一个
// `retryable` 布尔值，而这两个码在两侧的含义完全不同——
//
// - Rust Broker 的 `HAVEN_BROKER_TIMEOUT` 是 `retryable = true`，它的意思是
//   "**Broker** 可以再被问一次"；
// - MCP 侧的 `retryable` 回答的是另一个问题："调用方**自动重试**安全吗？"
//
// 对提案创建来说，这两个问题的答案相反：请求一旦发出去，Broker 可能已经落库了一条
// pending 提案。把它报成"可重试"会诱导客户端再发一次，用户于是在栖阅里看到两条一模一样
// 的待审批提案，而调用方以为自己只成功了一次。这类缺陷不会让任何"返回了什么"的断言失败
// ——错误码与文案骨架都照旧——只能靠直接断言**每一位**的 `retryable` 来钉住。
//
// 三条用例分工：
// - 逐工具：9 个工具全部被分类，且恰好 2 个提案工具为非重试；
// - 逐时机：用工具真正传的那组选项驱动 `withBridgeTimeout`，断言超时与取消两条路径
//   在提案上都不重试、在只读工具上都重试；
// - 跨层：MCP 的 15 秒必须**早于** Rust Broker 的 20 秒触发（从 Rust 源码现读，
//   不另抄一份数字）。

import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import {
  BRIDGE_TIMEOUT_MS,
  ERROR_CODES,
  PROPOSAL_TOOL_NAMES,
  TOOL_NAMES,
} from "../src/constants.js";
import type { ToolName } from "../src/constants.js";
import { withBridgeTimeout } from "../src/bridge.js";
import { retryableAfterCancel } from "../src/tools.js";

const PACKAGE_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const SESSION_RS = path.resolve(PACKAGE_ROOT, "../../src-tauri/src/agent_broker/session.rs");

const PROPOSALS: readonly string[] = PROPOSAL_TOOL_NAMES;

interface ErrorShape {
  code: string;
  retryable: boolean;
  message: string;
}

function asError(caught: unknown): ErrorShape {
  return caught as ErrorShape;
}

function never<T>(): Promise<T> {
  return new Promise<T>(() => undefined);
}

/**
 * 只有超时会落定的桥接调用。
 *
 * 选项与工具 handler 传的**完全一致**（`retryableOnCancel: retryableAfterCancel(name)`）：
 * 测试要钉的是生产路径的判定，不是测试自己拼的一组参数。
 */
async function timedOut(name: ToolName): Promise<never> {
  return await withBridgeTimeout(name, () => never<never>(), {
    timeoutMs: 5,
    retryableOnCancel: retryableAfterCancel(name),
  });
}

describe("MCP 超时 / 取消的重试语义", () => {
  it("9 个工具全部被分类：恰好提案工具不可自动重试", () => {
    const nonRetryable = TOOL_NAMES.filter((name) => !retryableAfterCancel(name));
    // 判定以"这次调用有没有可能已经落库"为准，因此非重试集合必须**恰好**是提案工具。
    // 多一个（例如把某个只读工具也标成不可重试）会让调用方白白放弃安全的恢复路径；
    // 少一个则是最坏情况：重复创建提案。
    expect([...nonRetryable].sort()).toEqual([...PROPOSAL_TOOL_NAMES].sort());
    expect(PROPOSAL_TOOL_NAMES.length).toBe(2);
    for (const name of TOOL_NAMES) {
      if (PROPOSALS.includes(name)) continue;
      expect(retryableAfterCancel(name), `${name} 是只读工具，重试是安全的`).toBe(true);
    }
  });

  it("提案在超时后**绝不**被标成可自动重试，且要求先回栖阅核对", async () => {
    for (const name of PROPOSALS) {
      const error = await timedOut(name as ToolName).then(() => null, asError);
      expect(error, `${name} 必须超时失败`).not.toBeNull();
      expect(error?.code).toBe(ERROR_CODES.BRIDGE_TIMEOUT);
      expect(error?.retryable, `${name} 的结果可能未确认，自动重试会重复创建提案`).toBe(
        false,
      );
      // 文案还必须是可行动的：告诉调用方去栖阅核对，而不是只说"超时了"。
      expect(error?.message).toContain("待审批提案");
      expect(error?.message).not.toContain("可以重试");
    }
  });

  it("只读工具在超时后仍可重试，不需要用户先核对", async () => {
    const error = await timedOut("get_settings_snapshot").then(() => null, asError);
    expect(error?.code).toBe(ERROR_CODES.BRIDGE_TIMEOUT);
    expect(error?.retryable).toBe(true);
    expect(error?.message).toContain("没有产生任何写入");
  });

  it("取消与超时在提案上给出同一个答案：不重试", async () => {
    for (const name of PROPOSALS) {
      const controller = new AbortController();
      const call = withBridgeTimeout(name, () => never<never>(), {
        signal: controller.signal,
        retryableOnCancel: retryableAfterCancel(name as ToolName),
      });
      controller.abort();
      const error = await call.then(() => null, asError);
      expect(error?.code).toBe(ERROR_CODES.BRIDGE_CANCELLED);
      expect(error?.retryable, `${name} 被取消后结果同样未确认`).toBe(false);
      expect(error?.message).toContain("待审批提案");
      expect(error?.message).not.toContain("没有产生任何写入");
    }
  });

  it("MCP 侧的 15 秒先于 Rust Broker 的 20 秒触发", () => {
    // 顺序反了的话，两层会同时超时：用户拿到的是 Broker 的通用超时，而不是 MCP 侧
    // 那条带着重试语义的稳定错误。数字从 Rust 源码现读，不另抄一份。
    const source = readFileSync(SESSION_RS, "utf8");
    const match = /pub const REQUEST_TIMEOUT: Duration = Duration::from_secs\((\d+)\);/.exec(
      source,
    );
    expect(match?.[1], "session.rs 里必须能找到 REQUEST_TIMEOUT").toBeTruthy();
    const brokerTimeoutMs = Number(match?.[1]) * 1_000;
    expect(BRIDGE_TIMEOUT_MS).toBeLessThan(brokerTimeoutMs);
    // 也钉一下握手上限：连上却不发首帧的客户端不能长期占着连接 permit，
    // 而它的收尾必须发生在请求超时之前。
    const handshake = /pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs\((\d+)\);/.exec(
      readFileSync(path.resolve(PACKAGE_ROOT, "../../src-tauri/src/agent_broker/listener.rs"), "utf8"),
    );
    expect(handshake?.[1], "listener.rs 里必须能找到 HANDSHAKE_TIMEOUT").toBeTruthy();
    expect(Number(handshake?.[1]) * 1_000).toBeLessThan(brokerTimeoutMs);
  });
});
