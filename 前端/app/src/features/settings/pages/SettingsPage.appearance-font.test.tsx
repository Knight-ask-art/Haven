// @vitest-environment jsdom

// 界面字体设置面板的组件测试（BE-INTERFACE-FONT-001）。
//
// 只挂载外观分区：不触达真实 IPC（gateway 被替换为可断言的替身），也不需要 Router。
// 覆盖：四张互斥卡片、本机字体与导入字体的选择语义、搜索（中文/拼音/英文）、
// 列表行自身字体、空态导入入口、导入/删除/取消/失败路径。

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { HavenError } from "@/lib/ipc/errors"
import type { InterfaceFontAsset } from "@/lib/ipc/interface-font-wire"
import type { AppearanceSettingsValue, SettingsPatch } from "@/lib/ipc/settings-wire"
import { defaultSettingsValue } from "@/lib/ipc/settings-wire"
import { INTERFACE_FONT_MODE_OPTIONS } from "../lib/settingsDisplay"
import { INTERFACE_FONT_PRESET_STACKS } from "../lib/interface-font"
import type { SettingsFormController } from "../lib/useSettingsForm"
import { InterfaceFontSettings } from "./SettingsPage"

const ASSET_ID = "0f8f7a2c-1b3d-4e5f-8a90-1c2d3e4f5a60"
const IMPORTED_ID = "00000000-0000-4000-8000-000000001001"

const importedAsset: InterfaceFontAsset = {
  id: ASSET_ID,
  familyName: "示例展示体",
  fileName: "DemoDisplay.woff2",
  extension: "woff2",
  mimeType: "font/woff2",
  byteSize: 40960,
  createdAt: 1735689600000,
}

const gatewayMock = vi.hoisted(() => ({
  systemList: vi.fn(),
  assetList: vi.fn(),
  assetImport: vi.fn(),
  assetDelete: vi.fn(),
}))

vi.mock("@/features/settings/ipc/interface-font-gateway", () => ({
  interfaceFontGateway: gatewayMock,
}))

vi.mock("@/lib/ipc/runtime", () => ({
  getHavenClient: () => {
    throw new Error("组件测试不提供 IPC 客户端")
  },
  getHavenClientMode: () => "mock",
  isTauriRuntime: () => false,
  resolveTauriRuntime: () => false,
  selectHavenClientMode: () => "mock",
}))

afterEach(cleanup)

beforeEach(() => {
  gatewayMock.systemList.mockReset()
  gatewayMock.assetList.mockReset()
  gatewayMock.assetImport.mockReset()
  gatewayMock.assetDelete.mockReset()
  gatewayMock.systemList.mockResolvedValue([
    { family: "SimSun", localizedFamily: "宋体" },
    { family: "Source Han Serif SC", localizedFamily: "思源宋体" },
    { family: "Inter", localizedFamily: null },
  ])
  gatewayMock.assetList.mockResolvedValue([importedAsset])
})

function appearance(overrides: Partial<AppearanceSettingsValue> = {}): AppearanceSettingsValue {
  const base = defaultSettingsValue("appearance")
  if (base.section !== "appearance") throw new Error("默认值必须是 appearance")
  return { ...base, ...overrides }
}

function controller(value: AppearanceSettingsValue, change: (patch: SettingsPatch) => void): SettingsFormController {
  return {
    section: "appearance",
    state: { status: "ready", saved: value, revision: null },
    displayValue: value,
    isLoading: false,
    isSaving: false,
    isDirty: false,
    hasError: false,
    errorMessage: null,
    change,
    save: () => undefined,
    retry: () => undefined,
    reload: () => undefined,
    resetToDefaults: () => undefined,
  }
}

function renderAppearance(
  value: AppearanceSettingsValue,
): { change: ReturnType<typeof vi.fn>; notices: string[] } {
  const change = vi.fn<(patch: SettingsPatch) => void>()
  const notices: string[] = []
  render(
    <InterfaceFontSettings
      form={controller(value, change)}
      value={value}
      showNotice={(message: string) => notices.push(message)}
    />,
  )
  return { change, notices }
}

function modeCard(label: string): HTMLButtonElement {
  const group = within(screen.getByRole("group", { name: "界面字体模式" }))
  const card = group.getByRole("button", { name: new RegExp(label) })
  return card as HTMLButtonElement
}

describe("界面字体卡片", () => {
  it("渲染四张互斥卡片，并标出当前选中项", () => {
    renderAppearance(appearance({ interfaceFontMode: "serif" }))
    for (const option of INTERFACE_FONT_MODE_OPTIONS) {
      expect(modeCard(option.label)).toBeTruthy()
    }
    expect(modeCard("人文衬线").getAttribute("aria-pressed")).toBe("true")
    expect(modeCard("跟随系统").getAttribute("aria-pressed")).toBe("false")
  })

  it("点击预设卡片只写 interfaceFontMode（不改动已保存的自定义选择）", () => {
    const { change } = renderAppearance(
      appearance({ interfaceFontMode: "custom", interfaceFontFamily: "SimSun" }),
    )
    fireEvent.click(modeCard("现代黑体"))
    expect(change).toHaveBeenCalledWith({ section: "appearance", interfaceFontMode: "sans" })
  })

  it("自定义字体卡片明确表示可展开，并控制字体选择面板", () => {
    renderAppearance(appearance({ interfaceFontMode: "custom" }))
    const card = modeCard("自定义系统字体")
    expect(card.getAttribute("aria-expanded")).toBe("true")
    expect(card.getAttribute("aria-controls")).toBe("interface-font-selection-panel")
    expect(document.getElementById("interface-font-selection-panel")?.hidden).toBe(false)
  })

  it("system 模式下不展开自定义面板（不请求本机字体）", () => {
    renderAppearance(appearance({ interfaceFontMode: "system" }))
    expect(screen.queryByLabelText("搜索字体")).toBeNull()
    expect(modeCard("自定义系统字体").getAttribute("aria-expanded")).toBe("false")
    expect(document.getElementById("interface-font-selection-panel")?.hidden).toBe(true)
    expect(gatewayMock.systemList).not.toHaveBeenCalled()
  })
})

describe("自定义选择", () => {
  it("选择本机字体时同时清除导入字体引用", async () => {
    const { change } = renderAppearance(
      appearance({ interfaceFontMode: "custom", interfaceFontAssetId: ASSET_ID }),
    )
    fireEvent.click(await screen.findByText("SimSun"))
    expect(change).toHaveBeenCalledWith({
      section: "appearance",
      interfaceFontMode: "custom",
      interfaceFontFamily: "SimSun",
      interfaceFontAssetId: "",
    })
  })

  it("选择导入字体时同时清除本机字体引用", async () => {
    const { change } = renderAppearance(appearance({ interfaceFontMode: "custom" }))
    fireEvent.click(await screen.findByText("示例展示体"))
    expect(change).toHaveBeenCalledWith({
      section: "appearance",
      interfaceFontMode: "custom",
      interfaceFontAssetId: ASSET_ID,
      interfaceFontFamily: "",
    })
  })

  it("列表行用该字体自己渲染", async () => {
    renderAppearance(appearance({ interfaceFontMode: "custom" }))
    const row = await screen.findByText("SimSun")
    expect((row as HTMLElement).style.fontFamily.startsWith('"SimSun"')).toBe(true)
    // 中文名作为副标题出现，且不改变行本身字体。
    expect(screen.getByText("宋体")).toBeTruthy()
  })

  it("按英文名、中文名与拼音过滤", async () => {
    renderAppearance(appearance({ interfaceFontMode: "custom" }))
    const search = screen.getByLabelText("搜索字体")
    await screen.findByText("SimSun")

    fireEvent.change(search, { target: { value: "songti" } })
    await waitFor(() => {
      expect(screen.queryByText("Inter")).toBeNull()
    })
    expect(screen.getByText("SimSun")).toBeTruthy()

    fireEvent.change(search, { target: { value: "source han" } })
    await waitFor(() => {
      expect(screen.queryByText("SimSun")).toBeNull()
    })
    expect(screen.getByText("Source Han Serif SC")).toBeTruthy()

    fireEvent.change(search, { target: { value: "思源" } })
    await waitFor(() => {
      expect(screen.queryByText("SimSun")).toBeNull()
    })
    expect(screen.getByText("Source Han Serif SC")).toBeTruthy()
  })

  it("搜索无结果时给出空态与导入入口", async () => {
    renderAppearance(appearance({ interfaceFontMode: "custom" }))
    await screen.findByText("SimSun")
    fireEvent.change(screen.getByLabelText("搜索字体"), { target: { value: "zzz-nothing" } })
    await waitFor(() => {
      expect(screen.getByText(/没有匹配/)).toBeTruthy()
    })
    // 空结果不会让导入区变成死胡同，标题栏入口仍可直接导入。
    expect(within(screen.getByRole("region", { name: "已导入字体" })).getByRole("button", { name: "导入字体" })).toBeTruthy()
  })

  it("资产与系统字体都为空时，空态明显给出导入动作", async () => {
    gatewayMock.assetList.mockResolvedValue([])
    gatewayMock.systemList.mockResolvedValue([])
    renderAppearance(appearance({ interfaceFontMode: "custom" }))
    await waitFor(() => {
      expect(screen.getByText("尚未导入字体")).toBeTruthy()
    })
    expect(within(screen.getByRole("region", { name: "已导入字体" })).getByRole("button", { name: "导入字体" })).toBeTruthy()
  })

  it("资产列表读取失败时显示原因与重试，而不是伪装成空列表", async () => {
    gatewayMock.assetList.mockRejectedValue(
      new HavenError({ code: "DATABASE_ERROR", userMessage: "查询导入字体失败", retryable: true }),
    )
    renderAppearance(appearance({ interfaceFontMode: "custom" }))
    await waitFor(() => {
      expect(screen.getByText(/字体列表读取失败/)).toBeTruthy()
    })
    fireEvent.click(screen.getByRole("button", { name: "重试" }))
    await waitFor(() => {
      expect(gatewayMock.assetList).toHaveBeenCalledTimes(2)
    })
  })

  it("本机字体枚举失败时显示原因与重试", async () => {
    gatewayMock.systemList.mockRejectedValue(
      new HavenError({ code: "INTERNAL_ERROR", userMessage: "枚举失败", retryable: true }),
    )
    renderAppearance(appearance({ interfaceFontMode: "custom" }))
    await waitFor(() => {
      expect(screen.getByText(/本机字体枚举失败/)).toBeTruthy()
    })
  })
})

describe("导入", () => {
  it("导入成功后刷新列表、选中新字体并提示", async () => {
    const newAsset: InterfaceFontAsset = {
      ...importedAsset,
      id: IMPORTED_ID,
      familyName: "新导入字体",
      fileName: "NewDisplay.ttf",
      extension: "ttf",
      mimeType: "font/ttf",
    }
    gatewayMock.assetImport.mockResolvedValue({ asset: newAsset, deduplicated: false })
    const { change, notices } = renderAppearance(appearance({ interfaceFontMode: "custom" }))
    await screen.findByText("示例展示体")

    fireEvent.click(screen.getByRole("button", { name: "导入字体" }))
    await waitFor(() => {
      expect(change).toHaveBeenCalledWith({
        section: "appearance",
        interfaceFontMode: "custom",
        interfaceFontAssetId: IMPORTED_ID,
        interfaceFontFamily: "",
      })
    })
    expect(await screen.findByText("新导入字体")).toBeTruthy()
    expect(notices.some((message) => message.includes("已导入"))).toBe(true)
  })

  it("重复导入同一份文件时复用既有资产并如实说明", async () => {
    gatewayMock.assetImport.mockResolvedValue({ asset: importedAsset, deduplicated: true })
    const { change, notices } = renderAppearance(appearance({ interfaceFontMode: "custom" }))
    await screen.findByText("示例展示体")

    fireEvent.click(screen.getByRole("button", { name: "导入字体" }))
    await waitFor(() => {
      expect(change).toHaveBeenCalledWith({
        section: "appearance",
        interfaceFontMode: "custom",
        interfaceFontAssetId: ASSET_ID,
        interfaceFontFamily: "",
      })
    })
    expect(notices.some((message) => message.includes("此前已导入"))).toBe(true)
  })

  it("用户取消不提示、不改变草稿", async () => {
    gatewayMock.assetImport.mockRejectedValue(
      new HavenError({ code: "OPERATION_CANCELLED", userMessage: "已取消选择字体文件", retryable: false }),
    )
    const { change, notices } = renderAppearance(appearance({ interfaceFontMode: "custom" }))
    await screen.findByText("示例展示体")

    fireEvent.click(screen.getByRole("button", { name: "导入字体" }))
    await waitFor(() => {
      expect(gatewayMock.assetImport).toHaveBeenCalledTimes(1)
    })
    expect(change).not.toHaveBeenCalled()
    expect(notices).toEqual([])
  })

  it("导入失败时展示后端原因", async () => {
    gatewayMock.assetImport.mockRejectedValue(
      new HavenError({ code: "FONT_FILE_INVALID", userMessage: "字体文件内容与扩展名不匹配", retryable: false }),
    )
    const { notices } = renderAppearance(appearance({ interfaceFontMode: "custom" }))
    await screen.findByText("示例展示体")

    fireEvent.click(screen.getByRole("button", { name: "导入字体" }))
    await waitFor(() => {
      expect(notices.some((message) => message.includes("内容与扩展名不匹配"))).toBe(true)
    })
  })
})

describe("删除", () => {
  it("正在使用的字体禁止删除（按钮禁用），不调用后端", async () => {
    renderAppearance(
      appearance({ interfaceFontMode: "custom", interfaceFontAssetId: ASSET_ID }),
    )
    const button = (await screen.findByLabelText(`删除 ${importedAsset.familyName}`)) as HTMLButtonElement
    expect(button.disabled).toBe(true)
    fireEvent.click(button)
    expect(gatewayMock.assetDelete).not.toHaveBeenCalled()
  })

  it("删除未使用的字体后从列表移除并提示", async () => {
    gatewayMock.assetDelete.mockResolvedValue(undefined)
    const { notices } = renderAppearance(appearance({ interfaceFontMode: "custom" }))
    const button = (await screen.findByLabelText(`删除 ${importedAsset.familyName}`)) as HTMLButtonElement
    expect(button.disabled).toBe(false)

    fireEvent.click(button)
    await waitFor(() => {
      expect(gatewayMock.assetDelete).toHaveBeenCalledWith(ASSET_ID)
    })
    await waitFor(() => {
      expect(screen.queryByText("示例展示体")).toBeNull()
    })
    expect(notices.some((message) => message.includes("已删除"))).toBe(true)
  })

  it("后端拒绝删除（仍被设置引用）时如实展示原因", async () => {
    gatewayMock.assetDelete.mockRejectedValue(
      new HavenError({
        code: "FONT_ASSET_IN_USE",
        userMessage: "该字体仍被界面字体设置引用，请先改选其他字体再删除",
        retryable: false,
      }),
    )
    const { notices } = renderAppearance(appearance({ interfaceFontMode: "custom" }))
    fireEvent.click(await screen.findByLabelText(`删除 ${importedAsset.familyName}`))
    await waitFor(() => {
      expect(notices.some((message) => message.includes("仍被界面字体设置引用"))).toBe(true)
    })
  })
})

describe("预览", () => {
  it("预览与应用端共用同一解析结果（导入字体时给出状态说明）", async () => {
    renderAppearance(
      appearance({ interfaceFontMode: "custom", interfaceFontAssetId: ASSET_ID }),
    )
    const status = await screen.findByTestId("interface-font-status")
    expect(status.textContent).toContain("示例展示体")
    // 预览注入 @font-face，且地址只含 opaque id。
    await waitFor(() => {
      const style = document.getElementById("haven-interface-font-preview-face")
      expect(style?.textContent).toContain(`haven-resource://font/${ASSET_ID}`)
    })
  })

  it("实时预览按所选模式切换实际字体栈", () => {
    const change = vi.fn<(patch: SettingsPatch) => void>()
    const showNotice = vi.fn<(message: string) => void>()
    const renderValue = (value: AppearanceSettingsValue) => (
      <InterfaceFontSettings form={controller(value, change)} value={value} showNotice={showNotice} />
    )
    const { rerender } = render(renderValue(appearance({ interfaceFontMode: "serif" })))
    const title = screen.getByTestId("interface-font-preview-title") as HTMLElement
    const sample = screen.getByTestId("interface-font-preview-sample") as HTMLElement
    expect(title.style.fontFamily).toBe(INTERFACE_FONT_PRESET_STACKS.serif)
    expect(sample.style.fontFamily).toBe(INTERFACE_FONT_PRESET_STACKS.serif)

    rerender(renderValue(appearance({ interfaceFontMode: "sans" })))
    expect(title.style.fontFamily).toBe(INTERFACE_FONT_PRESET_STACKS.sans)
    expect(sample.style.fontFamily).toBe(INTERFACE_FONT_PRESET_STACKS.sans)
  })

  it("引用已删除字体时明确说明已回退", async () => {
    gatewayMock.assetList.mockResolvedValue([])
    renderAppearance(
      appearance({ interfaceFontMode: "custom", interfaceFontAssetId: ASSET_ID }),
    )
    const status = await screen.findByTestId("interface-font-status")
    expect(status.textContent).toContain("已不存在")
    expect(document.getElementById("haven-interface-font-preview-face")).toBeNull()
  })
})
