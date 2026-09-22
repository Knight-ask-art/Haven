// 测试用 MCP 客户端/服务端夹具。
//
// 走真实的 in-memory transport，而不是直接调用 handler：
// 这样测试覆盖的是**协议层行为**（输入校验、outputSchema 校验、错误传递），
// 而不只是"我的函数返回了什么"。

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";

import type { HavenAgentBridge } from "../../src/bridge.js";
import { createHavenMcpServer } from "../../src/server.js";

export interface Harness {
  client: Client;
  close: () => Promise<void>;
}

export async function connectHarness(bridge: HavenAgentBridge): Promise<Harness> {
  const server = createHavenMcpServer(bridge);
  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  const client = new Client({ name: "haven-mcp-test", version: "0.0.0" });

  await Promise.all([server.connect(serverTransport), client.connect(clientTransport)]);

  return {
    client,
    close: async () => {
      await client.close();
      await server.close();
    },
  };
}

/** 取出工具结果的文本内容（本 server 永远只返回一个 text 块）。 */
export function textOf(result: { content?: unknown }): string {
  const content = result.content as { type: string; text?: string }[] | undefined;
  if (!Array.isArray(content) || content.length !== 1) {
    throw new Error(`期望恰好一个 content 块，实际：${JSON.stringify(content)}`);
  }
  const first = content[0];
  if (first === undefined || first.type !== "text" || typeof first.text !== "string") {
    throw new Error(`期望 text 内容块，实际：${JSON.stringify(first)}`);
  }
  return first.text;
}

/** 解析错误结果的稳定载荷。 */
export function errorPayloadOf(result: { content?: unknown }): {
  code: string;
  message: string;
  retryable: boolean;
} {
  const parsed = JSON.parse(textOf(result)) as { error?: { code: string; message: string; retryable: boolean } };
  if (parsed.error === undefined) throw new Error(`期望 error 载荷，实际：${JSON.stringify(parsed)}`);
  return parsed.error;
}
