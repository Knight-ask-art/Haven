import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react"
import type { CSSProperties, ReactNode } from "react"
import { useNavigate, useParams, useSearchParams } from "react-router"
import { createPortal } from "react-dom"
import {
  Bell,
  BookOpen,
  Check,
  Clipboard,
  ChevronDown,
  ChevronRight,
  CircleCheck,
  FileText,
  Cloud,
  Download,
  Folder,
  Globe2,
  HardDrive,
  History,
  Home,
  Info,
  LockKeyhole,
  MoreHorizontal,
  Moon,
  Palette,
  PanelsTopLeft,
  PlaySquare,
  Plug,
  RefreshCw,
  Search,
  Server,
  Settings2,
  Shield,
  SlidersHorizontal,
  Sparkles,
  Sun,
  TriangleAlert,
} from "lucide-react"
import { cn } from "@/lib/utils"
import { toHavenError, type HavenError } from "@/lib/ipc/errors"
import { getHavenClientMode } from "@/lib/ipc/runtime"
import { deriveLibrarySliceState, deriveScanSliceState, deriveStorageSliceState } from "@/lib/slice-state"
import {
  SCAN_PHASE_LABELS,
  cancelScan,
  listStorageLocations,
  pickLocalDirectory,
  rebindLocalDirectory,
  removeStorageLocation,
  startLibraryScan,
  type StorageLocationWire,
} from "../ipc/storage-gateway"
import type { ComicSettingsValue, GeneralSettingsValue, AppearanceSettingsValue, PlaybackSettingsValue, PreferenceComicPatchWire, PreferenceGetResult, PreferenceReadingPatchWire, PreferenceTargetWire, PrivacySettingsValue, ReadingSettingsValue, DownloadSettingsValue, SettingsValue } from "@/lib/ipc/settings-wire"
import type { SettingsFormController } from "@/features/settings/lib/useSettingsForm"
import { useSettingsForm } from "@/features/settings/lib/useSettingsForm"
import { settingsGateway } from "@/features/settings/ipc/gateway"
import { clearArtworkCache, clearSearchHistory } from "@/features/settings/ipc/privacy-gateway"
import {
  addSource,
  isCustomSourceId,
  listSources,
  removeSource,
  setSourceCredential,
  setSourceEnabled,
  setSourceEndpoint,
  updateSource,
  SOURCE_HEALTH_LABELS,
  type SourceDescriptorWire,
} from "../ipc/sources-gateway"
import type { SourceCategoryDto, SourceKindDto, SourceModeDto, SourceRegistryDto } from "@/lib/ipc/generated/wire"
import {
  SOURCE_CATEGORY_DESCRIPTIONS,
  SOURCE_CATEGORY_LABELS,
  SOURCE_CATEGORY_ORDER,
  SOURCE_KIND_LABELS,
  SOURCE_MODE_DESCRIPTIONS,
  SOURCE_MODE_LABELS,
  sourceMatchesCategory,
  sourceUsesConfiguredEndpoint,
} from "../lib/source-catalog"
import {
  DENSITY_OPTIONS,
  DOWNLOAD_CONCURRENCY_OPTIONS,
  DOWNLOAD_SPEED_LIMIT_OPTIONS,
  LANGUAGE_OPTIONS,
  LAUNCH_PAGE_OPTIONS,
  PLAYBACK_RATE_OPTIONS,
  READING_FONT_OPTIONS,
  READING_FONT_SIZE_OPTIONS,
  READING_LINE_HEIGHT_OPTIONS,
  READING_PAGINATION_OPTIONS,
  READING_THEME_OPTIONS,
  READING_WIDTH_OPTIONS,
  SIDEBAR_OPTIONS,
  THEME_OPTIONS,
  optionLabel,
  optionValue,
} from "@/features/settings/lib/settingsDisplay"
import {
  canUseSettingsSection,
  publishSettingsRuntimeValue,
  type SettingsSectionId,
} from "@/features/settings/lib/settings-runtime-state"
import type { AppDirectoryKindDto } from "@/lib/ipc/generated/wire"
import { useAppInfo } from "@/features/settings/lib/useAppInfo"
import { useUpdater } from "@/features/settings/lib/useUpdater"
import { ERROR_REPORT_LEVEL_LABELS } from "@/features/settings/ipc/error-report-gateway"
import { useErrorReport } from "@/features/settings/lib/useErrorReport"
import { useNotice } from "@/app/notice-center/notice-context"

interface SettingsSection {
  id: SettingsSectionId
  label: string
  description: string
  icon: typeof SlidersHorizontal
}

const SETTINGS_SECTIONS: SettingsSection[] = [
  { id: "general", label: "通用", description: "启动、语言与通知", icon: SlidersHorizontal },
  { id: "appearance", label: "外观", description: "主题、密度与动效", icon: Palette },
  { id: "playback", label: "播放", description: "播放行为与截图", icon: PlaySquare },
  { id: "reading", label: "阅读", description: "字体、版式与阅读主题", icon: BookOpen },
  { id: "comic", label: "漫画", description: "阅读方向与预加载", icon: PanelsTopLeft },
  { id: "sources", label: "来源", description: "来源包与健康状态", icon: Plug },
  { id: "storage", label: "存储", description: "媒体位置与空间管理", icon: HardDrive },
  { id: "downloads", label: "下载", description: "离线位置与队列策略", icon: Download },
  { id: "sync", label: "同步与备份", description: "用户自己的同步目标", icon: Cloud },
  { id: "ai", label: "智能功能", description: "本地配置的 AI Provider", icon: Sparkles },
  { id: "updates", label: "更新", description: "应用与 Source Pack", icon: RefreshCw },
  { id: "privacy", label: "隐私与网络", description: "本地数据与网络行为", icon: Shield },
  { id: "about", label: "关于", description: "版本、许可与路径", icon: Info },
]

const SETTINGS_NAV_GROUPS: Array<{ label: string; sectionIds: SettingsSectionId[] }> = [
  { label: "使用体验", sectionIds: ["general", "appearance", "playback"] },
  { label: "内容阅读", sectionIds: ["reading", "comic"] },
  { label: "资源与存储", sectionIds: ["sources", "downloads", "storage"] },
  { label: "数据与连接", sectionIds: ["sync", "privacy"] },
  { label: "系统", sectionIds: ["updates", "about"] },
]

const MORE_SETTINGS_GROUP = { label: "更多设置", sectionIds: ["ai"] as SettingsSectionId[] }

// playback 仅开放默认倍速与自动继续；截图使用播放器固定快捷键，不是设置事实；
// reading 开放文本类 Reader 的全局排版偏好；
// comic 开放已有 Comic Reader 消费的全局模式、方向、间距和预加载窗口。
// sync/ai/updates 仍不进入 Tauri；
// Downloads 只展示真实服务边界并将未接入策略置为 disabled。General/Appearance/Privacy
// 由 SettingsFormController 驱动，Sources/Storage/About 使用真实 Gateway。
// 注意：本对象不得包含 secret 原文（aiKey 恒为空字符串，只显示 configured/status）。
const DEFAULT_SETTINGS = {
  playbackRate: "1.0x",
  autoNext: true,
  autoResume: true,
  readingFont: "系统无衬线",
  fontSize: "中",
  lineHeight: "舒适",
  readingWidth: "适中",
  readingMode: "连续滚动",
  readingTheme: "跟随系统",
  comicMode: "单页",
  comicDirection: "从右向左",
  pageGap: "12 px",
  preloadPages: "3 页",
  ocrLanguage: "自动识别",
  translation: "关闭",
  // 下载目录由本地 DownloadService/StorageLocation 选择；这里仅保留浏览器 Mock 的
  // 逻辑展示值，不能伪造一个用户机器上的绝对路径。
  downloadPath: "下载 / 栖阅（默认）",
  concurrentDownloads: "3 个任务",
  quality: "自动",
  autoContinueDownloads: true,
  downloadNotifications: true,
  meteredNetwork: "询问",
  speedLimit: "不限速",
  syncEnabled: false,
  syncTarget: "尚未配置",
  syncProgress: true,
  syncFavorites: true,
  aiEnabled: false,
  aiProvider: "OpenAI Compatible",
  aiEndpoint: "https://api.example.com/v1",
  aiKey: "",
  defaultModel: "未配置",
  visionModel: "未配置",
  autoUpdate: true,
  playbackHistory: true,
  networkDiagnostics: false,
  keepLogs: "最近 30 天",
  proxyMode: "系统代理",
  customProxy: "",
  limitTracking: true,
}

type SettingsState = typeof DEFAULT_SETTINGS

type ResourcePreferenceContext = {
  workId: string | null
  editionId: string
  mediaItemId: string
}

function parseResourcePreferenceContext(searchParams: URLSearchParams): ResourcePreferenceContext | null {
  const editionId = searchParams.get("editionId")?.trim()
  const mediaItemId = searchParams.get("mediaItemId")?.trim()
  if (!editionId || !mediaItemId) return null
  return {
    workId: searchParams.get("workId")?.trim() || null,
    editionId,
    mediaItemId,
  }
}

export function SettingsPage() {
  if (getHavenClientMode() === "unavailable") {
    return <SettingsUnavailableState />
  }
  return <SettingsContent />
}

function SettingsUnavailableState() {
  return (
    <div className="flex h-full min-h-0 items-center justify-center bg-[#f5f5f7] px-6 dark:bg-[#000000]">
      <section
        aria-live="polite"
        className="w-full max-w-[560px] rounded-[20px] border border-black/[0.06] bg-white p-8 text-center shadow-[0_20px_60px_rgba(0,0,0,0.06)] dark:border-white/[0.08] dark:bg-[#1c1c1e] dark:shadow-[0_20px_60px_rgba(0,0,0,0.5)]"
      >
        <Settings2 className="mx-auto h-10 w-10 text-[#86868b]" strokeWidth={1.7} />
        <h1 className="mt-4 text-xl font-semibold text-[#1d1d1f] dark:text-[#f5f5f5]">设置暂不可用</h1>
        <p className="mt-2 text-sm leading-6 text-[#6e6e73] dark:text-[#98989d]">
          当前运行环境不支持应用数据访问，请在栖阅桌面应用内打开设置。
        </p>
      </section>
    </div>
  )
}

function SettingsSectionUnavailable() {
  return (
    <div className="flex min-h-[360px] w-full items-center justify-center rounded-3xl border border-black/[0.06] bg-white/70 p-8 text-center shadow-sm dark:border-white/[0.08] dark:bg-[#1c1c1e]/70">
      <div className="max-w-[520px]">
        <TriangleAlert className="mx-auto h-9 w-9 text-[#b7791f]" strokeWidth={1.8} />
        <h2 className="mt-4 text-xl font-semibold">当前版本不可用</h2>
        <p className="mt-2 text-sm leading-6 text-[#6e6e73] dark:text-[#98989d]">
          该设置分区尚未接入栖阅桌面端的数据服务，当前版本不会展示或保存模拟配置。
        </p>
      </div>
    </div>
  )
}

function SettingsContent() {
  const navigate = useNavigate()
  const { section: sectionParam } = useParams<{ section?: string }>()
  const [searchParams] = useSearchParams()
  const runtimeMode = getHavenClientMode()
  const [settings, setSettings] = useState<SettingsState>(DEFAULT_SETTINGS)
  const [searchQuery, setSearchQuery] = useState("")
  const [showMoreSettings, setShowMoreSettings] = useState(false)
  const requestedSection = sectionParam as SettingsSectionId | null
  const activeSection = SETTINGS_SECTIONS.some((section) => section.id === requestedSection)
    ? requestedSection as SettingsSectionId
    : "general"
  const sectionAvailable = canUseSettingsSection(runtimeMode, activeSection)
  const resourceQuery = searchParams.toString()
  const resourceContext = useMemo(() => parseResourcePreferenceContext(new URLSearchParams(resourceQuery)), [resourceQuery])

  const { push } = useNotice()
  const showNotice = useCallback((message: string) => {
    push({ kind: "info", title: "设置", message, dedupeKey: `settings:${message}` })
  }, [push])

  const onFormSaved = (changed: boolean, value: SettingsValue) => {
    // App Shell 只接收已成功写入 Rust Settings 的短生命周期投影；不建立第二事实源。
    publishSettingsRuntimeValue(value)
    if (changed) showNotice("已保存")
  }

  // 已接入分区表单（General / Appearance / Playback / Privacy；FE-SETTINGS-001）。
  // changed=true 才提示"已保存"；changed=false（幂等）静默收敛，不制造假保存状态。
  const generalForm = useSettingsForm("general", settingsGateway, onFormSaved)
  const appearanceForm = useSettingsForm("appearance", settingsGateway, onFormSaved)
  const playbackForm = useSettingsForm("playback", settingsGateway, onFormSaved)
  const readingForm = useSettingsForm("reading", settingsGateway, onFormSaved)
  const comicForm = useSettingsForm("comic", settingsGateway, onFormSaved)
  const downloadsForm = useSettingsForm("downloads", settingsGateway, onFormSaved)
  const privacyForm = useSettingsForm("privacy", settingsGateway, onFormSaved)
  const forms = { general: generalForm, appearance: appearanceForm, playback: playbackForm, reading: readingForm, comic: comicForm, downloads: downloadsForm, privacy: privacyForm }

  if (!sectionAvailable) {
    return <SettingsSectionUnavailable />
  }

  const update = <K extends keyof SettingsState>(key: K, value: SettingsState[K]) => {
    setSettings((current) => ({ ...current, [key]: value }))
  }

  const resetSettings = () => {
    generalForm.resetToDefaults()
    appearanceForm.resetToDefaults()
    playbackForm.resetToDefaults()
    readingForm.resetToDefaults()
    comicForm.resetToDefaults()
    downloadsForm.resetToDefaults()
    privacyForm.resetToDefaults()
    setSettings(DEFAULT_SETTINGS)
    showNotice("已恢复默认设置")
  }

  const reloadSettings = () => {
    generalForm.reload()
    appearanceForm.reload()
    playbackForm.reload()
    readingForm.reload()
    comicForm.reload()
    downloadsForm.reload()
    privacyForm.reload()
    setSettings(DEFAULT_SETTINGS)
    showNotice("已重新加载配置")
  }

  const selectSection = (section: SettingsSectionId) => {
    navigate(`/settings/${section}`, { replace: true })
  }

  const normalizedQuery = searchQuery.trim().toLocaleLowerCase()
  const visibleNavGroups = SETTINGS_NAV_GROUPS.map((group) => ({
    ...group,
    sections: group.sectionIds
      .map((id) => SETTINGS_SECTIONS.find((section) => section.id === id))
      .filter((section): section is SettingsSection => {
        if (!section || !canUseSettingsSection(runtimeMode, section.id)) return false
        if (!normalizedQuery) return true
        return `${section.label} ${section.description} ${group.label}`.toLocaleLowerCase().includes(normalizedQuery)
      }),
  })).filter((group) => group.sections.length > 0)
  const visibleMoreSettings = MORE_SETTINGS_GROUP.sectionIds
    .map((id) => SETTINGS_SECTIONS.find((section) => section.id === id))
    .filter((section): section is SettingsSection => {
      if (!section || !canUseSettingsSection(runtimeMode, section.id)) return false
      if (!normalizedQuery) return true
      return `${section.label} ${section.description} ${MORE_SETTINGS_GROUP.label}`.toLocaleLowerCase().includes(normalizedQuery)
    })
  const isMoreSettingsActive = MORE_SETTINGS_GROUP.sectionIds.includes(activeSection)
  const shouldShowMoreSettings = showMoreSettings || Boolean(normalizedQuery) || isMoreSettingsActive

  return (
    <div className="settings-page h-full min-h-0 overflow-hidden bg-[#f5f5f7] dark:bg-[#000000] text-[#1d1d1f] dark:text-[#f5f5f5]">
      <main className="mx-auto h-full min-h-0 w-full overflow-hidden px-[24px] pb-[16px] pt-[16px] sm:px-[32px]">
        <div className="grid h-full min-h-0 overflow-hidden rounded-[20px] border border-white/90 dark:border-white/10 bg-white/55 dark:bg-[#1c1c1e]/60 shadow-[0_20px_60px_rgba(0,0,0,0.06)] dark:shadow-[0_20px_60px_rgba(0,0,0,0.5)] backdrop-blur-2xl lg:grid-cols-[240px_minmax(0,1fr)]">
          <aside className="flex min-h-0 flex-col border-b border-black/[0.06] dark:border-white/[0.06] bg-white/[0.32] dark:bg-[#1c1c1e]/40 p-[24px] lg:border-b-0 lg:border-r">
            <div className="flex h-[48px] shrink-0 items-center gap-[10px] rounded-xl border border-black/[0.06] dark:border-white/[0.06] bg-white/75 dark:bg-[#2c2c2e]/75 px-[14px] shadow-[0_2px_8px_rgba(0,0,0,0.025)] focus-within:border-[#007aff]/40 focus-within:ring-4 focus-within:ring-[#007aff]/10">
              <Search className="h-[17px] w-[17px] shrink-0 text-[#86868b] dark:text-[#98989d]" strokeWidth={2} />
              <input
                value={searchQuery}
                onChange={(event) => setSearchQuery(event.target.value)}
                placeholder="搜索设置..."
                aria-label="搜索设置"
                className="min-w-0 flex-1 bg-transparent text-[13px] text-[#1d1d1f] dark:text-[#f5f5f5] outline-none placeholder:text-[#a1a1a6] dark:placeholder:text-[#8e8e93]"
              />
              <kbd className="hidden rounded-md bg-black/[0.04] dark:bg-white/[0.08] px-1.5 py-0.5 text-[10px] font-medium text-[#86868b] dark:text-[#98989d] sm:inline">Ctrl K</kbd>
            </div>

            <nav aria-label="设置分类" className="settings-scrollbar-hidden mt-[32px] min-h-0 flex-1 space-y-[20px] overflow-y-auto pr-1">
              {visibleNavGroups.length > 0 ? visibleNavGroups.map((group) => (
                <div key={group.label}>
                  <p className="mb-2 px-2 text-[11px] font-semibold tracking-[0.04em] text-[#86868b]">{group.label}</p>
                  <div className="space-y-0.5">
                    {group.sections.map((section) => {
                      const Icon = section.icon
                      const isActive = section.id === activeSection
                      return (
                        <button
                          key={section.id}
                          type="button"
                          onClick={() => selectSection(section.id)}
                          className={cn(
                            "group flex min-h-[46px] w-full items-center gap-3 rounded-xl px-2.5 py-1.5 text-left transition-colors duration-200",
                            isActive ? "bg-[#007aff]/[0.12] dark:bg-[#007aff]/[0.2] text-[#007aff]" : "text-[#1d1d1f] dark:text-[#f5f5f5] hover:bg-black/[0.04] dark:hover:bg-white/[0.06]"
                          )}
                        >
                          <Icon className={cn("h-[20px] w-[20px] shrink-0", isActive ? "text-[#007aff]" : "text-[#6e6e73] dark:text-[#98989d]")} strokeWidth={1.8} />
                          <span className="min-w-0 flex-1">
                            <span className={cn("block truncate text-[14px]", isActive ? "font-semibold" : "font-medium")}>{section.label}</span>
                            <span className="mt-0.5 block truncate text-[11px] text-[#86868b] dark:text-[#98989d]">{section.description}</span>
                          </span>
                          <ChevronRight className={cn("h-[15px] w-[15px] shrink-0", isActive ? "text-[#007aff]" : "text-[#c7c7cc] dark:text-[#636366]")} />
                        </button>
                      )
                    })}
                  </div>
                </div>
              )) : null}

              {(visibleNavGroups.length === 0 && visibleMoreSettings.length === 0) && (
                <p className="px-2 text-[13px] leading-5 text-[#86868b]">没有找到匹配的设置。</p>
              )}

              {visibleMoreSettings.length > 0 && <div className="pt-1">
                <button type="button" onClick={() => setShowMoreSettings((current) => !current)} className="flex min-h-[36px] w-full items-center justify-between rounded-xl px-2.5 text-left text-[13px] font-medium text-[#6e6e73] dark:text-[#98989d] transition-colors hover:bg-black/[0.04] dark:hover:bg-white/[0.06] hover:text-[#1d1d1f] dark:hover:text-[#f5f5f5]">
                  <span>{MORE_SETTINGS_GROUP.label}</span>
                  <ChevronRight className={cn("h-[15px] w-[15px] transition-transform", shouldShowMoreSettings && "rotate-90")} />
                </button>
                {shouldShowMoreSettings && visibleMoreSettings.length > 0 && (
                  <div className="mt-1 space-y-0.5">
                    {visibleMoreSettings.map((section) => {
                      const Icon = section.icon
                      const isActive = section.id === activeSection
                      return (
                        <button key={section.id} type="button" onClick={() => selectSection(section.id)} className={cn("group flex min-h-[46px] w-full items-center gap-3 rounded-xl px-2.5 py-1.5 text-left transition-colors duration-200", isActive ? "bg-[#007aff]/[0.12] dark:bg-[#007aff]/[0.2] text-[#007aff]" : "text-[#1d1d1f] dark:text-[#f5f5f5] hover:bg-black/[0.04] dark:hover:bg-white/[0.06]")}>
                          <Icon className={cn("h-[20px] w-[20px] shrink-0", isActive ? "text-[#007aff]" : "text-[#6e6e73] dark:text-[#98989d]")} strokeWidth={1.8} />
                          <span className="min-w-0 flex-1"><span className={cn("block truncate text-[14px]", isActive ? "font-semibold" : "font-medium")}>{section.label}</span><span className="mt-0.5 block truncate text-[11px] text-[#86868b] dark:text-[#98989d]">{section.description}</span></span>
                          <ChevronRight className={cn("h-[15px] w-[15px] shrink-0", isActive ? "text-[#007aff]" : "text-[#c7c7cc] dark:text-[#636366]")} />
                        </button>
                      )
                    })}
                  </div>
                )}
              </div>}
            </nav>

            <div className="mt-4 shrink-0 border-t border-black/[0.06] dark:border-white/[0.06] px-2 pb-1 pt-4">
              <p className="text-[11px] font-semibold uppercase tracking-[0.16em] text-[#86868b] dark:text-[#98989d]">Haven Local-first</p>
              <p className="mt-2 text-[12px] leading-relaxed text-[#6e6e73] dark:text-[#8e8e93]">没有中心化 Haven 账户。设置与内容状态默认留在本机。</p>
            </div>
          </aside>

          <section className="settings-scrollbar-hidden min-h-0 min-w-0 flex-1 overflow-y-auto bg-[#f5f5f7]/50 dark:bg-[#000000]/20 p-[20px] pb-[112px] sm:p-[32px] sm:pb-[112px] lg:px-[48px] lg:pt-[40px]">
            <div className="mx-auto flex max-w-[1240px] flex-col items-start gap-[32px] xl:flex-row xl:justify-center xl:gap-[48px]">
              <div className="min-w-0 w-full flex-1 space-y-6 xl:max-w-[860px]">
                {renderSettingsSection(activeSection, settings, update, forms, showNotice, resourceContext)}
              </div>
              <div className="w-full shrink-0 xl:w-[320px]">
                <SettingsSummaryRail activeSection={activeSection} settings={settings} forms={forms} onReset={resetSettings} onReload={reloadSettings} showNotice={showNotice} />
              </div>
            </div>
          </section>
        </div>
      </main>

    </div>
  )
}

function renderSettingsSection(
  section: SettingsSectionId,
  settings: SettingsState,
  update: <K extends keyof SettingsState>(key: K, value: SettingsState[K]) => void,
  forms: { general: SettingsFormController; appearance: SettingsFormController; playback: SettingsFormController; reading: SettingsFormController; comic: SettingsFormController; downloads: SettingsFormController; privacy: SettingsFormController },
  showNotice: (message: string) => void,
  resourceContext: ResourcePreferenceContext | null,
) {
  switch (section) {
    case "appearance":
      return <AppearanceSettings form={forms.appearance} />
    case "playback":
      return <PlaybackSettings form={forms.playback} />
    case "reading":
      return <ReadingSettings form={forms.reading} resourceContext={resourceContext} showNotice={showNotice} />
    case "comic":
      return <ComicSettings form={forms.comic} resourceContext={resourceContext} showNotice={showNotice} />
    case "sources":
      return <SourcesSettings showNotice={showNotice} />
    case "storage":
      return <StorageSettings showNotice={showNotice} />
    case "downloads":
      return <DownloadSettings form={forms.downloads} />
    case "sync":
      return <SyncSettings settings={settings} update={update} />
    case "ai":
      return <AiSettings settings={settings} update={update} showNotice={showNotice} />
    case "updates":
      return <UpdateSettings showNotice={showNotice} />
    case "privacy":
      return <PrivacySettings form={forms.privacy} showNotice={showNotice} />
    case "about":
      return <AboutSettings />
    case "general":
    default:
      return <GeneralSettings form={forms.general} />
  }
}

function SettingsIntro({ section, title, description }: { section: string; title: string; description: string }) {
  return (
    <div aria-label={section} className="px-2 pb-2 pt-2">
      <h2 className="text-[28px] font-bold tracking-[-0.025em] text-[#1d1d1f] dark:text-[#f5f5f5]">{title}</h2>
      <p className="mt-1.5 max-w-2xl text-[14px] leading-6 text-[#6e6e73] dark:text-[#98989d]">{description}</p>
    </div>
  )
}

function SettingsGroup({ title, description, children }: { title: string; description?: string; children: ReactNode }) {
  return (
    <section className="mb-6 last:mb-0">
      <div className="px-2 pb-2">
        <h3 className="text-[11px] font-semibold tracking-[0.12em] uppercase text-[#86868b] dark:text-[#98989d]">{title}</h3>
        {description && <p className="mt-1 text-[12px] leading-5 text-[#86868b] dark:text-[#8e8e93]">{description}</p>}
      </div>
      <div className="overflow-hidden rounded-[14px] border border-black/[0.04] dark:border-white/[0.04] bg-white dark:bg-[#1c1c1e] shadow-[0_2px_8px_rgba(0,0,0,0.02)] dark:shadow-[0_2px_8px_rgba(0,0,0,0.1)]">
        {children}
      </div>
    </section>
  )
}

function SettingRow({ title, description, children, icon, danger = false }: { title: string; description?: string; children: ReactNode; icon?: ReactNode; danger?: boolean }) {
  return (
    <div className="group/row flex min-h-[68px] items-center gap-4 border-b border-black/[0.04] dark:border-white/[0.04] px-6 py-4 transition-colors hover:bg-black/[0.012] dark:hover:bg-white/[0.015] last:border-b-0">
      <div className="flex min-w-0 flex-1 items-center gap-3.5">
        {icon && <span className="flex h-[28px] w-[28px] shrink-0 items-center justify-center rounded-lg bg-black/[0.03] dark:bg-white/[0.04] text-[#86868b] dark:text-[#98989d] group-hover/row:text-[#007aff] transition-colors">{icon}</span>}
        <div className="min-w-0">
          <p className={cn("text-[14px] font-semibold tracking-[-0.005em]", danger ? "text-[#ff3b30]" : "text-[#1d1d1f] dark:text-[#f5f5f5]")}>{title}</p>
          {description && <p className="mt-1 max-w-[520px] text-[12px] leading-[1.65] text-[#86868b] dark:text-[#8e8e93]">{description}</p>}
        </div>
      </div>
      <div className="shrink-0 sm:ml-8">{children}</div>
    </div>
  )
}

type SettingsSummaryItem = {
  label: string
  value: string
  icon: ReactNode
}

function SettingsSummaryRail({ activeSection, settings, forms, onReset, onReload, showNotice }: { activeSection: SettingsSectionId; settings: SettingsState; forms: { general: SettingsFormController; appearance: SettingsFormController; playback: SettingsFormController; reading: SettingsFormController; comic: SettingsFormController; downloads: SettingsFormController; privacy: SettingsFormController }; onReset: () => void; onReload: () => void; showNotice: (message: string) => void }) {
  const sectionLabel = SETTINGS_SECTIONS.find((item) => item.id === activeSection)?.label ?? "设置"
  const items = getSettingsSummaryItems(activeSection, settings, forms)

  return (
    <aside className="hidden min-w-0 xl:block">
      <div className="sticky top-0 space-y-[16px]">
        <div className="rounded-[18px] border border-black/[0.06] dark:border-white/[0.06] bg-white/70 dark:bg-[#1c1c1e]/80 p-[20px] shadow-[0_8px_24px_rgba(0,0,0,0.025)] dark:shadow-[0_8px_24px_rgba(0,0,0,0.2)]">
          <div className="flex items-center justify-between gap-3">
            <h3 className="text-[16px] font-semibold tracking-[-0.02em] text-[#1d1d1f] dark:text-[#f5f5f5]">当前配置</h3>
            <button type="button" onClick={onReset} className="text-[12px] font-semibold text-[#007aff] transition-colors hover:text-[#006fe6]">重置</button>
          </div>
          <p className="mt-[4px] text-[12px] text-[#86868b] dark:text-[#98989d]">{sectionLabel} · 本机保存</p>
          <div className="mt-[18px] space-y-[14px]">
            {items.map((item) => (
              <div key={item.label} className="flex min-w-0 items-center gap-[10px]">
                <span className="flex h-[30px] w-[30px] shrink-0 items-center justify-center rounded-[9px] bg-[#f2f2f4] dark:bg-[#2c2c2e] text-[#6e6e73] dark:text-[#98989d]">{item.icon}</span>
                <div className="min-w-0">
                  <p className="truncate text-[12px] text-[#6e6e73] dark:text-[#8e8e93]">{item.label}</p>
                  <p className="truncate text-[13px] font-medium text-[#1d1d1f] dark:text-[#f5f5f5]">{item.value}</p>
                </div>
              </div>
            ))}
          </div>
        </div>

        <div className="rounded-[18px] border border-black/[0.06] dark:border-white/[0.06] bg-white/70 dark:bg-[#1c1c1e]/80 p-[20px] shadow-[0_8px_24px_rgba(0,0,0,0.025)] dark:shadow-[0_8px_24px_rgba(0,0,0,0.2)]">
          <h3 className="text-[16px] font-semibold tracking-[-0.02em] text-[#1d1d1f] dark:text-[#f5f5f5]">快速操作</h3>
          <div className="mt-[14px] space-y-[6px]">
            <button type="button" onClick={onReload} className="flex min-h-[42px] w-full items-center gap-[10px] rounded-[10px] px-[8px] text-left transition-colors hover:bg-black/[0.04] dark:hover:bg-white/[0.06]">
              <RefreshCw className="h-[18px] w-[18px] shrink-0 text-[#5f9ed8]" strokeWidth={1.8} />
              <span className="min-w-0"><span className="block text-[13px] font-semibold text-[#1d1d1f] dark:text-[#f5f5f5]">重新加载配置</span><span className="mt-[2px] block truncate text-[11px] text-[#86868b] dark:text-[#98989d]">从本地设置恢复当前页面</span></span>
            </button>
            <button type="button" onClick={() => showNotice("配置目录将在本地 Storage Manager 接入后开放")} className="flex min-h-[42px] w-full items-center gap-[10px] rounded-[10px] px-[8px] text-left transition-colors hover:bg-black/[0.04] dark:hover:bg-white/[0.06]">
              <Folder className="h-[18px] w-[18px] shrink-0 text-[#6e6e73] dark:text-[#98989d]" strokeWidth={1.8} />
              <span className="min-w-0"><span className="block text-[13px] font-semibold text-[#1d1d1f] dark:text-[#f5f5f5]">打开配置目录</span><span className="mt-[2px] block truncate text-[11px] text-[#86868b] dark:text-[#98989d]">查看或编辑本地配置文件</span></span>
            </button>
          </div>
        </div>
      </div>
    </aside>
  )
}

function generalDisplayValue(form: SettingsFormController): GeneralSettingsValue {
  const value = form.displayValue
  if (value.section === "general") return value
  return { section: "general", launchPage: "home", restoreSession: false, language: "zh_cn", notifications: true }
}

function appearanceDisplayValue(form: SettingsFormController): AppearanceSettingsValue {
  const value = form.displayValue
  if (value.section === "appearance") return value
  return { section: "appearance", theme: "system", density: "comfortable", sidebar: "auto", reduceMotion: false }
}

function privacyDisplayValue(form: SettingsFormController): PrivacySettingsValue {
  const value = form.displayValue
  if (value.section === "privacy") return value
  return { section: "privacy", searchHistory: true, playbackHistory: true }
}

function playbackDisplayValue(form: SettingsFormController): PlaybackSettingsValue {
  const value = form.displayValue
  if (value.section === "playback") return value
  return { section: "playback", defaultPlaybackRate: "one", autoResume: true, autoNext: true }
}

function downloadsDisplayValue(form: SettingsFormController): DownloadSettingsValue {
  const value = form.displayValue
  if (value.section === "downloads") return value
  return { section: "downloads", concurrentTasks: "three", speedLimit: "unlimited", autoContinue: true }
}

function readingDisplayValue(form: SettingsFormController): ReadingSettingsValue {
  const value = form.displayValue
  if (value.section === "reading") return value
  return {
    section: "reading",
    fontFamily: "serif",
    customFontFamily: null,
    fontSize: "medium",
    lineHeight: "comfortable",
    contentWidth: "medium",
    theme: "warm",
    customBackground: null,
    customText: null,
    fontWeight: "regular",
    letterSpacing: "normal",
    systemAuto: true,
    pagination: "scroll",
  }
}

function comicDisplayValue(form: SettingsFormController): ComicSettingsValue {
  const value = form.displayValue
  if (value.section === "comic") return value
  return { section: "comic", viewMode: "single", direction: "rtl", pageGap: "twelve", preloadPages: "three" }
}

function getSettingsSummaryItems(section: SettingsSectionId, settings: SettingsState, forms: { general: SettingsFormController; appearance: SettingsFormController; playback: SettingsFormController; reading: SettingsFormController; comic: SettingsFormController; downloads: SettingsFormController; privacy: SettingsFormController }): SettingsSummaryItem[] {
  switch (section) {
    case "general": {
      const value = generalDisplayValue(forms.general)
      return [
        { label: "启动页", value: optionLabel(LAUNCH_PAGE_OPTIONS, value.launchPage), icon: <Home className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "恢复上次状态", value: value.restoreSession ? "已开启" : "已关闭", icon: <History className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "界面语言", value: `${optionLabel(LANGUAGE_OPTIONS, value.language)} · 当前版本不可用`, icon: <Globe2 className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "通知", value: "当前版本不可用", icon: <Bell className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
      ]
    }
    case "appearance": {
      const value = appearanceDisplayValue(forms.appearance)
      return [
        { label: "主题", value: optionLabel(THEME_OPTIONS, value.theme), icon: <Palette className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "界面密度", value: optionLabel(DENSITY_OPTIONS, value.density), icon: <SlidersHorizontal className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "侧栏行为", value: optionLabel(SIDEBAR_OPTIONS, value.sidebar), icon: <PanelsTopLeft className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "减少动效", value: value.reduceMotion ? "已开启" : "已关闭", icon: <Sparkles className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
      ]
    }
    case "reading": {
      const value = readingDisplayValue(forms.reading)
      return [
        { label: "默认字体", value: optionLabel(READING_FONT_OPTIONS, value.fontFamily), icon: <BookOpen className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "字号与行高", value: `${optionLabel(READING_FONT_SIZE_OPTIONS, value.fontSize)} · ${optionLabel(READING_LINE_HEIGHT_OPTIONS, value.lineHeight)}`, icon: <Settings2 className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "正文宽度", value: optionLabel(READING_WIDTH_OPTIONS, value.contentWidth), icon: <PanelsTopLeft className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "阅读模式", value: optionLabel(READING_PAGINATION_OPTIONS, value.pagination ?? "scroll"), icon: <BookOpen className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
      ]
    }
    case "playback": {
      const value = playbackDisplayValue(forms.playback)
      return [
        { label: "默认倍速", value: optionLabel(PLAYBACK_RATE_OPTIONS, value.defaultPlaybackRate), icon: <SlidersHorizontal className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "自动下一集", value: value.autoNext ? "已开启" : "已关闭", icon: <ChevronRight className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "自动继续", value: value.autoResume ? "已开启" : "已关闭", icon: <RefreshCw className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "截图快捷键", value: "Ctrl+Shift+S", icon: <PlaySquare className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
      ]
    }
    case "downloads": {
      const value = downloadsDisplayValue(forms.downloads)
      return [
        { label: "下载位置", value: "下载 / 栖阅（默认）", icon: <Download className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "同时下载数量", value: optionLabel(DOWNLOAD_CONCURRENCY_OPTIONS, value.concurrentTasks), icon: <SlidersHorizontal className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "下载速度", value: optionLabel(DOWNLOAD_SPEED_LIMIT_OPTIONS, value.speedLimit), icon: <Download className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "中断任务恢复", value: value.autoContinue ? "自动恢复" : "手动恢复", icon: <RefreshCw className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
      ]
    }
    case "comic": {
      const value = comicDisplayValue(forms.comic)
      return [
        { label: "阅读模式", value: value.viewMode === "single" ? "单页" : value.viewMode === "double" ? "双页" : "条漫", icon: <PanelsTopLeft className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "阅读方向", value: value.direction === "rtl" ? "从右向左" : "从左向右", icon: <ChevronRight className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "页面间距", value: value.pageGap === "zero" ? "0 px" : value.pageGap === "twelve" ? "12 px" : "24 px", icon: <SlidersHorizontal className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "预加载页数", value: value.preloadPages === "unlimited" ? "安全上限" : `${value.preloadPages === "one" ? 1 : value.preloadPages === "three" ? 3 : 5} 页`, icon: <Download className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
      ]
    }
    case "sync":
      return [
        { label: "同步状态", value: settings.syncEnabled ? "已开启" : "已关闭", icon: <Cloud className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "同步目标", value: settings.syncTarget, icon: <Server className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "同步进度与标记", value: settings.syncProgress ? "已开启" : "已关闭", icon: <RefreshCw className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
      ]
    case "privacy":
      {
        const value = privacyDisplayValue(forms.privacy)
      return [
        { label: "搜索历史", value: value.searchHistory ? "已开启" : "已关闭", icon: <History className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "播放与阅读历史", value: value.playbackHistory ? "已开启" : "已关闭", icon: <BookOpen className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "网络诊断信息", value: "当前版本不可用", icon: <Shield className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
      ]
      }
    default:
      return [
        { label: "当前状态", value: "本机保存", icon: <CircleCheck className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "配置来源", value: "Haven Local-first", icon: <HardDrive className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
        { label: "应用版本", value: "由关于页面读取", icon: <Info className="h-[16px] w-[16px]" strokeWidth={1.8} /> },
      ]
  }
}

function Toggle({ checked, onChange, label, disabled = false }: { checked: boolean; onChange: (value: boolean) => void; label: string; disabled?: boolean }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        "relative h-[32px] w-[52px] rounded-full border border-black/[0.04] dark:border-white/[0.04] p-[2px] transition-colors duration-300 ease-in-out",
        checked ? "bg-[#34c759]" : "bg-[#e9e9ea] dark:bg-[#39393d] hover:bg-[#e1e1e2] dark:hover:bg-[#454549]",
        disabled && "cursor-not-allowed opacity-50"
      )}
    >
      <span
        className={cn(
          "block h-[26px] w-[26px] rounded-full bg-white shadow-[0_2px_6px_rgba(0,0,0,0.15),0_0_1px_rgba(0,0,0,0.2)] dark:shadow-[0_2px_6px_rgba(0,0,0,0.5),0_0_1px_rgba(0,0,0,0.5)] transition-transform duration-300 ease-in-out",
          checked && "translate-x-[20px]"
        )}
      />
    </button>
  )
}

function SelectControl({ value, options, onChange, ariaLabel, disabled = false }: { value: string; options: string[]; onChange: (value: string) => void; ariaLabel: string; disabled?: boolean }) {
  const rootRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const menuRef = useRef<HTMLDivElement>(null)
  const menuId = useId()
  const selectedIndex = Math.max(0, options.indexOf(value))
  const [isOpen, setIsOpen] = useState(false)
  const [highlightedIndex, setHighlightedIndex] = useState(selectedIndex)
  const [menuStyle, setMenuStyle] = useState<CSSProperties>({})

  const updateMenuPosition = useCallback(() => {
    const trigger = triggerRef.current
    if (!trigger) return

    const rect = trigger.getBoundingClientRect()
    const viewportGap = 12
    const menuWidth = Math.max(rect.width, 160)
    const estimatedHeight = Math.min(260, options.length * 36 + 8)
    const canOpenAbove = rect.top > estimatedHeight + viewportGap
    const shouldOpenAbove = window.innerHeight - rect.bottom < estimatedHeight + viewportGap && canOpenAbove
    const maxTop = Math.max(viewportGap, window.innerHeight - estimatedHeight - viewportGap)
    const top = shouldOpenAbove
      ? Math.max(viewportGap, rect.top - estimatedHeight - 8)
      : Math.min(maxTop, rect.bottom + 8)
    const left = Math.min(
      Math.max(viewportGap, rect.right - menuWidth),
      Math.max(viewportGap, window.innerWidth - menuWidth - viewportGap)
    )

    setMenuStyle({
      left,
      maxHeight: Math.min(260, Math.max(120, window.innerHeight - viewportGap * 2)),
      position: "fixed",
      top,
      width: menuWidth,
      zIndex: 100,
    })
  }, [options.length])

  const openMenu = () => {
    if (disabled) return
    setHighlightedIndex(selectedIndex)
    updateMenuPosition()
    setIsOpen(true)
  }

  const selectOption = (option: string) => {
    onChange(option)
    setIsOpen(false)
  }

  useEffect(() => {
    if (!isOpen) return

    const handlePointerDown = (event: PointerEvent) => {
      const target = event.target as Node
      if (rootRef.current?.contains(target) || menuRef.current?.contains(target)) return
      setIsOpen(false)
    }
    const handleReposition = () => updateMenuPosition()

    document.addEventListener("pointerdown", handlePointerDown)
    document.addEventListener("scroll", handleReposition, true)
    window.addEventListener("resize", handleReposition)
    const frame = window.requestAnimationFrame(handleReposition)

    return () => {
      document.removeEventListener("pointerdown", handlePointerDown)
      document.removeEventListener("scroll", handleReposition, true)
      window.removeEventListener("resize", handleReposition)
      window.cancelAnimationFrame(frame)
    }
  }, [isOpen, updateMenuPosition])

  const handleTriggerKeyDown = (event: React.KeyboardEvent<HTMLButtonElement>) => {
    if (disabled || options.length === 0) return

    if (event.key === "ArrowDown") {
      event.preventDefault()
      if (!isOpen) {
        openMenu()
      } else {
        setHighlightedIndex((current) => (current + 1) % options.length)
      }
    } else if (event.key === "ArrowUp") {
      event.preventDefault()
      if (!isOpen) {
        openMenu()
      } else {
        setHighlightedIndex((current) => (current - 1 + options.length) % options.length)
      }
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault()
      if (!isOpen) {
        openMenu()
      } else {
        selectOption(options[highlightedIndex])
      }
    } else if (event.key === "Escape") {
      event.preventDefault()
      setIsOpen(false)
    }
  }

  return (
    <div ref={rootRef} className="relative">
        <button
          ref={triggerRef}
          type="button"
          aria-controls={menuId}
          aria-expanded={isOpen}
          aria-haspopup="listbox"
          aria-label={ariaLabel}
          disabled={disabled}
          onClick={() => (isOpen ? setIsOpen(false) : openMenu())}
          onKeyDown={handleTriggerKeyDown}
          className={cn(
            "inline-flex h-[36px] min-w-[140px] max-w-[280px] cursor-pointer items-center justify-between gap-[12px] rounded-xl border bg-black/[0.03] dark:bg-white/[0.04] px-[12px] text-[14px] font-medium text-[#1d1d1f] dark:text-[#f5f5f5] outline-none transition-all hover:bg-black/[0.05] dark:hover:bg-white/[0.08]",
            isOpen ? "border-[#007aff]/50 ring-4 ring-[#007aff]/10" : "border-black/[0.06] dark:border-white/[0.06]",
            disabled && "cursor-not-allowed opacity-50"
          )}
        >
          <span className="min-w-0 flex-1 truncate text-right">{value}</span>
          <ChevronDown className={cn("h-[16px] w-[16px] shrink-0 text-[#86868b] dark:text-[#98989d] transition-transform duration-200", isOpen && "rotate-180 text-[#007aff]")} strokeWidth={2.2} />
        </button>

      {isOpen && createPortal(
        <div
          ref={menuRef}
          id={menuId}
          role="listbox"
          aria-label={ariaLabel}
          style={menuStyle}
          className="settings-scrollbar-hidden overflow-y-auto rounded-[12px] border border-black/[0.08] dark:border-white/[0.08] bg-white/[0.97] dark:bg-[#2c2c2e]/97 p-[4px] shadow-[0_14px_36px_rgba(0,0,0,0.16),0_2px_8px_rgba(0,0,0,0.06)] dark:shadow-[0_14px_36px_rgba(0,0,0,0.4),0_2px_8px_rgba(0,0,0,0.2)] backdrop-blur-xl"
        >
          {options.map((option, index) => {
            const isSelected = option === value
            const isHighlighted = index === highlightedIndex
            return (
              <button
                key={option}
                type="button"
                role="option"
                aria-selected={isSelected}
                onMouseEnter={() => setHighlightedIndex(index)}
                onClick={() => selectOption(option)}
                className={cn(
                  "flex min-h-[34px] w-full items-center justify-between gap-[8px] rounded-[8px] px-[10px] text-right text-[13px] font-medium transition-colors",
                  isSelected
                    ? "bg-[#007aff] text-white"
                    : isHighlighted
                      ? "bg-[#007aff]/[0.08] dark:bg-[#007aff]/[0.2] text-[#007aff]"
                      : "text-[#1d1d1f] dark:text-[#f5f5f5] hover:bg-[#007aff]/[0.08] dark:hover:bg-[#007aff]/[0.2] hover:text-[#007aff]"
                )}
              >
                <span className="min-w-0 flex-1 truncate">{option}</span>
                {isSelected && <CircleCheck className="h-[14px] w-[14px] shrink-0" strokeWidth={2.4} />}
              </button>
            )
          })}
        </div>,
        document.body
      )}
    </div>
  )
}

function SegmentedControl({ value, options, onChange, ariaLabel, disabled = false }: { value: string; options: string[]; onChange: (value: string) => void; ariaLabel: string; disabled?: boolean }) {
  return (
    <div className="flex flex-wrap items-center gap-1 rounded-xl bg-black/[0.05] dark:bg-white/[0.05] p-1" role="group" aria-label={ariaLabel}>
      {options.map((option) => (
        <button
          key={option}
          type="button"
          disabled={disabled}
          onClick={() => onChange(option)}
          className={cn(
            "rounded-[8px] px-3.5 py-1.5 text-[13px] font-medium transition-all duration-200",
            option === value ? "bg-white dark:bg-[#3a3a3c] text-[#1d1d1f] dark:text-[#f5f5f5] shadow-sm" : "text-[#6e6e73] dark:text-[#98989d] hover:bg-black/[0.02] dark:hover:bg-white/[0.04] hover:text-[#1d1d1f] dark:hover:text-[#f5f5f5]",
            disabled && "cursor-not-allowed opacity-50"
          )}
        >
          {option}
        </button>
      ))}
    </div>
  )
}

/** 表单状态栏：loading / saving / dirty / saved；dirty 时显示"保存修改"（空 patch 永不进入 dirty）。 */
function SettingsFormStatusBar({ form, onReset }: { form: SettingsFormController; onReset: () => void }) {
  const state = form.state
  let status: ReactNode
  if (state.status === "loading") {
    status = <span className="flex items-center gap-2 text-[13px] text-[#86868b]"><RefreshCw className="h-[16px] w-[16px] animate-spin" strokeWidth={2.2} />正在加载…</span>
  } else if (state.status === "saving") {
    status = <span className="flex items-center gap-2 text-[13px] text-[#6e6e73]"><RefreshCw className="h-[16px] w-[16px] animate-spin" strokeWidth={2.2} />正在保存…</span>
  } else if (form.isDirty) {
    status = <span className="flex items-center gap-2 text-[13px] font-medium text-[#d97706]"><TriangleAlert className="h-[16px] w-[16px]" strokeWidth={2.2} />有未保存的修改</span>
  } else {
    status = <span className="flex items-center gap-2 text-[13px] text-[#6e6e73]"><CircleCheck className="h-[16px] w-[16px] text-[#34c759]" strokeWidth={2.2} />所有设置已保存</span>
  }

  return (
    <div className="mx-6 mb-1 flex items-center justify-between gap-4 border-b border-black/[0.05] py-4">
      <div className="flex items-center gap-2 text-[13px] text-[#6e6e73]">{status}</div>
      <div className="flex items-center gap-6">
        {form.isDirty && (
          <button
            type="button"
            onClick={() => form.save()}
            disabled={form.isSaving}
            className="text-[13px] font-semibold text-[#007aff] transition-colors hover:text-[#005bb5] hover:underline disabled:cursor-not-allowed disabled:opacity-50"
          >
            保存修改
          </button>
        )}
        <button type="button" onClick={onReset} className="text-[13px] font-medium text-[#86868b] dark:text-[#98989d] transition-colors hover:text-[#1d1d1f] dark:hover:text-[#f5f5f5] hover:underline">
          恢复默认
        </button>
      </div>
    </div>
  )
}

/** 表单错误横幅：load-error / save-error / validation-error / REVISION_CONFLICT（conflict → 重新加载）。 */
function SettingsFormError({ form }: { form: SettingsFormController }) {
  const state = form.state
  if (state.status !== "load-error" && state.status !== "save-error"
    && state.status !== "validation-error" && state.status !== "conflict") {
    return null
  }
  const retryLabel = state.status === "save-error" || state.status === "load-error" ? "重试" : "重新加载"
  return (
    <div className="mx-6 mb-1 flex flex-wrap items-center justify-between gap-3 rounded-2xl border border-[#d70015]/15 bg-[#fff1f0] px-4 py-3">
      <div className="flex min-w-0 items-center gap-2 text-[13px] leading-5 text-[#d70015]">
        <TriangleAlert className="h-[16px] w-[16px] shrink-0" strokeWidth={2.2} />
        <span className="min-w-0">{state.message}</span>
      </div>
      <button type="button" onClick={() => form.retry()} className="shrink-0 rounded-full border border-[#d70015]/20 bg-white/70 px-3 py-1.5 text-[12px] font-semibold text-[#d70015] transition-colors hover:bg-white">
        {retryLabel}
      </button>
    </div>
  )
}

function GeneralSettings({ form }: { form: SettingsFormController }) {
  const value = generalDisplayValue(form)
  return (
    <>
      <SettingsIntro section="General" title="通用" description="设置栖阅如何启动、如何回应你，以及默认使用哪种语言。" />
      <SettingsFormStatusBar form={form} onReset={() => form.resetToDefaults()} />
      <SettingsFormError form={form} />

      <SettingsGroup title="启动行为">
        <SettingRow icon={<Home className="h-[19px] w-[19px]" strokeWidth={1.8} />} title="默认启动页" description="打开栖阅时首先进入的空间。">
          <SelectControl value={optionLabel(LAUNCH_PAGE_OPTIONS, value.launchPage)} options={LAUNCH_PAGE_OPTIONS.map((option) => option.label)} onChange={(label) => form.change({ section: "general", launchPage: optionValue(LAUNCH_PAGE_OPTIONS, label) })} ariaLabel="默认启动页" />
        </SettingRow>
        <SettingRow icon={<History className="h-[19px] w-[19px]" strokeWidth={1.8} />} title="恢复上次状态" description="恢复上次打开的页面与正在进行的内容。">
          <Toggle checked={value.restoreSession} onChange={(checked) => form.change({ section: "general", restoreSession: checked })} label="恢复上次状态" />
        </SettingRow>
      </SettingsGroup>
      <SettingsGroup title="语言与区域">
        <SettingRow icon={<Globe2 className="h-[19px] w-[19px]" strokeWidth={1.8} />} title="界面语言" description="当前版本尚未接入语言包消费者，设置暂不可用。">
          <SelectControl value={optionLabel(LANGUAGE_OPTIONS, value.language)} options={LANGUAGE_OPTIONS.map((option) => option.label)} onChange={(label) => form.change({ section: "general", language: optionValue(LANGUAGE_OPTIONS, label) })} ariaLabel="界面语言" disabled />
        </SettingRow>
      </SettingsGroup>
      <SettingsGroup title="通知">
        <SettingRow icon={<Bell className="h-[19px] w-[19px]" strokeWidth={1.8} />} title="通知" description="当前版本尚未接入统一通知发送者，设置暂不可用。">
          <Toggle checked={value.notifications} onChange={(checked) => form.change({ section: "general", notifications: checked })} label="通知" disabled />
        </SettingRow>
      </SettingsGroup>
    </>
  )
}

function AppearanceSettings({ form }: { form: SettingsFormController }) {
  const value = appearanceDisplayValue(form)
  return (
    <>
      <SettingsIntro section="Appearance" title="外观" description="保持清晰、安静和内容优先。主题设置不会改变媒体内容本身。" />
      <SettingsFormStatusBar form={form} onReset={() => form.resetToDefaults()} />
      <SettingsFormError form={form} />
      <SettingsGroup title="界面外观">
        <SettingRow title="主题" description="跟随 Windows 系统设置，或为栖阅指定主题。">
          <SegmentedControl value={optionLabel(THEME_OPTIONS, value.theme)} options={THEME_OPTIONS.map((option) => option.label)} onChange={(label) => form.change({ section: "appearance", theme: optionValue(THEME_OPTIONS, label) })} ariaLabel="主题" />
        </SettingRow>
        <SettingRow title="界面密度" description="控制列表和卡片之间的留白。">
          <SegmentedControl value={optionLabel(DENSITY_OPTIONS, value.density)} options={DENSITY_OPTIONS.map((option) => option.label)} onChange={(label) => form.change({ section: "appearance", density: optionValue(DENSITY_OPTIONS, label) })} ariaLabel="界面密度" />
        </SettingRow>
        <SettingRow title="侧栏行为" description="当前桌面壳使用浮动 Dock，此选项控制内容区的导航留白。">
          <SelectControl value={optionLabel(SIDEBAR_OPTIONS, value.sidebar)} options={SIDEBAR_OPTIONS.map((option) => option.label)} onChange={(label) => form.change({ section: "appearance", sidebar: optionValue(SIDEBAR_OPTIONS, label) })} ariaLabel="侧栏行为" />
        </SettingRow>
        <SettingRow title="减少动效" description="减少背景动画、页面过渡和强调动效。">
          <Toggle checked={value.reduceMotion} onChange={(checked) => form.change({ section: "appearance", reduceMotion: checked })} label="减少动效" />
        </SettingRow>
      </SettingsGroup>
      <SettingsGroup title="当前视觉">
        <div className="grid gap-3 p-5 sm:grid-cols-3">
          <ThemePreview icon={<Sun className="h-5 w-5" />} title="浅色" detail="清晰、柔和" active={value.theme === "light"} onClick={() => form.change({ section: "appearance", theme: "light" })} />
          <ThemePreview icon={<Moon className="h-5 w-5" />} title="深色" detail="沉浸、低亮度" active={value.theme === "dark"} onClick={() => form.change({ section: "appearance", theme: "dark" })} />
          <ThemePreview icon={<Settings2 className="h-5 w-5" />} title="跟随系统" detail="推荐" active={value.theme === "system"} onClick={() => form.change({ section: "appearance", theme: "system" })} />
        </div>
      </SettingsGroup>

    </>
  )
}

function ThemePreview({ icon, title, detail, active, onClick }: { icon: ReactNode; title: string; detail: string; active: boolean; onClick: () => void }) {
  return (
    <button type="button" onClick={onClick} className={cn("group rounded-2xl border p-[16px] text-left transition-all duration-200", active ? "border-[#007aff]/50 bg-[#007aff]/[0.06] dark:bg-[#007aff]/[0.15] shadow-sm" : "border-black/[0.05] dark:border-white/[0.05] bg-black/[0.02] dark:bg-white/[0.02] hover:bg-black/[0.04] dark:hover:bg-white/[0.04] hover:border-black/[0.1] dark:hover:border-white/[0.1]")}>
      <span className={cn("flex h-10 w-10 items-center justify-center rounded-xl transition-all duration-200", active ? "bg-[#007aff] text-white shadow-md shadow-[#007aff]/20" : "bg-white dark:bg-[#2c2c2e] text-[#6e6e73] dark:text-[#98989d] shadow-sm group-hover:text-[#1d1d1f] dark:group-hover:text-[#f5f5f5]")}>{icon}</span>
      <span className={cn("mt-3 block text-[15px] font-medium transition-colors", active ? "text-[#007aff]" : "text-[#1d1d1f] dark:text-[#f5f5f5]")}>{title}</span>
      <span className="mt-1 block text-[13px] text-[#86868b] dark:text-[#8e8e93]">{detail}</span>
    </button>
  )
}

function PlaybackSettings({ form }: { form: SettingsFormController }) {
  const value = playbackDisplayValue(form)
  return (
    <>
      <SettingsIntro section="Playback" title="播放" description="让播放行为稳定、可恢复。字幕与音轨选择不属于当前产品范围；截图使用播放器固定快捷键。" />
      <SettingsFormStatusBar form={form} onReset={() => form.resetToDefaults()} />
      <SettingsFormError form={form} />
      <SettingsGroup title="默认播放偏好">
        <SettingRow title="默认倍速" description="新的视频播放会话默认使用此倍速；播放中的会话仍可在播放器内临时调整。">
          <SelectControl value={optionLabel(PLAYBACK_RATE_OPTIONS, value.defaultPlaybackRate)} options={PLAYBACK_RATE_OPTIONS.map((option) => option.label)} onChange={(label) => form.change({ section: "playback", defaultPlaybackRate: optionValue(PLAYBACK_RATE_OPTIONS, label) })} ariaLabel="默认倍速" />
        </SettingRow>
      </SettingsGroup>
      <SettingsGroup title="连续播放">
        <SettingRow title="自动下一集" description="当前 Edition 播放到最后一项后，自动打开下一项；播放列表结束时保持结束状态。">
          <Toggle checked={value.autoNext} onChange={(checked) => form.change({ section: "playback", autoNext: checked })} label="自动下一集" />
        </SettingRow>
        <SettingRow title="自动继续" description="从最近一次可靠保存的位置恢复，而不是只在正常退出时保存。">
          <Toggle checked={value.autoResume} onChange={(checked) => form.change({ section: "playback", autoResume: checked })} label="自动继续" />
        </SettingRow>
      </SettingsGroup>
      <SettingsGroup title="截图" description="截图不会写入进度、标记、Artwork Cache 或普通设置。">
        <SettingRow title="截图快捷键" description="仅在播放器页面响应；输入框、文本域、可编辑区域和重复按键不会触发。">
          <kbd className="rounded-lg border border-black/[0.08] bg-black/[0.03] px-3 py-2 font-mono text-xs font-semibold text-[#1d1d1f] dark:border-white/[0.1] dark:bg-white/[0.06] dark:text-[#f5f5f5]">Ctrl+Shift+S</kbd>
        </SettingRow>
        <SettingRow title="默认保存位置" description="保存时由 Windows 系统对话框确认位置；默认目录为下载 / 栖阅 / 截图。">
          <span className="text-xs font-semibold text-[#6e6e73] dark:text-[#98989d]">下载 / 栖阅 / 截图</span>
        </SettingRow>
        <SettingRow title="硬件解码" description="视频解码由 WebView2 和系统自动管理，当前版本不提供用户开关。">
          <span className="text-xs font-semibold text-[#86868b] dark:text-[#98989d]">系统默认（由 WebView2 管理）</span>
        </SettingRow>
      </SettingsGroup>
    </>
  )
}

function ReadingSettings({
  form,
  resourceContext,
  showNotice,
}: {
  form: SettingsFormController
  resourceContext: ResourcePreferenceContext | null
  showNotice: (message: string) => void
}) {
  const value = readingDisplayValue(form)
  return (
    <>
      <SettingsIntro section="Reading" title="阅读" description="统一图书、文章和部分报刊资料的阅读体验。全局默认值可保存，打开具体资源时也可以单独覆盖。" />
      <SettingsFormStatusBar form={form} onReset={() => form.resetToDefaults()} />
      <SettingsFormError form={form} />
      <SettingsGroup title="排版">
        <SettingRow title="默认字体">
          <SelectControl value={optionLabel(READING_FONT_OPTIONS, value.fontFamily)} options={READING_FONT_OPTIONS.map((option) => option.label)} onChange={(label) => form.change({ section: "reading", fontFamily: optionValue(READING_FONT_OPTIONS, label) })} ariaLabel="默认字体" />
        </SettingRow>
        <SettingRow title="字号">
          <SegmentedControl value={optionLabel(READING_FONT_SIZE_OPTIONS, value.fontSize)} options={READING_FONT_SIZE_OPTIONS.map((option) => option.label)} onChange={(label) => form.change({ section: "reading", fontSize: optionValue(READING_FONT_SIZE_OPTIONS, label) })} ariaLabel="字号" />
        </SettingRow>
        <SettingRow title="行高">
          <SegmentedControl value={optionLabel(READING_LINE_HEIGHT_OPTIONS, value.lineHeight)} options={READING_LINE_HEIGHT_OPTIONS.map((option) => option.label)} onChange={(label) => form.change({ section: "reading", lineHeight: optionValue(READING_LINE_HEIGHT_OPTIONS, label) })} ariaLabel="行高" />
        </SettingRow>
        <SettingRow title="正文宽度">
          <SegmentedControl value={optionLabel(READING_WIDTH_OPTIONS, value.contentWidth)} options={READING_WIDTH_OPTIONS.map((option) => option.label)} onChange={(label) => form.change({ section: "reading", contentWidth: optionValue(READING_WIDTH_OPTIONS, label) })} ariaLabel="正文宽度" />
        </SettingRow>
      </SettingsGroup>
      <SettingsGroup title="阅读行为">
        <SettingRow title="阅读模式" description="文本阅读器支持连续滚动、单页分页和双页分页；PDF 等非文本资源继续使用各自阅读器。">
          <SelectControl value={optionLabel(READING_PAGINATION_OPTIONS, value.pagination ?? "scroll")} options={READING_PAGINATION_OPTIONS.map((option) => option.label)} onChange={(label) => form.change({ section: "reading", pagination: optionValue(READING_PAGINATION_OPTIONS, label) })} ariaLabel="阅读模式" />
        </SettingRow>
        <SettingRow title="默认主题">
          <SelectControl value={optionLabel(READING_THEME_OPTIONS, value.theme)} options={READING_THEME_OPTIONS.map((option) => option.label)} onChange={(label) => form.change({ section: "reading", theme: optionValue(READING_THEME_OPTIONS, label) })} ariaLabel="默认主题" />
        </SettingRow>
      </SettingsGroup>
      <ResourcePreferencePanel section="reading" context={resourceContext} showNotice={showNotice} />
    </>
  )
}

function ComicSettings({
  form,
  resourceContext,
  showNotice,
}: {
  form: SettingsFormController
  resourceContext: ResourcePreferenceContext | null
  showNotice: (message: string) => void
}) {
  const value = comicDisplayValue(form)
  return (
    <>
      <SettingsIntro section="Comic" title="漫画" description="为单页、双页、条漫和从右向左的内容提供独立默认偏好；保存后用于新的漫画阅读会话。" />
      <SettingsFormStatusBar form={form} onReset={() => form.resetToDefaults()} />
      <SettingsFormError form={form} />
      <SettingsGroup title="阅读方式">
        <SettingRow title="默认阅读模式"><SelectControl value={value.viewMode === "single" ? "单页" : value.viewMode === "double" ? "双页" : "条漫"} options={["单页", "双页", "条漫"]} onChange={(label) => form.change({ section: "comic", viewMode: label === "单页" ? "single" : label === "双页" ? "double" : "strip" })} ariaLabel="默认漫画阅读模式" /></SettingRow>
        <SettingRow title="默认阅读方向"><SegmentedControl value={value.direction === "rtl" ? "从右向左" : "从左向右"} options={["从左向右", "从右向左"]} onChange={(label) => form.change({ section: "comic", direction: label === "从右向左" ? "rtl" : "ltr" })} ariaLabel="默认漫画阅读方向" /></SettingRow>
        <SettingRow title="页面间距"><SelectControl value={value.pageGap === "zero" ? "0 px" : value.pageGap === "twelve" ? "12 px" : "24 px"} options={["0 px", "12 px", "24 px"]} onChange={(label) => form.change({ section: "comic", pageGap: label === "0 px" ? "zero" : label === "12 px" ? "twelve" : "twenty_four" })} ariaLabel="页面间距" /></SettingRow>
        <SettingRow title="预加载页数" description="页数越多越顺滑，也会占用更多内存；不限制仍受阅读器安全窗口上限保护。"><SelectControl value={value.preloadPages === "unlimited" ? "不限制（安全上限）" : `${value.preloadPages === "one" ? 1 : value.preloadPages === "three" ? 3 : 5} 页`} options={["1 页", "3 页", "5 页", "不限制（安全上限）"]} onChange={(label) => form.change({ section: "comic", preloadPages: label.startsWith("1") ? "one" : label.startsWith("3") ? "three" : label.startsWith("5") ? "five" : "unlimited" })} ariaLabel="预加载页数" /></SettingRow>
      </SettingsGroup>
      <SettingsGroup title="辅助功能">
        <SettingRow title="OCR 默认语言" description="需要 OCR/AI Foundation；当前版本不会保存模拟配置。"><SelectControl value="当前版本不可用" options={["当前版本不可用"]} onChange={() => undefined} ariaLabel="OCR 默认语言" disabled /></SettingRow>
        <SettingRow title="翻译偏好" description="需要翻译/AI Foundation；当前版本不会保存模拟配置。"><SelectControl value="当前版本不可用" options={["当前版本不可用"]} onChange={() => undefined} ariaLabel="翻译偏好" disabled /></SettingRow>
      </SettingsGroup>
      <ResourcePreferencePanel section="comic" context={resourceContext} showNotice={showNotice} />
    </>
  )
}

type ResourcePreferenceSection = "reading" | "comic"
type ResourcePreferenceValue = ReadingSettingsValue | ComicSettingsValue

function resourcePreferenceValue(result: PreferenceGetResult, section: ResourcePreferenceSection): ResourcePreferenceValue {
  return section === "reading" ? result.effectiveReading : result.effectiveComic
}

function resourcePreferenceSource(result: PreferenceGetResult, section: ResourcePreferenceSection): string {
  const mediaPatch = section === "reading" ? result.mediaItemReadingPatch : result.mediaItemComicPatch
  const editionPatch = section === "reading" ? result.editionReadingPatch : result.editionComicPatch
  // A reset keeps an empty row/revision for CAS history, but that row no
  // longer contributes to the effective value. Report the layer that actually
  // supplies a field instead of treating row existence as an override.
  if (hasPreferencePatchValues(mediaPatch)) return "本资源"
  if (hasPreferencePatchValues(editionPatch)) return "版本"
  return "全局"
}

function hasPreferencePatchValues(patch: object | null): boolean {
  return patch !== null && Object.values(patch).some((value) => value !== null && value !== undefined)
}

function preferenceTargetRevision(result: PreferenceGetResult, target: PreferenceTargetWire): string | null {
  return target === "edition" ? result.editionRevision : result.mediaItemRevision
}

function preferenceTargetReadingPatch(result: PreferenceGetResult, target: PreferenceTargetWire): PreferenceReadingPatchWire | null {
  return target === "edition" ? result.editionReadingPatch : result.mediaItemReadingPatch
}

function preferenceTargetComicPatch(result: PreferenceGetResult, target: PreferenceTargetWire): PreferenceComicPatchWire | null {
  return target === "edition" ? result.editionComicPatch : result.mediaItemComicPatch
}

function preferenceTargetLabel(target: PreferenceTargetWire): string {
  return target === "edition" ? "当前版本" : "本资源"
}

/**
 * Keep resource overrides sparse.  The form starts from the effective value,
 * so only fields the user actually changed should be materialized at the
 * selected scope; otherwise an inherited global value would be frozen into
 * the Edition/MediaItem row and stop following future global changes.
 */
function buildReadingPreferencePatch(
  base: PreferenceReadingPatchWire | null,
  effective: Extract<ReadingSettingsValue, { section: "reading" }>,
  draft: Extract<ReadingSettingsValue, { section: "reading" }>,
): PreferenceReadingPatchWire | null {
  const patch: PreferenceReadingPatchWire = { ...(base ?? {}) }
  if (draft.fontFamily !== effective.fontFamily) patch.fontFamily = draft.fontFamily
  if (draft.fontSize !== effective.fontSize) patch.fontSize = draft.fontSize
  if (draft.lineHeight !== effective.lineHeight) patch.lineHeight = draft.lineHeight
  if (draft.contentWidth !== effective.contentWidth) patch.contentWidth = draft.contentWidth
  if (draft.theme !== effective.theme) patch.theme = draft.theme
  if ((draft.pagination ?? "scroll") !== (effective.pagination ?? "scroll")) {
    patch.pagination = draft.pagination ?? "scroll"
  }
  return Object.keys(patch).length > 0 ? patch : null
}

function buildComicPreferencePatch(
  base: PreferenceComicPatchWire | null,
  effective: Extract<ComicSettingsValue, { section: "comic" }>,
  draft: Extract<ComicSettingsValue, { section: "comic" }>,
): PreferenceComicPatchWire | null {
  const patch: PreferenceComicPatchWire = { ...(base ?? {}) }
  if (draft.viewMode !== effective.viewMode) patch.viewMode = draft.viewMode
  if (draft.direction !== effective.direction) patch.direction = draft.direction
  if (draft.pageGap !== effective.pageGap) patch.pageGap = draft.pageGap
  if (draft.preloadPages !== effective.preloadPages) patch.preloadPages = draft.preloadPages
  return Object.keys(patch).length > 0 ? patch : null
}

function ResourcePreferencePanel({
  section,
  context,
  showNotice,
}: {
  section: ResourcePreferenceSection
  context: ResourcePreferenceContext | null
  showNotice: (message: string) => void
}) {
  const mediaItemId = context?.mediaItemId ?? null
  const editionId = context?.editionId ?? null
  const workId = context?.workId ?? null
  const [result, setResult] = useState<PreferenceGetResult | null>(null)
  const [draft, setDraft] = useState<ResourcePreferenceValue | null>(null)
  const [status, setStatus] = useState<"idle" | "loading" | "ready" | "error">("idle")
  const [errorMessage, setErrorMessage] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const [target, setTarget] = useState<PreferenceTargetWire>("media_item")
  const requestIdRef = useRef(0)

  const load = useCallback(async () => {
    const requestId = ++requestIdRef.current
    if (!mediaItemId || !editionId) {
      setResult(null)
      setDraft(null)
      setStatus("idle")
      setErrorMessage(null)
      return
    }
    setStatus("loading")
    setErrorMessage(null)
    try {
      const next = await settingsGateway.preferenceGet({ mediaItemId, editionId })
      if (requestId !== requestIdRef.current) return
      setResult(next)
      setDraft(resourcePreferenceValue(next, section))
      setStatus("ready")
    } catch (error) {
      if (requestId !== requestIdRef.current) return
      setResult(null)
      setDraft(null)
      setStatus("error")
      setErrorMessage(toHavenError(error).dto.userMessage)
    }
  }, [editionId, mediaItemId, section])

  useEffect(() => {
    setTarget("media_item")
    void load()
    return () => {
      requestIdRef.current += 1
    }
  }, [load])

  const updateDraft = (patch: Partial<ReadingSettingsValue> | Partial<ComicSettingsValue>) => {
    setDraft((current) => current ? { ...current, ...patch } as ResourcePreferenceValue : current)
  }

  const selectTarget = (nextTarget: PreferenceTargetWire) => {
    setTarget(nextTarget)
    // 切换保存作用域时从后端 effective 值重新开始，避免把尚未保存的
    // 本资源草稿意外写入版本或反之。
    if (result) setDraft(resourcePreferenceValue(result, section))
    setErrorMessage(null)
  }

  const save = async () => {
    if (!result || !draft || !mediaItemId || !editionId || saving) return
    setSaving(true)
    const baseReading = preferenceTargetReadingPatch(result, target)
    const baseComic = preferenceTargetComicPatch(result, target)
    const readingPatch: PreferenceReadingPatchWire | null = section === "reading" && draft.section === "reading"
      ? buildReadingPreferencePatch(baseReading, result.effectiveReading, draft)
      : baseReading
    const comicPatch: PreferenceComicPatchWire | null = section === "comic" && draft.section === "comic"
      ? buildComicPreferencePatch(baseComic, result.effectiveComic, draft)
      : baseComic
    try {
      const next = await settingsGateway.preferenceUpdate({
        mediaItemId,
        editionId,
        target,
        readingPatch,
        comicPatch,
        expectedRevision: preferenceTargetRevision(result, target),
      })
      setResult(next.result)
      setDraft(resourcePreferenceValue(next.result, section))
      showNotice(next.changed ? `${preferenceTargetLabel(target)}设置已保存` : `${preferenceTargetLabel(target)}设置未改变`)
    } catch (error) {
      const normalized = toHavenError(error)
      setErrorMessage(normalized.dto.userMessage)
      setStatus("error")
      showNotice(normalized.dto.userMessage)
      if (normalized.code === "REVISION_CONFLICT") void load()
    } finally {
      setSaving(false)
    }
  }

  const reset = async () => {
    if (!result || !mediaItemId || !editionId || saving) return
    const revision = preferenceTargetRevision(result, target)
    if (revision === null) {
      showNotice(`${preferenceTargetLabel(target)}尚未设置覆盖，当前沿用上层配置`)
      return
    }
    setSaving(true)
    try {
      const next = await settingsGateway.preferenceUpdate({
        mediaItemId,
        editionId,
        target,
        readingPatch: section === "reading" ? null : preferenceTargetReadingPatch(result, target),
        comicPatch: section === "comic" ? null : preferenceTargetComicPatch(result, target),
        expectedRevision: revision,
      })
      setResult(next.result)
      setDraft(resourcePreferenceValue(next.result, section))
      showNotice(`已重置${preferenceTargetLabel(target)}覆盖`)
    } catch (error) {
      const normalized = toHavenError(error)
      setErrorMessage(normalized.dto.userMessage)
      setStatus("error")
      showNotice(normalized.dto.userMessage)
      if (normalized.code === "REVISION_CONFLICT") void load()
    } finally {
      setSaving(false)
    }
  }

  if (!context) {
    return (
      <SettingsGroup title="资源内设" description="从阅读器或漫画阅读器打开本资源设置后，可在不改变全局默认值的情况下覆盖单个资源。">
        <SettingRow title="当前资源" description="当前页面没有收到已验证的 Work / Edition / MediaItem 身份，不显示可编辑的资源配置。">
          <span className="text-[12px] font-medium text-[#86868b]">请从阅读器进入</span>
        </SettingRow>
      </SettingsGroup>
    )
  }

  return (
    <SettingsGroup
      title="资源内设"
      description="优先级：本资源 → 版本 → 全局。资源身份由当前会话提供，保存使用版本校验。"
    >
      <div className="border-b border-black/[0.05] px-6 py-4 dark:border-white/[0.05]">
        <div className="flex flex-wrap items-center gap-2 text-[11px] font-semibold text-[#6e6e73] dark:text-[#98989d]">
          <span className="rounded-md bg-black/[0.04] px-2 py-1 dark:bg-white/[0.06]">Work {workId ?? "未返回"}</span>
          <ChevronRight className="h-3.5 w-3.5" />
          <span className="rounded-md bg-black/[0.04] px-2 py-1 dark:bg-white/[0.06]">Edition {editionId}</span>
          <ChevronRight className="h-3.5 w-3.5" />
          <span className="rounded-md bg-[#007aff]/[0.10] px-2 py-1 text-[#007aff]">MediaItem {mediaItemId}</span>
        </div>
        {status === "ready" && result && (
          <p className="mt-2 text-[11px] text-[#86868b]">当前生效来源：{resourcePreferenceSource(result, section)}</p>
        )}
      </div>
      {status === "ready" && result && (
        <SettingRow title="保存到" description="版本默认会作用于该版本下的所有资源；本资源只覆盖当前 MediaItem。">
          <SegmentedControl
            value={preferenceTargetLabel(target)}
            options={["本资源", "当前版本"]}
            onChange={(label) => selectTarget(label === "当前版本" ? "edition" : "media_item")}
            ariaLabel="资源设置保存作用域"
            disabled={saving}
          />
        </SettingRow>
      )}
      {status === "loading" && <p className="px-6 py-5 text-sm text-[#86868b]">正在读取本资源配置…</p>}
      {status === "error" && (
        <div className="flex flex-wrap items-center justify-between gap-3 px-6 py-5">
          <p className="text-sm font-medium text-[#d70015]">{errorMessage ?? "本资源配置暂时不可用"}</p>
          <button type="button" onClick={() => { void load() }} className="rounded-full border border-[#007aff]/30 px-3 py-1.5 text-xs font-semibold text-[#007aff]">重试</button>
        </div>
      )}
      {status === "ready" && result && draft?.section === "reading" && section === "reading" && (
        <>
          <SettingRow title="本资源字体" description="只覆盖当前 MediaItem，未设置的字段继续继承版本或全局值。">
            <SelectControl value={optionLabel(READING_FONT_OPTIONS, draft.fontFamily)} options={READING_FONT_OPTIONS.map((option) => option.label)} onChange={(label) => updateDraft({ fontFamily: optionValue(READING_FONT_OPTIONS, label) })} ariaLabel="本资源字体" disabled={saving} />
          </SettingRow>
          <SettingRow title="本资源主题">
            <SelectControl value={optionLabel(READING_THEME_OPTIONS, draft.theme)} options={READING_THEME_OPTIONS.map((option) => option.label)} onChange={(label) => updateDraft({ theme: optionValue(READING_THEME_OPTIONS, label) })} ariaLabel="本资源主题" disabled={saving} />
          </SettingRow>
          <SettingRow title="本资源排版" description="字号、行高和正文宽度会在下次打开内容时保持。">
            <div className="flex flex-wrap justify-end gap-2">
              <SelectControl value={optionLabel(READING_FONT_SIZE_OPTIONS, draft.fontSize)} options={READING_FONT_SIZE_OPTIONS.map((option) => option.label)} onChange={(label) => updateDraft({ fontSize: optionValue(READING_FONT_SIZE_OPTIONS, label) })} ariaLabel="本资源字号" disabled={saving} />
              <SelectControl value={optionLabel(READING_LINE_HEIGHT_OPTIONS, draft.lineHeight)} options={READING_LINE_HEIGHT_OPTIONS.map((option) => option.label)} onChange={(label) => updateDraft({ lineHeight: optionValue(READING_LINE_HEIGHT_OPTIONS, label) })} ariaLabel="本资源行高" disabled={saving} />
            </div>
          </SettingRow>
          <SettingRow title="本资源阅读模式" description="只影响文本类阅读；PDF 仍使用原生页码。">
            <SelectControl
              value={draft.pagination === "paginated" ? "单页分页" : draft.pagination === "double" ? "双页分页" : "连续滚动"}
              options={["连续滚动", "单页分页", "双页分页"]}
              onChange={(label) => updateDraft({ pagination: label === "单页分页" ? "paginated" : label === "双页分页" ? "double" : "scroll" })}
              ariaLabel="本资源阅读模式"
              disabled={saving}
            />
          </SettingRow>
        </>
      )}
      {status === "ready" && result && draft?.section === "comic" && section === "comic" && (
        <>
          <SettingRow title="本资源阅读模式" description="覆盖当前漫画资源的单页、双页或条漫模式。">
            <SelectControl value={draft.viewMode === "single" ? "单页" : draft.viewMode === "double" ? "双页" : "条漫"} options={["单页", "双页", "条漫"]} onChange={(label) => updateDraft({ viewMode: label === "单页" ? "single" : label === "双页" ? "double" : "strip" })} ariaLabel="本资源阅读模式" disabled={saving} />
          </SettingRow>
          <SettingRow title="本资源阅读方向">
            <SegmentedControl value={draft.direction === "rtl" ? "从右向左" : "从左向右"} options={["从左向右", "从右向左"]} onChange={(label) => updateDraft({ direction: label === "从右向左" ? "rtl" : "ltr" })} ariaLabel="本资源阅读方向" disabled={saving} />
          </SettingRow>
          <SettingRow title="本资源页面间距与预加载">
            <div className="flex flex-wrap justify-end gap-2">
              <SelectControl value={draft.pageGap === "zero" ? "0 px" : draft.pageGap === "twelve" ? "12 px" : "24 px"} options={["0 px", "12 px", "24 px"]} onChange={(label) => updateDraft({ pageGap: label === "0 px" ? "zero" : label === "12 px" ? "twelve" : "twenty_four" })} ariaLabel="本资源页面间距" disabled={saving} />
              <SelectControl value={draft.preloadPages === "one" ? "1 页" : draft.preloadPages === "three" ? "3 页" : draft.preloadPages === "five" ? "5 页" : "不限制（安全上限）"} options={["1 页", "3 页", "5 页", "不限制（安全上限）"]} onChange={(label) => updateDraft({ preloadPages: label.startsWith("1") ? "one" : label.startsWith("3") ? "three" : label.startsWith("5") ? "five" : "unlimited" })} ariaLabel="本资源预加载页数" disabled={saving} />
            </div>
          </SettingRow>
        </>
      )}
      {status === "ready" && result && (
        <div className="flex flex-wrap items-center justify-end gap-4 border-t border-black/[0.05] px-6 py-4 dark:border-white/[0.05]">
          <button type="button" onClick={() => { void reset() }} disabled={saving} className="text-[13px] font-medium text-[#86868b] transition-colors hover:text-[#1d1d1f] disabled:cursor-not-allowed disabled:opacity-50 dark:hover:text-[#f5f5f5]">重置本资源</button>
          <button type="button" onClick={() => { void save() }} disabled={saving} className="rounded-full bg-[#007aff] px-4 py-2 text-[13px] font-semibold text-white transition-colors hover:bg-[#006fe6] disabled:cursor-not-allowed disabled:opacity-50">{saving ? "保存中…" : "保存本资源设置"}</button>
        </div>
      )}
    </SettingsGroup>
  )
}

/** CMS10 端点输入行：仅 cms10 显示；端点必须由用户明确填写。 */
function Cms10EndpointRow({
  source,
  showNotice,
  onChanged,
}: {
  source: SourceDescriptorWire
  showNotice: (message: string) => void
  onChanged: () => void
}) {
  const [endpoint, setEndpoint] = useState("")
  const [saving, setSaving] = useState(false)
  const save = async () => {
    setSaving(true)
    try {
      const result = await setSourceEndpoint({ sourceId: source.sourceId, endpoint })
      if (result.endpointConfigured) {
        showNotice("端点已保存")
      } else {
        showNotice("端点已清除")
      }
      onChanged()
    } catch (error) {
      showNotice(toHavenError(error).dto.userMessage)
    } finally {
      setSaving(false)
    }
  }
  return (
    <div className="flex flex-col gap-3 border-t border-black/[0.06] px-5 py-4">
      <div className="flex flex-col gap-2 border-t border-black/[0.06] pt-3 sm:flex-row sm:items-center">
        <label className="min-w-0 flex-1 text-xs text-[#86868b]" htmlFor={`endpoint-${source.sourceId}`}>
          用户配置采集接口地址（http/https，例如 https://host/api.php/provide/vod）
          <input
            id={`endpoint-${source.sourceId}`}
            value={endpoint}
            onChange={(event) => setEndpoint(event.target.value)}
            placeholder="https://…/api.php/provide/vod"
            className="mt-1 w-full rounded-xl border border-black/[0.12] bg-white px-3 py-2 font-mono text-xs text-[#1d1d1f] outline-none focus:border-[#007aff]"
            autoComplete="off"
            spellCheck={false}
          />
        </label>
        <button
          type="button"
          disabled={saving}
          onClick={() => { void save() }}
          className="shrink-0 self-start rounded-full bg-[#007aff] px-[16px] py-[8px] text-xs font-semibold text-white disabled:opacity-50 sm:self-center"
        >
          {saving ? "保存中…" : "保存端点"}
        </button>
      </div>
    </div>
  )
}

/** M3U 端点输入行：配置后由后端真实解析播放列表并参与视频搜索。 */
function M3uEndpointRow({
  source,
  showNotice,
  onChanged,
}: {
  source: SourceDescriptorWire
  showNotice: (message: string) => void
  onChanged: () => void
}) {
  const [endpoint, setEndpoint] = useState("")
  const [saving, setSaving] = useState(false)
  const save = async () => {
    setSaving(true)
    try {
      const result = await setSourceEndpoint({ sourceId: source.sourceId, endpoint })
      showNotice(result.endpointConfigured ? "M3U 地址已保存" : "M3U 地址已清除")
      onChanged()
    } catch (error) {
      showNotice(toHavenError(error).dto.userMessage)
    } finally {
      setSaving(false)
    }
  }
  return (
    <div className="flex flex-col gap-2 border-t border-black/[0.06] px-5 py-4 dark:border-white/[0.06]">
      <p className="text-xs font-semibold text-[#1d1d1f] dark:text-[#f5f5f5]">M3U 播放列表地址</p>
      <p className="text-[11px] leading-5 text-[#86868b]">填入以 http:// 或 https:// 开头的播放列表地址。保存后可按频道名称搜索；播放地址不会显示在搜索结果中。</p>
      <div className="flex flex-col gap-2 sm:flex-row sm:items-center">
        <input
          value={endpoint}
          onChange={(event) => { setEndpoint(event.target.value) }}
          placeholder="https://example.org/playlist.m3u"
          maxLength={500}
          autoComplete="off"
          spellCheck={false}
          className="min-w-0 flex-1 rounded-xl border border-black/[0.12] bg-white px-3 py-2 font-mono text-xs text-[#1d1d1f] outline-none focus:border-[#007aff] dark:border-white/[0.12] dark:bg-[#2c2c2e] dark:text-[#f5f5f5]"
          aria-label="M3U 播放列表地址"
        />
        <button
          type="button"
          disabled={saving}
          onClick={() => { void save() }}
          className="shrink-0 self-start rounded-full bg-[#007aff] px-4 py-2 text-xs font-semibold text-white disabled:opacity-50 sm:self-auto"
        >
          {saving ? "保存中…" : "保存地址"}
        </button>
      </div>
    </div>
  )
}

type SourceCategoryTone = {
  icon: typeof PlaySquare
  accent: string
  soft: string
  darkSoft: string
}

const SOURCE_CATEGORY_TONES: Record<SourceCategoryDto, SourceCategoryTone> = {
  video: {
    icon: PlaySquare,
    accent: "text-[#ff9500] dark:text-[#ffb340]",
    soft: "bg-[#fff7e8] border-[#ff9500]/20",
    darkSoft: "dark:bg-[#3a2b16] dark:border-[#ff9500]/25",
  },
  book: {
    icon: BookOpen,
    accent: "text-[#007aff] dark:text-[#5aa9ff]",
    soft: "bg-[#edf6ff] border-[#007aff]/20",
    darkSoft: "dark:bg-[#132b45] dark:border-[#007aff]/25",
  },
  comic: {
    icon: PanelsTopLeft,
    accent: "text-[#af52de] dark:text-[#d28bef]",
    soft: "bg-[#f8effc] border-[#af52de]/20",
    darkSoft: "dark:bg-[#34203e] dark:border-[#af52de]/25",
  },
  periodical: {
    icon: FileText,
    accent: "text-[#34a853] dark:text-[#64d98b]",
    soft: "bg-[#edf9f0] border-[#34a853]/20",
    darkSoft: "dark:bg-[#183522] dark:border-[#34a853]/25",
  },
}

const SOURCE_MODE_TONES: Record<SourceModeDto, {
  icon: typeof Server
  accent: string
  rail: string
  soft: string
  darkSoft: string
}> = {
  collection: {
    icon: Server,
    accent: "text-[#007aff] dark:text-[#5aa9ff]",
    rail: "bg-[#007aff]",
    soft: "bg-[#edf6ff] border-[#007aff]/20",
    darkSoft: "dark:bg-[#132b45] dark:border-[#007aff]/25",
  },
  single: {
    icon: Globe2,
    accent: "text-[#34a853] dark:text-[#64d98b]",
    rail: "bg-[#34a853]",
    soft: "bg-[#edf9f0] border-[#34a853]/20",
    darkSoft: "dark:bg-[#183522] dark:border-[#34a853]/25",
  },
}

const SOURCE_HEALTH_TONES: Record<string, { dot: string; pill: string }> = {
  unknown: { dot: "bg-[#8e8e93]", pill: "bg-black/[0.05] text-[#6e6e73] dark:bg-white/[0.08] dark:text-[#c7c7cc]" },
  ok: { dot: "bg-[#34c759]", pill: "bg-[#34c759]/[0.12] text-[#248a3d] dark:bg-[#34c759]/[0.18] dark:text-[#8de6a4]" },
  degraded: { dot: "bg-[#ff9500]", pill: "bg-[#ff9500]/[0.12] text-[#a85d00] dark:bg-[#ff9500]/[0.18] dark:text-[#ffc266]" },
  down: { dot: "bg-[#ff3b30]", pill: "bg-[#ff3b30]/[0.12] text-[#c9342b] dark:bg-[#ff3b30]/[0.18] dark:text-[#ff8b84]" },
}

function SourcesSettings({ showNotice }: { showNotice: (message: string) => void }) {
  const [registry, setRegistry] = useState<SourceRegistryDto | null>(null)
  const [isLoading, setIsLoading] = useState(true)
  const [loadError, setLoadError] = useState<HavenError | null>(null)
  const [togglingId, setTogglingId] = useState<string | null>(null)
  const [category, setCategory] = useState<SourceCategoryDto | "all">("all")

  const load = useCallback(async () => {
    setIsLoading(true)
    setLoadError(null)
    try {
      setRegistry(await listSources())
    } catch (error) {
      setLoadError(toHavenError(error))
    } finally {
      setIsLoading(false)
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  const toggleSource = async (source: SourceDescriptorWire) => {
    setTogglingId(source.sourceId)
    try {
      const result = await setSourceEnabled({
        sourceId: source.sourceId,
        enabled: !source.enabled,
      })
      showNotice(result.enabled ? `${source.displayName} 已启用` : `${source.displayName} 已停用`)
      await load()
    } catch (error) {
      showNotice(toHavenError(error).dto.userMessage)
    } finally {
      setTogglingId(null)
    }
  }

  const builtinSources = useMemo(
    () => (registry?.sources ?? []).filter((source) => !isCustomSourceId(source.sourceId)),
    [registry],
  )
  const categoryCounts = useMemo(() => {
    const counts = {} as Record<SourceCategoryDto, number>
    SOURCE_CATEGORY_ORDER.forEach((item) => {
      counts[item] = builtinSources.filter((source) => source.categories.includes(item)).length
    })
    return counts
  }, [builtinSources])
  return (
    <>
      <SettingsIntro section="Sources" title="来源" description="来源负责回答内容在哪里。把它们当作目录来管理：先看来源类型，再看它能做什么，最后决定是否启用。" />

      <SettingsGroup title="按内容类型浏览" description="一个来源可以同时属于多个类型；数字表示当前内置目录中的来源数量。">
        <div id="source-category-tabs" className="grid grid-cols-2 gap-2 p-4 sm:grid-cols-5" role="tablist" aria-label="来源内容类型">
          <button
            type="button"
            role="tab"
            aria-selected={category === "all"}
            aria-controls="source-catalog"
            onClick={() => setCategory("all")}
            className={cn(
              "group relative flex min-h-[64px] flex-col items-center justify-center gap-1 rounded-2xl border px-2 py-2 text-center transition-all duration-200",
              category === "all"
                ? "border-[#1d1d1f] bg-[#1d1d1f] text-white shadow-[0_6px_14px_rgba(29,29,31,0.16)] dark:border-white dark:bg-white dark:text-[#1d1d1f]"
                : "border-black/[0.06] bg-black/[0.025] text-[#6e6e73] hover:border-black/[0.14] hover:bg-black/[0.05] dark:border-white/[0.08] dark:bg-white/[0.04] dark:text-[#c7c7cc] dark:hover:bg-white/[0.07]",
            )}
          >
            <span className={cn("flex h-[28px] w-[28px] shrink-0 items-center justify-center rounded-[9px]", category === "all" ? "bg-white/[0.14] dark:bg-black/[0.08]" : "bg-black/[0.05] dark:bg-white/[0.08]")}>
              <Plug className="h-[16px] w-[16px]" strokeWidth={1.8} />
            </span>
            <span className="block max-w-full whitespace-nowrap text-[11px] font-semibold tracking-[-0.01em]">全部来源</span>
            <span className={cn("absolute right-1.5 top-1.5 rounded-full px-1.5 py-0.5 text-[9px] font-semibold tabular-nums", category === "all" ? "bg-white/[0.14] dark:bg-black/[0.08]" : "bg-black/[0.06] dark:bg-white/[0.1]")}>{registry ? builtinSources.length : "—"}</span>
          </button>
          {SOURCE_CATEGORY_ORDER.map((item) => {
            const Icon = SOURCE_CATEGORY_TONES[item].icon
            const active = category === item
            const tone = SOURCE_CATEGORY_TONES[item]
            return (
              <button
                key={item}
                type="button"
                role="tab"
                aria-selected={active}
                aria-controls="source-catalog"
                onClick={() => setCategory(item)}
                className={cn(
                  "group relative flex min-h-[64px] flex-col items-center justify-center gap-1 rounded-2xl border px-2 py-2 text-center transition-all duration-200",
                  active ? `${tone.soft} ${tone.darkSoft} shadow-[0_6px_14px_rgba(29,29,31,0.08)]` : "border-black/[0.06] bg-black/[0.025] hover:border-black/[0.14] hover:bg-black/[0.05] dark:border-white/[0.08] dark:bg-white/[0.04] dark:hover:bg-white/[0.07]",
                )}
              >
                <span className={cn("flex h-[28px] w-[28px] shrink-0 items-center justify-center rounded-[9px] bg-black/[0.05] dark:bg-white/[0.08]", active ? tone.accent : "text-[#6e6e73] dark:text-[#a1a1a6]")}>
                  <Icon className="h-[16px] w-[16px]" strokeWidth={1.8} />
                </span>
                <span className={cn("block max-w-full whitespace-nowrap text-[11px] font-semibold tracking-[-0.01em]", active ? tone.accent : "text-[#1d1d1f] dark:text-[#f5f5f5]")}>{SOURCE_CATEGORY_LABELS[item]}</span>
                <span className={cn("absolute right-1.5 top-1.5 rounded-full px-1.5 py-0.5 text-[9px] font-semibold tabular-nums", active ? `${tone.soft} ${tone.darkSoft} ${tone.accent}` : "bg-black/[0.06] text-[#86868b] dark:bg-white/[0.1]")}>{registry ? categoryCounts[item] : "—"}</span>
              </button>
            )
          })}
        </div>
        {category !== "all" && <p className="border-t border-black/[0.05] px-5 py-3 text-xs text-[#86868b] dark:border-white/[0.06]">{SOURCE_CATEGORY_DESCRIPTIONS[category]}</p>}
      </SettingsGroup>

      {isLoading && <SettingsGroup title="来源目录"><div className="flex items-center gap-3 px-5 py-7 text-sm text-[#86868b]"><span className="h-[8px] w-[8px] animate-pulse rounded-full bg-[#007aff]" />正在读取内置来源清单…</div></SettingsGroup>}
      {loadError && <SettingsGroup title="来源目录"><div className="flex flex-col gap-3 px-5 py-7"><div className="flex items-start gap-3"><span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-xl bg-[#ff9500]/[0.12] text-[#a85d00] dark:bg-[#ff9500]/[0.18] dark:text-[#ffc266]"><TriangleAlert className="h-4 w-4" /></span><p className="pt-1 text-sm font-semibold text-[#a85d00] dark:text-[#ffc266]">{loadError.dto.userMessage}</p></div><button type="button" onClick={() => { void load() }} className="self-start rounded-full bg-[#007aff] px-4 py-2 text-xs font-semibold text-white transition-colors hover:bg-[#006fe6]">重新读取来源</button></div></SettingsGroup>}
      {registry && !isLoading && !loadError && (
        <>
          {(["collection", "single"] as const).map((mode) => {
            const sources = builtinSources
              .filter((source) => !isCustomSourceId(source.sourceId) && source.mode === mode)
              .filter((source) => sourceMatchesCategory(source, category))
            return (
              <SourceModeSection
                key={mode}
                mode={mode}
                sources={sources}
                togglingId={togglingId}
                onToggle={toggleSource}
                showNotice={showNotice}
                onChanged={() => { void load() }}
              />
            )
          })}
        </>
      )}
      <SettingsGroup title="我的来源" description="这里显示你自己添加的 OPDS 书库。自定义来源默认停用，不会在未确认前参与搜索。">
        <CustomSourceManager registry={registry} category={category} showNotice={showNotice} onChanged={() => { void load() }} />
      </SettingsGroup>
    </>
  )
}

function SourceModeSection({
  mode,
  sources,
  togglingId,
  onToggle,
  showNotice,
  onChanged,
}: {
  mode: SourceModeDto
  sources: SourceDescriptorWire[]
  togglingId: string | null
  onToggle: (source: SourceDescriptorWire) => void
  showNotice: (message: string) => void
  onChanged: () => void
}) {
  const columnTemplate = mode === "collection"
    ? "md:grid-cols-[32px_minmax(0,2.25fr)_minmax(0,0.72fr)_minmax(0,1fr)_minmax(0,1.05fr)_40px]"
    : "md:grid-cols-[minmax(0,2.15fr)_minmax(0,0.72fr)_minmax(0,1fr)_minmax(0,1.05fr)_40px]"
  return (
    <section id={mode === "collection" ? "source-catalog" : undefined} className="mb-7 last:mb-0">
      <div className="mb-2 flex items-end gap-3 px-2">
        <div className="min-w-0 flex-1">
          <h3 className="text-[13px] font-semibold tracking-[-0.01em] text-[#1d1d1f] dark:text-[#f5f5f5]">{SOURCE_MODE_LABELS[mode]}</h3>
          <p className="mt-0.5 text-[11px] leading-5 text-[#86868b] dark:text-[#8e8e93]">{SOURCE_MODE_DESCRIPTIONS[mode]}</p>
        </div>
        <span className="shrink-0 rounded-full bg-black/[0.04] px-2.5 py-1 text-[10px] font-semibold tabular-nums text-[#86868b] dark:bg-white/[0.08] dark:text-[#a1a1a6]">{sources.length} 个来源</span>
      </div>
      <div className="relative overflow-visible rounded-[16px] border border-black/[0.06] bg-white shadow-[0_3px_12px_rgba(0,0,0,0.035)] dark:border-white/[0.08] dark:bg-[#1c1c1e] dark:shadow-[0_3px_12px_rgba(0,0,0,0.2)]">
        {sources.length === 0 ? (
          <div className="flex min-h-[118px] flex-col items-center justify-center gap-2 px-5 py-7 text-center">
            <p className="text-sm font-semibold text-[#6e6e73] dark:text-[#c7c7cc]">这个分类暂时没有{SOURCE_MODE_LABELS[mode]}</p>
            <p className="text-[11px] text-[#a1a1a6]">添加来源后，它会出现在这里。</p>
          </div>
        ) : (
          <>
            <div className={cn("hidden items-center gap-x-3 border-b border-black/[0.06] bg-black/[0.02] px-4 py-2.5 text-[10px] font-semibold tracking-[0.08em] text-[#86868b] dark:border-white/[0.06] dark:bg-white/[0.03] dark:text-[#8e8e93] md:grid", columnTemplate)}>
              {mode === "collection" && <span className="text-center">#</span>}
              <span>名称</span>
              <span>类型</span>
              <span>状态</span>
              <span>能力</span>
              <span className="text-right">操作</span>
            </div>
            <div className="divide-y divide-black/[0.06] dark:divide-white/[0.06]">
              {sources.map((source, index) => (
                <SourceCard
                  key={source.sourceId}
                  source={source}
                  index={mode === "collection" ? index + 1 : null}
                  columnTemplate={columnTemplate}
                  toggling={togglingId === source.sourceId}
                  onToggle={() => onToggle(source)}
                  showNotice={showNotice}
                  onChanged={onChanged}
                />
              ))}
            </div>
          </>
        )}
      </div>
    </section>
  )
}

function SourceCard({
  source,
  index,
  columnTemplate,
  toggling,
  onToggle,
  showNotice,
  onChanged,
}: {
  source: SourceDescriptorWire
  index: number | null
  columnTemplate: string
  toggling: boolean
  onToggle: () => void
  showNotice: (message: string) => void
  onChanged: () => void
}) {
  const healthLabel = SOURCE_HEALTH_LABELS[source.health] ?? "未检测"
  const healthTone = SOURCE_HEALTH_TONES[source.health] ?? SOURCE_HEALTH_TONES.unknown
  const modeTone = SOURCE_MODE_TONES[source.mode]
  const ModeIcon = modeTone.icon
  const supportsEndpoint = sourceUsesConfiguredEndpoint(source.sourceId)
  const [detailsOpen, setDetailsOpen] = useState(false)
  const [endpointOpen, setEndpointOpen] = useState(false)
  return (
    <div className="group relative px-4 py-3.5 transition-colors hover:bg-black/[0.018] dark:hover:bg-white/[0.025]">
      <span className={cn("absolute inset-y-0 left-0 w-0.5 opacity-90", modeTone.rail)} aria-hidden="true" />
      <div className={cn("grid min-w-0 gap-3 md:items-center md:gap-x-3", columnTemplate)}>
        {index !== null && <span className="hidden text-center text-[11px] font-semibold tabular-nums text-[#007aff] md:block">{String(index).padStart(2, "0")}</span>}
        <div className="min-w-0 flex items-start gap-3">
          <span className={cn("mt-0.5 flex h-[32px] w-[32px] shrink-0 items-center justify-center rounded-xl border", modeTone.soft, modeTone.darkSoft, modeTone.accent)}><ModeIcon className="h-[16px] w-[16px]" strokeWidth={1.8} /></span>
          <div className="min-w-0">
            <div className="flex min-w-0 flex-wrap items-center gap-1.5">
              <p className="min-w-0 max-w-full truncate text-[13px] font-semibold tracking-[-0.01em] text-[#1d1d1f] dark:text-[#f5f5f5]">{source.displayName}</p>
              <span className={cn("max-w-[74px] truncate rounded-md px-1.5 py-0.5 text-[9px] font-semibold", modeTone.soft, modeTone.darkSoft, modeTone.accent)}>{SOURCE_MODE_LABELS[source.mode]}</span>
            </div>
            <p className="mt-1 truncate text-[11px] text-[#86868b] dark:text-[#8e8e93]">{source.notes}</p>
            {detailsOpen && <div className="mt-2 rounded-xl border border-black/[0.05] bg-black/[0.025] px-3 py-2 text-[11px] leading-5 text-[#6e6e73] dark:border-white/[0.07] dark:bg-white/[0.04] dark:text-[#a1a1a6]">维护备注：{source.notes}</div>}
          </div>
        </div>
        <div className="min-w-0 flex items-center gap-2 md:block">
          <span className="mr-1 text-[10px] text-[#a1a1a6] md:hidden">类型</span>
          <div className="flex flex-wrap gap-1">
            {source.categories.map((item) => <span key={item} className={cn("rounded-md border px-1.5 py-0.5 text-[10px] font-medium", SOURCE_CATEGORY_TONES[item].soft, SOURCE_CATEGORY_TONES[item].darkSoft, SOURCE_CATEGORY_TONES[item].accent)}>{SOURCE_CATEGORY_LABELS[item]}</span>)}
          </div>
        </div>
        <div className="min-w-0 flex items-center gap-2 md:block">
          <span className="mr-1 text-[10px] text-[#a1a1a6] md:hidden">状态</span>
          <span className={cn("inline-flex items-center gap-1.5 rounded-full px-2 py-1 text-[10px] font-semibold", healthTone.pill)}>
            <span className={cn("h-[6px] w-[6px] rounded-full", healthTone.dot)} />
            {source.enabled ? "已启用" : "已停用"}
          </span>
          <span className="ml-1 whitespace-nowrap text-[10px] text-[#86868b] md:ml-0 md:block">{healthLabel}</span>
          {source.lastChecked && <span className="ml-1 hidden whitespace-nowrap text-[10px] text-[#a1a1a6] xl:inline">· {new Date(source.lastChecked).toLocaleDateString()}</span>}
        </div>
        <div className="min-w-0 flex items-center gap-1.5 md:flex-wrap">
          <span className="mr-1 text-[10px] text-[#a1a1a6] md:hidden">能力</span>
          {source.kinds.map((kind: SourceKindDto) => {
            const Icon = kind === "online_read" ? PlaySquare : kind === "offline_download" ? Download : Search
            return <span key={kind} title={SOURCE_KIND_LABELS[kind]} className="inline-flex h-[28px] w-[28px] items-center justify-center rounded-lg border border-black/[0.06] bg-black/[0.025] text-[#6e6e73] dark:border-white/[0.07] dark:bg-white/[0.05] dark:text-[#c7c7cc]"><Icon aria-hidden="true" className="h-[14px] w-[14px]" strokeWidth={1.8} /><span className="sr-only">{SOURCE_KIND_LABELS[kind]}</span></span>
          })}
          {supportsEndpoint && <span className="ml-1 truncate text-[10px] text-[#86868b]">{source.endpointConfigured ? "接口已配" : "待配置"}</span>}
        </div>
        <div className="flex items-center justify-end gap-1 md:gap-1">
          <SourceActionMenu
            source={source}
            detailsOpen={detailsOpen}
            endpointOpen={endpointOpen}
            busy={toggling}
            onToggle={onToggle}
            onToggleDetails={() => setDetailsOpen((current) => !current)}
            onToggleEndpoint={() => setEndpointOpen((current) => !current)}
          />
        </div>
      </div>
      {source.sourceId === "cms10" && (source.enabled || endpointOpen) && <Cms10EndpointRow source={source} showNotice={showNotice} onChanged={onChanged} />}
      {source.sourceId === "m3u" && (source.enabled || endpointOpen) && <M3uEndpointRow source={source} showNotice={showNotice} onChanged={onChanged} />}
    </div>
  )
}

function SourceActionMenu({
  source,
  detailsOpen,
  endpointOpen,
  variant = "builtin",
  busy = false,
  onToggle,
  onToggleDetails,
  onToggleEndpoint,
  onEdit,
  onCredential,
  onRemove,
}: {
  source: SourceDescriptorWire
  detailsOpen: boolean
  endpointOpen: boolean
  variant?: "builtin" | "custom"
  busy?: boolean
  onToggle: () => void
  onToggleDetails?: () => void
  onToggleEndpoint?: () => void
  onEdit?: () => void
  onCredential?: () => void
  onRemove?: () => void
}) {
  const [open, setOpen] = useState(false)
  const menuRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return undefined
    const close = (event: PointerEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) setOpen(false)
    }
    document.addEventListener("pointerdown", close)
    return () => document.removeEventListener("pointerdown", close)
  }, [open])

  const select = (action: () => void) => {
    action()
    setOpen(false)
  }

  return (
    <div ref={menuRef} className="relative">
      <button type="button" aria-label={`${source.displayName} 更多操作`} aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen((current) => !current)} className={cn("flex h-[28px] w-[28px] items-center justify-center rounded-lg text-[#86868b] transition-colors hover:bg-black/[0.06] hover:text-[#1d1d1f] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#007aff]/40 dark:hover:bg-white/[0.08] dark:hover:text-[#f5f5f5]", open && "bg-black/[0.06] text-[#1d1d1f] dark:bg-white/[0.08] dark:text-[#f5f5f5]")}>
        <MoreHorizontal className="h-[16px] w-[16px]" strokeWidth={2} />
      </button>
      {open && (
        <div role="menu" className="absolute right-0 top-9 z-30 min-w-[164px] overflow-hidden rounded-xl border border-black/[0.08] bg-white p-1 shadow-[0_10px_28px_rgba(0,0,0,0.14)] dark:border-white/[0.1] dark:bg-[#2c2c2e] dark:shadow-[0_10px_28px_rgba(0,0,0,0.45)]">
          <button type="button" role="menuitem" disabled={busy} onClick={() => select(onToggle)} className="flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left text-xs text-[#1d1d1f] hover:bg-black/[0.05] disabled:cursor-not-allowed disabled:opacity-50 dark:text-[#f5f5f5] dark:hover:bg-white/[0.08]"><Check className="h-[14px] w-[14px] shrink-0 text-[#34a853]" />{busy ? "处理中…" : source.enabled ? "停用来源" : "启用来源"}</button>
          {variant === "builtin" ? (
            <>
              <button type="button" role="menuitem" onClick={() => select(() => onToggleDetails?.())} className="flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left text-xs text-[#1d1d1f] hover:bg-black/[0.05] dark:text-[#f5f5f5] dark:hover:bg-white/[0.08]"><Info className="h-[14px] w-[14px] text-[#007aff]" />{detailsOpen ? "收起维护说明" : "查看维护说明"}</button>
              {(source.sourceId === "cms10" || source.sourceId === "m3u") && <button type="button" role="menuitem" onClick={() => select(() => onToggleEndpoint?.())} className="flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left text-xs text-[#1d1d1f] hover:bg-black/[0.05] dark:text-[#f5f5f5] dark:hover:bg-white/[0.08]"><Settings2 className="h-[14px] w-[14px] text-[#af52de]" />{endpointOpen ? "收起接口配置" : "配置接口"}</button>}
            </>
          ) : (
            <>
              {onEdit && <button type="button" role="menuitem" onClick={() => select(onEdit)} className="flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left text-xs text-[#1d1d1f] hover:bg-black/[0.05] dark:text-[#f5f5f5] dark:hover:bg-white/[0.08]"><Settings2 className="h-[14px] w-[14px] text-[#007aff]" />编辑来源</button>}
              {onCredential && <button type="button" role="menuitem" onClick={() => select(onCredential)} className="flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left text-xs text-[#1d1d1f] hover:bg-black/[0.05] dark:text-[#f5f5f5] dark:hover:bg-white/[0.08]"><LockKeyhole className="h-[14px] w-[14px] text-[#af52de]" />配置访问凭据</button>}
              {onRemove && <>
                <div className="my-1 h-px bg-black/[0.06] dark:bg-white/[0.08]" aria-hidden="true" />
                <button type="button" role="menuitem" onClick={() => select(onRemove)} className="flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left text-xs text-[#c9342b] hover:bg-[#ff3b30]/[0.08] dark:text-[#ff8b84] dark:hover:bg-[#ff3b30]/[0.12]"><TriangleAlert className="h-[14px] w-[14px]" />删除来源</button>
              </>}
            </>
          )}
        </div>
      )}
    </div>
  )
}

/** 自定义 OPDS 书源管理（V2-H 收尾批次）：添加 / 编辑 / 删除 / 凭据录入。 */
function CustomSourceManager({
  registry,
  category,
  showNotice,
  onChanged,
}: {
  registry: SourceRegistryDto | null
  category: SourceCategoryDto | "all"
  showNotice: (message: string) => void
  onChanged: () => void
}) {
  const { confirm } = useNotice()
  const [adding, setAdding] = useState(false)
  const [addStep, setAddStep] = useState<"type" | "details" | "credential" | "done">("type")
  const [displayName, setDisplayName] = useState("")
  const [endpoint, setEndpoint] = useState("")
  const [createdSourceId, setCreatedSourceId] = useState<string | null>(null)
  const [addError, setAddError] = useState<string | null>(null)
  const [submitting, setSubmitting] = useState(false)
  const [editingId, setEditingId] = useState<string | null>(null)
  const [editName, setEditName] = useState("")
  const [editEndpoint, setEditEndpoint] = useState("")
  const [credentialFor, setCredentialFor] = useState<string | null>(null)
  const [credentialValue, setCredentialValue] = useState("")
  const [credentialConfigured, setCredentialConfigured] = useState(false)
  const [customTogglingId, setCustomTogglingId] = useState<string | null>(null)

  const customSources = (registry?.sources ?? [])
    .filter((s) => isCustomSourceId(s.sourceId))
    .filter((s) => category === "all" || s.categories.includes(category))

  const resetAddForm = () => {
    setAdding(false)
    setAddStep("type")
    setDisplayName("")
    setEndpoint("")
    setCreatedSourceId(null)
    setAddError(null)
  }

  const submitAdd = async () => {
    if (submitting) return
    setSubmitting(true)
    try {
      const result = await addSource({ displayName, endpoint })
      setCreatedSourceId(result.sourceId)
      setAddError(null)
      setAddStep("credential")
      showNotice("来源已添加，可以现在配置凭据，也可以稍后再配")
      onChanged()
    } catch (error) {
      const message = toHavenError(error).dto.userMessage
      setAddError(message)
      showNotice(message)
    } finally {
      setSubmitting(false)
    }
  }

  const submitEdit = async (sourceId: string) => {
    if (submitting) return
    setSubmitting(true)
    try {
      await updateSource({ sourceId, displayName: editName || null, endpoint: editEndpoint || null })
      showNotice("来源已更新")
      setEditingId(null)
      onChanged()
    } catch (error) {
      showNotice(toHavenError(error).dto.userMessage)
    } finally {
      setSubmitting(false)
    }
  }

  const submitRemove = async (sourceId: string) => {
    const confirmed = await confirm({
      title: "删除自定义来源",
      message: "删除该自定义来源？其凭据将从系统凭据管理器一并清除。",
      confirmLabel: "删除",
      cancelLabel: "取消",
      dedupeKey: `settings:source:${sourceId}:remove-confirm`,
    })
    if (!confirmed) return
    try {
      await removeSource({ sourceId })
      showNotice("来源已删除")
      onChanged()
    } catch (error) {
      showNotice(toHavenError(error).dto.userMessage)
    }
  }

  const toggleCustomSource = async (source: SourceDescriptorWire) => {
    setCustomTogglingId(source.sourceId)
    try {
      const result = await setSourceEnabled({ sourceId: source.sourceId, enabled: !source.enabled })
      showNotice(result.enabled ? `${source.displayName} 已启用` : `${source.displayName} 已停用`)
      onChanged()
    } catch (error) {
      showNotice(toHavenError(error).dto.userMessage)
    } finally {
      setCustomTogglingId(null)
    }
  }

  const submitCredential = async (sourceId: string) => {
    if (submitting) return
    setSubmitting(true)
    try {
      await setSourceCredential({
        sourceId,
        secret: credentialValue.length > 0 ? credentialValue : null,
      })
      setCredentialConfigured(credentialValue.length > 0)
      setCredentialValue("")
      showNotice(credentialValue.length > 0 ? "凭据已保存到系统凭据管理器" : "凭据已清除")
      if (adding) setAddStep("done")
      onChanged()
    } catch (error) {
      showNotice(toHavenError(error).dto.userMessage)
    } finally {
      setSubmitting(false)
    }
  }

  if (adding) {
    return (
      <div className="rounded-3xl border border-black/[0.08] bg-white/70 px-5 py-5 dark:border-white/[0.08] dark:bg-[#1c1c1e]/70">
        <div className="flex items-start justify-between gap-4">
          <div>
            <p className="text-sm font-semibold">添加来源</p>
            <p className="mt-1 text-xs text-[#86868b]">跟着三步完成设置；新来源默认停用，确认无误后再启用。</p>
          </div>
          <button type="button" onClick={resetAddForm} className="text-xs font-semibold text-[#6e6e73] hover:text-[#1d1d1f]">取消</button>
        </div>
        <div className="mt-4 flex items-center gap-2" aria-label="添加来源步骤">
          {["选择类型", "填写信息", "配置凭据"].map((label, index) => {
            const stepIndex = addStep === "type" ? 0 : addStep === "details" ? 1 : 2
            return <span key={label} className={cn("rounded-full px-3 py-1 text-[11px] font-semibold", index <= stepIndex ? "bg-[#007aff]/[0.12] text-[#007aff]" : "bg-black/[0.05] text-[#86868b]")}>{index + 1}. {label}</span>
          })}
        </div>
        {addStep === "type" && (
          <div className="mt-5 grid gap-3 sm:grid-cols-2">
            <button type="button" disabled className="rounded-2xl border border-black/[0.08] bg-black/[0.025] p-4 text-left opacity-60 dark:border-white/[0.08] dark:bg-white/[0.04]">
              <span className="text-sm font-semibold">聚合来源</span>
              <span className="mt-1 block text-xs leading-5 text-[#86868b]">一个入口管理多个上游。自定义聚合来源暂未开放。</span>
              <span className="mt-2 inline-flex rounded-full bg-black/[0.06] px-2 py-1 text-[10px] font-semibold text-[#86868b]">暂不可用</span>
            </button>
            <button type="button" onClick={() => setAddStep("details")} className="rounded-2xl border border-[#007aff]/25 bg-[#007aff]/[0.05] p-4 text-left transition-colors hover:border-[#007aff]">
              <span className="text-sm font-semibold text-[#007aff]">单一来源</span>
              <span className="mt-1 block text-xs leading-5 text-[#6e6e73]">一个地址对应一个 OPDS 目录，适合个人书库或 Calibre-Web。</span>
              <span className="mt-2 inline-flex rounded-full bg-[#007aff]/[0.12] px-2 py-1 text-[10px] font-semibold text-[#007aff]">开始填写</span>
            </button>
          </div>
        )}
        {addStep === "details" && (
          <div className="mt-5 flex flex-col gap-4">
            <label className="flex flex-col gap-1.5">
              <span className="text-xs font-semibold">来源名称</span>
              <span className="text-[11px] text-[#86868b]">给自己看的名字，例如“我的 Calibre 书库”。</span>
              <input type="text" value={displayName} onChange={(e) => { setDisplayName(e.target.value); setAddError(null) }} maxLength={100} placeholder="我的 Calibre 书库" autoComplete="off" className="rounded-2xl border border-black/[0.12] bg-white px-4 py-2.5 text-sm outline-none focus:border-[#007aff] dark:border-white/[0.12] dark:bg-[#2c2c2e]" />
            </label>
            <label className="flex flex-col gap-1.5">
              <span className="text-xs font-semibold">OPDS 目录地址</span>
              <span className="text-[11px] leading-5 text-[#86868b]">这是书库的 OPDS/目录地址，不是浏览器首页；必须以 http:// 或 https:// 开头。</span>
              <input type="url" value={endpoint} onChange={(e) => { setEndpoint(e.target.value); setAddError(null) }} maxLength={500} placeholder="https://example.org/opds/" autoComplete="url" spellCheck={false} className="rounded-2xl border border-black/[0.12] bg-white px-4 py-2.5 text-sm outline-none focus:border-[#007aff] dark:border-white/[0.12] dark:bg-[#2c2c2e]" />
            </label>
            {addError && <p role="alert" className="rounded-xl bg-[#fff1f0] px-3 py-2 text-xs text-[#b42318]">{addError}</p>}
            <div className="flex justify-end gap-2 text-xs font-semibold">
              <button type="button" onClick={() => setAddStep("type")} className="rounded-full border border-black/[0.12] px-4 py-2">上一步</button>
              <button type="button" disabled={submitting || displayName.trim().length === 0 || endpoint.trim().length === 0} onClick={() => { void submitAdd() }} className="rounded-full bg-[#1d1d1f] px-4 py-2 text-white disabled:opacity-50">{submitting ? "添加中…" : "继续"}</button>
            </div>
          </div>
        )}
        {addStep === "credential" && createdSourceId && (
          <div className="mt-5 flex flex-col gap-4">
            <div className="rounded-2xl bg-[#f5f5f7] p-4 dark:bg-white/[0.06]">
              <p className="text-sm font-semibold">需要登录吗？</p>
              <p className="mt-1 text-xs leading-5 text-[#6e6e73]">如果书库需要密码，可以现在填写；凭据只保存到系统凭据管理器，不会出现在来源列表或日志中。</p>
            </div>
            <label className="flex flex-col gap-1.5">
              <span className="text-xs font-semibold">访问凭据（可选）</span>
              <input type="password" value={credentialValue} onChange={(e) => setCredentialValue(e.target.value)} autoComplete="new-password" placeholder="不需要登录可留空" className="rounded-2xl border border-black/[0.12] bg-white px-4 py-2.5 text-sm outline-none focus:border-[#007aff] dark:border-white/[0.12] dark:bg-[#2c2c2e]" />
            </label>
            <div className="flex justify-end gap-2 text-xs font-semibold">
              <button type="button" disabled={submitting} onClick={() => { setCredentialValue(""); setAddStep("done") }} className="rounded-full border border-black/[0.12] px-4 py-2">稍后配置</button>
              <button type="button" disabled={submitting || credentialValue.length === 0} onClick={() => { void submitCredential(createdSourceId) }} className="rounded-full bg-[#1d1d1f] px-4 py-2 text-white disabled:opacity-50">{submitting ? "保存中…" : "保存并完成"}</button>
            </div>
          </div>
        )}
        {addStep === "done" && (
          <div className="mt-5 flex flex-col gap-4">
            <div className="rounded-2xl border border-[#34a853]/20 bg-[#edf8f0] p-4 dark:bg-[#17351f]">
              <p className="text-sm font-semibold text-[#216e32] dark:text-[#9be3aa]">来源已准备好</p>
              <p className="mt-1 text-xs leading-5 text-[#4f7659] dark:text-[#a7cfad]">{displayName} · 单一来源 · OPDS 目录{credentialConfigured ? " · 凭据已配置" : " · 凭据稍后配置"}</p>
            </div>
            <div className="flex justify-end"><button type="button" onClick={resetAddForm} className="rounded-full bg-[#1d1d1f] px-4 py-2 text-xs font-semibold text-white">返回来源列表</button></div>
          </div>
        )}
      </div>
    )
  }

  return (
    <>
      {registry && customSources.length === 0 && (
        <p className="mb-3 rounded-2xl bg-black/[0.025] px-4 py-3 text-xs leading-5 text-[#86868b] dark:bg-white/[0.04] dark:text-[#a1a1a6]">
          {category === "all" || category === "book" ? "还没有添加自定义 OPDS 书库。" : "当前筛选下没有自定义来源；自定义 OPDS 书库统一归入图书。"}
        </p>
      )}
      {customSources.map((source) => {
        const editing = editingId === source.sourceId
        return (
          <div key={source.sourceId} className="group relative overflow-visible rounded-[16px] border border-black/[0.08] bg-white/70 dark:border-white/[0.1] dark:bg-[#1c1c1e]/80">
            <span className="absolute inset-y-0 left-0 w-0.5 bg-[#34a853] opacity-90" aria-hidden="true" />
            <div className="flex items-start gap-3 px-4 py-3.5">
              <span className="mt-0.5 flex h-[32px] w-[32px] shrink-0 items-center justify-center rounded-xl border border-[#34a853]/20 bg-[#edf9f0] text-[#34a853] dark:border-[#34a853]/25 dark:bg-[#183522] dark:text-[#64d98b]"><Globe2 className="h-[16px] w-[16px]" strokeWidth={1.8} /></span>
              <div className="min-w-0 flex-1">
                <div className="flex items-center justify-between gap-3">
                  <div>
                    <div className="flex min-w-0 flex-wrap items-center gap-1.5">
                      <p className="min-w-0 truncate text-[13px] font-semibold tracking-[-0.01em] text-[#1d1d1f] dark:text-[#f5f5f5]">{source.displayName}</p>
                      <span className="rounded-md bg-[#edf9f0] px-1.5 py-0.5 text-[9px] font-semibold text-[#248a3d] dark:bg-[#183522] dark:text-[#64d98b]">单一来源</span>
                    </div>
                    <p className="mt-1 truncate text-[11px] text-[#86868b] dark:text-[#8e8e93]">
                      图书 · 单一来源 · 自定义 OPDS 目录
                    </p>
                  </div>
                  <div className="flex shrink-0 items-center gap-2 text-xs font-semibold">
                    <span className="whitespace-nowrap text-[#86868b]">{source.enabled ? "已启用" : "已停用"}</span>
                    <SourceActionMenu
                      source={source}
                      variant="custom"
                      detailsOpen={false}
                      endpointOpen={false}
                      busy={customTogglingId === source.sourceId}
                      onToggle={() => { void toggleCustomSource(source) }}
                      onEdit={() => {
                        setEditingId(source.sourceId)
                        setEditName(source.displayName)
                        setEditEndpoint("")
                      }}
                      onCredential={() => {
                        setCredentialFor(source.sourceId)
                        setCredentialValue("")
                        setCredentialConfigured(false)
                      }}
                      onRemove={() => { void submitRemove(source.sourceId) }}
                    />
                  </div>
                </div>
                {editing && (
                  <div className="mt-3 flex flex-col gap-3">
                    <input
                      type="text"
                      value={editName}
                      onChange={(e) => { setEditName(e.target.value) }}
                      maxLength={100}
                      aria-label="显示名"
                      className="rounded-2xl border border-black/[0.12] bg-white px-4 py-2 text-sm text-[#1d1d1f] outline-none focus:border-[#007aff] dark:border-white/[0.12] dark:bg-[#2c2c2e] dark:text-[#f5f5f5]"
                    />
                    <input
                      type="url"
                      value={editEndpoint}
                      onChange={(e) => { setEditEndpoint(e.target.value) }}
                      maxLength={500}
                      placeholder="留空表示端点不变；填入新地址覆盖"
                      aria-label="OPDS 端点地址"
                      className="rounded-2xl border border-black/[0.12] bg-white px-4 py-2 text-sm text-[#1d1d1f] outline-none focus:border-[#007aff] dark:border-white/[0.12] dark:bg-[#2c2c2e] dark:text-[#f5f5f5] dark:placeholder:text-[#8e8e93]"
                    />
                    <div className="flex justify-end gap-2 text-xs font-semibold">
                      <button type="button" onClick={() => { setEditingId(null) }} className="rounded-full border border-black/[0.12] px-[16px] py-[8px] text-[#1d1d1f] dark:border-white/[0.12] dark:text-[#f5f5f5]">取消</button>
                      <button
                        type="button"
                        disabled={submitting || (editName.trim().length === 0 && editEndpoint.trim().length === 0)}
                        onClick={() => { void submitEdit(source.sourceId) }}
                        className="rounded-full bg-[#1d1d1f] px-[16px] py-[8px] text-white disabled:opacity-50 dark:bg-white dark:text-[#1d1d1f]"
                      >
                        保存
                      </button>
                    </div>
                  </div>
                )}
                {credentialFor === source.sourceId ? (
                  <div className="mt-3 flex flex-col gap-2">
                    <label className="flex flex-col gap-1">
                      <span className="text-xs text-[#86868b]">
                        访问密码（仅写入系统凭据管理器，不回显、不落库）
                      </span>
                      <input
                        type="password"
                        value={credentialValue}
                        onChange={(e) => { setCredentialValue(e.target.value) }}
                        autoComplete="new-password"
                        className="rounded-2xl border border-black/[0.12] bg-white px-4 py-2 text-sm text-[#1d1d1f] outline-none focus:border-[#007aff] dark:border-white/[0.12] dark:bg-[#2c2c2e] dark:text-[#f5f5f5]"
                      />
                    </label>
                    <div className="flex justify-end gap-2 text-xs font-semibold">
                      <button
                        type="button"
                        onClick={() => {
                          setCredentialFor(null)
                          setCredentialValue("")
                          setCredentialConfigured(false)
                        }}
                        className="rounded-full border border-black/[0.12] px-[14px] py-[6px] text-[#1d1d1f] dark:border-white/[0.12] dark:text-[#f5f5f5]"
                      >
                        收起
                      </button>
                      <button
                        type="button"
                        disabled={submitting}
                        onClick={() => { void submitCredential(source.sourceId) }}
                        className="rounded-full bg-[#1d1d1f] px-[14px] py-[6px] text-white disabled:opacity-50 dark:bg-white dark:text-[#1d1d1f]"
                      >
                        保存凭据
                      </button>
                      {credentialConfigured && (
                        <button
                          type="button"
                          disabled={submitting}
                          onClick={() => {
                            setCredentialValue("")
                            void submitCredential(source.sourceId).then(() => {
                              setCredentialConfigured(false)
                            })
                          }}
                          className="rounded-full px-[14px] py-[6px] text-[#d97706] dark:text-[#ffc266]"
                        >
                          清除凭据
                        </button>
                      )}
                    </div>
                  </div>
                ) : null}
              </div>
            </div>
          </div>
        )
      })}
      <div className="flex items-center justify-between rounded-3xl border border-dashed border-black/[0.12] bg-white/60 px-5 py-[16px] dark:border-white/[0.12] dark:bg-[#1c1c1e]/70">
        <div>
          <p className="text-sm font-semibold text-[#1d1d1f] dark:text-[#f5f5f5]">添加来源连接</p>
          <p className="mt-1 text-xs text-[#86868b] dark:text-[#8e8e93]">连接由用户拥有的存储或官方允许访问的来源。</p>
        </div>
        <button
          type="button"
          onClick={() => { setAdding(true) }}
          className="rounded-full bg-[#1d1d1f] px-[16px] py-[8px] text-xs font-semibold text-white"
        >
          添加来源
        </button>
      </div>
    </>
  )
}

/** 媒体库位置的扫描状态（设置页存储分组；SLICE-SCAN-001）。 */
interface ScanUiState {
  taskId: string | null
  phaseCode: string
  phase: string
  filesSeen: number
  newItem: number
  message: string | null
  terminal: boolean
}

function StorageSettings({ showNotice }: { showNotice: (message: string) => void }) {
  const [locations, setLocations] = useState<StorageLocationWire[]>([])
  const [scans, setScans] = useState<Record<string, ScanUiState>>({})
  const [removingId, setRemovingId] = useState<string | null>(null)
  const [cancellingTaskId, setCancellingTaskId] = useState<string | null>(null)
  const [isLoading, setIsLoading] = useState(true)
  const [loadError, setLoadError] = useState<HavenError | null>(null)
  const loadRequestRef = useRef(0)

  const loadLocations = async () => {
    const requestId = ++loadRequestRef.current
    setIsLoading(true)
    setLoadError(null)
    try {
      const nextLocations = await listStorageLocations()
      if (loadRequestRef.current === requestId) setLocations(nextLocations)
    } catch (error) {
      const normalized = toHavenError(error)
      if (loadRequestRef.current === requestId) {
        setLoadError(normalized)
        showNotice(normalized.message)
      }
    } finally {
      if (loadRequestRef.current === requestId) setIsLoading(false)
    }
  }

  useEffect(() => {
    void loadLocations()
    return () => {
      loadRequestRef.current += 1
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- showNotice 为稳定提示函数
  }, [])

  const handleAddDirectory = async () => {
    try {
      const locationId = await pickLocalDirectory()
      const items = await listStorageLocations()
      setLocations(items)
      const added = items.find((item) => item.locationId === locationId)
      showNotice(`已添加本地目录：${added?.displayName ?? ""}`)
    } catch (error) {
      showNotice(error instanceof Error ? error.message : "添加目录失败，请重试")
    }
  }

  const handleRebind = async (location: StorageLocationWire) => {
    try {
      await rebindLocalDirectory(location.locationId)
      const items = await listStorageLocations()
      setLocations(items)
      showNotice(`已重新绑定：${location.displayName}`)
    } catch (error) {
      showNotice(error instanceof Error ? error.message : "重新绑定失败，请重试")
    }
  }

  const handleRemove = async (location: StorageLocationWire) => {
    if (removingId !== location.locationId) {
      setRemovingId(location.locationId)
      return
    }
    setRemovingId(null)
    try {
      await removeStorageLocation(location.locationId)
      setLocations((prev) => prev.filter((item) => item.locationId !== location.locationId))
      setScans((prev) => {
        const next = { ...prev }
        delete next[location.locationId]
        return next
      })
      showNotice(`已移除：${location.displayName}（其内容退出媒体库，原始文件不受影响）`)
    } catch (error) {
      showNotice(error instanceof Error ? error.message : "移除失败，请重试")
    }
  }

  const handleScan = async (location: StorageLocationWire) => {
    try {
      const result = await startLibraryScan(location.locationId, (event) => {
        const label = SCAN_PHASE_LABELS[event.kind] ?? event.kind
        const terminal =
          event.kind === "completed" || event.kind === "cancelled" || event.kind === "failed"
        setScans((prev) => ({
          ...prev,
          [location.locationId]: {
            taskId: event.data.taskId,
            phaseCode: event.kind,
            phase: label,
            filesSeen: event.data.filesSeen,
            newItem: event.data.new,
            message: event.data.message ?? null,
            terminal,
          },
        }))
        if (terminal) showNotice(`${location.displayName}：${label}`)
      })
      setScans((prev) => ({
        ...prev,
        [location.locationId]: prev[location.locationId] ?? {
          taskId: result.taskId,
          phaseCode: "started",
          phase: SCAN_PHASE_LABELS.started,
          filesSeen: 0,
          newItem: 0,
          message: null,
          terminal: false,
        },
      }))
      if (result.alreadyRunning) showNotice("该目录已有扫描任务在运行，已合并到既有任务")
    } catch (error) {
      showNotice(error instanceof Error ? error.message : "扫描启动失败，请重试")
    }
  }

  const handleCancelScan = async (location: StorageLocationWire, scan: ScanUiState) => {
    if (!scan.taskId || cancellingTaskId) return
    setCancellingTaskId(scan.taskId)
    try {
      const result = await cancelScan(scan.taskId)
      const phase = result.phase
      setScans((prev) => ({
        ...prev,
        [location.locationId]: {
          ...scan,
          phaseCode: phase,
          phase: SCAN_PHASE_LABELS[phase] ?? phase,
          terminal: true,
        },
      }))
      showNotice(result.alreadyTerminal ? `${location.displayName}：扫描已经结束` : `${location.displayName}：已取消扫描`)
    } catch (error) {
      showNotice(error instanceof Error ? error.message : "取消扫描失败，请重试")
    } finally {
      setCancellingTaskId(null)
    }
  }

  const storageListState = deriveLibrarySliceState({
    loading: isLoading,
    itemCount: locations.length,
    error: loadError,
  })
  return (
    <>
      <SettingsIntro section="Storage" title="存储" description="统一管理媒体库位置、应用数据、缓存、下载内容和临时文件。" />
      <SettingsGroup title="媒体库位置" description="扫描已注册的本地目录，把影视、图书与漫画收进统一媒体库。">
        {storageListState.kind === "loading" && (
          <div data-slice-state="loading" className="px-5 py-[24px] text-sm text-[#86868b]">正在加载存储位置…</div>
        )}
        {storageListState.kind === "offline_partial" && (
          <div data-slice-state="offline_partial" className="flex items-center justify-between gap-4 px-5 py-[16px] text-sm text-[#8a5a00]">
            <span>{loadError?.message ?? "正在刷新可用存储位置"}</span>
            {storageListState.canRetry && <button type="button" onClick={() => void loadLocations()} className="font-semibold text-[#007aff]">重试</button>}
          </div>
        )}
        {(storageListState.kind === "retryable_error" || storageListState.kind === "terminal_error") && (
          <div data-slice-state={storageListState.kind} className="flex items-center justify-between gap-4 px-5 py-[24px] text-sm">
            <span>{storageListState.message}</span>
            {storageListState.canRetry && <button type="button" onClick={() => void loadLocations()} className="font-semibold text-[#007aff]">重试</button>}
          </div>
        )}
        {storageListState.kind === "empty" && (
          <div className="flex flex-col items-center justify-center px-5 py-[32px] text-center">
            <HardDrive className="h-6 w-6 text-[#86868b]" />
            <p className="mt-[10px] text-sm font-semibold">还没有注册媒体库位置</p>
            <p className="mt-1 max-w-sm text-xs leading-5 text-[#86868b]">添加一个本地目录（影片、电子书或漫画文件夹），栖阅会扫描并把内容整理进媒体库。</p>
          </div>
        )}
        {locations.map((location) => {
          const scan = scans[location.locationId]
          const running = scan !== undefined && !scan.terminal
          const storageState = deriveStorageSliceState(location.status)
          return (
            <div key={location.locationId} data-slice-state={storageState.kind} className="flex flex-col gap-[8px] border-b border-black/[0.06] px-5 py-[16px] last:border-b-0">
              <div className="flex items-center gap-3">
                <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-[#f2f2f4] text-[#6e6e73]"><HardDrive className="h-5 w-5" /></span>
                <div className="min-w-0 flex-1">
                  <p className="text-sm font-semibold">{location.displayName}</p>
                </div>
                <span className="text-xs font-semibold text-[#6e6e73]">{location.status}</span>
                {running && scan?.taskId && (
                  <button
                    type="button"
                    disabled={cancellingTaskId === scan.taskId}
                    onClick={() => void handleCancelScan(location, scan)}
                    className="rounded-full px-3 py-[8px] text-xs font-semibold text-[#d70015] transition-colors hover:bg-[#d70015]/[0.06] disabled:opacity-45"
                  >
                    {cancellingTaskId === scan.taskId ? "正在取消…" : "取消扫描"}
                  </button>
                )}
                <button
                  type="button"
                  disabled={running}
                  onClick={() => handleScan(location)}
                  className={cn(
                    "rounded-full px-3 py-[8px] text-xs font-semibold transition-colors",
                    running
                      ? "cursor-not-allowed text-[#86868b]"
                      : "text-[#007aff] hover:bg-[#007aff]/[0.06]",
                  )}
                >
                  {running ? "扫描中…" : "扫描"}
                </button>
                <button
                  type="button"
                  disabled={running}
                  onClick={() => handleRebind(location)}
                  className={cn(
                    "rounded-full px-3 py-[8px] text-xs font-semibold transition-colors",
                    running
                      ? "cursor-not-allowed text-[#86868b]"
                      : "text-[#6e6e73] hover:bg-black/[0.05]",
                  )}
                >
                  重绑
                </button>
                <button
                  type="button"
                  disabled={running}
                  onClick={() => handleRemove(location)}
                  className={cn(
                    "rounded-full px-3 py-[8px] text-xs font-semibold transition-colors",
                    running
                      ? "cursor-not-allowed text-[#86868b]"
                      : removingId === location.locationId
                        ? "bg-[#d70015] text-white"
                        : "text-[#d70015] hover:bg-[#d70015]/[0.06]",
                  )}
                >
                  {removingId === location.locationId ? "确认移除" : "移除"}
                </button>
              </div>
              {scan && (
                <div className="flex items-center gap-[8px] pl-[52px] text-[11px] text-[#86868b]">
                  <span data-slice-state={deriveScanSliceState(scan.phaseCode).kind} className={cn("font-semibold", scan.terminal ? "text-[#34a853]" : "text-[#007aff]")}>{scan.phase}</span>
                  <span>已见 {scan.filesSeen} 个文件 · 新增 {scan.newItem}</span>
                  {scan.message && <span className="truncate">{scan.message}</span>}
                </div>
              )}
            </div>
          )
        })}
        <div className="flex justify-end px-5 py-3">
          <button type="button" onClick={handleAddDirectory} className="rounded-full bg-[#1d1d1f] px-[16px] py-[8px] text-xs font-semibold text-white transition-transform hover:scale-[1.02]">添加本地目录</button>
        </div>
      </SettingsGroup>
    </>
  )
}

function DownloadSettings({ form }: { form: SettingsFormController }) {
  const value = downloadsDisplayValue(form)
  return (
    <>
      <SettingsIntro section="Downloads" title="下载" description="离线内容由本地 DownloadService 管理；设置页只展示已经接入的队列、断点恢复和存储边界。" />
      <SettingsFormStatusBar form={form} onReset={() => form.resetToDefaults()} />
      <SettingsFormError form={form} />
      <div className="mb-7 flex gap-3 rounded-3xl border border-[#007aff]/15 bg-[#007aff]/[0.06] p-5">
        <Download className="mt-0.5 h-5 w-5 shrink-0 text-[#007aff]" />
        <div>
          <p className="text-sm font-semibold">默认下载位置：下载 / 栖阅</p>
          <p className="mt-1 text-xs leading-5 text-[#6e6e73] dark:text-[#98989d]">首次需要本地离线位置时，桌面端会在当前用户的“下载”目录下创建“栖阅”文件夹。原始媒体和已有离线资源不会因设置页操作被移动或删除。</p>
        </div>
      </div>
      <SettingsGroup title="已接入能力">
        <SettingRow title="下载位置" description="实际目标由受控 StorageLocation 管理，设置页不接受任意路径。">
          <span className="text-sm font-semibold text-[#1d1d1f] dark:text-[#f5f5f5]">下载 / 栖阅（默认）</span>
        </SettingRow>
        <SettingRow title="队列状态" description="下载任务、暂停/继续、失败重试和完成后的 Offline Resource 已由下载服务管理。">
          <span className="text-xs font-semibold text-[#248a3d]">已接入</span>
        </SettingRow>
        <SettingRow title="自动继续中断任务" description="应用重启后自动恢复 Interrupted 任务；Queued 任务仍按已提交意图启动，用户主动暂停的任务不会被强制启动。">
          <Toggle checked={value.autoContinue} onChange={(checked) => form.change({ section: "downloads", autoContinue: checked })} label="自动继续中断任务" />
        </SettingRow>
      </SettingsGroup>
      <SettingsGroup title="下载策略" description="并发上限和速度限制由本地 Worker 在新任务开始时读取；自动继续只影响重启后的 Interrupted 任务，运行中的任务不会被强制中断。">
        <SettingRow title="同时下载数量" description="限制同时写入离线目录的任务数，等待中的任务会保留在队列。">
          <SelectControl value={optionLabel(DOWNLOAD_CONCURRENCY_OPTIONS, value.concurrentTasks)} options={DOWNLOAD_CONCURRENCY_OPTIONS.map((option) => option.label)} onChange={(label) => form.change({ section: "downloads", concurrentTasks: optionValue(DOWNLOAD_CONCURRENCY_OPTIONS, label) })} ariaLabel="同时下载数量" />
        </SettingRow>
        <SettingRow title="下载速度限制" description="对本地复制 Worker 进行软限速，暂停/继续后从当前断点继续计算。">
          <SelectControl value={optionLabel(DOWNLOAD_SPEED_LIMIT_OPTIONS, value.speedLimit)} options={DOWNLOAD_SPEED_LIMIT_OPTIONS.map((option) => option.label)} onChange={(label) => form.change({ section: "downloads", speedLimit: optionValue(DOWNLOAD_SPEED_LIMIT_OPTIONS, label) })} ariaLabel="下载速度限制" />
        </SettingRow>
      </SettingsGroup>
      <SettingsGroup title="尚未开放的策略" description="以下能力还没有完整的系统服务或来源选择契约，因此不会保存或伪装成已生效。">
        <SettingRow title="计费网络策略" description="当前没有统一网络状态探测和下载准入策略。">
          <SelectControl value="当前版本不可用" options={["当前版本不可用"]} onChange={() => undefined} ariaLabel="计费网络策略" disabled />
        </SettingRow>
        <SettingRow title="下载完成通知" description="当前没有统一桌面通知发送者。">
          <Toggle checked={false} onChange={() => undefined} label="下载完成通知" disabled />
        </SettingRow>
        <SettingRow title="默认视频质量" description="需要来源/远端资源选择契约后才能安全开放。">
          <SelectControl value="当前版本不可用" options={["当前版本不可用"]} onChange={() => undefined} ariaLabel="默认视频质量" disabled />
        </SettingRow>
      </SettingsGroup>
    </>
  )
}

function SyncSettings({ settings, update }: { settings: SettingsState; update: any }) {
  return (
    <>
      <SettingsIntro section="Sync" title="同步" description="同步目标属于你自己的存储空间。栖阅没有中心化账户，也不会同步云端凭据。" />
      <div className="mb-7 flex gap-3 rounded-3xl border border-[#007aff]/15 bg-[#007aff]/[0.06] p-5"><LockKeyhole className="mt-0.5 h-5 w-5 shrink-0 text-[#007aff]" /><div><p className="text-sm font-semibold">凭据不会同步</p><p className="mt-1 text-xs leading-5 text-[#6e6e73]">WebDAV 密码、OAuth Refresh Token 和 AI API Key 只保存在 Windows Credential Manager，并且每台设备独立授权。</p></div></div>
      <SettingsGroup title="同步状态">
        <SettingRow title="同步" description="关闭时，所有状态仍然保存在本机。"><Toggle checked={settings.syncEnabled} onChange={(value) => update("syncEnabled", value)} label="同步" /></SettingRow>
        <SettingRow title="同步目标"><SelectControl value={settings.syncTarget} options={["尚未配置", "WebDAV / HavenSync", "OneDrive / HavenSync", "Google Drive / HavenSync"]} onChange={(value) => update("syncTarget", value)} ariaLabel="同步目标" /></SettingRow>
        <SettingRow title="同步进度与标记"><Toggle checked={settings.syncProgress} onChange={(value) => update("syncProgress", value)} label="同步进度与标记" /></SettingRow>
        <SettingRow title="同步收藏与设置"><Toggle checked={settings.syncFavorites} onChange={(value) => update("syncFavorites", value)} label="同步收藏与设置" /></SettingRow>
      </SettingsGroup>
    </>
  )
}

function AiSettings({ settings, update, showNotice }: { settings: SettingsState; update: any; showNotice: (message: string) => void }) {
  return (
    <>
      <SettingsIntro section="AI" title="智能功能" description="AI 采用 BYOK。栖阅不为你的调用计费，也不会通过 Haven 中央服务器接收 API Key。" />
      <div className="mb-7 flex gap-3 rounded-3xl border border-[#f0b429]/25 bg-[#fff8e5] p-5"><Sparkles className="mt-0.5 h-5 w-5 shrink-0 text-[#b7791f]" /><p className="text-xs leading-5 text-[#7a5a1a]">实际费用由你配置的 AI 服务提供商收取。Endpoint、模型和 API Key 将由后端安全存储层接管。</p></div>
      <SettingsGroup title="Provider">
        <SettingRow title="启用智能功能"><Toggle checked={settings.aiEnabled} onChange={(value) => update("aiEnabled", value)} label="启用智能功能" /></SettingRow>
        <SettingRow title="AI Provider"><SelectControl value={settings.aiProvider} options={["OpenAI Compatible", "Azure OpenAI", "本地 Ollama"]} onChange={(value) => update("aiProvider", value)} ariaLabel="AI Provider" /></SettingRow>
        <SettingRow title="Endpoint"><input value={settings.aiEndpoint} onChange={(event) => update("aiEndpoint", event.target.value)} className="h-10 w-[260px] rounded-xl border border-black/[0.08] bg-[#f5f5f7] px-3 text-sm outline-none focus:border-[#007aff]/50" aria-label="AI Endpoint" /></SettingRow>
        <SettingRow title="API Key" description="密钥只单向写入 Windows 凭据管理器，设置界面不显示原文。"><div className="flex items-center gap-[8px]"><span className="flex items-center gap-1.5 text-[13px] font-semibold text-[#6e6e73]">{settings.aiKey ? "已配置" : "未配置"}{settings.aiKey && <CircleCheck className="h-[15px] w-[15px] text-[#34c759]" strokeWidth={2.2} />}</span><button type="button" onClick={() => showNotice("凭据写入将在后端 Credential Store 接入后开放")} className="rounded-full px-[8px] py-[8px] text-xs font-semibold text-[#007aff]">配置</button></div></SettingRow>
        <SettingRow title="Default Model"><div className="flex items-center gap-4"><SelectControl value={settings.defaultModel} options={["未配置", "gpt-4o", "claude-compatible"]} onChange={(value) => update("defaultModel", value)} ariaLabel="Default Model" /><button type="button" onClick={() => showNotice("正在向 Provider 获取可用模型")} className="text-[13px] font-medium text-[#007aff] transition-colors hover:text-[#005bb5] hover:underline">拉取模型</button></div></SettingRow>
        <SettingRow title="Vision Model"><div className="flex items-center gap-4"><SelectControl value={settings.visionModel} options={["未配置", "gpt-4o", "vision-compatible"]} onChange={(value) => update("visionModel", value)} ariaLabel="Vision Model" /><button type="button" onClick={() => showNotice("正在向 Provider 获取可用视觉模型")} className="text-[13px] font-medium text-[#007aff] transition-colors hover:text-[#005bb5] hover:underline">拉取模型</button></div></SettingRow>
      </SettingsGroup>
      <div className="flex justify-end"><button type="button" onClick={() => showNotice("连接测试将在 AI Provider 后端适配后执行")} className="inline-flex h-[36px] items-center justify-center rounded-full bg-[#1d1d1f] px-[16px] text-[12px] font-semibold leading-none text-white transition-colors hover:bg-[#2c2c2e]">测试连接</button></div>
    </>
  )
}

function UpdateSettings({ showNotice }: { showNotice: (message: string) => void }) {
  const { info, loading: infoLoading } = useAppInfo()
  const updater = useUpdater()
  const statusLabel = updater.status === "checking"
    ? "正在检查更新…"
    : updater.status === "available"
      ? `发现版本 ${updater.result?.availableVersion ?? "新版本"}`
      : updater.status === "installing"
        ? "正在下载并安装…"
        : updater.status === "up_to_date"
          ? "已是最新版本"
          : updater.status === "error"
            ? "检查失败，可重试"
            : "尚未检查"

  const runCheck = async () => {
    const ok = await updater.check()
    if (ok && updater.result?.status === "up_to_date") showNotice("当前已是最新版本")
  }

  const runInstall = async () => {
    const ok = await updater.install()
    if (ok) showNotice("更新已启动，应用将由系统安装器接管")
  }

  return (
    <>
      <SettingsIntro section="Updates" title="更新" description="应用更新与 Source Pack 更新相互独立。更新包必须通过签名校验，安装前不会修改本地媒体和数据库。" />
      <SettingsGroup title="版本">
        <SettingRow title="栖阅应用" description="当前安装版本"><span className="text-sm font-semibold text-[#1d1d1f]">{infoLoading ? "读取中…" : info?.appVersion ?? "当前版本不可用"}</span></SettingRow>
        <SettingRow title="Source Pack" description="来源包版本与最后更新时间"><span className="text-sm font-semibold text-[#1d1d1f]">{infoLoading ? "读取中…" : info?.sourcePackVersion ?? "未配置"}</span></SettingRow>
        <SettingRow title="更新策略" description="下载后由官方签名校验，Windows 使用被动安装模式。"><span className="text-sm font-semibold text-[#6e6e73]">启动时检查 · 安装前确认</span></SettingRow>
      </SettingsGroup>
      <div className="rounded-3xl border border-black/[0.07] bg-white px-5 py-[16px] shadow-sm">
        <div className="flex items-center justify-between gap-4">
          <div><p className="text-sm font-semibold">检查更新</p><p className="mt-1 text-xs text-[#86868b]">{statusLabel}</p></div>
          <div className="flex items-center gap-2">
            {updater.status === "available" && <button type="button" onClick={() => void runInstall()} className="inline-flex h-[36px] items-center justify-center gap-[8px] rounded-full bg-[#34c759] px-[16px] text-[12px] font-semibold leading-none text-white transition-colors hover:bg-[#2eaf4f] disabled:cursor-not-allowed disabled:opacity-50">安装更新</button>}
            <button type="button" onClick={() => void runCheck()} disabled={updater.status === "checking" || updater.status === "installing"} className="inline-flex h-[36px] items-center justify-center gap-[8px] rounded-full bg-[#007aff] px-[16px] text-[12px] font-semibold leading-none text-white transition-colors hover:bg-[#006fe6] disabled:cursor-not-allowed disabled:opacity-50"><RefreshCw className="h-[16px] w-[16px]" />{updater.status === "error" ? "重试" : "检查更新"}</button>
          </div>
        </div>
        {updater.result?.status === "available" && <div className="mt-4 rounded-2xl bg-[#f5f5f7] px-4 py-3 text-xs leading-5 text-[#6e6e73]"><p className="font-semibold text-[#1d1d1f]">{updater.result.availableVersion} 可用</p>{updater.result.releaseNotes && <p className="mt-1 whitespace-pre-wrap">{updater.result.releaseNotes}</p>}</div>}
        {updater.error && <div role="alert" className="mt-4 flex items-center justify-between gap-3 rounded-2xl border border-[#d70015]/15 bg-[#fff1f0] px-4 py-3 text-xs text-[#d70015]"><span>{updater.error.message || "更新服务暂时不可用"}</span><button type="button" onClick={() => void runCheck()} className="shrink-0 font-semibold text-[#007aff]">重试</button></div>}
      </div>
    </>
  )
}

function PrivacySettings({ form, showNotice }: { form: SettingsFormController; showNotice: (message: string) => void }) {
  const value = privacyDisplayValue(form)
  const [action, setAction] = useState<"search-history" | "artwork-cache" | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)

  const runClearSearchHistory = async () => {
    setAction("search-history")
    setActionError(null)
    try {
      await clearSearchHistory()
      showNotice("搜索历史已清除")
    } catch (error) {
      const haven = toHavenError(error)
      setActionError(haven.message || "清除搜索历史失败，请稍后重试")
    } finally {
      setAction(null)
    }
  }

  const runClearArtworkCache = async () => {
    setAction("artwork-cache")
    setActionError(null)
    try {
      const result = await clearArtworkCache()
      showNotice(result.removedEntries > 0n ? `已清除 ${result.removedEntries} 项 Artwork 缓存` : "Artwork 缓存已清除")
    } catch (error) {
      const haven = toHavenError(error)
      setActionError(haven.message || "清除 Artwork 缓存失败，请稍后重试")
    } finally {
      setAction(null)
    }
  }

  return (
    <>
      <SettingsIntro section="Privacy" title="隐私" description="明确哪些数据保存在本地、哪些行为会访问网络，以及如何清理本机数据。" />
      <div className="mb-7 flex gap-3 rounded-3xl border border-[#34a853]/20 bg-[#edf8f0] p-5"><Shield className="mt-0.5 h-5 w-5 shrink-0 text-[#248a3d]" /><div><p className="text-sm font-semibold text-[#216e32]">本地优先已启用</p><p className="mt-1 text-xs leading-5 text-[#4f7659]">栖阅不要求中心化账号。媒体历史、进度、收藏、标记和设置默认只写入本机。</p></div></div>
      <SettingsFormStatusBar form={form} onReset={() => form.resetToDefaults()} />
      <SettingsFormError form={form} />
      <SettingsGroup title="网络" description="配置本机发出的网络请求行为，如代理与跟踪限制。">
        <SettingRow title="代理模式" description="当前版本尚未提供独立的 Artwork/Provider 代理策略。"><SelectControl value="系统代理 · 当前版本不可用" options={["系统代理 · 当前版本不可用"]} onChange={() => undefined} ariaLabel="代理模式" disabled /></SettingRow>
        <SettingRow title="限制网络跟踪" description="当前版本尚未接入统一出站策略消费者。"><Toggle checked={false} onChange={() => undefined} label="限制网络跟踪" disabled /></SettingRow>
      </SettingsGroup>
      <SettingsGroup title="本地行为">
        <SettingRow title="搜索历史" description="关闭后不再记录新的搜索词；已有记录不会自动删除。"><Toggle checked={value.searchHistory} onChange={(checked) => form.change({ section: "privacy", searchHistory: checked })} label="搜索历史" /></SettingRow>
        <SettingRow title="播放与阅读历史" description="关闭后不再记录新的播放或阅读打开记录；已有历史不会自动删除，清除操作仍单独生效。"><Toggle checked={value.playbackHistory} onChange={(checked) => form.change({ section: "privacy", playbackHistory: checked })} label="播放与阅读历史" /></SettingRow>
        <SettingRow title="网络诊断信息" description="当前版本没有统一诊断数据消费者。"><Toggle checked={false} onChange={() => undefined} label="网络诊断信息" disabled /></SettingRow>
        <SettingRow title="日志保留时间" description="当前版本没有独立的日志清理设置消费者。"><SelectControl value="当前版本不可用" options={["当前版本不可用"]} onChange={() => undefined} ariaLabel="日志保留时间" disabled /></SettingRow>
      </SettingsGroup>
      <SettingsGroup title="数据操作" description="只清理明确的技术缓存或搜索词，不会删除离线资源、原始媒体、进度、标记或收藏。">
        <SettingRow title="清除搜索历史" danger><button type="button" disabled={action !== null} onClick={() => void runClearSearchHistory()} className="rounded-full px-3 py-[8px] text-xs font-semibold text-[#d70015] hover:bg-[#d70015]/[0.06] disabled:cursor-not-allowed disabled:opacity-50">{action === "search-history" ? "清除中…" : "清除"}</button></SettingRow>
        <SettingRow title="清除 Artwork 缓存" description="只删除已登记的海报/Artwork 技术缓存；下次联网请求会自动重建。" danger><button type="button" disabled={action !== null} onClick={() => void runClearArtworkCache()} className="rounded-full px-3 py-[8px] text-xs font-semibold text-[#d70015] hover:bg-[#d70015]/[0.06] disabled:cursor-not-allowed disabled:opacity-50">{action === "artwork-cache" ? "清除中…" : "清除"}</button></SettingRow>
        <SettingRow title="清除全部本地数据" description="包括设置、历史、进度和标记，不会删除用户原始媒体文件。该高风险操作尚未开放。" danger><button type="button" disabled className="rounded-full bg-[#fff1f0] px-3 py-[8px] text-xs font-semibold text-[#d70015] disabled:cursor-not-allowed disabled:opacity-50">当前版本不可用</button></SettingRow>
      </SettingsGroup>
      {actionError && <div role="alert" className="mx-2 rounded-2xl border border-[#d70015]/15 bg-[#fff1f0] px-4 py-3 text-[13px] text-[#d70015]">{actionError}</div>}
    </>
  )
}

const APP_DIRECTORY_KINDS: AppDirectoryKindDto[] = ["data", "logs", "cache"]

function AboutSettings() {
  const { info, loading, error, opening, openError, reload, openDirectory } = useAppInfo()
  const { push } = useNotice()
  const directoryByKind = new Map(info?.directories.map((directory) => [directory.kind, directory]))
  const versionLabel = loading
    ? "正在读取应用信息..."
      : info
        ? `${info.appVersion} · ${info.buildChannel}`
        : "当前版本不可用"

  useEffect(() => {
    if (!openError) return
    push({
      kind: "error",
      title: "无法打开目录",
      message: openError.dto.userMessage,
      code: openError.code,
      retryable: openError.retryable,
      dedupeKey: `about-directory:${openError.code}`,
    })
  }, [openError, push])

  return (
    <>
      <SettingsIntro section="About" title="关于栖阅" description="版本、开源许可、数据位置和项目边界。" />
      <div className="flex flex-col gap-5 rounded-3xl border border-black/[0.07] bg-white p-6 shadow-[0_12px_32px_rgba(0,0,0,0.05)] sm:flex-row sm:items-center">
        <div className="flex h-[112px] w-[112px] shrink-0 items-center justify-center overflow-hidden rounded-3xl border border-black/[0.07] bg-white"><img src="/logo.png" alt="栖阅 Haven 项目 Logo" className="h-full w-full object-contain p-[8px]" /></div>
        <div className="flex-1"><p className="text-xs font-semibold uppercase tracking-[0.18em] text-[#007aff]">Haven</p><h3 className="mt-[8px] text-2xl font-semibold">栖阅</h3><p className="mt-[8px] text-sm leading-6 text-[#6e6e73]">让所有故事，在一个地方继续。</p><p className="mt-3 text-xs text-[#86868b]">版本 {versionLabel}</p></div>
        <span className="inline-flex h-[32px] min-w-[112px] items-center justify-center rounded-[16px] bg-[#1d1d1f] px-[16px] text-[12px] font-semibold leading-none text-white">本机应用</span>
      </div>

      {error && (
        <div className="flex items-center justify-between gap-4 rounded-2xl border border-[#ff3b30]/20 bg-[#ff3b30]/[0.06] px-4 py-3 text-sm text-[#b42318]">
          <span>{error.message || "应用信息暂时不可用"}</span>
          <button type="button" onClick={() => void reload()} className="shrink-0 font-semibold text-[#007aff]">重试</button>
        </div>
      )}

      <SettingsGroup title="项目与许可">
        <SettingRow title="Source Pack 版本"><span className="text-sm font-semibold">{info?.sourcePackVersion ?? (loading ? "读取中..." : "未配置")}</span></SettingRow>
        <SettingRow title="IPC 协议版本"><span className="font-mono text-xs text-[#6e6e73]">{info?.protocolVersion ?? (loading ? "读取中..." : "不可用")}</span></SettingRow>
        <SettingRow title="数据库版本"><span className="font-mono text-xs text-[#6e6e73]">{info?.databaseVersion ?? (loading ? "读取中..." : "不可用")}</span></SettingRow>
        <SettingRow title="项目许可证" description="Haven 自有代码采用 MIT 协议；第三方清单中的依赖仍受各自许可证约束。"><span className="text-xs font-semibold text-[#1d1d1f]">{info?.appLicense ?? (loading ? "读取中..." : "MIT")}</span></SettingRow>
        <SettingRow title="第三方许可" description="摘要来自构建时登记的 THIRD_PARTY_NOTICES.md，不在 IPC 中传输完整正文。">
          <div className="max-w-[360px] text-right text-xs text-[#6e6e73]">
            {info?.thirdPartyNotices.length ? info.thirdPartyNotices.map((notice) => <div key={`${notice.name}-${notice.license}`}>{notice.name} · {notice.license}</div>) : loading ? "读取中..." : "暂无已登记摘要"}
          </div>
        </SettingRow>
      </SettingsGroup>

      <SettingsGroup title="本地目录" description="目录由桌面端固定注入；页面不会接收或拼接任意路径。">
        {APP_DIRECTORY_KINDS.map((kind) => {
          const directory = directoryByKind.get(kind)
          const label = directory?.displayName ?? (kind === "data" ? "应用数据目录" : kind === "logs" ? "日志目录" : "缓存目录")
          return (
            <SettingRow key={kind} title={label}>
              <div className="flex max-w-[420px] items-center gap-3">
                <span className="font-mono text-right text-xs text-[#6e6e73]">{directory?.displayPath ?? (loading ? "读取中..." : "不可用")}</span>
                <button type="button" disabled={!directory?.canOpen || opening === kind} onClick={() => void openDirectory(kind)} className="inline-flex shrink-0 items-center gap-1 text-xs font-semibold text-[#007aff] disabled:cursor-not-allowed disabled:text-[#a1a1a6]">
                  {opening === kind ? "打开中..." : "打开"}
                  <Folder className="h-3.5 w-3.5" />
                </button>
              </div>
            </SettingRow>
          )
        })}
      </SettingsGroup>

      <ErrorReportSettings />

      {openError && <p className="px-2 text-xs text-[#b42318]">{openError.message || "无法打开目录，请稍后重试"}</p>}
    </>
  )
}

function ErrorReportSettings() {
  const report = useErrorReport()
  const { push } = useNotice()
  const mode = getHavenClientMode()

  const generate = async () => {
    const success = await report.generate()
    if (!success && report.error) {
      push({ kind: "error", title: "诊断报告生成失败", message: report.error.dto.userMessage, code: report.error.code, retryable: report.error.retryable, dedupeKey: "error-report-generate" })
    }
  }

  const confirm = async () => {
    const success = await report.confirm()
    if (success) push({ kind: "success", title: "已确认诊断报告", message: "现在可以导出报告或打开 GitHub Issue 预填页面。", dedupeKey: "error-report-confirmed" })
  }

  const copySummary = async () => {
    if (!report.preview) return
    try {
      if (!navigator.clipboard) throw new Error("clipboard unavailable")
      await navigator.clipboard.writeText(report.preview.errorSummary)
      push({ kind: "success", title: "已复制错误摘要", message: "摘要只包含稳定错误码，不包含 URL、路径或用户内容。", dedupeKey: "error-report-copy" })
    } catch {
      push({ kind: "error", title: "复制失败", message: "当前环境不允许访问剪贴板，请手动选择摘要复制。", retryable: true, dedupeKey: "error-report-copy-failed" })
    }
  }

  const runExport = async () => {
    const success = await report.exportReport()
    if (success) push({ kind: "success", title: "诊断报告已导出", message: "报告已保存到应用数据目录的 Reports 文件夹。", dedupeKey: "error-report-exported" })
  }

  const runIssue = async () => {
    const success = await report.openIssue()
    if (success) push({ kind: "success", title: "已打开 GitHub Issue 页面", message: "页面只预填脱敏摘要；请在 GitHub 中检查后手动提交。", dedupeKey: "error-report-issue-opened" })
  }

  const retry = () => {
    if (!report.error) return
    if (report.action === "export" || report.error.code.includes("EXPORT")) void runExport()
    else if (report.action === "issue" || report.error.code.includes("ISSUE")) void runIssue()
    else void generate()
  }

  return (
    <SettingsGroup title="错误报告" description="主动生成脱敏诊断摘要，帮助在 GitHub 报告问题。报告不会自动上传，提交前由你最终确认。">
      <SettingRow title="报告等级" description="基础包含版本、系统和稳定错误码；标准增加协议状态；详细增加经过脱敏检查的有限诊断行。">
        <select value={report.level} onChange={(event) => report.setLevel(event.target.value as typeof report.level)} className="rounded-xl border border-black/[0.1] bg-white px-3 py-2 text-xs font-semibold text-[#1d1d1f] outline-none focus:border-[#007aff]" aria-label="报告等级">
          {(Object.keys(ERROR_REPORT_LEVEL_LABELS) as Array<typeof report.level>).map((value) => <option key={value} value={value}>{ERROR_REPORT_LEVEL_LABELS[value]}</option>)}
        </select>
      </SettingRow>
      <SettingRow title="生成脱敏报告" description={mode === "mock" ? "Browser Mock 只展示报告流程，不写入本地文件或打开外部页面。" : "报告 ID 会在本次流程中保持有效一段时间，过期后需重新生成。"}>
        <button type="button" onClick={() => void generate()} disabled={report.loading} className="rounded-full bg-[#007aff] px-4 py-2 text-xs font-semibold text-white transition hover:bg-[#006fe6] disabled:cursor-not-allowed disabled:opacity-50">{report.loading ? "生成中…" : "生成预览"}</button>
      </SettingRow>

      {report.preview && (
        <div className="mx-2 mt-2 rounded-2xl border border-[#007aff]/15 bg-[#f5f9ff] p-4">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div>
              <p className="text-sm font-semibold text-[#1d1d1f]">脱敏检查：{report.preview.redaction.status === "passed" ? "通过" : "未通过"}</p>
              <p className="mt-1 font-mono text-[11px] text-[#6e6e73]">报告 ID：{report.preview.reportId}</p>
            </div>
            <span className="rounded-full bg-[#dff3e5] px-2.5 py-1 text-[11px] font-semibold text-[#216e32]">未包含敏感数据</span>
          </div>
          <dl className="mt-3 grid gap-2 text-xs text-[#6e6e73] sm:grid-cols-3">
            <div><dt>Haven 版本</dt><dd className="mt-0.5 font-semibold text-[#1d1d1f]">{report.preview.appVersion}</dd></div>
            <div><dt>系统</dt><dd className="mt-0.5 font-semibold text-[#1d1d1f]">{report.preview.operatingSystem}</dd></div>
            <div><dt>运行模式</dt><dd className="mt-0.5 font-semibold text-[#1d1d1f]">{report.preview.runtimeMode}</dd></div>
          </dl>
          <p className="mt-3 rounded-xl bg-white px-3 py-2 text-xs leading-5 text-[#3a3a3c]">{report.preview.errorSummary}</p>
          {report.preview.details?.diagnosticLines.length ? <ul className="mt-3 space-y-1 font-mono text-[11px] text-[#6e6e73]">{report.preview.details.diagnosticLines.map((line) => <li key={line}>· {line}</li>)}</ul> : null}
          <div className="mt-3 flex flex-wrap gap-2">
            <button type="button" onClick={() => void copySummary()} className="inline-flex items-center gap-1.5 rounded-full border border-black/[0.1] bg-white px-3 py-2 text-xs font-semibold text-[#3a3a3c] hover:border-black/[0.2]"><Clipboard className="h-3.5 w-3.5" />复制错误摘要</button>
            {!report.confirmed ? <button type="button" onClick={() => void confirm()} disabled={report.preview.redaction.status !== "passed"} className="inline-flex items-center gap-1.5 rounded-full bg-[#1d1d1f] px-3 py-2 text-xs font-semibold text-white hover:bg-black disabled:cursor-not-allowed disabled:opacity-50"><Check className="h-3.5 w-3.5" />我已检查，允许使用</button> : <span className="inline-flex items-center gap-1.5 rounded-full bg-[#dff3e5] px-3 py-2 text-xs font-semibold text-[#216e32]"><Check className="h-3.5 w-3.5" />已确认</span>}
          </div>
          {report.confirmed && <div className="mt-3 flex flex-wrap gap-2 border-t border-[#007aff]/10 pt-3"><button type="button" onClick={() => void runExport()} disabled={report.action !== null} className="rounded-full bg-[#007aff] px-3 py-2 text-xs font-semibold text-white hover:bg-[#006fe6] disabled:cursor-not-allowed disabled:opacity-50">{report.action === "export" ? "导出中…" : "导出诊断报告"}</button><button type="button" onClick={() => void runIssue()} disabled={report.action !== null} className="rounded-full border border-[#007aff]/25 bg-white px-3 py-2 text-xs font-semibold text-[#007aff] hover:bg-[#eef6ff] disabled:cursor-not-allowed disabled:opacity-50">{report.action === "issue" ? "打开中…" : "打开 GitHub Issue"}</button></div>}
          {report.actionResult && <p className="mt-3 text-xs font-semibold text-[#216e32]">{report.actionResult.status === "exported" ? "报告导出成功，可在 Reports 文件夹中找到。" : "Issue 预填页面已打开，请在 GitHub 页面最终提交。"}</p>}
          {report.error && <div role="alert" className="mt-3 flex flex-wrap items-center justify-between gap-2 rounded-xl border border-[#d70015]/15 bg-[#fff1f0] px-3 py-2 text-xs text-[#b42318]"><span>{report.error.dto.userMessage}</span>{report.error.retryable && <button type="button" onClick={retry} className="font-semibold text-[#007aff]">重试</button>}</div>}
        </div>
      )}
    </SettingsGroup>
  )
}
