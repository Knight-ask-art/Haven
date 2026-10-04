// 设置与功能登记表（Settings Registry）。
//
// 这张表回答一个问题：**设置页上的每一项，背后到底有没有真实的数据通道和消费者。**
// 它是导航与分区可见性的唯一依据，也是产品文案的依据——「已实现 / 部分实现 / 未实现」
// 必须与代码里的绑定（gateway 方法、Tauri 命令）和消费者（真正读取这个值的模块）
// 一致，不允许出现「界面上写着未来支持、代码里没有任何消费者」的条目。
//
// 三条硬规则：
//   1. 没有消费者的设置项不出现在生产 UI 里（既不渲染禁用控件，也不写路线图文案）；
//   2. `absent` 的能力只登记在这里，不进导航、不进分区；
//   3. `partial` 必须写清楚缺的是哪一半（缺绑定还是缺消费者）。
//
// 新增设置项时先在这里登记：`status` 与 `features` 对不上时，settings-registry.test.ts
// 会失败——这正是防止文案与绑定再次漂移的那道闸。

import type { BootstrapIconName } from "@/components/ui/haven/BootstrapIcon"

/**
 * 设置分区的 id 词表。
 *
 * 它只是**名字**的封闭集合，不是「可导航分区」的清单：真正决定生产导航的是下面
 * `SETTINGS_REGISTRY` 里登记过的条目（`sync` 留在词表里，但没有任何条目，因此
 * `isSettingsSectionId` 对它是 false）。这里刻意不再维护第二份分区白名单
 * ——那正是本次收敛掉的重复真相。
 */
export type SettingsSectionId =
  | "general"
  | "appearance"
  | "playback"
  | "reading"
  | "comic"
  | "sources"
  | "storage"
  | "downloads"
  | "sync"
  | "ai"
  | "updates"
  | "privacy"
  | "about"

/** 能力状态：实现了 / 只实现了一半 / 没有实现。 */
export type CapabilityStatus = "implemented" | "partial" | "absent"

export type FeatureId =
  // 通用
  | "general.launchPage"
  | "general.restoreSession"
  | "general.language"
  | "general.notifications"
  // 外观
  | "appearance.theme"
  | "appearance.customTheme"
  | "appearance.density"
  | "appearance.sidebar"
  | "appearance.reduceMotion"
  | "appearance.interfaceFont"
  | "appearance.wallpaper"
  | "appearance.homeLayout"
  | "appearance.overviewLayout"
  // 播放
  | "playback.defaultPlaybackRate"
  | "playback.autoResume"
  | "playback.autoNext"
  // 阅读
  | "reading.typography"
  | "reading.customAppearance"
  | "reading.pagination"
  | "reading.overview"
  | "reading.agentProposal"
  // 漫画
  | "comic.layout"
  | "comic.ocr"
  | "comic.translation"
  // 来源与存储
  | "sources.registry"
  | "sources.feedSubscription"
  | "sources.comicLibrary"
  | "storage.locations"
  | "storage.googleDrivePdf"
  // 下载
  | "downloads.concurrency"
  | "downloads.speedLimit"
  | "downloads.autoContinue"
  | "downloads.meteredNetwork"
  | "downloads.quality"
  // 隐私
  | "privacy.searchHistory"
  | "privacy.playbackHistory"
  | "privacy.proxy"
  | "privacy.trackingLimit"
  | "privacy.diagnostics"
  | "privacy.logRetention"
  | "privacy.wipeAll"
  // 系统
  | "updates.application"
  | "updates.sourcePack"
  | "about.diagnostics"
  | "about.directories"
  // AI 与内置技能（A2 AI Provider / A5 外部 Agent Broker；docs/architecture/AI_SYSTEM.md）
  | "ai.providerProfile"
  | "ai.providerCredential"
  | "ai.providerModel"
  | "ai.recommendation"
  | "ai.agentBroker"
  | "ai.clientAutoConfig"
  | "skills.builtinRuntime"
  // 未接入的独立能力
  | "sync.cloudBackup"

export interface FeatureDescriptor {
  label: string
  status: CapabilityStatus
  /** 真实数据通道：gateway 方法或 Tauri 命令；没有绑定时为 null。 */
  binding: string | null
  /** 真实消费者：真正读取这个值的模块；没有消费者时为 null。 */
  consumer: string | null
  /** `partial` / `absent` 必须说明缺的是哪一半。 */
  note: string
}

/**
 * 功能登记表。
 *
 * `binding` 写的是前端唯一允许的入口（gateway 方法），不是裸命令名：设置页从不直接
 * invoke。`consumer` 写的是真正消费这个值的模块——「被保存了」不等于「被消费了」。
 */
export const FEATURE_REGISTRY: Readonly<Record<FeatureId, FeatureDescriptor>> = {
  "general.launchPage": {
    label: "默认启动页",
    status: "implemented",
    binding: "settingsGateway.settingsGet/settingsUpdate(section: general)",
    consumer: "features/settings/lib/settings-runtime-state.ts resolveLaunchRoute",
    note: "",
  },
  "general.restoreSession": {
    label: "恢复上次状态",
    status: "implemented",
    binding: "settingsGateway.settingsGet/settingsUpdate(section: general)",
    consumer: "features/settings/lib/settings-runtime-state.ts useSettingsRuntime",
    note: "",
  },
  "general.language": {
    label: "界面语言",
    status: "absent",
    binding: "settingsGateway.settingsGet/settingsUpdate(section: general)",
    consumer: null,
    note: "值会持久化，但工程里没有任何语言包或 i18n 消费者；生产界面不再展示该控件。",
  },
  "general.notifications": {
    label: "通知",
    status: "absent",
    binding: "settingsGateway.settingsGet/settingsUpdate(section: general)",
    consumer: null,
    note: "值会持久化，但没有统一通知发送者读取它；生产界面不再展示该控件。",
  },
  "appearance.theme": {
    label: "主题",
    status: "implemented",
    binding: "settingsGateway.settingsGet/settingsUpdate(section: appearance)",
    consumer: "features/settings/lib/settings-runtime-state.ts applyTheme",
    note: "",
  },
  "appearance.customTheme": {
    label: "自定义调色板与强调色",
    status: "implemented",
    binding: "settingsGateway.settingsUpdate(section: appearance, customTheme)",
    consumer: "features/settings/lib/appearance-runtime.ts customThemeTokenEntries",
    note: "",
  },
  "appearance.density": {
    label: "界面密度",
    status: "implemented",
    binding: "settingsGateway.settingsUpdate(section: appearance, density)",
    consumer: "app/layouts/AppShell.tsx shellDensityClass",
    note: "",
  },
  "appearance.sidebar": {
    label: "侧栏留白",
    status: "implemented",
    binding: "settingsGateway.settingsUpdate(section: appearance, sidebar)",
    consumer: "app/layouts/AppShell.tsx shellSidebarClass",
    note: "",
  },
  "appearance.reduceMotion": {
    label: "减少动效",
    status: "implemented",
    binding: "settingsGateway.settingsUpdate(section: appearance, reduceMotion)",
    consumer: "app/layouts/AppShell.tsx shellMotionClass / index.css",
    note: "",
  },
  "appearance.interfaceFont": {
    label: "界面字体",
    status: "implemented",
    binding: "settingsGateway.settingsGet/settingsUpdate(section: appearance, interfaceFontMode, interfaceFontFamily, interfaceFontAssetId) + interfaceFontGateway.systemList/assetList/assetImport/assetDelete",
    consumer: "features/settings/lib/settings-runtime-state.ts applyInterfaceFont -> index.css; SettingsPage InterfaceFontSettings",
    note: "",
  },
  "appearance.wallpaper": {
    label: "首页壁纸",
    status: "implemented",
    binding: "appearanceGateway.appearanceAssetsList/appearanceAssetImport/appearanceAssetDelete + settingsGateway.settingsUpdate(section: appearance, wallpaper)",
    consumer: "features/settings/lib/appearance-runtime.ts useAppearanceWallpaper + AppShell",
    note: "",
  },
  "appearance.homeLayout": {
    label: "首页布局",
    status: "absent",
    binding: "appearanceGateway.homeLayoutGet/homeLayoutSave/homeLayoutReset",
    consumer: null,
    note: "首页只保留欢迎区与留白，不再渲染内容模块；已有布局持久化契约保留，生产设置页不展示该编辑器。",
  },
  "appearance.overviewLayout": {
    label: "总览布局",
    status: "implemented",
    binding: "appearanceGateway.overviewLayoutGet/overviewLayoutSave/overviewLayoutReset",
    consumer: "features/settings/pages/SettingsPage.tsx SettingsOverview",
    note: "",
  },
  "playback.defaultPlaybackRate": {
    label: "默认倍速",
    status: "implemented",
    binding: "settingsGateway.settingsGet/settingsUpdate(section: playback)",
    consumer: "features/player/lib/usePlaybackSettings.ts + PlayerPage",
    note: "",
  },
  "playback.autoResume": {
    label: "自动继续",
    status: "implemented",
    binding: "settingsGateway.settingsUpdate(section: playback, autoResume)",
    consumer: "features/player/pages/PlayerPage.tsx",
    note: "",
  },
  "playback.autoNext": {
    label: "自动下一集",
    status: "implemented",
    binding: "settingsGateway.settingsUpdate(section: playback, autoNext)",
    consumer: "features/player/pages/PlayerPage.tsx",
    note: "",
  },
  "reading.typography": {
    label: "阅读排版",
    status: "implemented",
    binding: "settingsGateway + settingsGateway.preferenceGet/preferenceUpdate",
    consumer: "features/reader/lib/reading-settings-mapping.ts",
    note: "",
  },
  "reading.customAppearance": {
    label: "阅读自定义配色与字体",
    status: "implemented",
    binding: "settingsGateway.settingsUpdate(section: reading)",
    consumer: "features/reader/lib/reading-settings-mapping.ts",
    note: "",
  },
  "reading.pagination": {
    label: "阅读模式",
    status: "implemented",
    binding: "settingsGateway.settingsUpdate(section: reading, pagination)",
    consumer: "features/reader/pages/BookReaderPage.tsx",
    note: "",
  },
  "reading.overview": {
    label: "阅读总览",
    status: "implemented",
    binding: "readingOverviewGateway.readingOverviewGet",
    consumer: "features/settings/pages/SettingsPage.tsx SettingsOverview",
    note: "",
  },
  "reading.agentProposal": {
    label: "栖伴设置提案",
    status: "absent",
    binding: "HavenClient.agentSettingsProposalCreate/Approve/Reject",
    consumer: null,
    note: "Agent 通道是真的（提案创建 / 批准 / 拒绝都有 typed IPC），但模型服务尚未接入：提案只能由本机模板脚本生成，也就不存在真实消费者。生产设置页不再提供任何入口，组件与隔离单测保留为开发材料。",
  },
  "comic.layout": {
    label: "漫画阅读方式",
    status: "implemented",
    binding: "settingsGateway + settingsGateway.preferenceGet/preferenceUpdate",
    consumer: "features/comic/lib/comic-reader-model.ts",
    note: "",
  },
  "comic.ocr": {
    label: "OCR 默认语言",
    status: "absent",
    binding: null,
    consumer: null,
    note: "没有 OCR 服务、没有设置消费者；生产界面不再展示该控件。",
  },
  "comic.translation": {
    label: "翻译偏好",
    status: "absent",
    binding: null,
    consumer: null,
    note: "没有翻译服务、没有设置消费者；生产界面不再展示该控件。",
  },
  "sources.registry": {
    label: "来源注册表",
    status: "implemented",
    binding: "features/settings/ipc/sources-gateway.ts（source_registry_*）",
    consumer: "features/settings/components/SourcesSettings.tsx SourcesSettings",
    note: "",
  },
  "sources.feedSubscription": {
    label: "RSS/Atom 订阅源",
    status: "implemented",
    binding:
      "features/settings/ipc/sources-gateway.ts addSource/listSources/isFeedSourceId（source_add kind=feed / source_registry_list / source_update / source_remove）",
    consumer: "features/settings/components/SourcesSettings.tsx SourcesSettings（订阅源添加、编辑、启停与停用凭据入口）",
    note: "订阅源的搜索候选、导入、在线正文会话与离线快照都已接通同一受控 Provider；真实 Windows/Tauri 网络往返尚未验收。",
  },
  "sources.comicLibrary": {
    label: "Komga/Kavita 漫画库",
    status: "implemented",
    binding:
      "features/settings/ipc/sources-gateway.ts addSource/isComicLibrarySourceId（source_add kind=komga|kavita / source_registry_list / source_update / source_remove / source_set_credential）",
    consumer:
      "features/settings/components/SourcesSettings.tsx SourcesSettings（漫画库添加、编辑、启停与 API key 录入）",
    note: "漫画库的搜索候选、作品导入、章节目录、在线逐页读取与离线 CBZ 下载都已接通同一受控 Provider；API key 只经系统凭据库并作为请求头发送。真实 Windows/Tauri 与真实 Komga/Kavita 服务端往返尚未验收。",
  },
  "storage.locations": {
    label: "媒体库位置",
    status: "implemented",
    binding: "features/settings/ipc/storage-gateway.ts（storage_location_* / library_scan_*）",
    consumer: "features/settings/components/LocalStorageLocations.tsx / features/settings/lib/useStorageLocations.ts",
    note: "",
  },
  "storage.googleDrivePdf": {
    label: "Google Drive 只读 PDF",
    status: "implemented",
    binding: "features/settings/ipc/cloud-storage-gateway.ts（账户授权、目录登记、PDF 导入与断开）",
    consumer: "features/settings/components/StorageSettings.tsx / features/settings/components/CloudBrowsePanel.tsx / backend crates/haven-application/src/services/session.rs read_cloud",
    note: "首期只读 PDF，未提供上传、同步、离线副本或其他云盘适配；授权需要应用配置 Google OAuth 客户端。",
  },
  "downloads.concurrency": {
    label: "同时下载数量",
    status: "implemented",
    binding: "settingsGateway.settingsGet/settingsUpdate(section: downloads)",
    consumer: "backend crates/haven-infrastructure/src/download.rs load_download_policy",
    note: "",
  },
  "downloads.speedLimit": {
    label: "下载速度限制",
    status: "implemented",
    binding: "settingsGateway.settingsUpdate(section: downloads, speedLimit)",
    consumer: "backend crates/haven-infrastructure/src/download.rs load_download_policy",
    note: "",
  },
  "downloads.autoContinue": {
    label: "自动继续中断任务",
    status: "implemented",
    binding: "settingsGateway.settingsUpdate(section: downloads, autoContinue)",
    consumer: "backend crates/haven-application/src/services/download.rs",
    note: "",
  },
  "downloads.meteredNetwork": {
    label: "计费网络策略",
    status: "absent",
    binding: null,
    consumer: null,
    note: "没有网络类型感知与消费者；生产界面不再展示该控件。",
  },
  "downloads.quality": {
    label: "默认视频质量",
    status: "absent",
    binding: null,
    consumer: null,
    note: "没有转码/码率选择通道；生产界面不再展示该控件。",
  },
  "privacy.searchHistory": {
    label: "搜索历史开关",
    status: "implemented",
    binding: "settingsGateway.settingsUpdate(section: privacy, searchHistory)",
    consumer: "features/search/ipc/search-history-gateway.ts",
    note: "",
  },
  "privacy.playbackHistory": {
    label: "播放与阅读历史开关",
    status: "implemented",
    binding: "settingsGateway.settingsUpdate(section: privacy, playbackHistory)",
    consumer: "backend crates/haven-application/src/services/history.rs",
    note: "",
  },
  "privacy.proxy": {
    label: "代理模式",
    status: "absent",
    binding: null,
    consumer: null,
    note: "没有出站代理策略消费者；生产界面不再展示该控件。",
  },
  "privacy.trackingLimit": {
    label: "限制网络跟踪",
    status: "absent",
    binding: null,
    consumer: null,
    note: "没有统一出站策略；生产界面不再展示该控件。",
  },
  "privacy.diagnostics": {
    label: "网络诊断信息",
    status: "absent",
    binding: null,
    consumer: null,
    note: "诊断数据只有用户主动触发的错误报告，没有常驻开关；生产界面不再展示该控件。",
  },
  "privacy.logRetention": {
    label: "日志保留时间",
    status: "absent",
    binding: null,
    consumer: null,
    note: "没有独立的日志清理策略消费者；生产界面不再展示该控件。",
  },
  "privacy.wipeAll": {
    label: "清除全部本地数据",
    status: "absent",
    binding: null,
    consumer: null,
    note: "没有对应的后端命令；生产界面不再展示该入口。",
  },
  "updates.application": {
    label: "应用更新",
    status: "implemented",
    binding: "features/settings/ipc/updater-gateway.ts + app-info-gateway.ts",
    consumer: "features/settings/lib/useUpdater.ts + app/layouts/AppShell.tsx",
    note: "",
  },
  "updates.sourcePack": {
    label: "Source Pack 版本",
    status: "implemented",
    binding: "features/settings/ipc/app-info-gateway.ts",
    consumer: "features/settings/lib/useAppInfo.ts",
    note: "",
  },
  "about.diagnostics": {
    label: "运行诊断与环境信息",
    status: "implemented",
    binding: "features/settings/ipc/error-report-gateway.ts",
    consumer: "features/settings/lib/useErrorReport.ts",
    note: "",
  },
  "about.directories": {
    label: "应用目录",
    status: "implemented",
    binding: "features/settings/ipc/app-info-gateway.ts openDirectory",
    consumer: "features/settings/components/SystemSettings.tsx AboutSettings",
    note: "",
  },
  "ai.providerProfile": {
    label: "AI 服务配置行（新建 / 编辑 / 删除）",
    status: "implemented",
    binding:
      "HavenClient.aiProviderProfileList/aiProviderProfileGet/aiProviderProfileUpsert/aiProviderProfileDelete",
    consumer: "features/settings/lib/useAiProviderSettings.ts + SettingsPage.tsx AiSettings",
    note: "配置行的增删改查与 revision CAS 都有网关守卫与单测，但还没有在真实 Windows / WebView2 上操作过（verifiedLive=false）。",
  },
  "ai.providerCredential": {
    label: "API Key 凭据（单向写入 / 清除）",
    status: "implemented",
    binding: "HavenClient.credentialStatus/credentialSet/credentialDelete",
    consumer: "features/settings/lib/useAiProviderSettings.ts + SettingsPage.tsx AiSettings",
    note: "密钥原文只单向提交、不回填、不进任何前端状态；真实系统凭据管理器上的写入与清除尚未验收。",
  },
  "ai.providerModel": {
    label: "默认模型（来自 Provider 模型目录）",
    status: "implemented",
    binding: "HavenClient.aiProviderModelsList/aiProviderProfileUpsert",
    consumer: "features/settings/lib/useAiProviderSettings.ts + SettingsPage.tsx AiSettings",
    note: "模型目录从未对真实 Provider 发过一次请求；目录与能力的呈现只有契约 fixture 证据。",
  },
  "ai.recommendation": {
    label: "设置建议（栖伴）",
    status: "implemented",
    binding: "HavenClient.aiSettingsRecommendationGenerate",
    consumer: "features/settings/components/ai-assistant/AiAssistantDialog.tsx",
    note: "请求体、技能投影与 pending 提案各有单测，但没有对任何真实 Provider 发起过一次请求。",
  },
  "ai.agentBroker": {
    label: "外部 Agent 接入（MCP Broker）",
    status: "implemented",
    binding: "HavenClient.agentBrokerStatus/agentBrokerEnable/agentBrokerDisable",
    consumer: "features/settings/lib/useAgentBrokerSettings.ts + SettingsPage.tsx ExternalAgentAccessSettings",
    note: "Named Pipe / Unix socket 与 Node live 适配器都只有测试替身证据；Haven→Node→MCP 端到端与真实 Codex / Claude Code 客户端均未验收。",
  },
  "ai.clientAutoConfig": {
    label: "一键配置外部 MCP 客户端（Codex / Claude Code）",
    status: "implemented",
    binding: "HavenClient.mcpClientConfigStatus/mcpClientConfigApply",
    consumer: "features/settings/lib/useMcpClientConfig.ts + SettingsPage.tsx McpClientAutoConfigRows",
    note: "结构化写入、备份、原子替换、环境变量重定向 fail closed 与跨平台路径判定都有单测；但没有在真实安装包上跑过一次「写入 → 重启客户端 → 连上栖阅」的往返。",
  },
  "skills.builtinRuntime": {
    label: "内置技能运行时",
    status: "implemented",
    binding: "HavenClient.agentSkillList/agentSkillSetEnabled",
    consumer: "features/settings/lib/useAgentSkills.ts + SettingsPage.tsx AI 设置 / SkillsSettings",
    note: "启用状态与投影有单测；真实桌面 UI 与真实模型是否遵守技能正文都没有证据。",
  },
  "sync.cloudBackup": {
    label: "同步与云备份",
    status: "absent",
    binding: null,
    consumer: null,
    note: "没有同步服务、没有账号体系、没有任何同步通道；生产界面不再提供该分区。",
  },
}

export type SettingsNavGroupId = "configure" | "intelligence" | "resources" | "data" | "system"

export interface SettingsNavGroup {
  id: SettingsNavGroupId
  label: string
  sectionIds: readonly SettingsSectionId[]
}

export interface SettingsRegistryEntry {
  id: SettingsSectionId
  label: string
  description: string
  icon: BootstrapIconName
  group: SettingsNavGroupId
  status: CapabilityStatus
  /** 该分区实际使用的真实通道（写给人看的证据清单）。 */
  bindings: readonly string[]
  /** 该分区在生产 UI 里呈现的能力。 */
  features: readonly FeatureId[]
  /** 明确**不**在该分区呈现的能力（曾经的占位/路线图项）。 */
  hiddenFeatures: readonly FeatureId[]
}

/**
 * 生产设置分区。只登记真实可用的分区——`sync` 不在其中（它对应的能力在
 * FEATURE_REGISTRY 里是 `absent`，导航与路由都不再暴露）；`ai` / `skills` 在 A2/A5
 * 接线切片落地后已经有了真实 typed IPC 与消费者，因此作为生产分区登记进本表。
 */
export const SETTINGS_REGISTRY: readonly SettingsRegistryEntry[] = [
  {
    id: "general",
    label: "通用",
    description: "启动行为与恢复策略",
    icon: "gear",
    group: "system",
    status: "partial",
    bindings: ["settingsGateway.settingsGet/settingsUpdate(section: general)"],
    features: ["general.launchPage", "general.restoreSession"],
    hiddenFeatures: ["general.language", "general.notifications"],
  },
  {
    id: "appearance",
    label: "外观",
    description: "主题、字体、壁纸与布局",
    icon: "palette2",
    group: "configure",
    status: "implemented",
    bindings: [
      "settingsGateway.settingsGet/settingsUpdate(section: appearance)",
      "appearanceGateway.appearanceAssetsList/appearanceAssetImport/appearanceAssetDelete",
      "appearanceGateway.overviewLayoutGet/overviewLayoutSave/overviewLayoutReset",
    ],
    features: [
      "appearance.theme",
      "appearance.customTheme",
      "appearance.density",
      "appearance.sidebar",
      "appearance.reduceMotion",
      "appearance.interfaceFont",
      "appearance.wallpaper",
      "appearance.overviewLayout",
    ],
    hiddenFeatures: ["appearance.homeLayout"],
  },
  {
    id: "reading",
    label: "阅读",
    description: "文字、漫画与版式",
    icon: "book",
    group: "configure",
    status: "implemented",
    bindings: [
      "settingsGateway.settingsGet/settingsUpdate(section: reading)",
      "settingsGateway.preferenceGet/preferenceUpdate",
      "readingOverviewGateway.readingOverviewGet",
    ],
    features: [
      "reading.typography",
      "reading.customAppearance",
      "reading.pagination",
      "reading.overview",
    ],
    hiddenFeatures: ["reading.agentProposal"],
  },
  {
    id: "comic",
    label: "漫画",
    description: "阅读方向与预加载",
    icon: "grid",
    group: "configure",
    status: "partial",
    bindings: ["settingsGateway.settingsGet/settingsUpdate(section: comic)"],
    features: ["comic.layout"],
    hiddenFeatures: ["comic.ocr", "comic.translation"],
  },
  {
    id: "playback",
    label: "播放",
    description: "行为与默认策略",
    icon: "play-circle",
    group: "configure",
    status: "implemented",
    bindings: ["settingsGateway.settingsGet/settingsUpdate(section: playback)"],
    features: ["playback.defaultPlaybackRate", "playback.autoResume", "playback.autoNext"],
    hiddenFeatures: [],
  },
  {
    id: "sources",
    label: "来源",
    description: "连接与健康状态",
    icon: "link-45deg",
    group: "resources",
    status: "implemented",
    bindings: [
      "features/settings/ipc/sources-gateway.ts（source_registry_* / source_add / source_update / source_remove）",
    ],
    features: ["sources.registry", "sources.feedSubscription", "sources.comicLibrary"],
    hiddenFeatures: [],
  },
  {
    id: "downloads",
    label: "下载",
    description: "离线队列与目标",
    icon: "download",
    group: "resources",
    status: "implemented",
    bindings: ["settingsGateway.settingsGet/settingsUpdate(section: downloads)"],
    features: ["downloads.autoContinue", "downloads.concurrency", "downloads.speedLimit"],
    hiddenFeatures: ["downloads.meteredNetwork", "downloads.quality"],
  },
  {
    id: "storage",
    label: "存储",
    description: "媒体库与空间管理",
    icon: "database",
    group: "resources",
    status: "implemented",
    bindings: ["features/settings/ipc/storage-gateway.ts", "features/settings/ipc/cloud-storage-gateway.ts"],
    features: ["storage.locations", "storage.googleDrivePdf"],
    hiddenFeatures: [],
  },
  {
    id: "privacy",
    label: "隐私与网络",
    description: "本机数据边界",
    icon: "shield-lock",
    group: "data",
    status: "implemented",
    bindings: [
      "settingsGateway.settingsGet/settingsUpdate(section: privacy)",
      "features/settings/ipc/privacy-gateway.ts",
    ],
    features: ["privacy.searchHistory", "privacy.playbackHistory"],
    hiddenFeatures: [
      "privacy.proxy",
      "privacy.trackingLimit",
      "privacy.diagnostics",
      "privacy.logRetention",
      "privacy.wipeAll",
    ],
  },
  {
    id: "updates",
    label: "更新",
    description: "版本与更新策略",
    icon: "arrow-clockwise",
    group: "system",
    status: "implemented",
    bindings: [
      "features/settings/ipc/updater-gateway.ts",
      "features/settings/ipc/app-info-gateway.ts",
    ],
    features: ["updates.application", "updates.sourcePack"],
    hiddenFeatures: [],
  },
  {
    id: "about",
    label: "关于",
    description: "版本、目录与致谢",
    icon: "info-circle",
    group: "system",
    status: "implemented",
    bindings: [
      "features/settings/ipc/app-info-gateway.ts",
      "features/settings/ipc/error-report-gateway.ts",
    ],
    features: ["about.diagnostics", "about.directories"],
    hiddenFeatures: [],
  },
  {
    id: "ai",
    label: "AI 工作台",
    description: "对话提案与本机 Agent",
    icon: "stars",
    group: "intelligence",
    status: "implemented",
    bindings: [
      "HavenClient.aiProviderProfileList/aiProviderProfileGet/aiProviderProfileUpsert/aiProviderProfileDelete",
      "HavenClient.aiProviderModelsList",
      "HavenClient.credentialStatus/credentialSet/credentialDelete",
      "HavenClient.aiSettingsRecommendationGenerate",
      "HavenClient.agentBrokerStatus/agentBrokerEnable/agentBrokerDisable",
      "HavenClient.mcpClientConfigStatus/mcpClientConfigApply",
    ],
    features: [
      "ai.providerProfile",
      "ai.providerCredential",
      "ai.providerModel",
      "ai.recommendation",
      "ai.agentBroker",
      "ai.clientAutoConfig",
      "skills.builtinRuntime",
    ],
    hiddenFeatures: [],
  },
]

/** 导航分组；`overview` 是总览，不属于任何登记分区。 */
export const SETTINGS_NAV_GROUPS: readonly SettingsNavGroup[] = [
  { id: "configure", label: "配置", sectionIds: ["appearance", "reading", "comic", "playback"] },
  { id: "intelligence", label: "智能", sectionIds: ["ai"] },
  { id: "resources", label: "资源与存储", sectionIds: ["sources", "downloads", "storage"] },
  { id: "data", label: "数据与连接", sectionIds: ["privacy"] },
  { id: "system", label: "系统", sectionIds: ["updates", "general", "about"] },
]

/** 总览导航项：它不是一个设置分区，只是分区摘要的入口。 */
export const SETTINGS_OVERVIEW = {
  id: "overview",
  label: "总览",
  description: "设置摘要与偏好",
  icon: "grid",
} as const

/** 生产环境可导航的分区 id（顺序即登记顺序）。 */
export function productionSettingsSectionIds(): readonly SettingsSectionId[] {
  return SETTINGS_REGISTRY.map((entry) => entry.id)
}

export function settingsRegistryEntry(id: string): SettingsRegistryEntry | undefined {
  return SETTINGS_REGISTRY.find((entry) => entry.id === id)
}

export function isSettingsSectionId(value: string): value is SettingsSectionId {
  return SETTINGS_REGISTRY.some((entry) => entry.id === value)
}

/** 缺绑定或缺消费者的能力：它们不该出现在生产 UI 上。 */
export function unbackedFeatures(): readonly FeatureDescriptor[] {
  return Object.values(FEATURE_REGISTRY).filter((feature) => feature.status !== "implemented")
}

/** 某分区实际呈现的能力里，是否存在「没有绑定/没有消费者」的项。 */
export function entryHasUnbackedFeature(entry: SettingsRegistryEntry): boolean {
  return entry.features.some((id) => FEATURE_REGISTRY[id].status === "absent")
}
