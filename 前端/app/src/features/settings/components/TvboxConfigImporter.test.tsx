// @vitest-environment jsdom

// TVBox 配置导入组件的交互边界测试。
//
// 这里钉住的是「能点什么、能看见什么」：
// - 预览成功前保存不可达；预览失败不留摘要；地址一改摘要作废；
// - 摘要只出现聚合计数，绝不出现配置地址；
// - 保存成功后刷新来源列表；已导入的 TVBox 来源只读、停用、没有启用开关。

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({
  previewTvboxConfig: vi.fn(),
  saveTvboxConfig: vi.fn(),
}))

vi.mock("../ipc/sources-gateway", async (importOriginal) => {
  const original = await importOriginal<typeof import("../ipc/sources-gateway")>()
  return {
    ...original,
    previewTvboxConfig: mocks.previewTvboxConfig,
    saveTvboxConfig: mocks.saveTvboxConfig,
  }
})

import { HavenError } from "@/lib/ipc/errors"
import type { SourceDescriptorDto, SourceRegistryDto, TvboxConfigPreviewDto } from "@/lib/ipc/generated/wire"
import { TvboxConfigImporter, type TvboxConfigImporterProps } from "./TvboxConfigImporter"

const CONFIG_URL = "https://config.example.invalid/tvbox.json"
const SOURCE_ID = "custom_tvbox_0123456789ab"

function facts(overrides: Partial<TvboxConfigPreviewDto> = {}): TvboxConfigPreviewDto {
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

function source(overrides: Partial<SourceDescriptorDto> = {}): SourceDescriptorDto {
  return {
    sourceId: SOURCE_ID,
    displayName: "客厅电视源",
    kinds: [],
    categories: ["video"],
    mode: "single",
    notes: "这是你导入的 TVBox / FongMi 配置。该来源当前处于停用状态：本版本尚未提供影视搜索与播放能力。",
    enabled: false,
    health: "unknown",
    endpointConfigured: true,
    credentialConfigured: false,
    lastChecked: null,
    latencyMs: null,
    successRate: null,
    ...overrides,
  }
}

function registry(sources: SourceDescriptorDto[]): SourceRegistryDto {
  return { schemaVersion: 2, sources }
}

function renderImporter(overrides: Partial<TvboxConfigImporterProps> = {}) {
  const showNotice = vi.fn()
  const onChanged = vi.fn()
  const view = render(
    <TvboxConfigImporter
      registry={null}
      category="all"
      showNotice={showNotice}
      onChanged={onChanged}
      {...overrides}
    />,
  )
  return { showNotice, onChanged, view }
}

function openForm() {
  fireEvent.click(screen.getByRole("button", { name: "导入配置" }))
}

function fillForm(name: string, url: string) {
  fireEvent.change(screen.getByLabelText("来源名称"), { target: { value: name } })
  fireEvent.change(screen.getByLabelText("配置地址"), { target: { value: url } })
}

function saveButton(): HTMLButtonElement {
  return screen.getByRole("button", { name: /保存并导入|保存中…/ }) as HTMLButtonElement
}

beforeEach(() => {
  mocks.previewTvboxConfig.mockReset()
  mocks.saveTvboxConfig.mockReset()
})

afterEach(() => cleanup())

describe("TvboxConfigImporter 的预览闸门", () => {
  it("测试成功后只展示聚合摘要，绝不展示配置地址", async () => {
    mocks.previewTvboxConfig.mockResolvedValue(facts())
    renderImporter()
    openForm()
    fillForm("客厅电视源", CONFIG_URL)
    fireEvent.click(screen.getByRole("button", { name: "测试配置" }))

    await waitFor(() => {
      expect(mocks.previewTvboxConfig).toHaveBeenCalledWith({ url: CONFIG_URL })
    })
    const summary = await screen.findByLabelText("配置摘要")
    expect(summary.textContent).toContain("站点")
    expect(summary.textContent).toContain("HTTP 端点 1")
    expect(summary.textContent).toContain("未归类 1")
    expect(summary.textContent).toContain("直播源")
    expect(summary.textContent).toContain("JavaScript")
    expect(summary.textContent).toContain("未识别顶层字段")
    // 摘要是白名单投影：地址、端点、原始内容都不得出现在里面。
    expect(summary.textContent).not.toContain("example.invalid")
    expect(saveButton().disabled).toBe(false)
  })

  it("测试失败时不留下任何摘要，并且保存不可达", async () => {
    mocks.previewTvboxConfig.mockRejectedValue(
      new HavenError({ code: "SOURCE_UNAVAILABLE", userMessage: "配置暂时不可达", retryable: true }),
    )
    renderImporter()
    openForm()
    fillForm("客厅电视源", CONFIG_URL)
    fireEvent.click(screen.getByRole("button", { name: "测试配置" }))

    expect((await screen.findByRole("alert")).textContent).toContain("配置暂时不可达")
    expect(screen.queryByLabelText("配置摘要")).toBeNull()
    expect(saveButton().disabled).toBe(true)

    fireEvent.click(saveButton())
    expect(mocks.saveTvboxConfig).not.toHaveBeenCalled()
  })

  it("没有测试过就不能保存", () => {
    renderImporter()
    openForm()
    fillForm("客厅电视源", CONFIG_URL)

    expect(saveButton().disabled).toBe(true)
    fireEvent.click(saveButton())
    expect(mocks.saveTvboxConfig).not.toHaveBeenCalled()
  })

  it("改地址后上一次摘要作废，必须重新测试", async () => {
    mocks.previewTvboxConfig.mockResolvedValue(facts())
    renderImporter()
    openForm()
    fillForm("客厅电视源", CONFIG_URL)
    fireEvent.click(screen.getByRole("button", { name: "测试配置" }))
    await screen.findByLabelText("配置摘要")
    expect(saveButton().disabled).toBe(false)

    fireEvent.change(screen.getByLabelText("配置地址"), {
      target: { value: "https://config.example.invalid/other.json" },
    })

    expect(screen.queryByLabelText("配置摘要")).toBeNull()
    expect(saveButton().disabled).toBe(true)
    fireEvent.click(saveButton())
    expect(mocks.saveTvboxConfig).not.toHaveBeenCalled()
  })

  it("测试通过但显示名为空时也不能保存", async () => {
    mocks.previewTvboxConfig.mockResolvedValue(facts())
    renderImporter()
    openForm()
    fillForm("", CONFIG_URL)
    fireEvent.click(screen.getByRole("button", { name: "测试配置" }))
    await screen.findByLabelText("配置摘要")

    expect(saveButton().disabled).toBe(true)
  })
})

describe("TvboxConfigImporter 的保存", () => {
  it("只调用一次保存网关，随后刷新来源并说明来源保持停用", async () => {
    mocks.previewTvboxConfig.mockResolvedValue(facts())
    mocks.saveTvboxConfig.mockResolvedValue({
      schemaVersion: 1,
      sourceId: SOURCE_ID,
      preview: facts(),
    })
    const { showNotice, onChanged } = renderImporter()
    openForm()
    fillForm("  客厅电视源  ", CONFIG_URL)
    fireEvent.click(screen.getByRole("button", { name: "测试配置" }))
    await screen.findByLabelText("配置摘要")

    fireEvent.click(saveButton())
    // 双击不得产生第二次登记。
    fireEvent.click(saveButton())

    await waitFor(() => {
      expect(mocks.saveTvboxConfig).toHaveBeenCalledTimes(1)
    })
    expect(mocks.saveTvboxConfig).toHaveBeenCalledWith({
      displayName: "客厅电视源",
      url: CONFIG_URL,
    })
    expect(onChanged).toHaveBeenCalledTimes(1)
    expect(showNotice).toHaveBeenCalledWith("TVBox 配置已导入；来源保持停用，搜索与播放尚不可用")

    const saved = await screen.findByText("配置已保存，来源保持停用")
    expect(saved).toBeTruthy()
    expect(screen.getByText(/尚未提供影视搜索与播放能力/)).toBeTruthy()
    expect(screen.getByText(new RegExp(SOURCE_ID))).toBeTruthy()
  })

  it("保存失败时保留摘要与输入，可以重试，且不刷新来源", async () => {
    mocks.previewTvboxConfig.mockResolvedValue(facts())
    mocks.saveTvboxConfig.mockRejectedValue(
      new HavenError({ code: "SECURITY_POLICY_DENIED", userMessage: "配置地址不安全", retryable: false }),
    )
    const { onChanged } = renderImporter()
    openForm()
    fillForm("客厅电视源", CONFIG_URL)
    fireEvent.click(screen.getByRole("button", { name: "测试配置" }))
    await screen.findByLabelText("配置摘要")

    fireEvent.click(saveButton())

    expect((await screen.findByRole("alert")).textContent).toContain("配置地址不安全")
    expect(screen.getByLabelText("配置地址")).toHaveProperty("value", CONFIG_URL)
    expect(screen.getByLabelText("配置摘要")).toBeTruthy()
    expect(saveButton().disabled).toBe(false)
    expect(onChanged).not.toHaveBeenCalled()
  })
})

describe("已导入的 TVBox 来源行", () => {
  it("只读展示、标记停用，并且没有启用开关", () => {
    renderImporter({ registry: registry([source()]) })

    expect(screen.getByText("客厅电视源")).toBeTruthy()
    expect(screen.getByText("影视 · 单一来源 · TVBox / FongMi 配置")).toBeTruthy()
    expect(screen.getByText(/已停用。本版本尚未提供影视搜索与播放能力，暂不支持启用，因此没有启用开关。/)).toBeTruthy()
    // 唯一的按钮是"导入配置"：这类来源在页面上没有任何启用/停用入口。
    expect(screen.getAllByRole("button").map((button) => button.textContent)).toEqual(["导入配置"])
    expect(screen.queryByRole("menuitem")).toBeNull()
    expect(screen.queryByText(/启用来源|停用来源/)).toBeNull()
  })

  it("只把 custom_tvbox_ 前缀的行当作 TVBox 来源", () => {
    renderImporter({
      registry: registry([
        source(),
        source({ sourceId: "custom_feed_0123456789ab", displayName: "科技新闻订阅", categories: ["periodical"] }),
        source({ sourceId: "custom_0123456789ab", displayName: "我的书库", categories: ["book"] }),
      ]),
    })

    expect(screen.getByText("客厅电视源")).toBeTruthy()
    expect(screen.queryByText("科技新闻订阅")).toBeNull()
    expect(screen.queryByText("我的书库")).toBeNull()
  })

  it("目录为空时给出空状态，避免 '什么都没有' 的错觉", () => {
    renderImporter({ registry: registry([]) })
    expect(screen.getByText("还没有导入 TVBox / FongMi 配置。")).toBeTruthy()
  })

  it("按内容分类筛选：切到图书时 TVBox 行不出现", () => {
    renderImporter({ registry: registry([source()]), category: "book" })
    expect(screen.queryByText("客厅电视源")).toBeNull()
    expect(screen.queryByText("还没有导入 TVBox / FongMi 配置。")).toBeNull()
  })
})
