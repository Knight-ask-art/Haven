// 日志出口。
//
// **stdio 传输下 stdout 是 JSON-RPC 通道。** 任何一句 `console.log` 都会把协议流
// 破坏掉，而且症状是"客户端随机解析失败"，极难排查。因此本模块是唯一允许写日志的
// 地方，且只写 stderr；`test/source-guards.test.ts` 会扫描 src/ 确认没有任何
// `console.log` / `process.stdout.write`。

import { redactText } from "./redact.js";

function write(prefix: string, message: string): void {
  // 日志同样过脱敏：MCP 客户端会把 stderr 收进日志文件，而日志文件比响应更容易被分享。
  process.stderr.write(`[haven-mcp]${prefix} ${redactText(message)}\n`);
}

export function logInfo(message: string): void {
  write("", message);
}

export function logWarn(message: string): void {
  write("[warn]", message);
}

export function logError(message: string): void {
  write("[error]", message);
}
