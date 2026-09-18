// 外部 Agent 接入 Gateway 的纯函数测试（A5 接线切片）：
// - `AgentBrokerStatusResultDto` 严格守卫（含 symbol / 非枚举键）；
// - 端点「真实监听 / 浏览器预览 / 未识别」的三态判断；
// - 四个客户端模板的动态注入与 JSON 转义。
//
// 这些用例全部是纯函数断言：不读网络、不碰剪贴板、不依赖 Mock 客户端。

import { describe, expect, it } from "vitest"

import {
  AGENT_BROKER_CLIENT_TEMPLATES,
  AGENT_BROKER_TEMPLATE_NOTE,
  AGENT_BROKER_TEMPLATE_PATH,
  agentBrokerEndpointKind,
  buildAgentBrokerMcpTemplate,
  guardAgentBrokerStatusResult,
  isBrowserPreviewAgentBrokerEndpoint,
  isRealAgentBrokerEndpoint,
} from "./agent-broker-gateway"

/** 与 MockHavenClient 同值的浏览器预览标识。 */
const MOCK_ENDPOINT = "mock://browser-preview-agent-broker"
const PIPE_ENDPOINT = "\\\\.\\pipe\\haven-agent-v1-3f2a91c4"
const SOCKET_ENDPOINT = "/run/user/1000/haven/agent-v1.sock"

const VALID_RESULTS = [
  { schemaVersion: 1, status: "disabled", endpoint: null, reason: null },
  { schemaVersion: 1, status: "listening", endpoint: PIPE_ENDPOINT, reason: null },
  { schemaVersion: 1, status: "listening", endpoint: SOCKET_ENDPOINT, reason: null },
  { schemaVersion: 1, status: "busy", endpoint: null, reason: null },
  { schemaVersion: 1, status: "unavailable", endpoint: null, reason: "当前平台不支持本地端点" },
  // unavailable 没有原因时如实缺省，不编造理由。
  { schemaVersion: 1, status: "unavailable", endpoint: null, reason: null },
]

describe("外部 Agent 接入 wire 运行时守卫", () => {
  it("接受四种状态的自洽投影", () => {
    for (const result of VALID_RESULTS) {
      expect(guardAgentBrokerStatusResult(result), JSON.stringify(result)).toBe(true)
    }
  })

  it("拒绝字段集合不对、schemaVersion 不符或状态不在闭合集合内的响应", () => {
    const base = VALID_RESULTS[0]
    // 未知键一律拒绝，而不是"忽略"。
    expect(guardAgentBrokerStatusResult({ ...base, sessionId: "abc" })).toBe(false)
    expect(guardAgentBrokerStatusResult({ ...base, capabilities: {} })).toBe(false)
    expect(guardAgentBrokerStatusResult({ schemaVersion: 2, status: "disabled", endpoint: null, reason: null })).toBe(false)
    expect(guardAgentBrokerStatusResult({ status: "disabled", endpoint: null, reason: null })).toBe(false)
    expect(guardAgentBrokerStatusResult({ ...base, status: "paused" })).toBe(false)
    // 状态必须是字符串，`true` / 数字这类"真值"不是合法枚举。
    expect(guardAgentBrokerStatusResult({ ...base, status: true })).toBe(false)
    for (const notAnObject of [null, undefined, "disabled", [], 7]) {
      expect(guardAgentBrokerStatusResult(notAnObject)).toBe(false)
    }
  })

  it("拒绝 Object.keys 看不见的 symbol 与非枚举键", () => {
    const withSymbol = { ...VALID_RESULTS[1] } as Record<string | symbol, unknown>
    withSymbol[Symbol("sessionId")] = "s-1"
    expect(guardAgentBrokerStatusResult(withSymbol)).toBe(false)

    const hidden = { ...VALID_RESULTS[1] }
    Object.defineProperty(hidden, "rawError", { value: "C:\\Users\\a\\secret", enumerable: false })
    expect(guardAgentBrokerStatusResult(hidden)).toBe(false)
  })

  it("要求端点与状态互相印证：listening 必须有端点，其余状态必须没有", () => {
    // listening 缺端点 = 声称在监听却给不出地址。
    expect(guardAgentBrokerStatusResult({ schemaVersion: 1, status: "listening", endpoint: null, reason: null })).toBe(false)
    expect(guardAgentBrokerStatusResult({ schemaVersion: 1, status: "listening", endpoint: "", reason: null })).toBe(false)
    // disabled / busy / unavailable 带端点 = 端点泄漏到不该出现的状态。
    for (const status of ["disabled", "busy", "unavailable"]) {
      expect(guardAgentBrokerStatusResult({ schemaVersion: 1, status, endpoint: PIPE_ENDPOINT, reason: null })).toBe(false)
    }
    expect(guardAgentBrokerStatusResult({ schemaVersion: 1, status: "listening", endpoint: 42, reason: null })).toBe(false)
  })

  it("只允许 unavailable 携带原因，且原因不能是空串", () => {
    expect(guardAgentBrokerStatusResult({ schemaVersion: 1, status: "unavailable", endpoint: null, reason: "端点被另一实例占用" })).toBe(true)
    for (const status of ["disabled", "listening", "busy"]) {
      const result = status === "listening"
        ? { schemaVersion: 1, status, endpoint: PIPE_ENDPOINT, reason: "出错了" }
        : { schemaVersion: 1, status, endpoint: null, reason: "出错了" }
      expect(guardAgentBrokerStatusResult(result), status).toBe(false)
    }
    // 空串不是「没有原因」。
    expect(guardAgentBrokerStatusResult({ schemaVersion: 1, status: "unavailable", endpoint: null, reason: "" })).toBe(false)
  })
})

describe("外部 Agent 端点真实 / 预览判断", () => {
  it("只把平台限定形态当作真实端点", () => {
    expect(isRealAgentBrokerEndpoint(PIPE_ENDPOINT)).toBe(true)
    expect(isRealAgentBrokerEndpoint(SOCKET_ENDPOINT)).toBe(true)
    // 闭合集合之外的一切都不是可复制端点。
    for (const fake of [
      MOCK_ENDPOINT,
      "https://gateway.example.invalid/v1",
      "tcp://127.0.0.1:9",
      "file:///etc/passwd",
      "haven-agent-v1.sock",
      null,
      "",
    ]) {
      expect(isRealAgentBrokerEndpoint(fake), String(fake)).toBe(false)
    }
    // 前缀对但后缀不是 8 位小写十六进制：大写、位数不对、多带字符都要拒绝。
    expect(isRealAgentBrokerEndpoint("\\\\.\\pipe\\haven-agent-v1-3F2A91C4")).toBe(false)
    expect(isRealAgentBrokerEndpoint("\\\\.\\pipe\\haven-agent-v1-3f2a91c")).toBe(false)
    expect(isRealAgentBrokerEndpoint("\\\\.\\pipe\\haven-agent-v1-3f2a91c4x")).toBe(false)
  })

  it("把浏览器 Mock 的演示标识标成预览，而不是端点", () => {
    expect(isBrowserPreviewAgentBrokerEndpoint(MOCK_ENDPOINT)).toBe(true)
    expect(isBrowserPreviewAgentBrokerEndpoint(PIPE_ENDPOINT)).toBe(false)
    expect(isBrowserPreviewAgentBrokerEndpoint(null)).toBe(false)
  })

  it("三态判断：只有 listening 才有端点，且预览 / 未识别都不等于可用", () => {
    expect(agentBrokerEndpointKind("listening", PIPE_ENDPOINT)).toBe("live")
    expect(agentBrokerEndpointKind("listening", SOCKET_ENDPOINT)).toBe("live")
    expect(agentBrokerEndpointKind("listening", MOCK_ENDPOINT)).toBe("preview")
    // listening 却给了无法识别的地址：既不能复制，也不能说成浏览器预览。
    expect(agentBrokerEndpointKind("listening", "https://gateway.example.invalid/v1")).toBe("unknown")
    expect(agentBrokerEndpointKind("listening", null)).toBe("none")
    // busy / unavailable 不能因为"没有端点"就被当成 disabled 之外的东西——它们没有端点。
    expect(agentBrokerEndpointKind("busy", null)).toBe("none")
    expect(agentBrokerEndpointKind("unavailable", null)).toBe("none")
    expect(agentBrokerEndpointKind("disabled", null)).toBe("none")
    expect(agentBrokerEndpointKind(null, null)).toBe("none")
  })
})

describe("四个客户端共用的 MCP 连接模板", () => {
  it("四个客户端都在，且模板 JSON 完全相同（差异只在说明文案）", () => {
    expect(AGENT_BROKER_CLIENT_TEMPLATES.map((item) => item.id))
      .toEqual(["codex", "claude-code", "dsh", "pi"])
    expect(AGENT_BROKER_CLIENT_TEMPLATES.map((item) => item.label))
      .toEqual(["Codex", "Claude Code", "DSH", "Pi"])
    for (const item of AGENT_BROKER_CLIENT_TEMPLATES) {
      expect(item.hint).toContain(item.label)
    }
  })

  it("只包含 stdio 启动配置与两个环境变量，端点动态注入", () => {
    const template = buildAgentBrokerMcpTemplate(PIPE_ENDPOINT)
    const parsed = JSON.parse(template) as Record<string, unknown>
    expect(Object.keys(parsed)).toEqual(["command", "args", "env"])
    expect(parsed.command).toBe("node")
    expect(parsed.args).toEqual([AGENT_BROKER_TEMPLATE_PATH])
    expect(AGENT_BROKER_TEMPLATE_PATH).toBe("<path-to-haven>/mcp/haven-mcp/dist/index.js")
    expect(parsed.env).toEqual({
      HAVEN_MCP_BRIDGE: "live",
      HAVEN_MCP_ENDPOINT: PIPE_ENDPOINT,
    })
    // 路径必须是占位符：写死本机路径会把别人的仓库位置写进配置。
    expect(template).toContain("<path-to-haven>")
  })

  it("Windows 管道端点按 JSON 规则转义后仍能原样解析回来", () => {
    const template = buildAgentBrokerMcpTemplate(PIPE_ENDPOINT)
    // JSON 文本里必须是双反斜杠，否则配置根本解析不了。
    expect(template).toContain(JSON.stringify(PIPE_ENDPOINT))
    expect(template).toContain("\\\\")
    const parsed = JSON.parse(template) as { env: { HAVEN_MCP_ENDPOINT: string } }
    expect(parsed.env.HAVEN_MCP_ENDPOINT).toBe(PIPE_ENDPOINT)
    expect(parsed.env.HAVEN_MCP_ENDPOINT.startsWith("\\\\.\\pipe\\")).toBe(true)
  })

  it("端点里的引号、换行与反斜杠不会撑破模板", () => {
    const hostile = "/tmp/haven/agent-v1.sock\"},\n{\"command\":\"rm -rf /"
    const template = buildAgentBrokerMcpTemplate(hostile)
    const parsed = JSON.parse(template) as {
      env: { HAVEN_MCP_BRIDGE: string; HAVEN_MCP_ENDPOINT: string }
    }
    // 转义正确：注入内容原样留在字符串值里，没有变成新的 JSON 结构。
    expect(parsed.env.HAVEN_MCP_ENDPOINT).toBe(hostile)
    expect(Object.keys(parsed)).toEqual(["command", "args", "env"])
    expect(Object.keys(parsed.env)).toEqual(["HAVEN_MCP_BRIDGE", "HAVEN_MCP_ENDPOINT"])
  })

  it("模板不写任何模型名，并明确它不授予批准或应用权限", () => {
    const template = buildAgentBrokerMcpTemplate(SOCKET_ENDPOINT)
    for (const modelName of ["gpt-4", "gpt-4o", "claude-3", "sonnet", "gemini", "text-embedding"]) {
      expect(template.toLowerCase()).not.toContain(modelName)
    }
    expect(AGENT_BROKER_TEMPLATE_NOTE).toContain("MCP 连接配置")
    expect(AGENT_BROKER_TEMPLATE_NOTE).toContain("approve")
    expect(AGENT_BROKER_TEMPLATE_NOTE).toContain("apply")
    expect(AGENT_BROKER_TEMPLATE_NOTE).toContain("栖阅")
  })
})
