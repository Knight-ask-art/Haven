// 界面字体应用逻辑测试（BE-INTERFACE-FONT-001）。
//
// 覆盖：预设栈、导入字体 @font-face 与受控资源地址、stale/非法选择的拒绝、
// 列表行自身字体、搜索（英文 + 中文 + 拼音）。

import { describe, expect, it } from "vitest"

import type { InterfaceFontAsset, InterfaceFontFamily } from "../../../lib/ipc/interface-font-wire"
import {
  describeResolvedInterfaceFont,
  filterFontFamilies,
  filterInterfaceFontAssets,
  fontFamilyPreviewStyle,
  importedFontCssFamily,
  INTERFACE_FONT_PRESET_STACKS,
  resolveInterfaceFont,
} from "./interface-font"
import { fontSearchKeys, normalizeFontQuery } from "./pinyin"

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

describe("resolveInterfaceFont", () => {
  it("system 不覆盖既有字体栈", () => {
    const resolved = resolveInterfaceFont({ interfaceFontMode: "system" }, [])
    expect(resolved.fontFamily).toBeNull()
    expect(resolved.fontFaceCss).toBeNull()
    expect(resolved.appliedKey).toBe("system")
  })

  it("预设返回对应字体栈且不需要 @font-face", () => {
    const sans = resolveInterfaceFont({ interfaceFontMode: "sans" }, [])
    expect(sans.fontFamily).toBe(INTERFACE_FONT_PRESET_STACKS.sans)
    expect(sans.fontFaceCss).toBeNull()
    expect(sans.appliedKey).toBe("preset:sans")

    const serif = resolveInterfaceFont({ interfaceFontMode: "serif" }, [])
    expect(serif.fontFamily).toBe(INTERFACE_FONT_PRESET_STACKS.serif)
    expect(serif.appliedKey).toBe("preset:serif")
  })

  it("custom + 本机族名生成带兜底的字体栈", () => {
    const resolved = resolveInterfaceFont(
      { interfaceFontMode: "custom", interfaceFontFamily: "SimSun" },
      [],
    )
    expect(resolved.fontFamily?.startsWith('"SimSun"')).toBe(true)
    // 兜底必须存在，否则缺字会变成豆腐块。
    expect(resolved.fontFamily).toContain("sans-serif")
    expect(resolved.fontFaceCss).toBeNull()
    expect(resolved.appliedKey).toBe("local:SimSun")
  })

  it("custom 未选择时保持系统字体栈（不伪造字体）", () => {
    const resolved = resolveInterfaceFont({ interfaceFontMode: "custom" }, [])
    expect(resolved.fontFamily).toBeNull()
    expect(resolved.appliedKey).toBe("system")
  })

  it("custom + 导入字体生成 opaque-id 资源地址的 @font-face", () => {
    const resolved = resolveInterfaceFont(
      { interfaceFontMode: "custom", interfaceFontAssetId: ASSET_ID },
      [asset],
    )
    const cssFamily = importedFontCssFamily(ASSET_ID)
    expect(resolved.appliedKey).toBe(`asset:${ASSET_ID}`)
    expect(resolved.fontFamily).toContain(`"${cssFamily}"`)
    expect(resolved.fontFaceCss).toContain(`font-family:"${cssFamily}"`)
    expect(resolved.fontFaceCss).toContain(`haven-resource://font/${ASSET_ID}`)
    expect(resolved.fontFaceCss).toContain('format("woff2")')
    expect(resolved.fontFaceCss).toContain("font-display:swap")
    // 地址与规则里都不得出现文件名或路径。
    expect(resolved.fontFaceCss).not.toContain("DemoDisplay")
    expect(resolved.fontFaceCss).not.toContain("\\")
  })

  it("Windows 兼容形态由 resolveUri 注入，不硬编码在规则里", () => {
    const resolved = resolveInterfaceFont(
      { interfaceFontMode: "custom", interfaceFontAssetId: ASSET_ID },
      [asset],
      (uri) => uri.replace("haven-resource://", "http://haven-resource."),
    )
    expect(resolved.fontFaceCss).toContain(`http://haven-resource.font/${ASSET_ID}`)
  })

  it("ttf / otf 使用各自 format()", () => {
    const ttf = resolveInterfaceFont(
      { interfaceFontMode: "custom", interfaceFontAssetId: ASSET_ID },
      [{ ...asset, extension: "ttf", mimeType: "font/ttf" }],
    )
    expect(ttf.fontFaceCss).toContain('format("truetype")')
    const otf = resolveInterfaceFont(
      { interfaceFontMode: "custom", interfaceFontAssetId: ASSET_ID },
      [{ ...asset, extension: "otf", mimeType: "font/otf" }],
    )
    expect(otf.fontFaceCss).toContain('format("opentype")')
  })

  it("已删除的 stale 资产 id 被拒绝应用（回退系统字体并可被识别）", () => {
    const resolved = resolveInterfaceFont(
      { interfaceFontMode: "custom", interfaceFontAssetId: ASSET_ID },
      [],
    )
    expect(resolved.fontFamily).toBeNull()
    expect(resolved.fontFaceCss).toBeNull()
    expect(resolved.appliedKey).toBe(`stale-asset:${ASSET_ID}`)
    expect(describeResolvedInterfaceFont(resolved, [])).toContain("已不存在")
  })

  it("非规范资产 id 同样按 stale 处理", () => {
    const resolved = resolveInterfaceFont(
      { interfaceFontMode: "custom", interfaceFontAssetId: "not-a-uuid" },
      [],
    )
    expect(resolved.appliedKey).toBe("stale-asset:not-a-uuid")
    expect(resolved.fontFamily).toBeNull()
  })

  it("不安全族名不进入 CSS", () => {
    const resolved = resolveInterfaceFont(
      { interfaceFontMode: "custom", interfaceFontFamily: 'Bad";color:red' },
      [],
    )
    expect(resolved.appliedKey).toBe("invalid-family")
    expect(resolved.fontFamily).toBeNull()
    expect(describeResolvedInterfaceFont(resolved, [])).toContain("重新选择有效的字体名称")
  })

  it("非 custom 模式不会应用残留的自定义选择", () => {
    const resolved = resolveInterfaceFont(
      { interfaceFontMode: "serif", interfaceFontAssetId: ASSET_ID, interfaceFontFamily: "SimSun" },
      [asset],
    )
    expect(resolved.appliedKey).toBe("preset:serif")
    expect(resolved.fontFaceCss).toBeNull()
  })
})

describe("列表行自身字体", () => {
  it("合法族名生成内联样式，且带兜底", () => {
    const style = fontFamilyPreviewStyle("SimSun")
    expect(style?.fontFamily.startsWith('"SimSun"')).toBe(true)
    expect(style?.fontFamily).toContain("sans-serif")
  })

  it("非法族名返回 undefined（该行回退默认字体）", () => {
    expect(fontFamilyPreviewStyle('Bad";}')).toBeUndefined()
    expect(fontFamilyPreviewStyle("   ")).toBeUndefined()
  })

  it("导入字体行的族名来自 opaque id，不来自文件名", () => {
    expect(importedFontCssFamily(ASSET_ID)).toBe(`HavenImportedFont${ASSET_ID.replace(/-/g, "")}`)
    expect(importedFontCssFamily(ASSET_ID)).not.toContain("DemoDisplay")
  })
})

describe("字体搜索", () => {
  const families: InterfaceFontFamily[] = [
    { family: "Microsoft YaHei UI", localizedFamily: "微软雅黑" },
    { family: "Source Han Serif SC", localizedFamily: "思源宋体" },
    { family: "SimSun", localizedFamily: "宋体" },
    { family: "Inter", localizedFamily: null },
  ]

  it("空查询返回全部", () => {
    expect(filterFontFamilies(families, "")).toHaveLength(4)
    expect(filterFontFamilies(families, "   ")).toHaveLength(4)
  })

  it("英文名（含大小写与空格归一）可搜", () => {
    const hit = filterFontFamilies(families, "yaheiui")
    expect(hit.map((entry) => entry.family)).toEqual(["Microsoft YaHei UI"])
    expect(filterFontFamilies(families, "SOURCE HAN")).toHaveLength(1)
    expect(filterFontFamilies(families, "inter").map((entry) => entry.family)).toEqual(["Inter"])
  })

  it("中文原名可搜", () => {
    expect(filterFontFamilies(families, "微软").map((entry) => entry.family)).toEqual([
      "Microsoft YaHei UI",
    ])
    expect(filterFontFamilies(families, "宋体").map((entry) => entry.family)).toEqual([
      "Source Han Serif SC",
      "SimSun",
    ])
  })

  it("拼音全拼与首字母可搜", () => {
    // 微软雅黑 → weiruanyahei / wryh
    expect(filterFontFamilies(families, "weiruan").map((entry) => entry.family)).toEqual([
      "Microsoft YaHei UI",
    ])
    expect(filterFontFamilies(families, "wryh").map((entry) => entry.family)).toEqual([
      "Microsoft YaHei UI",
    ])
    // 思源宋体 → siyuansongti / syst
    expect(filterFontFamilies(families, "siyuan").map((entry) => entry.family)).toEqual([
      "Source Han Serif SC",
    ])
  })

  it("罕见汉字也可按原名或拼音检索", () => {
    const rare: InterfaceFontFamily[] = [{ family: "龘", localizedFamily: null }]
    expect(filterFontFamilies(rare, "龘")).toHaveLength(1)
    expect(filterFontFamilies(rare, "da")).toHaveLength(1)
    expect(filterFontFamilies(rare, "d")).toHaveLength(1)
  })

  it("导入字体可用族名、文件名与 opaque id 搜到", () => {
    expect(filterInterfaceFontAssets([asset], "示例")).toHaveLength(1)
    expect(filterInterfaceFontAssets([asset], "demodisplay")).toHaveLength(1)
    expect(filterInterfaceFontAssets([asset], ASSET_ID.slice(0, 8))).toHaveLength(1)
    expect(filterInterfaceFontAssets([asset], "nothing")).toHaveLength(0)
    expect(filterInterfaceFontAssets([asset], "")).toHaveLength(1)
  })
})

describe("拼音检索键", () => {
  it("搜索键同时给出原文、全拼与首字母", () => {
    const keys = fontSearchKeys("微软雅黑")
    expect(keys).toContain("微软雅黑")
    expect(keys).toContain("weiruanyahei")
    expect(keys).toContain("wryh")
  })

  it("混合中英文名同时产出两类键", () => {
    const keys = fontSearchKeys("思源宋体 SC")
    expect(keys).toContain("siyuansongtisc")
    expect(keys).toContain("systsc")
  })

  it("归一化去掉空格与分隔符", () => {
    expect(normalizeFontQuery("Microsoft  YaHei-UI")).toBe("microsoftyaheiui")
    expect(normalizeFontQuery(" Source·Han ")).toBe("sourcehan")
  })
})

describe("describeResolvedInterfaceFont", () => {
  it("对各状态给出如实说明", () => {
    expect(describeResolvedInterfaceFont({ fontFamily: null, fontFaceCss: null, appliedKey: "system" }, [])).toContain("系统默认")
    expect(
      describeResolvedInterfaceFont(
        resolveInterfaceFont({ interfaceFontMode: "custom", interfaceFontAssetId: ASSET_ID }, [asset]),
        [asset],
      ),
    ).toContain("示例展示体")
    expect(
      describeResolvedInterfaceFont(
        resolveInterfaceFont({ interfaceFontMode: "custom", interfaceFontAssetId: ASSET_ID }, []),
        [],
      ),
    ).toContain("已不存在")
  })
})
