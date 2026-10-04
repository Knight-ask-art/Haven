// 界面字体契约层测试（BE-INTERFACE-FONT-001）。
//
// 覆盖：命令 DTO 运行时守卫、opaque id 规范判定、受控资源 URI、
// 族名安全校验、appearance 的 JSON 向后兼容与 patch/CAS 语义。

import { describe, expect, it } from "vitest"

import {
  guardInterfaceFontAsset,
  guardInterfaceFontAssetList,
  guardInterfaceFontFamilyList,
  guardInterfaceFontImportResult,
  INTERFACE_FONT_FAMILY_MAX_CHARS,
  interfaceFontResourceUri,
  isCanonicalInterfaceFontAssetId,
  isSafeInterfaceFontFamily,
} from "./interface-font-wire"
import {
  applySettingsPatch,
  buildSettingsPatch,
  defaultSettingsValue,
  guardSettingsSnapshot,
  guardSettingsValue,
  settingsValuesEqual,
} from "./settings-wire"
import type { AppearanceSettingsValue, AppearancePatchWire } from "./settings-wire"

const ASSET_ID = "0f8f7a2c-1b3d-4e5f-8a90-1c2d3e4f5a60"

function asset(overrides: Record<string, unknown> = {}) {
  return {
    id: ASSET_ID,
    familyName: "示例展示体",
    fileName: "DemoDisplay.woff2",
    extension: "woff2",
    mimeType: "font/woff2",
    byteSize: 40960,
    createdAt: 1735689600000,
    ...overrides,
  }
}

function appearance(overrides: Partial<AppearanceSettingsValue> = {}): AppearanceSettingsValue {
  const base = defaultSettingsValue("appearance")
  if (base.section !== "appearance") throw new Error("默认值必须是 appearance")
  return { ...base, ...overrides }
}

describe("字体命令 DTO 守卫", () => {
  it("接受契约形状", () => {
    expect(guardInterfaceFontAsset(asset())).toBe(true)
    expect(guardInterfaceFontAssetList([asset()])).toBe(true)
    expect(guardInterfaceFontFamilyList([{ family: "Inter", localizedFamily: null }])).toBe(true)
    expect(guardInterfaceFontFamilyList([{ family: "微软雅黑", localizedFamily: "微软雅黑" }])).toBe(true)
    expect(guardInterfaceFontImportResult({ asset: asset(), deduplicated: true })).toBe(true)
  })

  it("拒绝非规范 id / 越界枚举 / 空字节数", () => {
    expect(guardInterfaceFontAsset(asset({ id: "not-a-uuid" }))).toBe(false)
    expect(guardInterfaceFontAsset(asset({ id: ASSET_ID.toUpperCase() }))).toBe(false)
    expect(guardInterfaceFontAsset(asset({ extension: "ttc" }))).toBe(false)
    expect(guardInterfaceFontAsset(asset({ mimeType: "application/octet-stream" }))).toBe(false)
    expect(guardInterfaceFontAsset(asset({ byteSize: 0 }))).toBe(false)
    expect(guardInterfaceFontAsset(asset({ byteSize: 1.5 }))).toBe(false)
    expect(guardInterfaceFontAsset(asset({ familyName: "  " }))).toBe(false)
    expect(guardInterfaceFontAsset(asset({ fileName: "" }))).toBe(false)
    expect(guardInterfaceFontAsset({ ...asset(), path: "C:\\Windows\\Fonts" })).toBe(true)
  })

  it("拒绝缺字段与错误类型", () => {
    expect(guardInterfaceFontAsset(null)).toBe(false)
    expect(guardInterfaceFontAsset({ id: ASSET_ID })).toBe(false)
    expect(guardInterfaceFontAssetList([{ id: ASSET_ID }])).toBe(false)
    expect(guardInterfaceFontImportResult({ asset: asset() })).toBe(false)
    expect(guardInterfaceFontFamilyList([{ family: "" }])).toBe(false)
    expect(guardInterfaceFontFamilyList([{ family: "Inter", localizedFamily: 7 }])).toBe(false)
  })
})

describe("受控资源地址", () => {
  it("只用规范 id 生成 haven-resource://font/<id>", () => {
    expect(interfaceFontResourceUri(ASSET_ID)).toBe(`haven-resource://font/${ASSET_ID}`)
  })

  it("非规范 id 不生成地址（调用方必须回退）", () => {
    expect(interfaceFontResourceUri("../etc/passwd")).toBeNull()
    expect(interfaceFontResourceUri("")).toBeNull()
    expect(interfaceFontResourceUri(ASSET_ID.toUpperCase())).toBeNull()
    expect(isCanonicalInterfaceFontAssetId(ASSET_ID)).toBe(true)
    expect(isCanonicalInterfaceFontAssetId("abc")).toBe(false)
  })

  it("地址里不出现文件名或路径成分", () => {
    expect(interfaceFontResourceUri(ASSET_ID)).not.toContain(".ttf")
    expect(interfaceFontResourceUri(ASSET_ID)).not.toContain("\\")
  })
})

describe("族名安全校验（与 Rust is_safe_interface_font_family 同规则）", () => {
  it("接受常见族名", () => {
    expect(isSafeInterfaceFontFamily("Microsoft YaHei UI")).toBe(true)
    expect(isSafeInterfaceFontFamily(" 思源黑体 ")).toBe(true)
    expect(isSafeInterfaceFontFamily("Noto Sans CJK SC")).toBe(true)
  })

  it("拒绝可逃出 font-family 声明的字符", () => {
    expect(isSafeInterfaceFontFamily("")).toBe(false)
    expect(isSafeInterfaceFontFamily("   ")).toBe(false)
    expect(isSafeInterfaceFontFamily('Bad";color:red')).toBe(false)
    expect(isSafeInterfaceFontFamily("Bad;color:red")).toBe(false)
    expect(isSafeInterfaceFontFamily("Bad{}")).toBe(false)
    expect(isSafeInterfaceFontFamily("Bad\\65scape")).toBe(false)
    expect(isSafeInterfaceFontFamily("a/*b*/")).toBe(false)
    expect(isSafeInterfaceFontFamily("<script>")).toBe(false)
    expect(isSafeInterfaceFontFamily("bad\nnewline")).toBe(false)
    expect(isSafeInterfaceFontFamily("x".repeat(INTERFACE_FONT_FAMILY_MAX_CHARS + 1))).toBe(false)
  })
})

describe("appearance 值守卫（含新增界面字体字段）", () => {
  it("接受缺失的可选选择字段与合法值", () => {
    expect(guardSettingsValue(appearance())).toBe(true)
    expect(
      guardSettingsValue(appearance({ interfaceFontMode: "custom", interfaceFontFamily: "SimSun" })),
    ).toBe(true)
    expect(
      guardSettingsValue(appearance({ interfaceFontMode: "custom", interfaceFontAssetId: ASSET_ID })),
    ).toBe(true)
  })

  it("拒绝未知模式、不安全族名与非规范资产 id", () => {
    expect(guardSettingsValue({ ...appearance(), interfaceFontMode: "comic-sans" })).toBe(false)
    expect(guardSettingsValue({ ...appearance(), interfaceFontMode: undefined })).toBe(false)
    expect(
      guardSettingsValue(appearance({ interfaceFontMode: "custom", interfaceFontFamily: 'x";}' })),
    ).toBe(false)
    expect(
      guardSettingsValue(appearance({ interfaceFontMode: "custom", interfaceFontAssetId: "abc" })),
    ).toBe(false)
  })

  it("旧快照缺少 interfaceFontMode 时按契约拒绝（Rust 始终序列化该字段）", () => {
    const legacy = {
      section: "appearance",
      theme: "dark",
      density: "compact",
      sidebar: "collapsed",
      reduceMotion: true,
    }
    expect(guardSettingsValue(legacy)).toBe(false)
    // 旧行兼容由 Rust 侧的 serde default 负责：前端看到的一定是补齐后的形状。
    expect(defaultSettingsValue("appearance")).toEqual(appearance())
  })
})

describe("appearance patch 语义", () => {
  it("空串表示清除选择，缺字段表示保留", () => {
    const current = appearance({
      interfaceFontMode: "custom",
      interfaceFontFamily: "SimSun",
      interfaceFontAssetId: ASSET_ID,
    })
    const keep = applySettingsPatch(current, { section: "appearance", theme: "dark" })
    expect(keep.section === "appearance" && keep.interfaceFontFamily).toBe("SimSun")
    expect(keep.section === "appearance" && keep.interfaceFontAssetId).toBe(ASSET_ID)

    const cleared = applySettingsPatch(current, {
      section: "appearance",
      interfaceFontFamily: "",
      interfaceFontAssetId: "   ",
    })
    expect(cleared.section === "appearance" && cleared.interfaceFontFamily).toBeUndefined()
    expect(cleared.section === "appearance" && cleared.interfaceFontAssetId).toBeUndefined()
    expect(cleared.section === "appearance" && cleared.interfaceFontMode).toBe("custom")
  })

  it("去除首尾空白后落库", () => {
    const next = applySettingsPatch(appearance({ interfaceFontMode: "custom" }), {
      section: "appearance",
      interfaceFontFamily: "  SimSun  ",
    })
    expect(next.section === "appearance" && next.interfaceFontFamily).toBe("SimSun")
  })

  it("选择字段参与相等比较（否则 dirty 检测会漏掉字体切换）", () => {
    const base = appearance({ interfaceFontMode: "custom", interfaceFontFamily: "SimSun" })
    expect(settingsValuesEqual(base, appearance({ interfaceFontMode: "custom", interfaceFontFamily: "SimSun" }))).toBe(true)
    expect(settingsValuesEqual(base, appearance({ interfaceFontMode: "custom", interfaceFontFamily: "KaiTi" }))).toBe(false)
    expect(settingsValuesEqual(base, appearance({ interfaceFontMode: "serif", interfaceFontFamily: "SimSun" }))).toBe(false)
    expect(
      settingsValuesEqual(
        appearance({ interfaceFontMode: "custom", interfaceFontAssetId: ASSET_ID }),
        appearance({ interfaceFontMode: "custom" }),
      ),
    ).toBe(false)
  })

  it("buildSettingsPatch 只在真的变化时产出字段，清空用空串表达", () => {
    const saved = appearance({ interfaceFontMode: "custom", interfaceFontAssetId: ASSET_ID })
    const untouched = buildSettingsPatch(saved, appearance({ interfaceFontMode: "custom", interfaceFontAssetId: ASSET_ID }))
    expect(untouched).toBeNull()

    const switched = buildSettingsPatch(saved, appearance({ interfaceFontMode: "serif", interfaceFontAssetId: ASSET_ID }))
    expect(switched && "interfaceFontMode" in switched && switched.interfaceFontMode).toBe("serif")

    const clearedSelection = buildSettingsPatch(saved, appearance({ interfaceFontMode: "custom" }))
    expect(clearedSelection && "interfaceFontAssetId" in clearedSelection).toBe(true)
    expect(
      clearedSelection && (clearedSelection as AppearancePatchWire).interfaceFontAssetId,
    ).toBe("")
  })
})

describe("快照守卫", () => {
  it("默认 appearance 快照通过契约守卫", () => {
    const snapshot = { value: defaultSettingsValue("appearance"), revision: null }
    expect(guardSettingsSnapshot(snapshot)).toBe(true)
  })
})
