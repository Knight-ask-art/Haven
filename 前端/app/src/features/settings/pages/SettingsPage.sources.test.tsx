// @vitest-environment jsdom

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { MemoryRouter, Route, Routes } from "react-router"
import { afterEach, describe, expect, it, vi } from "vitest"

import { getHavenClient } from "@/lib/ipc/runtime"
import { HavenError } from "@/lib/ipc/errors"
import type {
  SourceDescriptorDto,
  SourceRegistryDto,
  TvboxConfigPreviewDto,
} from "@/lib/ipc/generated/wire"
import { SettingsPage } from "./SettingsPage"

const TVBOX_CONFIG_URL = "https://config.example.invalid/tvbox.json"
const TVBOX_SOURCE_ID = "custom_tvbox_0123456789ab"

function tvboxFacts(overrides: Partial<TvboxConfigPreviewDto> = {}): TvboxConfigPreviewDto {
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
    opaqueTopLevelFieldNames: ["wallpaper"],
    unrecognizedTopLevelFieldCount: 0,
    withheldTopLevelFieldCount: 0,
    ...overrides,
  }
}

/** 后端登记后的 TVBox 来源投影：kinds 为空、分类为影视、默认停用。 */
function tvboxDescriptor(): SourceDescriptorDto {
  return {
    sourceId: TVBOX_SOURCE_ID,
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
  }
}

function registryWith(sources: SourceDescriptorDto[]): SourceRegistryDto {
  return { schemaVersion: 2, sources }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => { resolve = done })
  return { promise, resolve }
}

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

function renderSourcesSettings() {
  return render(
    <MemoryRouter initialEntries={["/settings/sources"]}>
      <Routes>
        <Route path="/settings/:section" element={<SettingsPage />} />
      </Routes>
    </MemoryRouter>,
  )
}

describe("来源页的设计骨架", () => {
  it("在列表上方提供紧凑分类、搜索和添加入口，不使用示例来源占位", async () => {
    renderSourcesSettings()

    expect(await screen.findByRole("heading", { name: "来源" })).toBeTruthy()
    expect(screen.getByRole("tablist", { name: "来源类型" })).toBeTruthy()
    expect(screen.getByRole("searchbox", { name: "搜索来源" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "添加来源" })).toBeTruthy()
    expect(document.body.textContent).not.toContain("示例数据")
  })
})

describe("来源设置中的 RSS/Atom 添加流程", () => {
  it("把用户选择的订阅源类型沿 UI 传入 source_add，并跳过凭据表单", async () => {
    const sourceAdd = vi.spyOn(getHavenClient(), "sourceAdd").mockResolvedValue({
      schemaVersion: 1,
      sourceId: "custom_feed_0123456789ab",
    })

    renderSourcesSettings()
    fireEvent.click(await screen.findByRole("button", { name: "添加来源" }))
    fireEvent.click(await screen.findByRole("menuitem", { name: /RSS \/ Atom 订阅/ }))
    fireEvent.change(screen.getByPlaceholderText("科技新闻订阅"), {
      target: { value: "科技新闻订阅" },
    })
    fireEvent.change(screen.getByPlaceholderText("https://example.org/feed.xml"), {
      target: { value: "https://example.org/feed.xml" },
    })
    fireEvent.click(screen.getByRole("button", { name: "继续" }))

    await waitFor(() => {
      expect(sourceAdd).toHaveBeenCalledWith({
        displayName: "科技新闻订阅",
        endpoint: "https://example.org/feed.xml",
        kind: "feed",
      })
    })
    expect(await screen.findByText("来源已准备好")).toBeTruthy()
    expect(screen.getByText(/RSS\/Atom 订阅源（不使用访问凭据）/)).toBeTruthy()
    expect(screen.queryByLabelText("访问凭据（可选）")).toBeNull()
  })
})

describe("来源设置中的订阅源列表行", () => {
  it("只提供编辑入口与订阅源地址提示，不提供凭据入口", async () => {
    await getHavenClient().sourceAdd({
      displayName: "我的科技订阅",
      endpoint: "https://feeds.example.invalid/rss.xml",
      kind: "feed",
    })

    renderSourcesSettings()
    expect(await screen.findByText("我的科技订阅")).toBeTruthy()
    expect(screen.getByText("自定义 · RSS / Atom · 单一来源")).toBeTruthy()

    fireEvent.click(screen.getByRole("button", { name: "我的科技订阅 更多操作" }))
    expect(await screen.findByRole("menuitem", { name: /编辑来源/ })).toBeTruthy()
    // 订阅源没有受控的凭据路径：这一项必须缺席，而不是点进去才发现是假能力。
    expect(screen.queryByRole("menuitem", { name: /配置访问凭据/ })).toBeNull()

    fireEvent.click(screen.getByRole("menuitem", { name: /编辑来源/ }))
    expect(await screen.findByLabelText("订阅源地址")).toBeTruthy()
    expect(
      screen.getByPlaceholderText("留空表示地址不变；填入新的 HTTPS 地址覆盖（不能带查询串）"),
    ).toBeTruthy()
  })

  it("OPDS 书源仍然保留凭据入口", async () => {
    await getHavenClient().sourceAdd({
      displayName: "我的书库",
      endpoint: "https://books.example.invalid/opds/",
    })

    renderSourcesSettings()
    expect(await screen.findByText("我的书库")).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "我的书库 更多操作" }))
    expect(await screen.findByRole("menuitem", { name: /配置访问凭据/ })).toBeTruthy()
  })
})

describe("来源设置中的自托管漫画库（Komga/Kavita）", () => {
  it("把 Komga 种类沿 UI 传入 source_add，并给出 API key 输入", async () => {
    const sourceAdd = vi.spyOn(getHavenClient(), "sourceAdd").mockResolvedValue({
      schemaVersion: 1,
      sourceId: "custom_komga_0123456789ab",
    })

    renderSourcesSettings()
    fireEvent.click(await screen.findByRole("button", { name: "添加来源" }))
    fireEvent.click(await screen.findByRole("menuitem", { name: /Komga 漫画库/ }))
    fireEvent.change(screen.getByPlaceholderText("我的 Komga"), {
      target: { value: "我的 Komga" },
    })
    fireEvent.change(screen.getByPlaceholderText("https://comics.example.org"), {
      target: { value: "https://comics.example.org" },
    })
    fireEvent.click(screen.getByRole("button", { name: "继续" }))

    await waitFor(() => {
      expect(sourceAdd).toHaveBeenCalledWith({
        displayName: "我的 Komga",
        endpoint: "https://comics.example.org",
        kind: "komga",
      })
    })
    // 漫画库需要 API key：凭据步骤必须出现，文案说明它只作为请求头发送。
    expect(await screen.findByLabelText("API key（可选）")).toBeTruthy()
    expect(screen.getByText(/作为请求头发送，不会出现在地址、来源列表或日志中/)).toBeTruthy()
  })

  it("漫画库列表行显示漫画分类与 API key 配置状态，并保留凭据入口", async () => {
    await getHavenClient().sourceAdd({
      displayName: "家里的 Komga",
      endpoint: "https://comics.example.invalid",
      kind: "komga",
    })

    renderSourcesSettings()
    expect(await screen.findByText("家里的 Komga")).toBeTruthy()
    expect(screen.getByText("自定义 · Komga · 单一来源")).toBeTruthy()
    expect(screen.getByText("API key 未配置")).toBeTruthy()

    fireEvent.click(screen.getByRole("button", { name: "家里的 Komga 更多操作" }))
    expect(await screen.findByRole("menuitem", { name: /配置访问凭据/ })).toBeTruthy()

    fireEvent.click(screen.getByRole("menuitem", { name: /配置访问凭据/ }))
    expect(await screen.findByText(/API key（仅写入系统凭据管理器/)).toBeTruthy()
  })
})

describe("来源设置中的 TVBox / FongMi 配置导入", () => {
  it("演示环境里预览失败时如实报错，保存保持不可用", async () => {
    // 浏览器预览没有受控 HTTP：mock 客户端不编造摘要，页面必须把失败如实呈现。
    const tvboxConfigSave = vi.spyOn(getHavenClient(), "tvboxConfigSave")

    renderSourcesSettings()
    fireEvent.click(await screen.findByRole("button", { name: "添加来源" }))
    fireEvent.click(screen.getByRole("menuitem", { name: /TVBox \/ FongMi/ }))
    fireEvent.change(await screen.findByLabelText("来源名称"), { target: { value: "客厅电视源" } })
    fireEvent.change(screen.getByLabelText("配置地址"), { target: { value: TVBOX_CONFIG_URL } })
    fireEvent.click(screen.getByRole("button", { name: "测试配置" }))

    expect((await screen.findByRole("alert")).textContent).toContain("演示环境不获取远端配置")
    expect(screen.queryByLabelText("配置摘要")).toBeNull()

    const saveButton = screen.getByRole("button", { name: "保存并导入" }) as HTMLButtonElement
    expect(saveButton.disabled).toBe(true)
    fireEvent.click(saveButton)
    expect(tvboxConfigSave).not.toHaveBeenCalled()
  })

  it("测试通过后保存一次、刷新来源列表，并把 TVBox 来源展示成不可启用的只读行", async () => {
    const listSourcesSpy = vi
      .spyOn(getHavenClient(), "sourceRegistryList")
      .mockResolvedValue(registryWith([tvboxDescriptor()]))
    vi.spyOn(getHavenClient(), "tvboxConfigPreview").mockResolvedValue(tvboxFacts())
    const tvboxConfigSave = vi
      .spyOn(getHavenClient(), "tvboxConfigSave")
      .mockResolvedValue({ schemaVersion: 1, sourceId: TVBOX_SOURCE_ID, preview: tvboxFacts() })
    const setSourceEnabled = vi.spyOn(getHavenClient(), "sourceRegistrySet")

    renderSourcesSettings()
    // 已登记的 TVBox 来源：只读、停用，且没有启用开关。
    expect(await screen.findByText("客厅电视源")).toBeTruthy()
    expect(screen.getByText("外部 · TVBox / FongMi · 单一来源")).toBeTruthy()
    expect(screen.getByText("当前版本不支持启用")).toBeTruthy()
    expect(screen.queryByRole("switch", { name: "客厅电视源 启用" })).toBeNull()
    const listCallsBeforeImport = listSourcesSpy.mock.calls.length

    fireEvent.click(screen.getByRole("button", { name: "添加来源" }))
    fireEvent.click(screen.getByRole("menuitem", { name: /TVBox \/ FongMi/ }))
    fireEvent.change(screen.getByLabelText("来源名称"), { target: { value: "客厅电视源" } })
    fireEvent.change(screen.getByLabelText("配置地址"), { target: { value: TVBOX_CONFIG_URL } })
    fireEvent.click(screen.getByRole("button", { name: "测试配置" }))
    const summary = await screen.findByLabelText("配置摘要")
    // 摘要只渲染聚合计数，配置地址不出现在页面上。
    expect(summary.textContent).toContain("HTTP 端点 1")
    expect(summary.textContent).not.toContain("example.invalid")

    fireEvent.click(screen.getByRole("button", { name: "保存并导入" }))

    await waitFor(() => {
      expect(tvboxConfigSave).toHaveBeenCalledTimes(1)
    })
    expect(tvboxConfigSave).toHaveBeenCalledWith({
      displayName: "客厅电视源",
      url: TVBOX_CONFIG_URL,
    })
    await waitFor(() => {
      expect(listSourcesSpy.mock.calls.length).toBeGreaterThan(listCallsBeforeImport)
    })
    expect(screen.getByText("只读")).toBeTruthy()
    expect(screen.getByText("当前版本不支持启用")).toBeTruthy()
    // 没有任何启用/停用入口，也没有调用过注册表开关。
    expect(screen.queryByRole("menuitem")).toBeNull()
    expect(screen.queryByRole("button", { name: /启用来源|停用来源/ })).toBeNull()
    expect(setSourceEnabled).not.toHaveBeenCalled()
  })
})

describe("来源管理异步交互回归", () => {
  it("初次读取完成前禁用添加，读取后 TVBox 大表单可以立即打开、取消和重开", async () => {
    const initial = deferred<SourceRegistryDto>()
    vi.spyOn(getHavenClient(), "sourceRegistryList").mockReturnValue(initial.promise)
    const save = vi.spyOn(getHavenClient(), "tvboxConfigSave")
    renderSourcesSettings()
    const add = screen.getByRole("button", { name: "添加来源" }) as HTMLButtonElement
    expect(add.disabled).toBe(true)
    await act(async () => { initial.resolve(registryWith([])) })
    expect(add.disabled).toBe(false)

    fireEvent.click(add)
    fireEvent.click(screen.getByRole("menuitem", { name: /TVBox \/ FongMi/ }))
    expect(await screen.findByRole("dialog", { name: "导入 TVBox / FongMi 配置" })).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "取消" }))
    expect(screen.queryByRole("dialog")).toBeNull()
    expect(document.activeElement).toBe(add)
    fireEvent.click(add)
    fireEvent.click(screen.getByRole("menuitem", { name: /TVBox \/ FongMi/ }))
    expect(await screen.findByLabelText("配置地址")).toBeTruthy()
    expect(save).not.toHaveBeenCalled()
  })

  it("添加书库后刷新目录尚未返回时，不卸载凭据步骤或清掉已输入的值", async () => {
    const refresh = deferred<SourceRegistryDto>()
    const list = vi.spyOn(getHavenClient(), "sourceRegistryList")
      .mockResolvedValueOnce(registryWith([])).mockReturnValue(refresh.promise)
    vi.spyOn(getHavenClient(), "sourceAdd").mockResolvedValue({ schemaVersion: 1, sourceId: "custom_opds_0123456789ab" })
    renderSourcesSettings()
    await waitFor(() => { expect((screen.getByRole("button", { name: "添加来源" }) as HTMLButtonElement).disabled).toBe(false) })
    fireEvent.click(screen.getByRole("button", { name: "添加来源" }))
    fireEvent.click(screen.getByRole("menuitem", { name: /OPDS 书库/ }))
    fireEvent.change(screen.getByPlaceholderText("我的 Calibre 书库"), { target: { value: "我的书库" } })
    fireEvent.change(screen.getByPlaceholderText("https://example.org/opds/"), { target: { value: "https://books.example.invalid/opds/" } })
    fireEvent.click(screen.getByRole("button", { name: "继续" }))
    const password = await screen.findByLabelText("访问凭据（可选）") as HTMLInputElement
    expect(list).toHaveBeenCalledTimes(2)
    fireEvent.change(password, { target: { value: "mock-only-not-a-credential" } })
    await act(async () => { refresh.resolve(registryWith([])) })
    expect(screen.getByLabelText("访问凭据（可选）")).toBe(password)
    expect(password.value).toBe("mock-only-not-a-credential")
    fireEvent.click(screen.getByRole("button", { name: "稍后配置" }))
    expect(screen.getByText("来源已准备好")).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "返回来源列表" }))
    expect(screen.queryByRole("dialog")).toBeNull()
  })

  it("已有凭据时空白保存不可用，清除显式发送 null 而不是当前输入", async () => {
    const source: SourceDescriptorDto = {
      ...tvboxDescriptor(), sourceId: "custom_opds_0123456789ab", displayName: "已认证书库",
      categories: ["book"], kinds: ["search", "online_read"], credentialConfigured: true,
    }
    vi.spyOn(getHavenClient(), "sourceRegistryList").mockResolvedValue(registryWith([source]))
    const write = vi.spyOn(getHavenClient(), "sourceSetCredential").mockResolvedValue(undefined)
    renderSourcesSettings()
    fireEvent.click(await screen.findByRole("button", { name: "已认证书库 更多操作" }))
    fireEvent.click(screen.getByRole("menuitem", { name: /配置访问凭据/ }))
    expect((screen.getByRole("button", { name: "保存凭据" }) as HTMLButtonElement).disabled).toBe(true)
    fireEvent.change(screen.getByLabelText(/访问密码（仅写入/), { target: { value: "mock-only-not-a-credential" } })
    fireEvent.click(screen.getByRole("button", { name: "清除凭据" }))
    await waitFor(() => { expect(write).toHaveBeenCalledWith({ sourceId: source.sourceId, secret: null }) })
    await waitFor(() => { expect(screen.queryByRole("button", { name: "清除凭据" })).toBeNull() })
    expect((screen.getByLabelText(/访问密码（仅写入/) as HTMLInputElement).value).toBe("")
  })

  it("更多菜单 Escape 关闭并把焦点还给触发按钮", async () => {
    const source: SourceDescriptorDto = { ...tvboxDescriptor(), sourceId: "custom_feed_0123456789ab", displayName: "新闻订阅", categories: ["periodical"] }
    vi.spyOn(getHavenClient(), "sourceRegistryList").mockResolvedValue(registryWith([source]))
    renderSourcesSettings()
    const trigger = await screen.findByRole("button", { name: "新闻订阅 更多操作" })
    fireEvent.click(trigger)
    expect(screen.getByRole("menu")).toBeTruthy()
    fireEvent.keyDown(document, { key: "Escape" })
    expect(screen.queryByRole("menu")).toBeNull()
    expect(document.activeElement).toBe(trigger)
  })

  it("清除凭据失败显示就地错误，不把已有凭据误标为已清除", async () => {
    const source: SourceDescriptorDto = { ...tvboxDescriptor(), sourceId: "custom_opds_0123456789ab", displayName: "凭据书库", categories: ["book"], credentialConfigured: true }
    vi.spyOn(getHavenClient(), "sourceRegistryList").mockResolvedValue(registryWith([source]))
    vi.spyOn(getHavenClient(), "sourceSetCredential").mockRejectedValue(new HavenError({ code: "KEYRING_UNAVAILABLE", userMessage: "系统凭据服务暂不可用", retryable: true }))
    renderSourcesSettings()
    fireEvent.click(await screen.findByRole("button", { name: "凭据书库 更多操作" }))
    fireEvent.click(screen.getByRole("menuitem", { name: /配置访问凭据/ }))
    fireEvent.click(screen.getByRole("button", { name: "清除凭据" }))
    expect((await screen.findByRole("alert")).textContent).toBe("系统凭据服务暂不可用")
    expect(screen.getByRole("button", { name: "清除凭据" })).toBeTruthy()
  })
})
