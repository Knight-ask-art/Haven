// TVBox / FongMi 配置预览的网关与 wire 守卫测试（Film/TV Provider 基础切片）。
//
// 预览/保存的网关与 Wire 边界测试。这里断言的重点是**响应里不能有什么**：
// - 精确键集合：任何多余的键（尤其是配置地址、端点、header、ext、凭据）都判非法；
// - `Reflect.ownKeys` 而不是 `Object.keys`：symbol 键与非枚举键同样参与比对；
// - 畸形载荷不得被当成半份摘要交给页面。

import { beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({ getHavenClient: vi.fn() }))

vi.mock("@/lib/ipc/runtime", () => ({ getHavenClient: mocks.getHavenClient }))

import { HavenError } from "@/lib/ipc/errors"
import { guardTvboxConfigPreview, guardTvboxConfigSaveResult, previewTvboxConfig, saveTvboxConfig } from "./sources-gateway"

function preview(overrides: Record<string, unknown> = {}) {
  return {
    schemaVersion: 1,
    siteCount: 3,
    liveCount: 1,
    parserCount: 2,
    skippedSiteRows: 0,
    skippedLiveRows: 0,
    skippedParserRows: 0,
    spiderConfigured: true,
    spiderKind: "javascript",
    spiderHasIntegrityDigest: true,
    httpEndpointSiteCount: 1,
    spiderSiteCount: 1,
    unclassifiedSiteCount: 1,
    opaqueTopLevelFieldNames: ["wallpaper", "epg"],
    unrecognizedTopLevelFieldCount: 4,
    withheldTopLevelFieldCount: 0,
    ...overrides,
  }
}

beforeEach(() => {
  mocks.getHavenClient.mockReset()
})

describe("tvbox_config_preview 运行时守卫", () => {
  it("接受一份只有形态摘要的响应", () => {
    expect(guardTvboxConfigPreview(preview())).toBe(true)
    // `spiderKind` 为 null 是合法事实：配置里没有 spider，不是缺字段。
    expect(guardTvboxConfigPreview(preview({ spiderConfigured: false, spiderKind: null }))).toBe(
      true,
    )
    for (const kind of ["jar", "javascript", "python", "json_manifest", "unidentified"]) {
      expect(guardTvboxConfigPreview(preview({ spiderKind: kind })), kind).toBe(true)
    }
  })

  it("拒绝被 snake_case 拆错的实现种类", () => {
    // `java_script` 是 rename_all 的产物，不是任何权威实现使用的名字。
    expect(guardTvboxConfigPreview(preview({ spiderKind: "java_script" }))).toBe(false)
    expect(guardTvboxConfigPreview(preview({ spiderKind: "JAR" }))).toBe(false)
    expect(guardTvboxConfigPreview(preview({ spiderKind: "" }))).toBe(false)
  })

  it("拒绝任何多余的键，而不是忽略它们", () => {
    for (const extra of [
      "url",
      "configUrl",
      "endpoint",
      "rawJson",
      "headers",
      "headerValues",
      "ext",
      "extContent",
      "cookie",
      "token",
      "secret",
      "api",
      "body",
    ]) {
      expect(guardTvboxConfigPreview(preview({ [extra]: "任意值" })), extra).toBe(false)
    }
    // 少一个键同样是漂移。
    const missing = preview()
    delete (missing as Record<string, unknown>).withheldTopLevelFieldCount
    expect(guardTvboxConfigPreview(missing)).toBe(false)
  })

  it("用 Reflect.ownKeys 比对键集合：symbol 与非枚举键都算多余", () => {
    const withSymbol = preview() as unknown as Record<string | symbol, unknown>
    withSymbol[Symbol("hidden")] = "任意值"
    expect(guardTvboxConfigPreview(withSymbol)).toBe(false)

    const withHidden = preview()
    Object.defineProperty(withHidden, "hidden", { value: "任意值", enumerable: false })
    expect(guardTvboxConfigPreview(withHidden)).toBe(false)
  })

  it("拒绝类型漂移的计数与标志位", () => {
    for (const broken of [
      { schemaVersion: 2 },
      { siteCount: -1 },
      { liveCount: 1.5 },
      { parserCount: "2" },
      { skippedSiteRows: null },
      { spiderConfigured: "true" },
      { spiderHasIntegrityDigest: 1 },
      { unclassifiedSiteCount: Number.NaN },
      { opaqueTopLevelFieldNames: "wallpaper" },
      { opaqueTopLevelFieldNames: [""] },
      { opaqueTopLevelFieldNames: [1] },
      { unrecognizedTopLevelFieldCount: undefined },
    ] as Array<Record<string, unknown>>) {
      expect(guardTvboxConfigPreview(preview(broken)), JSON.stringify(broken)).toBe(false)
    }
    for (const notAnObject of [null, undefined, 1, "preview", []]) {
      expect(guardTvboxConfigPreview(notAnObject)).toBe(false)
    }
  })
})

describe("previewTvboxConfig", () => {
  it("把请求原样交给客户端并返回已守卫的响应", async () => {
    const response = preview()
    mocks.getHavenClient.mockReturnValue({
      tvboxConfigPreview: vi.fn().mockResolvedValue(response),
    })
    const request = { url: "https://config.example.invalid/tvbox.json" }

    await expect(previewTvboxConfig(request)).resolves.toEqual(response)
    expect(mocks.getHavenClient().tvboxConfigPreview).toHaveBeenCalledWith(request)
  })

  it("把畸形响应当作内部错误，而不是半份摘要", async () => {
    mocks.getHavenClient.mockReturnValue({
      tvboxConfigPreview: vi.fn().mockResolvedValue(preview({ endpoint: "https://api.invalid" })),
    })
    await expect(previewTvboxConfig({ url: "https://config.example.invalid/a.json" })).rejects.toThrow(
      "非法数据",
    )
  })

  it("把后端稳定错误归一成 HavenError（code / retryable 不变）", async () => {
    mocks.getHavenClient.mockReturnValue({
      tvboxConfigPreview: vi.fn().mockRejectedValue({
        code: "SOURCE_UNAVAILABLE",
        userMessage: "TVBox 配置暂时不可达",
        retryable: true,
      }),
    })
    const rejection = await previewTvboxConfig({
      url: "https://config.example.invalid/a.json",
    }).catch((error: unknown) => error)
    expect(rejection).toBeInstanceOf(HavenError)
    expect((rejection as HavenError).code).toBe("SOURCE_UNAVAILABLE")
    expect((rejection as HavenError).retryable).toBe(true)
    // 错误文案里不得出现被拒绝的地址。
    expect((rejection as HavenError).message).not.toContain("example.invalid")
  })
})

function saved(overrides: Record<string, unknown> = {}) {
  return {
    schemaVersion: 1,
    sourceId: "custom_tvbox_0123456789ab",
    preview: preview(),
    ...overrides,
  }
}

describe("tvbox_config_save 运行时守卫", () => {
  it("接受稳定 sourceId 与同一份形态摘要", () => {
    expect(guardTvboxConfigSaveResult(saved())).toBe(true)
  })

  it("拒绝任何多余的键，而不是忽略它们", () => {
    for (const extra of ["url", "endpoint", "enabled", "raw", "body", "credentials"]) {
      expect(guardTvboxConfigSaveResult(saved({ [extra]: "任意值" })), extra).toBe(false)
    }
    const missing = saved()
    delete (missing as Record<string, unknown>).preview
    expect(guardTvboxConfigSaveResult(missing)).toBe(false)

    const withSymbol = saved() as unknown as Record<string | symbol, unknown>
    withSymbol[Symbol("hidden")] = "任意值"
    expect(guardTvboxConfigSaveResult(withSymbol)).toBe(false)
  })

  it("只接受注册表生成的 TVBox 前缀 sourceId", () => {
    for (const sourceId of [
      "custom_0123456789ab",
      "custom_feed_0123456789ab",
      "custom_tvbox_",
      "tvbox_0123456789ab",
      "",
      42,
      null,
    ]) {
      expect(guardTvboxConfigSaveResult(saved({ sourceId })), String(sourceId)).toBe(false)
    }
  })

  it("内嵌摘要仍受预览守卫约束", () => {
    // 保存路径不得出现第二套脱敏规则：摘要里多一个键就必须整份判非法。
    expect(
      guardTvboxConfigSaveResult(saved({ preview: preview({ endpoint: "https://api.invalid" }) })),
    ).toBe(false)
    expect(guardTvboxConfigSaveResult(saved({ preview: null }))).toBe(false)
  })

  it("拒绝类型漂移的 schemaVersion", () => {
    for (const broken of [0, 2, "1", null]) {
      expect(guardTvboxConfigSaveResult(saved({ schemaVersion: broken }))).toBe(false)
    }
  })
})

describe("saveTvboxConfig", () => {
  it("把请求原样交给客户端并返回已守卫的响应", async () => {
    const response = saved()
    mocks.getHavenClient.mockReturnValue({
      tvboxConfigSave: vi.fn().mockResolvedValue(response),
    })
    const request = {
      displayName: "电视源",
      url: "https://config.example.invalid/tvbox.json",
    }

    await expect(saveTvboxConfig(request)).resolves.toEqual(response)
    expect(mocks.getHavenClient().tvboxConfigSave).toHaveBeenCalledWith(request)
  })

  it("把畸形响应当作内部错误，而不是半份结果", async () => {
    mocks.getHavenClient.mockReturnValue({
      tvboxConfigSave: vi.fn().mockResolvedValue(saved({ sourceId: "custom_other" })),
    })
    await expect(
      saveTvboxConfig({ displayName: "电视源", url: "https://config.example.invalid/a.json" }),
    ).rejects.toThrow("非法数据")
  })

  it("把后端稳定错误归一成 HavenError（code / retryable 不变）", async () => {
    mocks.getHavenClient.mockReturnValue({
      tvboxConfigSave: vi.fn().mockRejectedValue({
        code: "SECURITY_POLICY_DENIED",
        userMessage: "TVBox 配置地址不安全",
        retryable: false,
      }),
    })
    const rejection = await saveTvboxConfig({
      displayName: "电视源",
      url: "http://127.0.0.1/tvbox.json",
    }).catch((error: unknown) => error)
    expect(rejection).toBeInstanceOf(HavenError)
    expect((rejection as HavenError).code).toBe("SECURITY_POLICY_DENIED")
    expect((rejection as HavenError).retryable).toBe(false)
    expect((rejection as HavenError).message).not.toContain("127.0.0.1")
  })
})
