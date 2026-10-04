// @ts-expect-error node:fs 不在本工程可解析的模块里（types 里补上 "node" 后即可删除本行）
import { existsSync, readFileSync } from "node:fs"
import { describe, expect, it } from "vitest"
import {
  FEATURE_REGISTRY,
  SETTINGS_NAV_GROUPS,
  SETTINGS_OVERVIEW,
  SETTINGS_REGISTRY,
  entryHasUnbackedFeature,
  isSettingsSectionId,
  productionSettingsSectionIds,
  settingsRegistryEntry,
  unbackedFeatures,
} from "./settings-registry"
import type { FeatureDescriptor, FeatureId } from "./settings-registry"

const FEATURE_ENTRIES = Object.entries(FEATURE_REGISTRY) as Array<[FeatureId, FeatureDescriptor]>

/** 前端源码根（前端/app/src），与进程工作目录无关。 */
const APP_SRC = new URL("../../../", import.meta.url)
/** 仓库根（栖阅/）。 */
const REPO_ROOT = new URL("../../../../../../", import.meta.url)

/**
 * 登记表里写的证据是给人看的字符串（"settingsGateway.settingsGet" 与
 * "features/player/pages/PlayerPage.tsx" 混在一起）。这里把其中**带目录的源码路径**
 * 抽出来，逐个到磁盘上确认真的存在——登记的消费者/绑定不能是一个想象出来的文件。
 *
 * 只抽取含 `/` 且带源码扩展名的 token：像 "gateway.ts" 这种裸文件名会带来假阴性，
 * 与「证据是否真实」无关。
 */
const SOURCE_TOKEN = /[A-Za-z0-9_][A-Za-z0-9_./-]*\.(?:ts|tsx|rs|css)\b/g

function sourceTokens(evidence: string | null): string[] {
  if (!evidence) return []
  return (evidence.match(SOURCE_TOKEN) ?? []).filter((token) => token.includes("/"))
}

function tokenExists(token: string): boolean {
  const [prefix] = token.split("/")
  if (prefix === "features" || prefix === "app" || prefix === "lib" || prefix === "components") {
    return existsSync(new URL(token, APP_SRC))
  }
  if (token.startsWith("crates/")) return existsSync(new URL(token, new URL("后端/", REPO_ROOT)))
  return false
}

describe("settings registry", () => {
  it("registers Google Drive only as the real read-only PDF slice", () => {
    const feature = FEATURE_REGISTRY["storage.googleDrivePdf"]
    expect(feature.status).toBe("implemented")
    expect(feature.binding).toContain("features/settings/ipc/cloud-storage-gateway.ts")
    expect(feature.consumer).toContain("features/settings/components/CloudBrowsePanel.tsx")
    expect(feature.consumer).toContain("crates/haven-application/src/services/session.rs read_cloud")
    expect(feature.note).toContain("只读 PDF")
    expect(feature.note).toContain("未提供上传、同步、离线副本")
    const section = settingsRegistryEntry("storage")
    expect(section?.bindings).toContain("features/settings/ipc/cloud-storage-gateway.ts")
    expect(section?.features).toContain("storage.googleDrivePdf")
    expect(FEATURE_REGISTRY["storage.locations"].consumer).toContain("useStorageLocations.ts")
  })

  it("records the native interface-font channel as the single font binding", () => {
    expect(FEATURE_REGISTRY["appearance.interfaceFont"].binding).toContain(
      "interfaceFontMode, interfaceFontFamily, interfaceFontAssetId",
    )
    expect(FEATURE_REGISTRY["appearance.interfaceFont"].consumer).toContain("applyInterfaceFont")
    expect(FEATURE_REGISTRY["appearance.wallpaper"].binding).toContain(
      "settingsGateway.settingsUpdate(section: appearance, wallpaper)",
    )
  })

  it("exposes exactly one interface-font capability in the appearance section", () => {
    const features = settingsRegistryEntry("appearance")?.features ?? []
    expect(features.filter((id) => id === "appearance.interfaceFont")).toHaveLength(1)
    expect(features).not.toContain("appearance.fontAsset")
    expect(features).not.toContain("appearance.uiFont")
  })

  it("hides the retired home layout while preserving its persistence contract", () => {
    const feature = FEATURE_REGISTRY["appearance.homeLayout"]
    expect(feature.status).toBe("absent")
    expect(feature.consumer).toBeNull()
    expect(feature.binding).toContain("homeLayoutGet/homeLayoutSave/homeLayoutReset")
    expect(settingsRegistryEntry("appearance")?.features).not.toContain("appearance.homeLayout")
    expect(settingsRegistryEntry("appearance")?.hiddenFeatures).toContain("appearance.homeLayout")
    expect(FEATURE_REGISTRY["appearance.overviewLayout"].status).toBe("implemented")
  })

  it("registers the RSS/Atom source family as an implemented capability", () => {
    // Task 1：订阅源有真实 gateway 绑定（source_add kind=feed / source_registry_list
    // / source_update / source_remove）与真实页面消费者，因此登记为 implemented，
    // 而不是留在未实现列表里当占位。
    const feature = FEATURE_REGISTRY["sources.feedSubscription"]
    expect(feature.status).toBe("implemented")
    expect(feature.binding).toContain("features/settings/ipc/sources-gateway.ts")
    expect(feature.binding).toContain("addSource")
    expect(feature.consumer).toContain("features/settings/components/SourcesSettings.tsx")
    // 它不能同时出现在「没有绑定/没有消费者」的未实现集合里。
    expect(unbackedFeatures()).not.toContain(feature)

    const sources = settingsRegistryEntry("sources")
    expect(sources?.features).toContain("sources.feedSubscription")
    expect(sources?.hiddenFeatures).not.toContain("sources.feedSubscription")
    for (const id of sources?.features ?? []) {
      expect(FEATURE_REGISTRY[id].status, `${id} 以未实现状态出现在来源分区`).not.toBe("absent")
    }
  })

  it("registers the self-hosted comic library family as an implemented capability", () => {
    // Task 2：Komga/Kavita 漫画库有真实 gateway 绑定（source_add kind=komga|kavita
    // / source_registry_list / source_update / source_remove / source_set_credential）
    // 与真实页面消费者，因此登记为 implemented。
    const feature = FEATURE_REGISTRY["sources.comicLibrary"]
    expect(feature.status).toBe("implemented")
    expect(feature.binding).toContain("features/settings/ipc/sources-gateway.ts")
    expect(feature.binding).toContain("addSource")
    expect(feature.consumer).toContain("features/settings/components/SourcesSettings.tsx")
    expect(unbackedFeatures()).not.toContain(feature)

    const sources = settingsRegistryEntry("sources")
    expect(sources?.features).toContain("sources.comicLibrary")
    expect(sources?.hiddenFeatures).not.toContain("sources.comicLibrary")
  })

  it("attaches every implemented capability to a production section", () => {
    // 防假实现：登记为 implemented 的能力必须真的在某个生产分区里呈现。只写进
    // FEATURE_REGISTRY 却没有任何分区消费它，等于把「后端有」说成「用户能用」。
    const presented = new Set(SETTINGS_REGISTRY.flatMap((entry) => entry.features))
    for (const [id, feature] of FEATURE_ENTRIES) {
      if (feature.status !== "implemented") continue
      expect(presented.has(id), `${id} 是 implemented 却没有出现在任何生产分区`).toBe(true)
    }
    // 订阅源能力必须同时具备绑定、消费者与分区归属，否则就是假实现。
    expect(FEATURE_REGISTRY["sources.feedSubscription"].status).toBe("implemented")
    expect(presented.has("sources.feedSubscription")).toBe(true)
  })

  it("docks every claimed source path to a file that really exists", () => {
    const checked: string[] = []
    for (const [id, feature] of FEATURE_ENTRIES) {
      for (const token of [...sourceTokens(feature.binding), ...sourceTokens(feature.consumer)]) {
        // "backend crates/..." 的证据在 Rust 侧，前缀是 crates/。
        expect(tokenExists(token), `${id} 的证据路径不存在：${token}`).toBe(true)
        checked.push(token)
      }
    }
    // 至少覆盖到前端与后端两侧，否则这条测试会退化成恒真。
    expect(checked.some((token) => token.startsWith("features/"))).toBe(true)
    expect(checked.some((token) => token.startsWith("crates/"))).toBe(true)
  })

  it("never claims implemented without both a binding and a consumer", () => {
    for (const [id, feature] of FEATURE_ENTRIES) {
      if (feature.status === "implemented") {
        expect(feature.binding, `${id} 缺少绑定`).toBeTruthy()
        expect(feature.consumer, `${id} 缺少消费者`).toBeTruthy()
        // `implemented` 默认不带缺口说明；带说明时必须是「尚未验收」这一类的诚实补充，
        // 而不是"还缺一半"——那是 `partial` 的语义。
        if (feature.note !== "") {
          expect(
            ["没有", "未", "尚未", "从不"].some((marker) => feature.note.includes(marker)),
            `${id} 的已实现说明没有点名尚未验收的部分：${feature.note}`,
          ).toBe(true)
        }
        continue
      }
      // partial / absent 必须说清楚缺什么。
      expect(feature.note.length, `${id} 必须说明缺口`).toBeGreaterThan(0)
    }
  })

  it("marks every capability that lacks a consumer as non-implemented", () => {
    // 这一组是本次收敛的对象：它们曾经以禁用控件或路线图文案出现在生产设置页上；
    // reading.agentProposal 是后加入的那一个——Agent 通道是真的，但模型服务尚未
    // 接入，提案只能由本机模板脚本生成，因此没有真实消费者。
    const unbacked: FeatureId[] = [
      "general.language",
      "general.notifications",
      "comic.ocr",
      "comic.translation",
      "privacy.proxy",
      "privacy.trackingLimit",
      "privacy.diagnostics",
      "privacy.logRetention",
      "privacy.wipeAll",
      "downloads.meteredNetwork",
      "downloads.quality",
      "reading.agentProposal",
      "sync.cloudBackup",
    ]
    for (const id of unbacked) {
      expect(FEATURE_REGISTRY[id].status, `${id} 必须登记为未实现`).not.toBe("implemented")
    }
    expect(unbackedFeatures().map((feature) => feature.status)).not.toContain("implemented")
  })

  it("keeps the production section list in this file only", () => {
    // settings-runtime-state 曾经自带 PRODUCTION_SETTINGS_SECTIONS / ALL_SETTINGS_SECTIONS
    // 两份白名单，与登记表形成第二套「哪些分区可导航」的真相：两边各写一遍，任何一边
    // 漂移都不会被发现。分区可用性现在只由本文件的 SETTINGS_REGISTRY 回答。
    const source = readFileSync(
      new URL("features/settings/lib/settings-runtime-state.ts", APP_SRC),
      "utf8",
    )
    for (const gone of [
      "PRODUCTION_SETTINGS_SECTIONS",
      "ALL_SETTINGS_SECTIONS",
      "canUseSettingsSection",
      "visibleSettingsSectionIds",
    ]) {
      expect(source, `${gone} 不得再出现在运行时状态模块里`).not.toContain(gone)
    }
    expect(isSettingsSectionId("appearance")).toBe(true)
    expect(isSettingsSectionId("sync")).toBe(false)
  })

  it("keeps未实现的能力 out of the production sections", () => {
    for (const entry of SETTINGS_REGISTRY) {
      expect(entry.status, `${entry.id} 不该以 absent 出现在生产分区里`).not.toBe("absent")
      expect(entryHasUnbackedFeature(entry), `${entry.id} 呈现了没有绑定的能力`).toBe(false)
    }
  })

  it("lists hidden features as registered, absent, and not also presented", () => {
    for (const entry of SETTINGS_REGISTRY) {
      for (const hidden of entry.hiddenFeatures) {
        expect(FEATURE_REGISTRY[hidden], `${hidden} 未登记`).toBeDefined()
        expect(FEATURE_REGISTRY[hidden].status, `${hidden} 被隐藏却不是未实现`).toBe("absent")
        expect([...entry.features], `${entry.id} 不可能既呈现又隐藏 ${hidden}`).not.toContain(hidden)
      }
    }
  })

  it("keeps every registered feature attached to exactly one section", () => {
    const presented = SETTINGS_REGISTRY.flatMap((entry) => [...entry.features, ...entry.hiddenFeatures])
    expect(new Set(presented).size).toBe(presented.length)
    for (const entry of SETTINGS_REGISTRY) {
      for (const id of entry.features) expect(FEATURE_REGISTRY[id], `${id} 未登记`).toBeDefined()
    }
  })

  it("does not expose sync as a settings section", () => {
    const sectionIds: readonly string[] = productionSettingsSectionIds()
    expect(isSettingsSectionId("sync")).toBe(false)
    expect(settingsRegistryEntry("sync")).toBeUndefined()
    expect(sectionIds).not.toContain("sync")
    // 它对应的能力仍然登记在表里，只是状态是未实现。
    expect(FEATURE_REGISTRY["sync.cloudBackup"].status).toBe("absent")
  })

  it("keeps built-in skills inside AI settings instead of a separate settings section", () => {
    const sectionIds: readonly string[] = productionSettingsSectionIds()
    expect(isSettingsSectionId("ai")).toBe(true)
    expect(isSettingsSectionId("skills")).toBe(false)
    expect(settingsRegistryEntry("ai")?.label).toBe("AI 工作台")
    expect(sectionIds).toContain("ai")
    const intelligence = SETTINGS_NAV_GROUPS.find((group) => group.id === "intelligence")
    expect(intelligence?.sectionIds).toEqual(["ai"])
    expect(SETTINGS_NAV_GROUPS.flatMap((group) => group.sectionIds)).not.toContain("skills")
    expect(settingsRegistryEntry("ai")?.features).toContain("skills.builtinRuntime")
    expect(FEATURE_REGISTRY["skills.builtinRuntime"].status).toBe("implemented")
  })

  it("covers every production section in exactly one navigation group", () => {
    const groups = [...SETTINGS_NAV_GROUPS]
    const grouped = groups.flatMap((group) => group.sectionIds)
    expect(grouped.length).toBe(SETTINGS_REGISTRY.length)
    expect([...grouped].sort()).toEqual([...productionSettingsSectionIds()].sort())
    for (const group of groups) expect(group.sectionIds.length).toBeGreaterThan(0)
    for (const entry of SETTINGS_REGISTRY) {
      expect(groups.filter((group) => group.sectionIds.includes(entry.id))).toHaveLength(1)
      expect(groups.find((group) => group.sectionIds.includes(entry.id))?.id).toBe(entry.group)
    }
  })

  it("keeps the overview entry out of the section registry", () => {
    expect(SETTINGS_OVERVIEW.id).toBe("overview")
    const sectionIds: readonly string[] = productionSettingsSectionIds()
    expect(sectionIds).not.toContain(SETTINGS_OVERVIEW.id)
  })

  it("gives every registered section its own renderer instead of a silent General fallback", () => {
    // 曾经 `renderSettingsSection` 的 `default:` 直接返回 `<GeneralSettings>`：登记表里
    // 新增一个分区却忘了加 case 时，用户点开它看到的是通用设置，改的却是别的分区——
    // 一个静默的错误渲染。现在每个登记分区都必须有自己的 case，未知值渲染明确的
    // 「暂未注册渲染器」状态。AI 工作台是完整页面视图，因此由 SettingsContent 的显式分支渲染，
    // 不经过普通设置分区 renderer。
    const source = readFileSync(
      new URL("features/settings/pages/SettingsPage.tsx", APP_SRC),
      "utf8",
    )
    const regions = source.split("function renderSettingsSection(")
    expect(regions.length, "renderSettingsSection 必须存在").toBe(2)
    // 只取 renderSettingsSection 这一个函数体：它的下一个函数就是那个不可用态的组件。
    const [renderer] = (regions[1] ?? "").split("\nexport function SettingsSectionUnavailable(")
    for (const entry of SETTINGS_REGISTRY) {
      if (entry.id === "ai") {
        expect(source).toContain('activeSection === "ai"')
        expect(source).toContain("<AiAssistantDialog")
        continue
      }
      expect(renderer, `${entry.id} 没有自己的 renderer 分支`).toContain(`case "${entry.id}":`)
    }
    // 兜底分支只能是「暂未注册渲染器」，绝不能是通用设置。
    const fallback = renderer.split("default:")[1] ?? ""
    expect(fallback, "未知分区必须渲染不可用态").toContain("SettingsSectionUnavailable")
    expect(fallback, "未知分区不得回落到通用设置").not.toContain("GeneralSettings")
    // 通用设置自己仍然是显式分支，不再兼任兜底。
    expect(renderer).toContain('case "general":')
  })

  it("names every section and capability for the UI", () => {
    for (const entry of SETTINGS_REGISTRY) {
      expect(entry.label.length).toBeGreaterThan(0)
      expect(entry.description.length).toBeGreaterThan(0)
      expect(entry.bindings.length).toBeGreaterThan(0)
      expect(entry.features.length).toBeGreaterThan(0)
    }
    for (const [id, feature] of FEATURE_ENTRIES) {
      expect(feature.label.length, `${id} 缺少名称`).toBeGreaterThan(0)

    }
  })
})
