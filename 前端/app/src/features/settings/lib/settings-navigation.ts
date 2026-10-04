import { FEATURE_REGISTRY, SETTINGS_REGISTRY, type FeatureId, type SettingsSectionId } from "./settings-registry"

export interface SettingsNavigationSearchItem {
  label: string
  description: string
  keywords?: readonly string[]
}

export interface SettingsNavigationSearchGroup<T extends SettingsNavigationSearchItem> {
  label: string
  items: readonly T[]
}

/**
 * Filters the visible Settings navigation without touching settings data.
 * Matching the group label is intentional: searching for "系统" or
 * "资源" should reveal the whole corresponding navigation branch.
 */
export function filterSettingsNavigationGroups<T extends SettingsNavigationSearchItem>(
  groups: readonly SettingsNavigationSearchGroup<T>[],
  query: string,
): Array<{ label: string; items: T[] }> {
  const normalizedQuery = query.trim().toLocaleLowerCase()
  return groups
    .map((group) => ({
      label: group.label,
      items: group.items.filter((item) => {
        if (!normalizedQuery) return true
        return [item.label, item.description, group.label, ...(item.keywords ?? [])].join(" ").toLocaleLowerCase().includes(normalizedQuery)
      }),
    }))
    .filter((group) => group.items.length > 0)
}

/** 搜索别名只描述现有控件；可见能力仍由 Registry 决定，不登记另一份功能清单。 */
const FEATURE_SEARCH_TERMS: Partial<Record<FeatureId, readonly string[]>> = {
  "appearance.interfaceFont": ["界面字体", "自定义字体", "字体导入", "系统字体"],
  "appearance.homeLayout": ["首页模块", "首页布局"],
  "reading.typography": ["默认字体", "字号", "字体大小", "行高", "正文宽度"],
  "reading.customAppearance": ["阅读底色", "页面底色", "正文颜色", "暖光"],
  "reading.pagination": ["阅读模式", "滚动", "分页", "单页", "双页"],
  "comic.layout": ["漫画", "条漫", "阅读方向", "预加载", "页面间距"],
  "sources.registry": ["添加来源", "影视", "TVBox", "FongMi", "接口"],
  "sources.feedSubscription": ["RSS", "Atom", "订阅"],
  "storage.googleDrivePdf": ["Google Drive", "谷歌云盘", "网盘"],
  "downloads.concurrency": ["同时下载数量", "并发"],
  "downloads.speedLimit": ["下载速度限制", "限速"],
  "ai.providerProfile": ["AI 服务", "提供商", "API 地址", "OpenAI Compatible"],
  "ai.providerCredential": ["API Key", "apikey", "密钥"],
  "ai.providerModel": ["默认模型", "模型列表", "识图模型"],
  "ai.agentBroker": ["MCP", "外部 Agent", "接入"],
  "ai.clientAutoConfig": ["MCP", "Codex", "Claude Code", "自动配置"],
  "skills.builtinRuntime": ["Skills", "内置技能"],
}

export interface SettingsFeatureSearchMatch {
  section: SettingsSectionId
  /** 只读阅读统计的实际消费者在总览，不在阅读表单中。 */
  navId?: "overview"
  featureId: FeatureId
  label: string
}

export function settingsSectionSearchKeywords(section: SettingsSectionId): string[] {
  const entry = SETTINGS_REGISTRY.find((item) => item.id === section)
  return (entry?.features ?? []).flatMap((id) =>
    FEATURE_REGISTRY[id].status === "absent" ? [] : [FEATURE_REGISTRY[id].label, ...(FEATURE_SEARCH_TERMS[id] ?? [])],
  )
}

export function findSettingsFeatureMatches(query: string): SettingsFeatureSearchMatch[] {
  const normalized = query.trim().toLocaleLowerCase()
  if (!normalized) return []
  return SETTINGS_REGISTRY.flatMap((entry) => entry.features.flatMap((id) => {
    const feature = FEATURE_REGISTRY[id]
    if (feature.status === "absent") return []
    const terms = [feature.label, ...(FEATURE_SEARCH_TERMS[id] ?? [])]
    if (!terms.some((term) => term.toLocaleLowerCase().includes(normalized))) return []
    return [{ section: entry.id, featureId: id, label: feature.label, ...(id === "reading.overview" ? { navId: "overview" as const } : {}) }]
  }))
}
