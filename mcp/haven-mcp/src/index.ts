#!/usr/bin/env node
// Haven MCP server 入口（stdio）。
//
// 规范：docs/architecture/AI_SYSTEM.md §5、§10。
//
// 生产入口**不注入** fixture 工厂，因此：
// - 未配置 → `UnavailableHavenAgentBridge`（每个操作返回稳定的 HAVEN_BRIDGE_UNAVAILABLE）；
// - 要求 `live` 且未配置端点 → 显式不可用桥接；配置合法端点后才连接 Rust Broker；
// - 要求 `fixture` → 显式拒绝并退出（测试替身不能通过环境变量在生产进程里启用）。
//
// 绝不在缺少桥接时静默降级：那会让调用方以为"桥接通了，只是没有数据"。

import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";

import { selectBridge } from "./bridge-selection.js";
import { logError, logInfo, logWarn } from "./log.js";
import { createHavenMcpServer } from "./server.js";

async function main(): Promise<void> {
  const selection = selectBridge(process.env);

  if (!selection.ok) {
    logError(selection.reason);
    process.exitCode = 1;
    return;
  }

  const bridge = selection.bridge;
  if (bridge.available) {
    logInfo(`Haven 桥接已接通（kind=${bridge.kind}）。`);
  } else {
    // 仍然启动：只读工具会给出稳定错误，而 `get_system_capabilities` 能如实报告
    // "为什么现在什么都做不了"。直接退出会让客户端只看到"server 启动失败"，
    // 反而丢掉了诊断信息。
    logWarn(`${bridge.statusDetail} 工具将返回 HAVEN_BRIDGE_UNAVAILABLE 与 HAVEN_CAPABILITY_UNAVAILABLE。`);
  }

  const server = createHavenMcpServer(bridge);
  const transport = new StdioServerTransport();
  await server.connect(transport);
  logInfo("Haven MCP server 已通过 stdio 启动。");
}

main().catch((error: unknown) => {
  // 只打印归一化后的消息，绝不打印可能带用户数据的原始堆栈。
  logError(error instanceof Error ? error.message : "启动失败");
  process.exitCode = 1;
});
