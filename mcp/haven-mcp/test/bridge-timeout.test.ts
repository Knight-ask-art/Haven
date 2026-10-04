// `withBridgeTimeout` 的直接用例（超时 / 客户端取消 / 释放底层工作）。
//
// 为什么单独一个文件：这个函数是 MCP 侧**唯一**把"无限等待"变成稳定错误的地方，也是
// 客户端取消唯一能到达传输层的地方。它过去只有间接覆盖（走真实工具的整条链路），
// 于是"取消"这件事根本没有被断言过——而取消恰恰是唯一一个**不能**靠"重试一次"来掩盖
// 的失败：请求可能已经在 Haven 里落了库。

import { describe, expect, it, vi } from "vitest";

import { withBridgeTimeout } from "../src/bridge.js";
import { ERROR_CODES } from "../src/constants.js";
import { toErrorPayload } from "../src/errors.js";

function never<T>(): Promise<T> {
  return new Promise<T>(() => undefined);
}

describe("withBridgeTimeout", () => {
  it("returns the call result and leaves the signal untouched", async () => {
    let seen: AbortSignal | null = null;
    const value = await withBridgeTimeout(
      "读取能力清单",
      async (signal) => {
        seen = signal;
        return 42;
      },
      { timeoutMs: 1_000 },
    );
    expect(value).toBe(42);
    expect(seen).not.toBeNull();
    expect((seen as unknown as AbortSignal).aborted).toBe(false);
  });

  it("maps a timeout to the stable timeout code and aborts the transport signal", async () => {
    let seen: AbortSignal | null = null;
    const error = await withBridgeTimeout(
      "读取设置快照",
      (signal) => {
        seen = signal;
        return never();
      },
      { timeoutMs: 5, retryableOnCancel: true },
    ).catch((caught: unknown) => caught);

    expect(toErrorPayload(error).code).toBe(ERROR_CODES.BRIDGE_TIMEOUT);
    // 只读操作：超时之后重试是安全的。
    expect(toErrorPayload(error).retryable).toBe(true);
    // 关键：底层连接必须被告知释放工作，否则超时只是一个"我们不等了"的假象。
    expect((seen as unknown as AbortSignal).aborted).toBe(true);
  });

  it("keeps a timeout non-retryable when the caller does not declare it safe", async () => {
    const error = await withBridgeTimeout("创建设置提案", () => never(), {
      timeoutMs: 5,
    }).catch((caught: unknown) => caught);
    const payload = toErrorPayload(error);
    expect(payload.code).toBe(ERROR_CODES.BRIDGE_TIMEOUT);
    expect(payload.retryable).toBe(false);
    expect(payload.message).toContain("待审批提案");
  });

  it("maps a client cancellation to a distinct code", async () => {
    const controller = new AbortController();
    const promise = withBridgeTimeout("读取媒体库摘要", () => never(), {
      timeoutMs: 10_000,
      signal: controller.signal,
      retryableOnCancel: true,
    });
    controller.abort();
    const payload = toErrorPayload(await promise.catch((caught: unknown) => caught));
    // 取消与超时必须是两个码：一个是"调用方不想等了"，一个是"我们没等到"。
    expect(payload.code).toBe(ERROR_CODES.BRIDGE_CANCELLED);
    expect(payload.retryable).toBe(true);
    expect(payload.message).toContain("没有产生任何写入");
  });

  it("never lets a cancelled proposal look retryable or claim nothing was written", async () => {
    const controller = new AbortController();
    const promise = withBridgeTimeout("创建设置提案", () => never(), {
      timeoutMs: 10_000,
      signal: controller.signal,
      retryableOnCancel: false,
    });
    controller.abort();
    const payload = toErrorPayload(await promise.catch((caught: unknown) => caught));

    expect(payload.code).toBe(ERROR_CODES.BRIDGE_CANCELLED);
    // 提案可能已经落库：非重试，且必须让调用方回栖阅核对而不是再发一次。
    expect(payload.retryable).toBe(false);
    expect(payload.message).toContain("可能已经在栖阅中创建了待审批提案");
    expect(payload.message).not.toContain("没有产生任何写入");
  });

  it("propagates an already-aborted signal before doing any work", async () => {
    const controller = new AbortController();
    controller.abort();
    const call = vi.fn(() => never());
    const payload = toErrorPayload(
      await withBridgeTimeout("读取设置快照", call, {
        timeoutMs: 10_000,
        signal: controller.signal,
        retryableOnCancel: true,
      }).catch((caught: unknown) => caught),
    );
    expect(payload.code).toBe(ERROR_CODES.BRIDGE_CANCELLED);
    // 调用体仍然会被调用一次（它立刻收到一个已取消的信号，从而不必真的去连接）。
    expect(call).toHaveBeenCalledTimes(1);
  });

  it("reports a timeout rather than a cancellation when the local timer fires first", async () => {
    // 超时内部也会 abort（为了释放底层连接）。若两者的落定顺序写反，用户看到的会是
    // "被取消"——把我们的超时说成调用方的决定，是一个会误导诊断的结论。
    const payload = toErrorPayload(
      await withBridgeTimeout("读取设置快照", () => never(), {
        timeoutMs: 5,
        retryableOnCancel: true,
      }).catch((caught: unknown) => caught),
    );
    expect(payload.code).toBe(ERROR_CODES.BRIDGE_TIMEOUT);
  });
});
