// McpServer 组装。
//
// 与传输无关：`index.ts` 负责 stdio，测试用 in-memory transport 驱动同一个 server。
// 这样"注册了哪些工具"这件事只有一个实现，测试断言的就是生产行为。

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";

import { SERVER_NAME, SERVER_VERSION } from "./constants.js";
import type { HavenAgentBridge } from "./bridge.js";
import { registerHavenTools } from "./tools.js";

/**
 * 创建 Haven MCP server。
 *
 * `bridge` 是**必填**参数而不是可选项：没有"忘了传就静默用某个默认实现"的余地。
 * 生产入口显式传入 `UnavailableHavenAgentBridge`（或未来某个真实适配器）。
 */
export function createHavenMcpServer(bridge: HavenAgentBridge): McpServer {
  const server = new McpServer(
    { name: SERVER_NAME, version: SERVER_VERSION },
    {
      capabilities: { tools: {} },
      instructions: [
        "栖阅（Haven）MCP server。",
        "本 server 只能读取已脱敏的上下文，或创建**待批准**的设置提案。",
        "没有任何工具可以批准、应用、删除或写入；真正的写入必须由用户在 Haven 应用界面里完成。",
        "调用顺序：先 get_system_capabilities 了解本构建实际可用的能力，再决定是否调用其它工具。",
        "创建提案前必须先用读取工具取得 context_id / context_hash / base_revision 并原样回传。",
      ].join("\n"),
    },
  );

  registerHavenTools(server, bridge);
  return server;
}
