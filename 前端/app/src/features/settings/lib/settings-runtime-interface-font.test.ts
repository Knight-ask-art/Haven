// @vitest-environment jsdom

// 界面字体的全局消费者测试（BE-INTERFACE-FONT-001）。
//
// 钉住「设置 → 文档」这一段：applyInterfaceFont 把草稿/已保存值落成
// documentElement 上的字体标记与 @font-face 宿主元素；
// 以及 loadSettingsRuntimeSnapshot 会把导入资产一起读入投影。

import { afterEach, describe, expect, it, vi } from "vitest"

import type { InterfaceFontAsset } from "../../../lib/ipc/interface-font-wire"
import type { AppearanceSettingsValue, SettingsSnapshot } from "../../../lib/ipc/settings-wire"
import { defaultSettingsValue } from "../../../lib/ipc/settings-wire"
import {
  applyInterfaceFont,
  captureSettingsRuntimeEpochs,
  getSettingsRuntimeSnapshot,
  loadSettingsRuntimeSnapshot,
  publishInterfaceFontAssets,
  publishLoadedSettingsRuntime,
  publishSettingsRuntimeSnapshot,
} from "./settings-runtime-state"

const ASSET_ID = "0f8f7a2c-1b3d-4e5f-8a90-1c2d3e4f5a60"

const asset: InterfaceFontAsset = {
  id: ASSET_ID,
  familyName: "示例展示体",
  fileName: "DemoDisplay.woff2",
  extension: "woff2",
  mimeType: "font/woff2",
  byteSize: 40960,
  createdAt: 1735689600000,
}

function appearance(overrides: Partial<AppearanceSettingsValue> = {}): AppearanceSettingsValue {
  const base = defaultSettingsValue("appearance")
  if (base.section !== "appearance") throw new Error("默认值必须是 appearance")
  return { ...base, ...overrides }
}

afterEach(() => {
  document.documentElement.removeAttribute("style")
  document.documentElement.removeAttribute("data-haven-interface-font")
  document.getElementById("haven-interface-font-face")?.remove()
})

describe("applyInterfaceFont", () => {
  it("system 模式不覆盖字体栈，也不注入 @font-face", () => {
    applyInterfaceFont(appearance({ interfaceFontMode: "system" }), [])
    expect(document.documentElement.dataset.havenInterfaceFont).toBe("system")
    expect(document.getElementById("haven-interface-font-face")).toBeNull()
  })

  it("预设模式记录预设标识，且不注入 @font-face", () => {
    applyInterfaceFont(appearance({ interfaceFontMode: "serif" }), [])
    expect(document.documentElement.dataset.havenInterfaceFont).toBe("preset:serif")
    expect(document.getElementById("haven-interface-font-face")).toBeNull()
  })

  it("导入字体注入一条只含 opaque 地址的 @font-face", () => {
    applyInterfaceFont(appearance({ interfaceFontMode: "custom", interfaceFontAssetId: ASSET_ID }), [asset])
    expect(document.documentElement.dataset.havenInterfaceFont).toBe(`asset:${ASSET_ID}`)
    const style = document.getElementById("haven-interface-font-face")
    expect(style).not.toBeNull()
    expect(style?.textContent).toContain(`haven-resource://font/${ASSET_ID}`)
    expect(style?.textContent).toContain("@font-face")
    // 展示用的文件名不得进入 CSS。
    expect(style?.textContent ?? "").not.toContain("DemoDisplay")
  })

  it("切回 system 会移除 @font-face，切换资产会改写它", () => {
    applyInterfaceFont(appearance({ interfaceFontMode: "custom", interfaceFontAssetId: ASSET_ID }), [asset])
    expect(document.querySelectorAll("#haven-interface-font-face")).toHaveLength(1)

    applyInterfaceFont(appearance({ interfaceFontMode: "system" }), [asset])
    expect(document.getElementById("haven-interface-font-face")).toBeNull()

    const other = "1a2b3c4d-5e6f-4a8b-9c0d-1e2f3a4b5c60"
    applyInterfaceFont(appearance({ interfaceFontMode: "custom", interfaceFontAssetId: other }), [
      asset,
      { ...asset, id: other },
    ])
    const style = document.getElementById("haven-interface-font-face")
    expect(style?.textContent).toContain(other)
    expect(document.querySelectorAll("#haven-interface-font-face")).toHaveLength(1)
  })

  it("stale 资产 id 不产生任何字体或 @font-face", () => {
    applyInterfaceFont(appearance({ interfaceFontMode: "custom", interfaceFontAssetId: ASSET_ID }), [])
    expect(document.documentElement.dataset.havenInterfaceFont).toBe(`stale-asset:${ASSET_ID}`)
    expect(document.getElementById("haven-interface-font-face")).toBeNull()
  })

  // jsdom/cssstyle 对自定义属性的序列化支持并不完整，因此这里对 `--ds-font-interface`
  // 用「已写入或已移除」的双向断言；字体栈本身的值由 interface-font.test.ts
  // 对 resolveInterfaceFont 的严格断言覆盖，真实渲染在 Windows/Tauri 运行时验证。
  it("写入与移除 --ds-font-interface 与字体选择一致", () => {
    applyInterfaceFont(appearance({ interfaceFontMode: "serif" }), [])
    const declared = document.documentElement.style.getPropertyValue("--ds-font-interface")
    const inline = document.documentElement.getAttribute("style") ?? ""
    expect(declared.length > 0 || inline.includes("--ds-font-interface")).toBe(true)

    applyInterfaceFont(appearance({ interfaceFontMode: "system" }), [])
    const afterRemoval = document.documentElement.style.getPropertyValue("--ds-font-interface")
    const inlineAfter = document.documentElement.getAttribute("style") ?? ""
    expect(afterRemoval === "" && !inlineAfter.includes("--ds-font-interface")).toBe(true)
  })
})

describe("运行时投影", () => {
  it("发布资产列表后进入投影，且相同 id 的重复发布不改变引用", () => {
    publishInterfaceFontAssets([asset])
    const first = getSettingsRuntimeSnapshot().interfaceFontAssets
    expect(first.map((entry) => entry.id)).toEqual([ASSET_ID])

    publishInterfaceFontAssets([{ ...asset }])
    expect(getSettingsRuntimeSnapshot().interfaceFontAssets).toBe(first)

    publishInterfaceFontAssets([])
    expect(getSettingsRuntimeSnapshot().interfaceFontAssets).toEqual([])
  })

  it("发布设置值不会丢掉资产投影", () => {
    publishInterfaceFontAssets([asset])
    publishSettingsRuntimeSnapshot({
      general: getSettingsRuntimeSnapshot().general,
      appearance: appearance({ interfaceFontMode: "serif" }),
      interfaceFontAssets: getSettingsRuntimeSnapshot().interfaceFontAssets,
    })
    const snapshot = getSettingsRuntimeSnapshot()
    expect(snapshot.appearance.interfaceFontMode).toBe("serif")
    expect(snapshot.interfaceFontAssets.map((entry) => entry.id)).toEqual([ASSET_ID])
  })

  it("首屏加载不会覆盖加载期间刚发布的新字体资产", () => {
    publishInterfaceFontAssets([])
    const initial = getSettingsRuntimeSnapshot()
    const epochsAtRequestStart = captureSettingsRuntimeEpochs()
    publishInterfaceFontAssets([asset])

    publishLoadedSettingsRuntime(
      {
        general: initial.general,
        appearance: initial.appearance,
        interfaceFontAssets: [],
      },
      epochsAtRequestStart,
    )

    expect(getSettingsRuntimeSnapshot().interfaceFontAssets.map((entry) => entry.id)).toEqual([ASSET_ID])
  })

  it("loadSettingsRuntimeSnapshot 一并读取导入资产", async () => {
    const general = defaultSettingsValue("general")
    const appearanceValue = appearance({ interfaceFontMode: "custom", interfaceFontAssetId: ASSET_ID })
    const gateway = {
      settingsGet: vi.fn(async (section: string): Promise<SettingsSnapshot> => {
        if (section === "general") return { value: general, revision: null }
        return { value: appearanceValue, revision: "set-1" }
      }),
    }
    const fonts = { assetList: vi.fn(async () => [asset]) }

    const loaded = await loadSettingsRuntimeSnapshot(gateway, fonts)
    expect(loaded.degraded).toBe(false)
    expect(loaded.snapshot.appearance.interfaceFontAssetId).toBe(ASSET_ID)
    expect(loaded.snapshot.interfaceFontAssets.map((entry) => entry.id)).toEqual([ASSET_ID])
  })

  it("资产读取失败按空列表处理，且不把整个 Shell 判成 degraded", async () => {
    const general = defaultSettingsValue("general")
    const gateway = {
      settingsGet: vi.fn(async (section: string): Promise<SettingsSnapshot> => ({
        value: section === "general" ? general : appearance(),
        revision: null,
      })),
    }
    const fonts = {
      assetList: vi.fn(async () => {
        throw new Error("boom")
      }),
    }
    const loaded = await loadSettingsRuntimeSnapshot(gateway, fonts)
    expect(loaded.degraded).toBe(false)
    expect(loaded.snapshot.interfaceFontAssets).toEqual([])
  })

  it("设置读取失败回退默认值并标记 degraded", async () => {
    const gateway = {
      settingsGet: vi.fn(async () => {
        throw new Error("boom")
      }),
    }
    const fonts = { assetList: vi.fn(async () => []) }
    const loaded = await loadSettingsRuntimeSnapshot(gateway, fonts)
    expect(loaded.degraded).toBe(true)
    expect(loaded.snapshot.appearance.interfaceFontMode).toBe("system")
  })
})
