// 外部 MCP 客户端自动配置的 wire 守卫与状态投影测试。
//
// 这些用例只断言纯函数：不读网络、不碰客户端单例、不依赖 React。
//
// 断言的安全边界：
// - 响应必须精确匹配后端 DTO：未知键一律拒绝，而不是"忽略"；
// - **畸形载荷不得变成"未配置、可写"**：那会让界面给出一个按钮，而用户按下时栖阅已经
//   在碰他的配置文件了；
// - `configured` / `malformed` / `blocked` 三种"没有要写的东西"的状态都不许说成可写；
// - 状态响应必须两个客户端各一条：少一条意味着界面少显示一个客户端而用户看不出来。

import { describe, expect, it } from "vitest"

import {
  guardMcpClientConfigStatus,
  guardMcpClientTargetStatus,
} from "@/lib/ipc/mcp-client-wire"
import {
  mcpClientActionLabel,
  mcpClientCanWrite,
  mcpClientStateLabel,
  mcpClientStatusNote,
} from "./mcp-client-gateway"

function entry(overrides: Record<string, unknown> = {}) {
  return {
    target: "codex",
    label: "Codex",
    configPath: "C:\\Users\\tester\\.codex\\config.toml",
    state: "not_configured",
    writable: true,
    detail: "写入只会新增 Haven 这一条。",
    ...overrides,
  }
}

function status(overrides: Record<string, unknown> = {}) {
  return {
    schemaVersion: 1,
    runtimeReady: true,
    runtimeDetail: "",
    targets: [entry(), entry({ target: "claude_code", label: "Claude Code", configPath: null })],
    ...overrides,
  }
}

describe("客户端自动配置 wire 运行时守卫", () => {
  it("接受两个客户端各一条的自洽状态", () => {
    expect(guardMcpClientConfigStatus(status())).toBe(true)
    // 被重定向时没有路径可显示，这是合法事实而不是缺字段。
    expect(guardMcpClientTargetStatus(entry({ configPath: null }))).toBe(true)
  })

  it("拒绝未知键，而不是忽略它们 —— 尤其是不该出现的文件内容", () => {
    for (const extra of ["content", "raw", "text", "secret", "apiKey", "path", "extra"]) {
      expect(guardMcpClientTargetStatus(entry({ [extra]: "任意内容" })), extra).toBe(false)
      expect(guardMcpClientConfigStatus(status({ [extra]: "任意内容" })), extra).toBe(false)
    }
  })

  it("拒绝 Object.keys 看不见的 symbol 与非枚举键", () => {
    const withSymbol = entry() as Record<string | symbol, unknown>
    withSymbol[Symbol("content")] = "隐藏的文件内容"
    expect(guardMcpClientTargetStatus(withSymbol)).toBe(false)

    const hidden = entry()
    Object.defineProperty(hidden, "content", { value: "隐藏", enumerable: false })
    expect(guardMcpClientTargetStatus(hidden)).toBe(false)
  })

  it("拒绝未知目标与未知状态：闭合集合之外的枚举值一律不接受", () => {
    expect(guardMcpClientTargetStatus(entry({ target: "cursor" }))).toBe(false)
    expect(guardMcpClientTargetStatus(entry({ state: "configured_maybe" }))).toBe(false)
    expect(guardMcpClientTargetStatus(entry({ label: "" }))).toBe(false)
    expect(guardMcpClientTargetStatus(entry({ writable: "true" }))).toBe(false)
  })

  it("缺失或重复的客户端条目都不接受", () => {
    // 少一条：界面会少显示一个客户端，而用户看不出少了什么。
    expect(guardMcpClientConfigStatus(status({ targets: [entry()] }))).toBe(false)
    // 重复同一个目标：另一个客户端就此消失。
    expect(
      guardMcpClientConfigStatus(status({ targets: [entry(), entry()] })),
    ).toBe(false)
    // 多一条也不行。
    expect(
      guardMcpClientConfigStatus(
        status({ targets: [entry(), entry({ target: "claude_code" }), entry()] }),
      ),
    ).toBe(false)
  })

  it("没有写入的状态不得说成可写", () => {
    for (const state of ["blocked", "malformed", "configured"]) {
      expect(
        guardMcpClientConfigStatus(
          status({
            targets: [
              entry({ state, writable: true }),
              entry({ target: "claude_code", state, writable: false }),
            ],
          }),
        ),
        state,
      ).toBe(false)
    }
    // 可写的状态说成不可写是允许的（后端可能因为别的原因收紧），只是界面不给按钮。
    expect(
      guardMcpClientConfigStatus(
        status({
          targets: [
            entry({ state: "not_configured", writable: false }),
            entry({ target: "claude_code", state: "outdated", writable: false }),
          ],
        }),
      ),
    ).toBe(true)
  })
})

describe("客户端自动配置的状态投影", () => {
  it("五种状态各有独立中文标签，互不折叠", () => {
    const labels = ([
      "configured",
      "not_configured",
      "outdated",
      "malformed",
      "blocked",
    ] as const).map((state) => mcpClientStateLabel(state))
    expect(new Set(labels).size).toBe(5)
    expect(mcpClientStateLabel("malformed")).not.toBe(mcpClientStateLabel("not_configured"))
    expect(mcpClientStateLabel("blocked")).not.toBe(mcpClientStateLabel("not_configured"))
  })

  it("按钮文案区分「写入」与「重新写入」，可写性只认后端字段", () => {
    expect(mcpClientActionLabel("not_configured")).toBe("写入配置")
    expect(mcpClientActionLabel("outdated")).toBe("写入配置")
    expect(mcpClientActionLabel("configured")).toBe("重新写入")
    expect(mcpClientCanWrite(entry({ writable: true }) as never)).toBe(true)
    expect(mcpClientCanWrite(entry({ writable: false }) as never)).toBe(false)
  })

  it("结论句不许把「已写入」说成「已生效」", () => {
    const configured = mcpClientStatusNote(entry({ state: "configured" }) as never)
    expect(configured).toContain("重启客户端")
    // "已生效 / 已连接" 是我们在界面里没有证据的结论。
    expect(configured).not.toContain("已生效")
    expect(configured).not.toContain("已连接")
    // 被拦下与畸形都必须说清"不会写入 / 不会改写"。
    expect(mcpClientStatusNote(entry({ state: "blocked" }) as never)).toContain("不会写入")
    expect(mcpClientStatusNote(entry({ state: "malformed" }) as never)).toContain("不会改写")
  })
})
