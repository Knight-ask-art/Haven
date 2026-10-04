import { describe, expect, it } from "vitest"
import {
  filterSettingsNavigationGroups,
  findSettingsFeatureMatches,
  settingsSectionSearchKeywords,
  type SettingsNavigationSearchGroup,
  type SettingsNavigationSearchItem,
} from "./settings-navigation"

type TestItem = SettingsNavigationSearchItem & { id: string }

const GROUPS: SettingsNavigationSearchGroup<TestItem>[] = [
  {
    label: "配置",
    items: [
      { id: "overview", label: "总览", description: "设置摘要与偏好" },
      { id: "appearance", label: "外观", description: "主题、字体与首页" },
    ],
  },
  {
    label: "系统",
    items: [
      { id: "general", label: "通用", description: "应用与交互" },
      { id: "about", label: "关于", description: "版本与致谢" },
    ],
  },
]

describe("settings navigation search", () => {
  it("returns every item for an empty query", () => {
    const result = filterSettingsNavigationGroups(GROUPS, "  ")

    expect(result.map((group) => group.items.map((item) => item.id))).toEqual([
      ["overview", "appearance"],
      ["general", "about"],
    ])
  })

  it("matches labels, descriptions, and group labels", () => {
    expect(filterSettingsNavigationGroups(GROUPS, "字体")[0]?.items.map((item) => item.id)).toEqual(["appearance"])
    expect(filterSettingsNavigationGroups(GROUPS, "系统")[0]?.items.map((item) => item.id)).toEqual(["general", "about"])
    expect(filterSettingsNavigationGroups(GROUPS, "版本")[0]?.items.map((item) => item.id)).toEqual(["about"])
  })

  it("removes empty groups when nothing matches", () => {
    expect(filterSettingsNavigationGroups(GROUPS, "不存在")).toEqual([])
  })

  it("indexes actual feature labels and control aliases, excluding hidden and absent features", () => {
    expect(findSettingsFeatureMatches("API Key")).toEqual(expect.arrayContaining([
      expect.objectContaining({ section: "ai", featureId: "ai.providerCredential" }),
    ]))
    expect(findSettingsFeatureMatches("字号")).toEqual([
      expect.objectContaining({ section: "reading", featureId: "reading.typography" }),
    ])
    expect(findSettingsFeatureMatches("  APIKEY  ")).toEqual(findSettingsFeatureMatches("apikey"))
    expect(findSettingsFeatureMatches("OCR")).toEqual([])
    expect(findSettingsFeatureMatches("云备份")).toEqual([])
    expect(findSettingsFeatureMatches("  ")).toEqual([])
    expect(settingsSectionSearchKeywords("reading")).toContain("字号")
    expect(settingsSectionSearchKeywords("sync")).toEqual([])
  })
})
