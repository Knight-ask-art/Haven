// 真实 stdio 端到端测试。
//
// 前面的工具测试走 in-memory transport，覆盖协议行为，但**证明不了**两件事：
// 1. stdout 是否干净（stdio 传输下 stdout 就是 JSON-RPC 通道，一句杂音就毁掉协议流）；
// 2. 生产入口的桥接选择是否 fail-closed（环境变量能不能把假桥接打开）。
//
// 这两条只能靠真的把 `dist/index.js` 当子进程跑起来。若 `dist/` 不存在则跳过，
// 并在测试名里说明——完整门禁是 `npm run build && npm test`。

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const PACKAGE_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const ENTRY = path.join(PACKAGE_ROOT, "dist", "index.js");
const BUILT = existsSync(ENTRY);
const maybe = BUILT ? it : it.skip;

interface RunResult {
  code: number | null;
  stdout: string;
  stderr: string;
}

function runServer(
  lines: string[],
  env: Record<string, string> = {},
): Promise<RunResult> {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [ENTRY], {
      cwd: PACKAGE_ROOT,
      env: { ...process.env, ...env },
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
    });
    child.on("error", reject);
    child.on("close", (code) => resolve({ code, stdout, stderr }));

    for (const line of lines) child.stdin.write(`${line}\n`);
    // 关闭 stdin：stdio server 会随之结束。
    child.stdin.end();
  });
}

const INITIALIZE =
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}';
const INITIALIZED = '{"jsonrpc":"2.0","method":"notifications/initialized"}';

describe("stdio 端到端", () => {
  maybe("stdout 只有 JSON-RPC，stderr 承载日志", async () => {
    const result = await runServer([
      INITIALIZE,
      INITIALIZED,
      '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}',
      '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_system_capabilities","arguments":{}}}',
    ]);

    expect(result.code).toBe(0);

    const lines = result.stdout.split("\n").filter((line) => line.trim().length > 0);
    expect(lines).toHaveLength(3);
    // 每一行都必须是合法 JSON —— 这就是"stdout 没被日志污染"的定义。
    const messages = lines.map((line, index) => {
      try {
        return JSON.parse(line) as Record<string, unknown>;
      } catch {
        throw new Error(`stdout 第 ${index + 1} 行不是 JSON：${line.slice(0, 200)}`);
      }
    });
    expect(messages.every((message) => message.jsonrpc === "2.0")).toBe(true);

    const toolsList = messages.find((message) => message.id === 2) as {
      result?: { tools?: { name: string }[] };
    };
    expect(toolsList.result?.tools).toHaveLength(9);
    expect(toolsList.result?.tools?.map((tool) => tool.name)).toContain("get_system_capabilities");

    // 桥接不可用是**如实报告**，不是启动失败。
    const call = messages.find((message) => message.id === 3) as {
      result?: { isError?: boolean; structuredContent?: { haven?: { available?: boolean } } };
    };
    expect(call.result?.isError).toBeFalsy();
    expect(call.result?.structuredContent?.haven?.available).toBe(false);

    // 日志确实走了 stderr。
    expect(result.stderr).toContain("HAVEN_BRIDGE_UNAVAILABLE");
    expect(result.stderr).toContain("haven-mcp");
  });

  maybe("HAVEN_MCP_BRIDGE=fixture 在没有注入工厂时拒绝启动（生产不得启用测试替身）", async () => {
    for (const env of [
      { HAVEN_MCP_BRIDGE: "fixture" },
      { HAVEN_MCP_BRIDGE: "fixture", HAVEN_MCP_ALLOW_TEST_BRIDGE: "1" },
    ]) {
      const result = await runServer([INITIALIZE], env);
      expect(result.code, JSON.stringify(env)).not.toBe(0);
      // 拒绝必须发生在写任何协议帧之前。
      expect(result.stdout.trim(), JSON.stringify(env)).toBe("");
      expect(result.stderr).toContain("fixture");
    }
  });

  maybe("HAVEN_MCP_BRIDGE=live 没有端点时启动诊断桥接，不静默猜测", async () => {
    const result = await runServer([INITIALIZE], {
      HAVEN_MCP_BRIDGE: "live",
      HAVEN_MCP_ENDPOINT: "",
    });
    expect(result.code).toBe(0);
    expect(result.stdout.trim()).not.toBe("");
    expect(result.stderr).toContain("HAVEN_MCP_ENDPOINT");
    expect(result.stderr).toContain("HAVEN_BRIDGE_UNAVAILABLE");
  });

  maybe("未知桥接取值同样拒绝启动", async () => {
    const result = await runServer([INITIALIZE], { HAVEN_MCP_BRIDGE: "production" });
    expect(result.code).not.toBe(0);
    expect(result.stdout.trim()).toBe("");
    expect(result.stderr).toContain("未知");
  });

  it.runIf(!BUILT)("dist 缺失时请先运行 npm run build 以启用端到端用例", () => {
    expect(BUILT).toBe(false);
  });
});
