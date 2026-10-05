import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react"
import type { CSSProperties, DragEvent, KeyboardEvent as ReactKeyboardEvent, ReactNode } from "react"
import { useNavigate, useParams, useSearchParams } from "react-router"
import { createPortal } from "react-dom"
import {
  Check,
  Clipboard,
  ChevronDown,
  ChevronRight,
  CircleCheck,
  FileText,
  GripVertical,
  Home,
  LockKeyhole,
  PlaySquare,
  Plus,
  RefreshCw,
  Search,
  Settings2,
  Shield,
  Trash2,
  TriangleAlert,
  Upload,
} from "lucide-react"
import { cn } from "@/lib/utils"
import { AppearanceColorPicker } from "@/features/settings/components/AppearanceColorPicker"
import { StorageSettings } from "../components/StorageSettings"
import { DownloadSettings } from "../components/DownloadSettings"
import { SettingsToggle as Toggle } from "../components/SettingsToggle"
import { SourcesSettings } from "../components/SourcesSettings"
import { SettingsDialog } from "../components/SettingsDialog"
import { SettingsNavigation, type SettingsNavId } from "../components/SettingsNavigation"
import { AboutSettings, GeneralSettings, UpdateSettings } from "../components/SystemSettings"
import { SystemSettingsRow, SystemSettingsSection } from "../components/SystemSettingsLayout"
import { toHavenError } from "@/lib/ipc/errors"
import { getHavenClientMode } from "@/lib/ipc/runtime"
import type { AppearanceAssetKindWire, AppearanceAssetWire, ComicSettingsValue, GeneralSettingsValue, AppearanceSettingsValue, PlaybackSettingsValue, PreferenceComicPatchWire, PreferenceGetResult, PreferenceReadingPatchWire, PreferenceTargetWire, PrivacySettingsValue, ReadingFontFamilyWire, ReadingPatchWire, ReadingSettingsValue, SettingsValue, ThemeWire, UiFontPresetWire, WallpaperSelection } from "@/lib/ipc/settings-wire"
import type { SettingsFormController } from "@/features/settings/lib/useSettingsForm"
import { useSettingsForm } from "@/features/settings/lib/useSettingsForm"
import { useAiProviderSettings } from "@/features/settings/lib/useAiProviderSettings"
import { INSECURE_ENDPOINT_CODE, NO_MODEL_SELECTED } from "@/features/settings/ipc/ai-provider-gateway"
import { useAgentBrokerSettings } from "@/features/settings/lib/useAgentBrokerSettings"
import { useAgentSkills } from "@/features/settings/lib/useAgentSkills"
import { useMcpClientConfig } from "@/features/settings/lib/useMcpClientConfig"
import {
  mcpClientActionLabel,
  mcpClientStateLabel,
  mcpClientStatusNote,
  type McpClientTargetStatusWire,
} from "@/features/settings/ipc/mcp-client-gateway"
import {
  NO_BUILTIN_SKILLS,
  agentSkillActionLabel,
  agentSkillCharsLabel,
  agentSkillIsEffective,
  agentSkillToggleTarget,
  agentSkillStateLabel,
} from "@/features/settings/ipc/agent-skill-gateway"
import {
  AGENT_BROKER_CLIENT_TEMPLATES,
  AGENT_BROKER_TEMPLATE_NOTE,
  buildAgentBrokerMcpTemplate,
  type AgentBrokerClientTemplateId,
} from "@/features/settings/ipc/agent-broker-gateway"
import { settingsGateway } from "@/features/settings/ipc/gateway"
import { appearanceGateway } from "@/features/settings/ipc/appearance-gateway"
import { clearArtworkCache, clearSearchHistory } from "@/features/settings/ipc/privacy-gateway"
import type { AgentBrokerStatusDto } from "@/lib/ipc/generated/wire"
import {
  DENSITY_OPTIONS,
  LAUNCH_PAGE_OPTIONS,
  PLAYBACK_RATE_OPTIONS,
  READING_CUSTOM_APPEARANCE_HINT,
  READING_CUSTOM_BACKGROUND_LABEL,
  READING_CUSTOM_COLOR_DISABLED_HINT,
  READING_CUSTOM_FONT_HINT,
  READING_CUSTOM_FONT_INVALID_HINT,
  READING_CUSTOM_FONT_LABEL,
  READING_CUSTOM_FONT_PLACEHOLDER,
  READING_CUSTOM_TEXT_LABEL,
  READING_FONT_PREVIEW_CLASS,
  READING_FONT_SIZE_OPTIONS,
  READING_LINE_HEIGHT_OPTIONS,
  READING_PAGE_SUBMODE_OPTIONS,
  READING_PAGINATION_MODE_OPTIONS,
  READING_PREVIEW_FULL_MEASURE_PX,
  READING_THEME_OPTIONS,
  READING_WIDTH_OPTIONS,
  SIDEBAR_OPTIONS,
  THEME_OPTIONS,
  INTERFACE_FONT_MODE_DETAILS,
  INTERFACE_FONT_MODE_OPTIONS,
  customFontFamilyCommit,
  customReadingColorInputValue,
  customReadingColorPatch,
  optionLabel,
  optionValue,
  readingFontLabel,
  readingFontPickerOptions,
  readingPageSubMode,
  readingPaginationMode,
  readingPaginationWire,
  readingPreviewFontLabel,
  readingPreviewMeasureRatio,
  readingPreviewPalette,
  readingThemePatch,
  readingThemeSwatch,
  type ReadingPageSubMode,
  type ReadingPaginationMode,
} from "@/features/settings/lib/settingsDisplay"
import {
  READING_CUSTOM_BACKGROUND_FALLBACK,
  READING_CUSTOM_TEXT_FALLBACK,
  resolveCustomFontFamilyCss,
  resolveReadingPresentation,
} from "@/features/reader/lib/reading-settings-mapping"
import {
  publishSettingsRuntimeValue,
  reloadSettingsRuntime,
  useSettingsRuntimeStatus,
} from "@/features/settings/lib/settings-runtime-state"
import {
  describeResolvedInterfaceFont,
  filterFontFamilies,
  filterInterfaceFontAssets,
  fontFamilyPreviewStyle,
  importedFontCssFamily,
  INTERFACE_FONT_PRESET_STACKS,
  resolveInterfaceFont,
} from "@/features/settings/lib/interface-font"
import {
  useInjectedFontFace,
  useInterfaceFontAssets,
  useSystemFontFamilies,
} from "@/features/settings/lib/useInterfaceFonts"
import { interfaceFontGateway } from "@/features/settings/ipc/interface-font-gateway"
import type {
  InterfaceFontAsset,
  InterfaceFontModeWire,
} from "@/lib/ipc/interface-font-wire"
import { controlledResourceUri } from "@/lib/artwork-url"
import type { SettingsFeatureSearchMatch } from "@/features/settings/lib/settings-navigation"
import {
  isSettingsSectionId,
  type SettingsRegistryEntry,
  type SettingsSectionId,
  type FeatureId,
} from "@/features/settings/lib/settings-registry"
import { AiAssistantDialog } from "@/features/settings/components/ai-assistant/AiAssistantDialog"
import { ERROR_REPORT_LEVEL_LABELS } from "@/features/settings/ipc/error-report-gateway"
import { useErrorReport } from "@/features/settings/lib/useErrorReport"
import { useNotice } from "@/app/notice-center/notice-context"
import {
  READING_OVERVIEW_WINDOW_LABEL,
  contentCategoryLabel,
  formatReadingDuration,
  formatReadingStreak,
  hasReadingOverviewData,
  readingOverviewStats,
  type ReadingOverviewState,
} from "@/features/settings/lib/reading-overview"
import { useReadingOverview } from "@/features/settings/lib/useReadingOverview"
import {
  THEME_PRESETS,
  normalizeAppTheme,
  paletteColorInputValue,
  themePresetAppTheme,
  withAccentColor,
  withPaletteRoleColor,
  type PaletteColorRole,
  type ThemePresetId,
} from "@/features/settings/lib/appearance-palette"
import {
  appearanceAssetRequestUri,
  installAppearanceFontPreview,
  requestWallpaperRetry,
  useHomeWallpaperFailure,
  useSystemPreference,
  useWallpaperRetryEpoch,
} from "@/features/settings/lib/appearance-runtime"
import { UI_FONT_PRESETS, uiFontCssStack } from "@/features/settings/lib/appearance-fonts"
import { querySystemFontCatalog, searchSystemFonts, type SystemFontCatalogResult, type SystemFontEntry } from "@/features/settings/lib/system-font-catalog"
import {
  appearanceAssetDeleteBlockedReason,
  appearanceAssetLabel,
  isUsableAppearanceAsset,
  useAppearanceAssets,
  type AppearanceAssetActionResult,
  type AppearanceAssetDeleteSelections,
  type AppearanceAssetLists,
  type AppearanceAssetSelection,
  type AppearanceAssetsController,
} from "@/features/settings/lib/useAppearanceAssets"
import {
  OVERVIEW_MODULES,
  OVERVIEW_MODULE_SIZES,
  overviewModuleLabel,
  overviewModuleSettingsToLayout,
  type OverviewModuleId,
  type OverviewModulePlacement,
  type OverviewModuleSize,
} from "@/features/settings/lib/overview-layout"
import { overviewModuleColumnSpan } from "@/lib/ipc/settings-wire"
import {
  useOverviewLayout,
  type OverviewLayoutController,
} from "@/features/settings/lib/useOverviewLayout"
import {
  overviewLayoutProjection,
  type OverviewLayoutProjection,
} from "@/features/settings/lib/overview-layout-projection"

type SettingsFormSet = {
  general: SettingsFormController
  appearance: SettingsFormController
  playback: SettingsFormController
  reading: SettingsFormController
  comic: SettingsFormController
  downloads: SettingsFormController
  privacy: SettingsFormController
}

type RegisterSettingsSectionReloader = (
  section: SettingsSectionId,
  reload: () => void,
) => () => void


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
    <div className="flex h-full min-h-0 items-center justify-center bg-[var(--haven-settings-control)] px-6">
      <section
        aria-live="polite"
        className="w-full max-w-[560px] rounded-[20px] border border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-card)] p-8 text-center shadow-[0_20px_60px_rgba(0,0,0,0.06)]"
      >
        <Settings2 className="mx-auto h-10 w-10 text-[var(--haven-settings-muted)]" strokeWidth={1.7} />
        <h1 className="mt-4 text-xl font-semibold text-[var(--haven-settings-foreground)]">设置暂不可用</h1>
        <p className="mt-2 text-sm leading-6 text-[var(--haven-settings-muted-strong)]">
          当前运行环境不支持应用数据访问，请在栖阅桌面应用内打开设置。
        </p>
      </section>
    </div>
  )
}

/**
 * 运行时设置读盘失败时的用户可见提示。
 *
 * AppRoot 会在读盘失败后把外观/启动页投影收敛到契约默认值；这不是一份可以被当成
 * 用户已保存配置的结果，所以设置页必须把「当前使用默认值」和「可以重读」明确说出来。
 * 组件本身只消费状态，不自行触达 IPC；重读仍然通过 settings-runtime-state 的统一入口
 * 让 AppRoot 重跑原来的加载 effect。
 */
export function SettingsRuntimeStatusNotice({
  status,
  onRetry,
}: {
  status: "loading" | "ready" | "degraded"
  onRetry: () => void
}) {
  if (status !== "degraded") return null

  return (
    <div
      role="alert"
      className="mb-[24px] flex w-full items-start gap-[12px] rounded-[14px] border border-[var(--haven-settings-danger-20)] bg-[var(--haven-settings-danger-surface)] px-[16px] py-[13px] text-[var(--haven-settings-foreground)]"
    >
      <TriangleAlert className="mt-[1px] h-[17px] w-[17px] shrink-0 text-[var(--haven-settings-danger)]" strokeWidth={1.8} />
      <div className="min-w-0 flex-1">
        <p className="text-[13px] font-medium leading-5">外观配置读取不完整</p>
        <p className="mt-[2px] text-[12px] leading-5 text-[var(--haven-settings-muted-strong)]">
          当前页面使用安全默认值，不会覆盖已保存配置。重新读取后可恢复你的外观设置。
        </p>
      </div>
      <button
        type="button"
        onClick={onRetry}
        className="inline-flex shrink-0 items-center gap-[6px] rounded-[9px] border border-[var(--haven-settings-danger-20)] px-[10px] py-[7px] text-[12px] font-medium text-[var(--haven-settings-danger)] transition-colors hover:bg-[var(--haven-settings-danger-06)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-danger-20)]"
      >
        <RefreshCw className="h-[13px] w-[13px]" strokeWidth={1.8} />
        重新读取
      </button>
    </div>
  )
}

function SettingsContent() {
  const navigate = useNavigate()
  const { section: sectionParam } = useParams<{ section?: string }>()
  const [searchParams] = useSearchParams()
  const isLegacySkillsRoute = sectionParam === "skills"
  const [searchDestination, setSearchDestination] = useState<SettingsFeatureSearchMatch | null>(null)
  const contentRef = useRef<HTMLElement>(null)
  // 只有登记表里的分区可导航；未知或已隐藏的分区（sync / 拼错的名字）一律回落到
  // 总览，而不是渲染一块不可用的占位。
  const requestedNavId: SettingsNavId | null = isLegacySkillsRoute
    ? "ai"
    : sectionParam === "overview"
    ? "overview"
    : sectionParam !== undefined && isSettingsSectionId(sectionParam)
      ? sectionParam
      : null
  const activeNavId: SettingsNavId = requestedNavId ?? "overview"
  const activeSection = activeNavId === "overview" ? null : activeNavId
  const [aiSettingsOpen, setAiSettingsOpen] = useState(isLegacySkillsRoute)
  const aiSettingsTitleId = useId()
  /**
   * 用户在「智能功能」里选中的 AI Provider 配置 id。
   *
   * 它必须活在**分区切换之上**：每个分区各自挂载自己的设置组件，切到别的分区再切回来，
   * `AiSettings` 会连同 `useAiProviderSettings` 一起重新挂载。选择只留在那个组件里，
   * 重挂载就会静默回落到列表第一项——用户选的是 B，回来变成 A，而「打开栖伴」取的正是
   * 这个值，于是外部模型请求被发给了另一个 Provider。
   *
   * 它只是本次 WebView 会话内的记忆：不持久化、不进 settings wire，刷新页面即失效。
   */
  const [aiProfileId, setAiProfileId] = useState<string | null>(null)
  // 没有表单控制器的分区（「智能功能」/「内置技能」）把自己的重读入口登记在这里，
  // 「重新加载配置」按当前分区转调，页面不需要知道具体 hook。
  const sectionReloaders = useRef(new Map<SettingsSectionId, Set<() => void>>())
  const registerSectionReloader = useCallback<RegisterSettingsSectionReloader>((section, reload) => {
    const reloaders = sectionReloaders.current.get(section) ?? new Set<() => void>()
    reloaders.add(reload)
    sectionReloaders.current.set(section, reloaders)
    return () => {
      reloaders.delete(reload)
      if (reloaders.size === 0) sectionReloaders.current.delete(section)
    }
  }, [])

  const resourceQuery = searchParams.toString()
  const resourceContext = useMemo(() => parseResourcePreferenceContext(new URLSearchParams(resourceQuery)), [resourceQuery])

  useLayoutEffect(() => {
    if (!searchDestination || activeNavId !== (searchDestination.navId ?? searchDestination.section)) return
    const inAiSettings = searchDestination.section === "ai" && searchDestination.featureId !== "ai.recommendation"
    // 模态表单负责自己的初始焦点，避免父子生命周期互相覆盖。
    if (inAiSettings) return
    const scope = contentRef.current
    if (!scope) return
    const target = scope.querySelector<HTMLElement>(`[data-settings-features~="${searchDestination.featureId}"]`)
      ?? scope.querySelector<HTMLElement>("h2") ?? scope
    target.tabIndex = -1
    target.focus({ preventScroll: true })
    target.scrollIntoView?.({ block: "center", behavior: "auto" })
    setSearchDestination(null)
  }, [activeNavId, searchDestination])

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
  // 阅读总览：真实聚合（7 天窗口 + 当前环境的显式 UTC 偏移）。loading / empty / ready /
  // error 四态都由后端事实决定，页面不再持有一份手写的统计。
  const readingOverview = useReadingOverview()
  const readingOverviewState: ReadingOverviewState = readingOverview.state
  // 总览布局只读一次、只持有一个 revision：总览渲染器与外观分区里的布局编辑器共用同一份
  // 状态，因此「编辑器保存后总览立刻按新排列渲染」是同一次读取的结果，而不是两个各自
  // 缓存的副本。两个独立 hook 实例会让编辑器拿着过期 revision 去保存。
  const overviewLayout = useOverviewLayout(appearanceGateway, showNotice)
  const overviewLayoutState = overviewLayout.state
  // 外观资产列表保持页面级状态，分区切换不会丢掉导入 / 删除的异步结果。
  const appearanceAssets = useAppearanceAssets(
    appearanceAssetSelection(appearanceForm),
    appearanceGateway,
  )
  const runtimeStatus = useSettingsRuntimeStatus()

  useEffect(() => {
    if (isLegacySkillsRoute) setAiSettingsOpen(true)
  }, [isLegacySkillsRoute])

  const selectSection = (section: SettingsNavId) => {
    if (section === "sync") return
    if (section !== "overview" && !isSettingsSectionId(section)) return
    setSearchDestination(null)
    if (section === "ai") setAiSettingsOpen(false)
    navigate(`/settings/${section}`, { replace: true })
  }

  return (
    <div className="settings-page h-full min-h-0 overflow-hidden bg-[var(--haven-settings-background)] text-[var(--haven-settings-foreground)]">
      <main className="mx-auto h-full min-h-0 w-full overflow-hidden px-0 pb-0 pt-0">
        <div className="settings-layout h-full min-h-0 overflow-hidden rounded-[24px] border border-[var(--haven-settings-border)] bg-[var(--haven-settings-background)] shadow-none">
          <SettingsNavigation activeId={activeNavId} onSelectSection={selectSection} onSelectFeature={(match) => {
            selectSection(match.navId ?? match.section)
            setSearchDestination(match)
            if (match.section === "ai") setAiSettingsOpen(match.featureId !== "ai.recommendation")
          }} />

          <section ref={contentRef} className="settings-scrollbar-hidden min-h-0 min-w-0 flex-1 overflow-y-auto bg-[var(--haven-settings-background)] p-[24px] pb-[112px] sm:p-[32px] sm:pb-[112px] lg:px-[52px] lg:pb-[112px] lg:pt-[24px]">
            <div className="mx-auto w-full max-w-[1240px]">
              <SettingsRuntimeStatusNotice status={runtimeStatus} onRetry={reloadSettingsRuntime} />
            </div>
            <div className={cn("mx-auto w-full", activeSection === "ai" ? "max-w-[1108px]" : activeSection ? "max-w-[1052px]" : "max-w-[1080px]")}>
              <div className={cn("relative min-w-0 w-full space-y-6", activeSection === "ai" && "min-h-0")} data-settings-features={activeSection === "ai" ? "ai.recommendation" : undefined} tabIndex={activeSection === "ai" ? -1 : undefined}>
                {activeNavId === "overview"
                  ? <SettingsOverview
                      state={readingOverviewState}
                      forms={forms}
                      layout={overviewLayoutProjection(overviewLayoutState)}
                      onSelectSection={selectSection}
                      onRetryOverview={readingOverview.reload}
                      onRetryLayout={overviewLayout.reload}
                    />
                  : activeSection === "ai"
                    ? <AiAssistantDialog
                        open
                        embedded
                        onOpenChange={() => undefined}
                        onOpenSettings={() => { setSearchDestination(null); setAiSettingsOpen(true) }}
                        profileId={aiProfileId}
                      />
                    : activeSection && renderSettingsSection(
                      activeSection,
                      forms,
                      showNotice,
                      resourceContext,
                      overviewLayout,
                      appearanceAssets,
                    )}
                {activeSection === "ai" && aiSettingsOpen && (
                  <SettingsDialog
                    title="AI 设置"
                    titleId={aiSettingsTitleId}
                    description="管理模型服务、外部 Agent / MCP 接入与本机 Skills。"
                    initialFocusSelector={searchDestination?.section === "ai" ? `[data-settings-features~="${searchDestination.featureId}"]` : undefined}
                    closeLabel="关闭 AI 设置"
                    onClose={() => { setAiSettingsOpen(false); setSearchDestination(null) }}
                  >
                      <div className="space-y-7">
                        <AiSettings
                          showNotice={showNotice}
                          registerReloader={registerSectionReloader}
                          aiSelection={{ profileId: aiProfileId, onSelect: setAiProfileId }}
                        />
                        <SkillsSettings showNotice={showNotice} registerReloader={registerSectionReloader} />
                      </div>
                  </SettingsDialog>
                )}
              </div>
            </div>
          </section>
        </div>
      </main>

    </div>
  )
}

type ReadingOverviewMetric = {
  label: string
  value: string
  foot: string
  dark?: boolean
}

/**
 * 总览错误态的固定文案。
 *
 * 「读不到」与「没有记录」是两种不同的事实：前者可以说清是读取失败、并给出重试，
 * 后者才能说「暂无本机阅读记录」。错误态绝不能借用空态那句话——那会把一次失败的
 * 读取伪装成一份「你确实什么都没读」的结论。
 */
const OVERVIEW_UNAVAILABLE_VALUE = "不可用"
const OVERVIEW_ERROR_TITLE = "统计暂时不可用"
const OVERVIEW_ERROR_HINT = "读取失败，请重试。"

/**
 * 总览渲染器：页头 + 按**已保存布局**排列的模块。
 *
 * 页头（标题与统计范围）与阅读统计错误横幅**不是**可管理的模块，因此不参与布局：它们
 * 是页面的身份与失败事实，做成可隐藏的模块等于允许用户把一次读取失败藏起来。
 *
 * 模块排列来自 `layout`（见 [`overviewLayoutProjection`]）：`order` 决定 DOM 顺序，
 * **存储里的 `row` / `column` 决定它在三列网格里的起始格**，档位决定它占几列。位置不再由
 * CSS 自动猜（旧实现是写死的四段，坐标只挂在 data 属性上、不参与任何摆放），但也不接受
 * 任意坐标——见 `overview-layout.ts` 对「确定性网格摆放」的说明。
 * 三列网格只在 `xl` 分列，坐标因此也只在 `xl` 生效（见 index.css 的
 * `.haven-overview-module-positioned`）；窄屏保持自然单列流。
 * 导出是为了让状态语义（loading / empty / ready / error）能被直接挂载断言。
 */
export function SettingsOverview({ state, forms, layout, onSelectSection, onRetryOverview, onRetryLayout }: { state: ReadingOverviewState; forms: SettingsFormSet; layout: OverviewLayoutProjection; onSelectSection: (section: SettingsNavId) => void; onRetryOverview: () => void; onRetryLayout: () => void }) {
  const stats = readingOverviewStats(state)
  const hasData = state.status === "ready" && hasReadingOverviewData(stats)
  const isLoading = state.status === "loading"
  const isError = state.status === "error"
  const appearance = appearanceDisplayValue(forms.appearance)
  const reading = readingDisplayValue(forms.reading)
  const general = generalDisplayValue(forms.general)
  const playback = playbackDisplayValue(forms.playback)
  const preferences = [
    { label: "当前主题", value: optionLabel(THEME_OPTIONS, appearance.theme), section: "appearance" as const },
    { label: "阅读字体", value: readingFontLabel(reading.fontFamily), section: "reading" as const },
    { label: "启动页", value: optionLabel(LAUNCH_PAGE_OPTIONS, general.launchPage), section: "general" as const },
    { label: "默认播放倍速", value: optionLabel(PLAYBACK_RATE_OPTIONS, playback.defaultPlaybackRate), section: "playback" as const },
  ]
  // 四种状态各自的取值/脚注：loading 说在读、error 说读不到、ready 有数据才下结论、
  // empty 才是真正的「暂无记录」。
  const metricValue = (ready: string) =>
    isLoading ? "…" : isError ? OVERVIEW_UNAVAILABLE_VALUE : ready
  const metricFoot = (ready: string) =>
    isLoading ? "正在读取" : isError ? OVERVIEW_ERROR_TITLE : hasData ? ready : "暂无本机阅读记录"
  const metrics: ReadingOverviewMetric[] = [
    {
      label: "首选类型",
      value: metricValue(contentCategoryLabel(stats.summary.preferredType)),
      foot: metricFoot("按阅读时长计算"),
    },
    {
      label: "最长连续",
      value: metricValue(formatReadingStreak(stats.summary.longestStreakDays)),
      foot: metricFoot("连续活跃阅读日"),
    },
    {
      label: "日均时长",
      value: metricValue(formatReadingDuration(stats.summary.averageDailyMinutes)),
      foot: metricFoot(`按${READING_OVERVIEW_WINDOW_LABEL}计算`),
    },
    {
      // 后端算的是「窗口末尾最近 7 个本地日」，不是自然周：跨周也照样取满 7 天。
      // 因此标签与脚注都只能说「最近 7 天」，说「本周」等于给用户一个后端从没算过的范围。
      label: READING_OVERVIEW_WINDOW_LABEL,
      value: metricValue(formatReadingDuration(stats.summary.recentSevenDayMinutes)),
      foot: metricFoot(`本地时间 · ${READING_OVERVIEW_WINDOW_LABEL}`),
      dark: true,
    },
  ]
  const rangeLabel = state.status === "empty" || state.status === "ready"
    ? `${stats.range.startLocalDate} 至 ${stats.range.endLocalDate}`
    : READING_OVERVIEW_WINDOW_LABEL
  const moduleContext: OverviewModuleContext = { state, preferences, metrics, onSelectSection }

  return (
    <div className="w-full max-w-[1044px] space-y-[34px] pb-[24px]" data-settings-features="reading.overview" tabIndex={-1}>
      <header className="flex items-start justify-between gap-6">
        <div>
          <p className="text-[11px] font-normal tracking-[0.1em] text-[var(--haven-settings-muted-subtle)]">READING OVERVIEW / PERSONAL SIGNAL</p>
          <h2 className="mt-[8px] text-[31px] font-bold leading-[1.12] tracking-[-0.025em] text-[var(--haven-settings-foreground)]">总览</h2>
          <p className="mt-[10px] max-w-[720px] text-[15px] leading-6 text-[var(--haven-settings-muted-subtle)]">先看你花了多少时间，再看你把时间交给了什么。</p>
        </div>
        <div className="shrink-0 pt-[18px] text-right">
          <div className="flex h-[25px] w-[164px] items-center justify-center rounded-full border border-[var(--haven-settings-border)] text-[10px] text-[var(--haven-settings-muted-subtle)]">{READING_OVERVIEW_WINDOW_LABEL}</div>
          <p className="mt-[13px] text-[10px] text-[var(--haven-settings-muted-subtle)]">{state.status === "empty" || state.status === "ready" ? `${stats.range.timezone} · ${rangeLabel}` : "统计范围 · 本机阅读记录"}</p>
        </div>
      </header>

      {state.status === "error" && (
        <div role="alert" className="flex items-center justify-between gap-4 rounded-[14px] border border-[var(--haven-settings-danger-20)] bg-[var(--haven-settings-danger-surface)] px-[16px] py-[12px] text-[12px] text-[var(--haven-settings-danger)]">
          <span>阅读统计暂时不可用：{state.message}</span>
          <button type="button" onClick={onRetryOverview} className="shrink-0 rounded-full border border-[var(--haven-settings-danger-20)] px-[10px] py-[4px] text-[11px] font-semibold text-[var(--haven-settings-danger)] transition-colors hover:bg-[var(--haven-settings-card-hover)]">重试</button>
        </div>
      )}

      {/* 布局读不到时绝不能装作「这就是你保存的排列」：横幅说明当前显示的是默认布局，
          并给出重新读取的入口。这与 stats 的错误态是两件独立的事（一个读不到统计，
          一个读不到排列），所以各自有自己的横幅。 */}
      {layout.status === "error" && (
        <div role="alert" className="flex items-center justify-between gap-4 rounded-[14px] border border-[var(--haven-settings-danger-20)] bg-[var(--haven-settings-danger-surface)] px-[16px] py-[12px] text-[12px] text-[var(--haven-settings-danger)]">
          <span>总览布局暂时不可用：{layout.message} 以下按默认布局显示，不会覆盖你保存过的排列。</span>
          <button type="button" onClick={onRetryLayout} className="shrink-0 rounded-full border border-[var(--haven-settings-danger-20)] px-[10px] py-[4px] text-[11px] font-semibold text-[var(--haven-settings-danger)] transition-colors hover:bg-[var(--haven-settings-card-hover)]">重新读取布局</button>
        </div>
      )}

      {layout.status === "ready" && layout.stale && (
        <p role="status" className="rounded-[14px] border border-[var(--haven-settings-border)] bg-[var(--haven-settings-control)] px-[16px] py-[10px] text-[11px] leading-5 text-[var(--haven-settings-muted-strong)]">
          总览布局可能已被其他窗口更新；以下按最近一次读取到的排列显示。
        </p>
      )}

      {layout.status === "loading" ? (
        <p role="status" className="text-[12px] text-[var(--haven-settings-muted)]">正在读取总览布局…</p>
      ) : layout.placements.length === 0 ? (
        <div className="rounded-[22px] border border-dashed border-[var(--haven-settings-border)] px-[20px] py-[28px] text-center">
          <p className="text-[14px] font-semibold text-[var(--haven-settings-foreground)]">已隐藏全部总览模块</p>
          <p className="mt-[6px] text-[12px] leading-5 text-[var(--haven-settings-muted)]">
            这是你保存过的排列，不是读取失败。可在「外观 › 总览布局」里重新打开需要的模块，页头与统计范围始终保留。
          </p>
        </div>
      ) : (
        <div className="grid grid-cols-1 gap-x-[16px] gap-y-[34px] xl:grid-cols-3" aria-label="总览模块">
          {layout.placements.map((placement) => (
            <div
              key={placement.module}
              data-overview-module={placement.module}
              data-overview-module-size={placement.size}
              data-overview-module-row={placement.row}
              data-overview-module-column={placement.column}
              style={{
                "--haven-overview-module-row": placement.row + 1,
                "--haven-overview-module-column": placement.column + 1,
              } as OverviewModulePositionStyle}
              className={cn("haven-overview-module-positioned min-w-0", OVERVIEW_MODULE_SPAN_CLASS[placement.size])}
            >
              {renderOverviewModule(placement, moduleContext)}
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

/** 档位 → 模块在外层三列网格里占几列（与 Rust `OverviewModuleSize::column_span` 同源）。 */
const OVERVIEW_MODULE_SPAN_CLASS: Record<OverviewModuleSize, string> = {
  small: "xl:col-span-1",
  medium: "xl:col-span-2",
  large: "xl:col-span-3",
}

/**
 * 摆放坐标以 CSS 自定义属性交给 index.css 里的 `xl` 媒体查询（`.haven-overview-module-positioned`）。
 *
 * 为什么不是内联的 `gridRow` / `gridColumn`：三列网格只在 `xl` 生效，窄屏是自然单列流，
 * 而内联样式没有断点，因此坐标交给自定义属性，断点由 CSS 控制。
 *
 * 取值是 **1 基**的网格线编号：存储里的 `row` / `column` 是 0 基的起始格。
 */
type OverviewModulePositionStyle = CSSProperties & {
  "--haven-overview-module-row": number
  "--haven-overview-module-column": number
}

/**
 * 模块**内部**的卡片网格。
 *
 * 必须跟着档位走：把「当前偏好」放进 1/3 宽的格子里却仍然排四列，卡片会被挤成一条
 * 无法阅读的窄条。基准列数沿用升级前的那一套（偏好从 `sm` 起两列、指标从 `md` 起两列），
 * 只在 `xl`——也就是外层网格真正分列的那一档——按档位覆盖。
 */
const OVERVIEW_MODULE_INNER_COLUMNS: Record<OverviewModuleSize, string> = {
  small: "xl:grid-cols-1",
  medium: "xl:grid-cols-2",
  large: "xl:grid-cols-4",
}

type OverviewModuleContext = {
  state: ReadingOverviewState
  preferences: Array<{ label: string; value: string; section: SettingsNavId }>
  metrics: ReadingOverviewMetric[]
  onSelectSection: (section: SettingsNavId) => void
}

/**
 * 一个总览模块的实际渲染。
 *
 * 每个分支都是升级前那一段真实内容原样搬进来（同一批卡片、同一批图表组件），因此
 * 「模块化」没有改变任何一块的内容或空/错态语义，只改变了它们的位置与宽度。
 *
 * `default` 分支在类型上不可达：`OverviewModuleId` 是闭合联合，而落到这里的未知 ID 早已
 * 被 `layoutToOverviewModulePlacements` 丢掉。这里返回 null 不是「静默回落」，而是让闭合
 * 联合新增成员时能在类型检查里暴露出来。
 */
function renderOverviewModule(
  placement: OverviewModulePlacement,
  context: OverviewModuleContext,
): ReactNode {
  switch (placement.module) {
    case "preferences":
      return (
        <section className={cn("grid gap-[14px] sm:grid-cols-2", OVERVIEW_MODULE_INNER_COLUMNS[placement.size])} aria-label="当前偏好">
          {context.preferences.map((preference) => (
            <button
              key={preference.label}
              type="button"
              onClick={() => context.onSelectSection(preference.section)}
              className="group min-h-[112px] rounded-[22px] bg-[var(--haven-settings-card)] p-[20px] text-left transition-colors hover:bg-[var(--haven-settings-card-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary-35)]"
            >
              <div className="flex items-center justify-between gap-3">
                <p className="text-[11px] text-[var(--haven-settings-muted-subtle)]">{preference.label}</p>
                <ChevronRight className="h-[14px] w-[14px] text-[var(--haven-settings-muted-faint)] transition-transform group-hover:translate-x-0.5" strokeWidth={1.8} aria-hidden="true" />
              </div>
              <p className="mt-[15px] truncate text-[19px] font-semibold leading-none tracking-[-0.03em] text-[var(--haven-settings-foreground)]">{preference.value}</p>
              <p className="mt-[13px] text-[10px] text-[var(--haven-settings-muted-faint)]">前往设置</p>
            </button>
          ))}
        </section>
      )
    case "metrics":
      return (
        <section className={cn("grid gap-[14px] md:grid-cols-2", OVERVIEW_MODULE_INNER_COLUMNS[placement.size])} aria-label="阅读指标">
          {context.metrics.map((metric) => <ReadingOverviewMetricCard key={metric.label} metric={metric} />)}
        </section>
      )
    case "reading-minutes":
      return <ReadingMinutesChart state={context.state} />
    case "type-share":
      return <ReadingTypeShareChart state={context.state} />
    case "reading-heatmap":
      return <ReadingHeatmap state={context.state} />
    default:
      return null
  }
}

function ReadingOverviewMetricCard({ metric }: { metric: ReadingOverviewMetric }) {
  return (
    <article className={cn("min-h-[138px] rounded-[22px] p-[23px]", metric.dark ? "bg-[var(--haven-settings-strong-surface)] text-[var(--haven-settings-strong-foreground)]" : "bg-[var(--haven-settings-card)] text-[var(--haven-settings-foreground)]")}>
      <div className="flex items-center justify-between gap-3">
        <p className={cn("text-[11px]", metric.dark ? "text-[var(--haven-settings-strong-foreground)]/75" : "text-[var(--haven-settings-muted-subtle)]")}>{metric.label}</p>
        <span className={cn("h-[8px] w-[8px] rounded-full", metric.dark ? "bg-[var(--haven-settings-strong-foreground)]" : "bg-[var(--haven-settings-strong-surface)]")} aria-hidden="true" />
      </div>
      <p className={cn("mt-[12px] text-[28px] font-bold leading-none tracking-[-0.04em]", metric.dark ? "text-[var(--haven-settings-strong-foreground)]" : "text-[var(--haven-settings-foreground)]")}>{metric.value}</p>
      <div className={cn("mt-[19px] border-t pt-[11px] text-[10px]", metric.dark ? "border-[var(--haven-settings-strong-foreground)]/30 text-[var(--haven-settings-strong-foreground)]/75" : "border-[var(--haven-settings-border)] text-[var(--haven-settings-muted-subtle)]")}>{metric.foot}</div>
    </article>
  )
}

function ReadingMinutesChart({ state }: { state: ReadingOverviewState }) {
  const items = readingOverviewStats(state).dailyMinutes
  const hasData = items.some((item) => item.minutes > 0)
  const maxMinutes = Math.max(...items.map((item) => item.minutes), 1)

  return (
    <section className="min-h-[258px] rounded-[22px] bg-[var(--haven-settings-card)] p-[28px]" aria-label="过去七天阅读时长">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h3 className="text-[18px] font-bold tracking-[-0.025em] text-[var(--haven-settings-foreground)]">每日阅读时长</h3>
          <p className="mt-[5px] text-[11px] text-[var(--haven-settings-muted-subtle)]">过去 7 天 · 阅读时长</p>
        </div>
        <span className="pt-[3px] text-[10px] text-[var(--haven-settings-muted-subtle)]">阅读分钟</span>
      </div>
      <div className="mt-[20px] flex h-[166px] items-center justify-center border-t border-b border-[var(--haven-settings-border)]">
        {state.status === "loading" ? (
          <p className="text-[12px] text-[var(--haven-settings-muted-subtle)]" aria-live="polite">正在读取阅读数据…</p>
        ) : state.status === "error" ? (
          <div className="text-center" role="status">
            <p className="text-[13px] font-semibold text-[var(--haven-settings-muted-strong)]">{OVERVIEW_ERROR_TITLE}</p>
            <p className="mt-[6px] text-[10px] text-[var(--haven-settings-muted-faint)]">{OVERVIEW_ERROR_HINT}</p>
          </div>
        ) : hasData ? (
          <div className="flex h-full w-full items-end justify-between gap-3 px-[8px] pb-[18px] pt-[18px]">
            {items.map((item) => (
              <div key={item.localDate} className="flex h-full min-w-0 flex-1 flex-col items-center justify-end gap-[8px]">
                <span className="text-[9px] text-[var(--haven-settings-muted-subtle)]">{item.minutes > 0 ? `${Math.round(item.minutes)}M` : ""}</span>
                <div className="flex h-[106px] w-full items-end justify-center">
                  <div
                    className="w-full max-w-[38px] rounded-t-[8px] bg-[var(--haven-settings-muted-strong)] transition-[height] duration-300"
                    style={{ height: `${item.minutes > 0 ? Math.max(8, (item.minutes / maxMinutes) * 100) : 3}%` }}
                    aria-label={`${item.label} ${Math.round(item.minutes)} 分钟`}
                  />
                </div>
                <span className="text-[9px] text-[var(--haven-settings-muted-subtle)]">{item.label}</span>
              </div>
            ))}
          </div>
        ) : (
          <div className="text-center">
            <p className="text-[13px] font-semibold text-[var(--haven-settings-muted-strong)]">暂无阅读数据</p>
            <p className="mt-[6px] text-[10px] text-[var(--haven-settings-muted-faint)]">开始阅读后，这里会显示最近 7 天的阅读时长。</p>
          </div>
        )}
      </div>
      <p className="mt-[14px] text-[10px] text-[var(--haven-settings-muted-faint)]">来源 · 本机阅读记录</p>
    </section>
  )
}

function ReadingTypeShareChart({ state }: { state: ReadingOverviewState }) {
  const shares = readingOverviewStats(state).typeShares.filter((item) => item.minutes > 0)
  const hasData = shares.length > 0
  // 「读不到」时圆环与图例都必须说读取失败：把它们显示成「无 / 暂无数据」等于替用户
  // 断言「这段时间没有阅读」，而那正是错误态无从知道的事。
  const isError = state.status === "error"
  const totalMinutes = shares.reduce((total, item) => total + item.minutes, 0)
  const colors = [
    "var(--haven-settings-strong-surface)",
    "var(--haven-settings-muted-strong)",
    "var(--haven-settings-muted)",
    "var(--haven-settings-muted-faint)",
  ]
  let angle = 0
  const gradient = shares.map((share, index) => {
    const start = angle
    angle += (share.minutes / Math.max(totalMinutes, 1)) * 360
    return `${colors[index % colors.length]} ${start}deg ${angle}deg`
  }).join(", ")

  return (
    <section className="min-h-[258px] rounded-[22px] bg-[var(--haven-settings-card)] p-[24px]" aria-label="作品类型占比">
      <h3 className="text-[18px] font-bold tracking-[-0.025em] text-[var(--haven-settings-foreground)]">作品类型分布</h3>
      <p className="mt-[5px] text-[11px] text-[var(--haven-settings-muted-subtle)]">按阅读时长计算</p>
      <div className="mt-[13px] flex min-h-[144px] items-center gap-[18px]">
        <div className="relative h-[144px] w-[144px] shrink-0 rounded-full" style={{ background: hasData ? `conic-gradient(${gradient})` : "var(--haven-settings-control)" }}>
          <div className="absolute inset-[27px] flex flex-col items-center justify-center rounded-full bg-[var(--haven-settings-card)]">
            <span className="text-[22px] font-bold leading-none tracking-[-0.04em] text-[var(--haven-settings-muted-strong)]">{hasData ? `${Math.round(shares[0]?.percentage ?? 0)}%` : isError ? "—" : "无"}</span>
            <span className="mt-[5px] text-[10px] text-[var(--haven-settings-muted-subtle)]">{hasData ? contentCategoryLabel(shares[0]?.category ?? null) : isError ? OVERVIEW_UNAVAILABLE_VALUE : "暂无数据"}</span>
          </div>
        </div>
        <div className="min-w-0 flex-1">
          {hasData ? (
            <div className="space-y-[8px]">
              {shares.map((share, index) => (
                <div key={share.category} className="flex items-center justify-between gap-3 text-[10px]">
                  <span className="flex min-w-0 items-center gap-2 text-[var(--haven-settings-muted-strong)]"><span className="h-[7px] w-[7px] shrink-0 rounded-full" style={{ backgroundColor: colors[index % colors.length] }} />{contentCategoryLabel(share.category)}</span>
                  <span className="shrink-0 text-[var(--haven-settings-muted-subtle)]">{Math.round(share.percentage)}%</span>
                </div>
              ))}
            </div>
          ) : isError ? (
            <p className="text-[11px] leading-5 text-[var(--haven-settings-muted-subtle)]">{OVERVIEW_ERROR_TITLE}：{OVERVIEW_ERROR_HINT}</p>
          ) : (
            <p className="text-[11px] leading-5 text-[var(--haven-settings-muted-subtle)]">开始阅读后，这里会显示小说、漫画、影视与报刊的占比。</p>
          )}
        </div>
      </div>
      <p className="mt-[12px] text-[10px] text-[var(--haven-settings-muted-faint)]">来源 · 本机阅读记录 · 类型分布</p>
    </section>
  )
}

function ReadingHeatmap({ state }: { state: ReadingOverviewState }) {
  const heatmap = readingOverviewStats(state).heatmap
  const cells = heatmap.cells.filter((cell) => cell.minutes > 0)
  const hours = Array.from(new Set(heatmap.cells.map((cell) => cell.hour))).sort((a, b) => a - b)
  const hasData = cells.length > 0
  const maxMinutes = Math.max(...cells.map((cell) => cell.minutes), 1)
  const dayLabels = ["一", "二", "三", "四", "五", "六", "日"]
  const cellMap = new Map(heatmap.cells.map((cell) => [`${cell.dayOfWeek}:${cell.hour}`, cell.minutes]))
  // 错误态与空态必须分开：热力图的「无高峰 / 暂无本机阅读记录」是在断言这段时间没有
  // 阅读，而读取失败时页面根本不知道这件事。
  const isError = state.status === "error"
  const peak = heatmap.peakStartHour != null && heatmap.peakEndHour != null
    ? `${heatmap.peakStartHour}–${heatmap.peakEndHour} 点`
    : isError ? OVERVIEW_UNAVAILABLE_VALUE : "无"
  const heatmapGrid = hours.flatMap((hour) => [
    <span key={`hour-${hour}`} className="text-right">{String(hour).padStart(2, "0")}</span>,
    ...dayLabels.map((_, index) => {
      const minutes = cellMap.get(`${index + 1}:${hour}`) ?? 0
      return <span key={`${hour}-${index}`} title={`${hour} 点 · ${Math.round(minutes)} 分钟`} className="h-[10px] rounded-full" style={{ backgroundColor: minutes > 0 ? `color-mix(in srgb, var(--haven-settings-strong-surface) ${Math.round((0.18 + (minutes / maxMinutes) * 0.72) * 100)}%, transparent)` : "var(--haven-settings-control)" }} />
    }),
  ])

  return (
    <section className="min-h-[204px] rounded-[22px] bg-[var(--haven-settings-card)] p-[28px]" aria-label="阅读时段热力图">
      <div>
        <h3 className="text-[18px] font-bold tracking-[-0.025em] text-[var(--haven-settings-foreground)]">阅读时段</h3>
        <p className="mt-[5px] text-[11px] text-[var(--haven-settings-muted-subtle)]">最近 7 天 · 按阅读时长着色</p>
      </div>
      <div className="mt-[18px] grid gap-[24px] md:grid-cols-[260px_minmax(0,1fr)]">
        <div className="border-r border-[var(--haven-settings-border)] pr-[24px]">
          <p className="text-[10px] text-[var(--haven-settings-muted-subtle)]">高峰时段</p>
          <p className="mt-[8px] text-[26px] font-normal leading-none tracking-[-0.035em] text-[var(--haven-settings-foreground)]">{peak}</p>
          <p className="mt-[8px] text-[11px] text-[var(--haven-settings-muted-subtle)]">{hasData ? "阅读时长最集中" : isError ? OVERVIEW_ERROR_TITLE : "暂无本机阅读记录"}</p>
        </div>
        <div className="flex min-h-[94px] items-center justify-center border border-dashed border-[var(--haven-settings-border)] px-[24px] text-center">
          {hasData ? (
            <div className="grid w-full grid-cols-[28px_repeat(7,minmax(0,1fr))] items-center gap-[5px] text-[9px] text-[var(--haven-settings-muted-subtle)]">
              <span aria-hidden="true" />
              {dayLabels.map((label) => <span key={label} className="text-center">{label}</span>)}
              {heatmapGrid}
            </div>
          ) : isError ? (
            <div role="status">
              <p className="text-[13px] font-semibold text-[var(--haven-settings-muted-strong)]">{OVERVIEW_ERROR_TITLE}</p>
              <p className="mt-[6px] text-[10px] text-[var(--haven-settings-muted-faint)]">{OVERVIEW_ERROR_HINT}</p>
            </div>
          ) : (
            <div>
              <p className="text-[13px] font-semibold text-[var(--haven-settings-muted-strong)]">暂无阅读数据</p>
              <p className="mt-[6px] text-[10px] text-[var(--haven-settings-muted-faint)]">开始阅读后，会显示最近 7 天的阅读时段。</p>
            </div>
          )}
        </div>
      </div>
      <p className="mt-[10px] text-[10px] text-[var(--haven-settings-muted-faint)]">来源 · 本机阅读记录 · 最近 7 天</p>
    </section>
  )
}

/**
 * 分区渲染：登记表里的每个分区都有自己的真实组件。
 *
 * 没有 `sync` 分支——它不在登记表里，也就是不可导航、不可渲染。栖伴入口挂在
 * 「智能功能」分区上（`ai.recommendation`，绑定 `aiSettingsRecommendationGenerate`）：
 * 模型服务已经通过 A2 AI Provider 切片接入，请求仍只生成 pending 提案，批准只能回到
 * 这个界面完成。
 *
 * 未知分区**绝不回落到通用设置**：那会把「这里没有渲染器」伪装成一个看起来正常的
 * 通用页面，用户以为自己在改 A，实际改的是 B。漏注册是开发期错误，所以这里渲染一个
 * 说清楚的不可用态；settings-registry.test.ts 另外钉住「登记表的每个 id 都必须有
 * 自己的 case」，让新增分区在测试里就暴露，而不是等用户看见一个错的分区。
 */

/**
 * 分区之上的 AI Provider 选择。
 *
 * 由 `SettingsContent` 持有并传给「智能功能」分区，因此切换分区（组件重新挂载）不会
 * 把它重置成列表第一项——那会让栖伴的请求静默改投另一个 Provider。
 */
type AiProviderSelection = {
  profileId: string | null
  onSelect: (profileId: string | null) => void
}

function renderSettingsSection(
  section: SettingsRegistryEntry["id"],
  forms: SettingsFormSet,
  showNotice: (message: string) => void,
  resourceContext: ResourcePreferenceContext | null,
  overviewLayout: OverviewLayoutController,
  appearanceAssets: AppearanceAssetsController,
) {
  switch (section) {
    case "appearance":
      return (
        <AppearanceSettings
          form={forms.appearance}
          showNotice={showNotice}
          overviewLayout={overviewLayout}
          assets={appearanceAssets}
        />
      )
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
    case "updates":
      return <UpdateSettings showNotice={showNotice} />
    case "privacy":
      return <PrivacySettings form={forms.privacy} showNotice={showNotice} />
    case "about":
      return <AboutSettings diagnostics={<ErrorReportSettings />} />
    case "general":
      return <GeneralSettings form={forms.general} />
    default:
      return <SettingsSectionUnavailable section={section} />
  }
}

/**
 * 没有渲染器的分区：登记表里存在、但 `renderSettingsSection` 还没有对应的 case。
 *
 * 这不是给用户准备的功能，而是把「开发期漏注册」变成一个看得见的错误——显示通用设置
 * 会让人以为分区是正常的，从而把配置改到错误的位置。导出以便组件测试直接挂载。
 */
export function SettingsSectionUnavailable({ section }: { section: string }) {
  return (
    <div aria-label={section} role="alert" className="px-2 pb-2 pt-2">
      <h2 className="text-[28px] font-bold tracking-[-0.025em] text-[var(--haven-settings-foreground)]">该设置分区暂未注册渲染器</h2>
      <p className="mt-1.5 max-w-2xl text-[14px] leading-6 text-[var(--haven-settings-muted-strong)]">
        分区「{section}」在设置登记表里存在，但设置页没有对应它的渲染器。为避免把配置写到错误的分区，
        这里不显示任何设置项；请更新设置页后再打开该分区。
      </p>
    </div>
  )
}

function SettingsIntro({ section, title, description }: { section: string; title: string; description: string }) {
  return (
    <div aria-label={section} className="px-2 pb-2 pt-2">
      <h2 className="text-[28px] font-bold tracking-[-0.025em] text-[var(--haven-settings-foreground)]">{title}</h2>
      <p className="mt-1.5 max-w-2xl text-[14px] leading-6 text-[var(--haven-settings-muted-strong)]">{description}</p>
    </div>
  )
}

function SettingsGroup({ title, description, children, action, features }: { title: string; description?: string; children: ReactNode; action?: ReactNode; features?: readonly FeatureId[] }) {
  return (
    <section className="mb-6 last:mb-0" data-settings-features={features?.join(" ")} tabIndex={features ? -1 : undefined}>
      <div className="flex items-start justify-between gap-3 px-2 pb-2">
        <div className="min-w-0">
          <h3 className="text-[11px] font-semibold tracking-[0.12em] uppercase text-[var(--haven-settings-muted)]">{title}</h3>
          {description && <p className="mt-1 text-[12px] leading-5 text-[var(--haven-settings-muted)]">{description}</p>}
        </div>
        {action}
      </div>
      <div className="overflow-hidden rounded-[14px] border border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-card)] shadow-[0_2px_8px_rgba(0,0,0,0.02)]">
        {children}
      </div>
    </section>
  )
}

function SettingRow({ title, description, children, icon, danger = false }: { title: string; description?: string; children: ReactNode; icon?: ReactNode; danger?: boolean }) {
  return (
    <div data-settings-row="true" className="group/row flex min-h-[68px] items-center gap-4 border-b border-[var(--haven-settings-border-subtle)] px-6 py-4 transition-colors hover:bg-[var(--haven-settings-card-hover)] last:border-b-0">
      <div className="flex min-w-0 flex-1 items-center gap-3.5">
        {icon && <span className="flex h-[28px] w-[28px] shrink-0 items-center justify-center rounded-lg bg-[var(--haven-settings-control)] text-[var(--haven-settings-muted)] transition-colors group-hover/row:text-[var(--haven-settings-primary)]">{icon}</span>}
        <div className="min-w-0">
          <p className={cn("text-[14px] font-semibold tracking-[-0.005em]", danger ? "text-[var(--haven-settings-danger)]" : "text-[var(--haven-settings-foreground)]")}>{title}</p>
          {description && <p className="mt-1 max-w-[520px] text-[12px] leading-[1.65] text-[var(--haven-settings-muted)]">{description}</p>}
        </div>
      </div>
      <div className="shrink-0 sm:ml-8">{children}</div>
    </div>
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
  return { section: "appearance", theme: "system", density: "comfortable", sidebar: "auto", reduceMotion: false, interfaceFontMode: "system" }
}

/**
 * 外观表单的草稿与最近一次成功读取的持久化选择。
 *
 * 未保存草稿用于避免用户误删正在配置的资产；saved 用于保护运行时真正正在使用的资产。
 * 尚未读取到持久化值时 saved 为 null，删除守卫会失败关闭。
 */
function appearanceAssetSelection(form: SettingsFormController): AppearanceAssetDeleteSelections {
  const toSelection = (value: AppearanceSettingsValue): AppearanceAssetSelection => ({
    fontAssetId: value.customFontAssetId ?? null,
    wallpaper: value.wallpaper,
  })
  const saved = "saved" in form.state && form.state.saved.section === "appearance"
    ? form.state.saved
    : null
  return {
    draft: toSelection(appearanceDisplayValue(form)),
    saved: saved ? toSelection(saved) : null,
  }
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

function SelectControl({ value, options, onChange, ariaLabel, disabled = false }: { value: string; options: string[]; onChange: (value: string) => void; ariaLabel: string; disabled?: boolean }) {
  const rootRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const menuRef = useRef<HTMLDivElement>(null)
  const menuId = useId()
  const selectedIndex = Math.max(0, options.indexOf(value))
  const [isOpen, setIsOpen] = useState(false)
  const [highlightedIndex, setHighlightedIndex] = useState(selectedIndex)
  const [menuStyle, setMenuStyle] = useState<CSSProperties>({})
  const [menuContainer, setMenuContainer] = useState<HTMLElement | null>(null)

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
    setMenuContainer(triggerRef.current?.closest("dialog") ?? document.body)
    setHighlightedIndex(selectedIndex)
    updateMenuPosition()
    setIsOpen(true)
  }

  const selectOption = (option: string) => {
    onChange(option)
    setIsOpen(false)
    triggerRef.current?.focus()
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
    } else if (event.key === "Escape" && isOpen) {
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
            "inline-flex h-[36px] min-w-[140px] max-w-[280px] cursor-pointer items-center justify-between gap-[12px] rounded-xl border bg-[var(--haven-settings-control)] px-[12px] text-[14px] font-medium text-[var(--haven-settings-foreground)] outline-none transition-all hover:bg-[var(--haven-settings-card-hover)]",
            isOpen ? "border-[var(--haven-settings-primary)] ring-4 ring-[var(--haven-settings-primary-10)]" : "border-[var(--haven-settings-control-border)] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--haven-settings-primary)]",
            disabled && "cursor-not-allowed opacity-50"
          )}
        >
          <span className="min-w-0 flex-1 truncate text-right">{value}</span>
          <ChevronDown className={cn("h-[16px] w-[16px] shrink-0 text-[var(--haven-settings-muted)] transition-transform duration-200", isOpen && "rotate-180 text-[var(--haven-settings-primary)]")} strokeWidth={2.2} />
        </button>

      {isOpen && menuContainer && createPortal(
        <div
          ref={menuRef}
          id={menuId}
          role="listbox"
          aria-label={ariaLabel}
          style={menuStyle}
          onKeyDown={(event) => {
            if (event.key !== "Escape") return
            event.preventDefault()
            event.stopPropagation()
            setIsOpen(false)
            triggerRef.current?.focus()
          }}
          className="settings-scrollbar-hidden overflow-y-auto rounded-[12px] border border-[var(--haven-settings-border)] bg-[var(--haven-settings-card)] p-[4px] shadow-[0_14px_36px_rgba(0,0,0,0.16),0_2px_8px_rgba(0,0,0,0.06)] backdrop-blur-xl"
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
                    ? "bg-[var(--haven-settings-primary)] text-[var(--haven-settings-primary-foreground)]"
                    : isHighlighted
                      ? "bg-[var(--haven-settings-primary-08)] text-[var(--haven-settings-primary)]"
                      : "text-[var(--haven-settings-foreground)] hover:bg-[var(--haven-settings-primary-08)] hover:text-[var(--haven-settings-primary)]"
                )}
              >
                <span className="min-w-0 flex-1 truncate">{option}</span>
                {isSelected && <CircleCheck className="h-[14px] w-[14px] shrink-0" strokeWidth={2.4} />}
              </button>
            )
          })}
        </div>,
        menuContainer
      )}
    </div>
  )
}

function SegmentedControl({ value, options, onChange, ariaLabel, disabled = false }: { value: string; options: string[]; onChange: (value: string) => void; ariaLabel: string; disabled?: boolean }) {
  return (
    <div className="flex flex-wrap items-center gap-1 rounded-xl bg-[var(--haven-settings-control)] p-1" role="group" aria-label={ariaLabel}>
      {options.map((option) => (
        <button
          key={option}
          type="button"
          // 选中态此前只有视觉差异；补上 aria-pressed 后读屏与测试都能确认选中的是哪一项。
          aria-pressed={option === value}
          disabled={disabled}
          onClick={() => onChange(option)}
          className={cn(
            "rounded-[8px] px-3.5 py-1.5 text-[13px] font-medium transition-all duration-200",
            option === value ? "bg-[var(--haven-settings-primary)] text-[var(--haven-settings-primary-foreground)] shadow-sm" : "text-[var(--haven-settings-muted-strong)] hover:bg-[var(--haven-settings-card-hover)] hover:text-[var(--haven-settings-foreground)]",
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
    status = <span className="flex items-center gap-2 text-[13px] text-[var(--haven-settings-muted)]"><RefreshCw className="h-[16px] w-[16px] animate-spin" strokeWidth={2.2} />正在加载…</span>
  } else if (state.status === "saving") {
    status = <span className="flex items-center gap-2 text-[13px] text-[var(--haven-settings-muted-strong)]"><RefreshCw className="h-[16px] w-[16px] animate-spin" strokeWidth={2.2} />正在保存…</span>
  } else if (form.isDirty) {
    status = <span className="flex items-center gap-2 text-[13px] font-medium text-[var(--haven-settings-warning)]"><TriangleAlert className="h-[16px] w-[16px]" strokeWidth={2.2} />有未保存的修改</span>
  } else {
    status = <span className="flex items-center gap-2 text-[13px] text-[var(--haven-settings-muted-strong)]"><CircleCheck className="h-[16px] w-[16px] text-[#34c759]" strokeWidth={2.2} />所有设置已保存</span>
  }

  return (
    <div className="mx-6 mb-1 flex items-center justify-between gap-4 border-b border-[var(--haven-settings-border-subtle)] py-4">
      <div className="flex items-center gap-2 text-[13px] text-[var(--haven-settings-muted-strong)]">{status}</div>
      <div className="flex items-center gap-6">
        {form.isDirty && (
          <button
            type="button"
            onClick={() => form.save()}
            disabled={form.isSaving}
            className="text-[13px] font-semibold text-[var(--haven-settings-primary)] transition-colors hover:text-[var(--haven-settings-primary-hover)] hover:underline disabled:cursor-not-allowed disabled:opacity-50"
          >
            保存修改
          </button>
        )}
        <button type="button" onClick={onReset} className="text-[13px] font-medium text-[var(--haven-settings-muted)] transition-colors hover:text-[var(--haven-settings-foreground)] hover:underline">
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
    <div className="mx-6 mb-1 flex flex-wrap items-center justify-between gap-3 rounded-2xl border border-[var(--haven-settings-danger-15)] bg-[var(--haven-settings-danger-surface)] px-4 py-3">
      <div className="flex min-w-0 items-center gap-2 text-[13px] leading-5 text-[var(--haven-settings-danger)]">
        <TriangleAlert className="h-[16px] w-[16px] shrink-0" strokeWidth={2.2} />
        <span className="min-w-0">{state.message}</span>
      </div>
      <button type="button" onClick={() => form.retry()} className="shrink-0 rounded-full border border-[var(--haven-settings-danger-20)] bg-[var(--haven-settings-card)] px-3 py-1.5 text-[12px] font-semibold text-[var(--haven-settings-danger)] transition-colors hover:bg-[var(--haven-settings-card-hover)]">
        {retryLabel}
      </button>
    </div>
  )
}

/**
 * 外观分区。
 *
 * 能力都走真实通道，且都是「先保存、再投影」：
 *   - 主题 / 密度 / 侧栏留白 / 减少动效 → settingsGateway（appearance 分区）；
 *   - 自定义调色板与强调色 → 同一个分区里的 customTheme（运行时投影成 CSS token）；
 *   - 字体与壁纸资产 → appearanceGateway（列表 / 导入 / 删除，只有不透明 assetId）；
 *   - 总览布局 → appearanceGateway 的 overviewLayoutGet/Save/Reset（**另一份**布局、
 *     另一条 revision、三列网格）。
 *
 * 没有任何一项是本地假状态：控件改的是表单草稿，草稿经 SettingsFormController 提交；
 * 资产与总览布局各自的异步结果由对应 Hook 呈现（loading / empty / ready / conflict /
 * error）。
 *
 * 总览布局与资产列表的控制器都由 `SettingsContent` 持有并传入（不是在这里 `use*`）：
 * 总览页渲染的就是同一份已保存状态，所以保存成功后切回总览立刻按新排列渲染；而外观分区
 * 只是它们的编辑器——切到别分区再切回来不会重读列表，也不会丢掉还没保存的布局草稿或
 * 上一次导入/删除的结果。
 */
const PALETTE_COLOR_ROLES: ReadonlyArray<{ role: PaletteColorRole; label: string }> = [
  { role: "background", label: "页面底色" },
  { role: "card", label: "卡片" },
  { role: "foreground", label: "文字" },
]

function ThemePalettePreview({
  palette,
  accentColor,
  mode,
}: {
  palette: ReturnType<typeof normalizeAppTheme>["light"]
  accentColor: string
  mode: "light" | "dark"
}) {
  const modeLabel = mode === "light" ? "浅色" : "深色"
  // 预览仅把当前表单草稿的颜色映射到小型界面样例；它不会写入 CSS 全局状态或持久化。
  return (
    <div className="min-w-0 rounded-[14px] border border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-sidebar)] p-4">
      <div className="mb-3 flex items-center justify-between gap-3">
        <p className="text-[13px] font-semibold text-[var(--haven-settings-foreground)]">实时预览</p>
        <span className="text-[11px] text-[var(--haven-settings-muted)]">{modeLabel}</span>
      </div>
      <div
        role="img"
        aria-label={`${modeLabel}主题配色预览`}
        className="overflow-hidden rounded-[12px] border p-3"
        style={{ backgroundColor: palette.background, borderColor: palette.border, color: palette.foreground }}
      >
        <div className="flex items-center justify-between gap-3">
          <span className="text-[13px] font-semibold">栖阅</span>
          <span className="rounded-md px-2 py-1 text-[10px]" style={{ backgroundColor: palette.secondary, color: palette.foreground }}>首页</span>
        </div>
        <div className="mt-3 rounded-[10px] border p-3" style={{ backgroundColor: palette.card, borderColor: palette.border }}>
          <p className="text-[10px]" style={{ color: palette.mutedForeground }}>最近阅读</p>
          <p className="mt-1 text-[12px] font-medium">在喜欢的配色里，继续阅读</p>
          <span className="mt-3 inline-flex rounded-md px-2.5 py-1.5 text-[10px] font-semibold" style={{ backgroundColor: accentColor, color: palette.primaryForeground }}>继续阅读</span>
        </div>
      </div>
    </div>
  )
}

function AppearanceSettings({ form, showNotice, overviewLayout, assets }: { form: SettingsFormController; showNotice: (message: string) => void; overviewLayout: OverviewLayoutController; assets: AppearanceAssetsController }) {
  const value = appearanceDisplayValue(form)
  const selection = appearanceAssetSelection(form)
  // 预览的减少动效结论与 AppShell 的壁纸投影同源：设置项（appearance.reduceMotion）与
  // 系统 prefers-reduced-motion **任一**为真就抑制动态预览。只读设置项会让「系统已经
  // 要求减少动效」的用户在设置页里看到一个正在循环播放的视频。
  const systemReducedMotion = useSystemPreference("(prefers-reduced-motion: reduce)")
  const previewReducedMotion = value.reduceMotion || systemReducedMotion
  const [paletteMode, setPaletteMode] = useState<"light" | "dark">("light")
  const [recentColors, setRecentColors] = useState<string[]>([])
  const [palettePreview, setPalettePreview] = useState<{
    kind: "role"
    mode: "light" | "dark"
    role: PaletteColorRole
    color: string
  } | {
    kind: "accent"
    color: string
  } | null>(null)
  const customTheme = value.customTheme == null
    ? themePresetAppTheme("ink")
    : normalizeAppTheme(value.customTheme)
  const previewTheme = palettePreview === null
    ? customTheme
    : palettePreview.kind === "accent"
      ? withAccentColor(customTheme, palettePreview.color)
      : {
        ...customTheme,
        [palettePreview.mode]: withPaletteRoleColor(
          customTheme[palettePreview.mode],
          palettePreview.role,
          palettePreview.color,
        ),
      }
  const rememberColor = (color: string) => {
    const normalized = color.toLowerCase()
    setRecentColors((current) => [normalized, ...current.filter((item) => item !== normalized)].slice(0, 6))
  }
  // 删除守卫与提交路径共用同一份判断（appearanceAssetDeleteBlockedReason）。
  const blockedReasonFor = (assetId: string) => appearanceAssetDeleteBlockedReason(assetId, selection)

  const changeTheme = (theme: ThemeWire) => {
    // 初次进入自定义时从墨黑预设起步，避免把用户直接带入刺眼的蓝色。
    if (theme === "custom" && value.customTheme == null) {
      form.change({ section: "appearance", theme, customTheme: themePresetAppTheme("ink") })
      return
    }
    form.change({ section: "appearance", theme })
  }

  const updatePaletteRole = (role: PaletteColorRole, input: string) => {
    const palette = customTheme[paletteMode]
    if (paletteColorInputValue(palette[role]) === input.toLowerCase()) return
    const nextPalette = withPaletteRoleColor(palette, role, input)
    if (nextPalette === palette) return
    const nextTheme = paletteMode === "light"
      ? { ...customTheme, light: nextPalette }
      : { ...customTheme, dark: nextPalette }
    form.change({ section: "appearance", theme: "custom", customTheme: nextTheme })
  }

  const updateAccent = (input: string) => {
    if (paletteColorInputValue(customTheme.accentColor) === input.toLowerCase()) return
    const nextTheme = withAccentColor(customTheme, input)
    if (nextTheme === customTheme) return
    form.change({
      section: "appearance",
      theme: "custom",
      customTheme: nextTheme,
    })
  }

  const applyThemePreset = (presetId: ThemePresetId) => {
    form.change({
      section: "appearance",
      theme: "custom",
      customTheme: themePresetAppTheme(presetId),
    })
  }

  return (
    <>
      <SettingsIntro section="Appearance" title="外观" description="主题、字体与首页壁纸可以自定义；首页和总览的模块也能分别调整。" />
      <SettingsFormStatusBar form={form} onReset={() => form.resetToDefaults()} />
      <SettingsFormError form={form} />

      <SettingsGroup title="界面外观" features={["appearance.theme", "appearance.density", "appearance.sidebar", "appearance.reduceMotion"]}>
        <SettingRow title="主题" description="跟随系统，或选择浅色、深色界面。选择下方的配色方案后会使用自定义主题。">
          <SegmentedControl value={optionLabel(THEME_OPTIONS, value.theme)} options={THEME_OPTIONS.map((option) => option.label)} onChange={(label) => changeTheme(optionValue(THEME_OPTIONS, label))} ariaLabel="主题" />
        </SettingRow>
        <SettingRow title="界面密度" description="控制列表和卡片之间的留白。">
          <SegmentedControl value={optionLabel(DENSITY_OPTIONS, value.density)} options={DENSITY_OPTIONS.map((option) => option.label)} onChange={(label) => form.change({ section: "appearance", density: optionValue(DENSITY_OPTIONS, label) })} ariaLabel="界面密度" />
        </SettingRow>
        <SettingRow title="内容边距" description="调整桌面页面内容与窗口边缘之间的留白。">
          <SelectControl value={optionLabel(SIDEBAR_OPTIONS, value.sidebar)} options={SIDEBAR_OPTIONS.map((option) => option.label)} onChange={(label) => form.change({ section: "appearance", sidebar: optionValue(SIDEBAR_OPTIONS, label) })} ariaLabel="内容边距" />
        </SettingRow>
        <SettingRow title="减少动效" description="减少背景动画与页面过渡效果，让界面更安静。">
          <Toggle checked={value.reduceMotion} onChange={(checked) => form.change({ section: "appearance", reduceMotion: checked })} label="减少动效" />
        </SettingRow>
      </SettingsGroup>
      <SettingsGroup title="主题与配色" features={["appearance.customTheme"]} description="先选择一种预设，再用推荐色或色值微调。预览会随调整同步变化，保存后应用。">
        <div className="border-b border-[var(--haven-settings-border-subtle)] px-6 py-5">
          <p className="text-[13px] font-semibold text-[var(--haven-settings-foreground)]">预设配色</p>
          <p className="mt-1 text-[12px] text-[var(--haven-settings-muted)]">四种低饱和方案，也可以继续自选颜色。</p>
          <div role="group" aria-label="预设配色" className="mt-4 grid grid-cols-2 gap-2 sm:grid-cols-4">
            {THEME_PRESETS.map((preset) => {
              const selected = value.theme === "custom"
                && customTheme.accentColor === preset.theme.accentColor
                && customTheme.light.background === preset.theme.light.background
                && customTheme.light.card === preset.theme.light.card
                && customTheme.light.foreground === preset.theme.light.foreground
                && customTheme.dark.background === preset.theme.dark.background
                && customTheme.dark.card === preset.theme.dark.card
                && customTheme.dark.foreground === preset.theme.dark.foreground
              return (
                <button
                  key={preset.id}
                  type="button"
                  aria-pressed={selected}
                  onClick={() => applyThemePreset(preset.id)}
                  className={cn(
                    "flex min-h-[58px] min-w-0 items-center gap-2.5 rounded-[10px] border px-3 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary)]",
                    selected
                      ? "border-[var(--haven-settings-primary)] bg-[var(--haven-settings-card-hover)]"
                      : "border-[var(--haven-settings-control-border)] hover:bg-[var(--haven-settings-card-hover)]",
                  )}
                >
                  <span className="flex shrink-0 gap-1" aria-hidden="true">
                    {preset.swatches.map((color, index) => (
                      <span key={index} className="h-[17px] w-[9px] rounded-sm border border-black/[0.08]" style={{ backgroundColor: color }} />
                    ))}
                  </span>
                  <span className="min-w-0">
                    <span className="block truncate text-[12px] font-semibold text-[var(--haven-settings-foreground)]">{preset.label}</span>
                    <span className="block truncate text-[10px] text-[var(--haven-settings-muted)]">{preset.description}</span>
                  </span>
                </button>
              )
            })}
          </div>
        </div>
        <div className="grid gap-4 px-6 py-5 lg:grid-cols-[minmax(0,1fr)_minmax(280px,0.9fr)]">
          <div className="min-w-0">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div>
                <p className="text-[13px] font-semibold text-[var(--haven-settings-foreground)]">自选颜色</p>
                <p className="mt-1 text-[12px] text-[var(--haven-settings-muted)]">选择浅色或深色版本进行微调。</p>
              </div>
              <SegmentedControl value={paletteMode === "light" ? "浅色" : "深色"} options={["浅色", "深色"]} onChange={(label) => setPaletteMode(label === "浅色" ? "light" : "dark")} ariaLabel="预览配色模式" />
            </div>
            <div className="mt-4 grid grid-cols-2 gap-2">
              {PALETTE_COLOR_ROLES.map(({ role, label }) => {
                const modeLabel = paletteMode === "light" ? "浅色" : "深色"
                const contrastTargets = role === "background"
                  ? [{ label: "文字", color: customTheme[paletteMode].foreground }]
                  : role === "card"
                    ? [{ label: "卡片文字", color: customTheme[paletteMode].cardForeground }]
                    : [
                      { label: "页面底色", color: customTheme[paletteMode].background },
                      { label: "卡片", color: customTheme[paletteMode].card },
                    ]
                return (
                  <AppearanceColorPicker
                    key={paletteMode + "-" + role}
                    label={modeLabel + label}
                    value={paletteColorInputValue(customTheme[paletteMode][role])}
                    contrastTargets={contrastTargets}
                    recentColors={recentColors}
                    onDraftChange={(color) => setPalettePreview(color
                      ? { kind: "role", mode: paletteMode, role, color }
                      : null)}
                    onApply={(color) => updatePaletteRole(role, color)}
                    onRecentColor={rememberColor}
                  />
                )
              })}
              <AppearanceColorPicker
                key="accent"
                label="强调色"
                value={paletteColorInputValue(customTheme.accentColor)}
                contrastTargets={[{ label: "按钮文字", color: customTheme[paletteMode].primaryForeground }]}
                recentColors={recentColors}
                onDraftChange={(color) => setPalettePreview(color ? { kind: "accent", color } : null)}
                onApply={updateAccent}
                onRecentColor={rememberColor}
              />
            </div>
          </div>
          <ThemePalettePreview palette={previewTheme[paletteMode]} accentColor={previewTheme.accentColor} mode={paletteMode} />
        </div>
      </SettingsGroup>

      <InterfaceFontSettings form={form} value={value} showNotice={showNotice} />

      <AppearanceWallpaperGroup
        assets={assets.state.status === "ready" ? assets.state.lists : null}
        loading={assets.state.status === "loading"}
        error={assets.state.status === "error" ? assets.state.message : null}
        selection={value.wallpaper ?? { kind: "none" }}
        pending={assets.pending === "import"}
        onImport={(kind) => assets.importAsset(kind)}
        onSelect={(wallpaper) => form.change({ section: "appearance", wallpaper })}
        onDelete={assets.deleteAsset}
        blockedReasonFor={blockedReasonFor}
        reducedMotion={previewReducedMotion}
        action={assets.action}
      />

      <OverviewLayoutEditor controller={overviewLayout} />
    </>
  )
}

/** 预览专用 @font-face 宿主；与 AppShell 注入的那条（haven-interface-font-face）分开。 */
const INTERFACE_FONT_PREVIEW_STYLE_ID = "haven-interface-font-preview-face"
const INTERFACE_FONT_SELECTION_PANEL_ID = "interface-font-selection-panel"

/**
 * 界面字体（外观 → 界面字体；BE-INTERFACE-FONT-001）。
 *
 * 四张互斥卡片分别是跟随系统 / 人文衬线 / 现代黑体 / 自定义系统字体，选择直接写进
 * appearance 表单草稿（保存走真实 CAS，这里不落任何本地状态、不用 localStorage）。
 * 自定义展开两条真实来源：本机已安装字体（Rust 枚举，只回传族名）与导入字体
 * （Native 选择器 + 迁移 045 持久化，前端只拿 opaque id）。
 *
 * 预览与应用端共用 `resolveInterfaceFont`：预览看到的字体栈就是 AppShell 会写进
 * `--ds-font-interface` 的那一个，不存在「预览一套、生效另一套」。
 */
export function InterfaceFontSettings({
  form,
  value,
  showNotice,
}: {
  form: SettingsFormController
  value: AppearanceSettingsValue
  showNotice: (message: string) => void
}) {
  const custom = value.interfaceFontMode === "custom"
  const assetsState = useInterfaceFontAssets()
  const familiesState = useSystemFontFamilies(custom)
  const [query, setQuery] = useState("")
  const [importing, setImporting] = useState(false)
  const [deletingId, setDeletingId] = useState<string | null>(null)

  const { assets } = assetsState
  const selection = {
    interfaceFontMode: value.interfaceFontMode,
    interfaceFontFamily: value.interfaceFontFamily,
    interfaceFontAssetId: value.interfaceFontAssetId,
  }
  // 预览用**草稿**解析：选择后立即看到效果；保存后整个界面才跟随。
  const resolved = resolveInterfaceFont(selection, assets, controlledResourceUri)
  useInjectedFontFace(INTERFACE_FONT_PREVIEW_STYLE_ID, custom ? resolved.fontFaceCss : null)

  const visibleAssets = filterInterfaceFontAssets(assets, query)
  const visibleFamilies = filterFontFamilies(familiesState.families, query)
  const queryHasNoResults =
    query.trim().length > 0 &&
    assetsState.status === "ready" &&
    familiesState.status === "ready" &&
    visibleAssets.length === 0 &&
    visibleFamilies.length === 0

  const selectFamily = (family: string) => {
    // 两个选择字段互斥：选本机字体即清除导入字体引用（后端把空串当「清除」）。
    form.change({ section: "appearance", interfaceFontMode: "custom", interfaceFontFamily: family, interfaceFontAssetId: "" })
  }
  const selectAsset = (assetId: string) => {
    form.change({ section: "appearance", interfaceFontMode: "custom", interfaceFontAssetId: assetId, interfaceFontFamily: "" })
  }

  const handleImport = async () => {
    setImporting(true)
    try {
      const result = await interfaceFontGateway.assetImport()
      const alreadyKnown = assets.some((asset) => asset.id === result.asset.id)
      assetsState.replace(
        alreadyKnown
          ? assets.map((asset) => (asset.id === result.asset.id ? result.asset : asset))
          : [result.asset, ...assets],
      )
      selectAsset(result.asset.id)
      showNotice(
        result.deduplicated
          ? `「${result.asset.familyName}」此前已导入，已选中`
          : `已导入「${result.asset.familyName}」并选中`,
      )
    } catch (cause) {
      const haven = toHavenError(cause)
      // 用户取消不是错误，不提示。
      if (haven.code !== "OPERATION_CANCELLED") showNotice(`导入字体失败：${haven.message}`)
    } finally {
      setImporting(false)
    }
  }

  const handleDelete = async (asset: InterfaceFontAsset) => {
    setDeletingId(asset.id)
    try {
      await interfaceFontGateway.assetDelete(asset.id)
      assetsState.replace(assets.filter((candidate) => candidate.id !== asset.id))
      showNotice(`已删除「${asset.familyName}」`)
    } catch (cause) {
      // 正在使用的字体由后端在同一事务内拒绝（FONT_ASSET_IN_USE）；如实展示，不重试。
      showNotice(`删除字体失败：${toHavenError(cause).message}`)
    } finally {
      setDeletingId(null)
    }
  }

  const previewStyle = {
    fontFamily: resolved.fontFamily ?? "var(--ds-font-interface-fallback)",
  }

  return (
    <SettingsGroup title="界面字体" features={["appearance.interfaceFont"]} description="栖阅界面（导航、列表、设置页）使用的字体。阅读器正文与漫画页有各自的排版设置，不受这里影响。">
      <div role="group" aria-label="界面字体模式" className="grid gap-3 p-5 sm:grid-cols-2 lg:grid-cols-4">
        {INTERFACE_FONT_MODE_OPTIONS.map((option) => (
          <InterfaceFontModeCard
            key={option.value}
            mode={option.value}
            label={option.label}
            detail={INTERFACE_FONT_MODE_DETAILS[option.value]}
            active={value.interfaceFontMode === option.value}
            onSelect={() => form.change({ section: "appearance", interfaceFontMode: option.value })}
          />
        ))}
      </div>

      <div className="border-t border-black/[0.04] px-6 py-5">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0">
            <p data-testid="interface-font-preview-title" style={previewStyle} className="text-[19px] leading-8 text-[#1d1d1f] dark:text-white">栖阅 Haven · 界面字体预览</p>
            <p data-testid="interface-font-preview-sample" style={previewStyle} className="mt-1 text-[13px] text-[var(--haven-settings-muted-strong)] dark:text-white/55">The quick brown fox jumps over the lazy dog · 0123456789 · 字体渲染</p>
          </div>
          <span data-testid="interface-font-status" className="shrink-0 rounded-full bg-black/[0.04] px-3 py-1 text-[11px] font-semibold text-[var(--haven-settings-muted-strong)] dark:bg-white/[0.08] dark:text-white/65">
            {describeResolvedInterfaceFont(resolved, assets)}
          </span>
        </div>
      </div>

      <div id={INTERFACE_FONT_SELECTION_PANEL_ID} hidden={!custom}>
      {custom && (
        <>
          <div className="flex flex-wrap items-center gap-3 border-t border-black/[0.04] px-6 py-4 dark:border-white/[0.08]">
            <div className="relative min-w-[220px] flex-1">
              <Search className="pointer-events-none absolute left-3 top-1/2 h-[15px] w-[15px] -translate-y-1/2 text-[var(--haven-settings-muted-strong)]" strokeWidth={1.8} />
              <input
                type="search"
                value={query}
                spellCheck={false}
                autoComplete="off"
                aria-label="搜索字体"
                placeholder="搜索字体：中文、拼音或英文名"
                onChange={(event) => setQuery(event.target.value)}
                className="h-10 w-full rounded-xl border border-[var(--haven-settings-border)] bg-[var(--haven-settings-control)] pl-9 pr-3 text-sm text-[#1d1d1f] outline-none transition-colors placeholder:text-[var(--haven-settings-muted-strong)] focus:border-[#8b806d] focus-visible:ring-2 focus-visible:ring-[#a89c87]/30 dark:border-white/[0.10] dark:bg-white/[0.04] dark:text-white dark:placeholder:text-white/40 dark:focus:border-white/40 dark:focus-visible:ring-white/15"
              />
            </div>
          </div>

          <div className="max-h-[360px] overflow-y-auto border-t border-black/[0.04]">
            <section aria-label="已导入字体">
              <div className="flex items-center justify-between gap-3 px-6 pb-2 pt-4">
                <div className="flex items-center gap-2">
                  <h4 className="text-[12px] font-semibold text-[#4c4942] dark:text-white/85">已导入字体</h4>
                  <span className="text-[11px] tabular-nums text-[var(--haven-settings-muted-strong)] dark:text-white/45">
                    {assetsState.status === "loading" ? "读取中" : assetsState.status === "error" ? "—" : assets.length}
                  </span>
                </div>
                <button
                  type="button"
                  disabled={importing || deletingId !== null}
                  onClick={() => void handleImport()}
                  className="inline-flex min-h-9 items-center gap-2 rounded-xl bg-[#302d27] px-3.5 text-[12px] font-semibold text-white transition-colors hover:bg-[#48443c] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#a89c87]/45 disabled:cursor-not-allowed disabled:opacity-50 dark:bg-[#e8e2d6] dark:text-[#25231f] dark:hover:bg-[#f2ede3] dark:focus-visible:ring-white/25"
                >
                  <Upload className="h-[14px] w-[14px]" strokeWidth={2} />
                  {importing ? "正在导入…" : "导入字体"}
                </button>
              </div>

              {assetsState.status === "loading" && (
                <p className="px-6 pb-4 text-[12px] text-[var(--haven-settings-muted-strong)] dark:text-white/50">正在读取已导入字体…</p>
              )}
              {assetsState.error !== null && (
                <div className="flex items-center justify-between gap-3 px-6 pb-4">
                  <p className="text-[12px] text-[#b42318] dark:text-[#ff8178]">字体列表读取失败：{assetsState.error}</p>
                  <button type="button" onClick={() => void assetsState.reload()} className="text-[12px] font-semibold text-[#62594b] underline-offset-2 hover:underline dark:text-white/80">重试</button>
                </div>
              )}
              {assetsState.status === "ready" && visibleAssets.length === 0 && query.trim().length === 0 && (
                <div className="mx-4 mb-3 rounded-xl border border-dashed border-black/[0.12] bg-black/[0.015] px-4 py-3 dark:border-white/[0.14] dark:bg-white/[0.025]">
                  <p className="text-[12px] font-medium text-[#4c4942] dark:text-white/80">尚未导入字体</p>
                  <p className="mt-1 text-[11px] text-[var(--haven-settings-muted-strong)] dark:text-white/45">支持 .ttf、.otf 和 .woff2 字体文件</p>
                </div>
              )}
              {visibleAssets.map((asset) => {
                const isReferenced = value.interfaceFontAssetId === asset.id
                const isActive = value.interfaceFontMode === "custom" && isReferenced
                return (
                  <InterfaceFontRow
                    key={asset.id}
                    label={asset.familyName}
                    detail={`${asset.fileName} · ${formatFontByteSize(asset.byteSize)}`}
                    previewFamily={importedFontCssFamily(asset.id)}
                    selected={isActive}
                    onSelect={() => selectAsset(asset.id)}
                    action={
                      <button
                        type="button"
                        disabled={isReferenced || deletingId !== null || importing}
                        title={isReferenced ? "该字体仍保留在界面字体设置中，改选其他字体后才能删除" : "删除该导入字体"}
                        aria-label={`删除 ${asset.familyName}`}
                        onClick={() => void handleDelete(asset)}
                        className="ml-2 shrink-0 rounded-lg p-2 text-[var(--haven-settings-muted-strong)] transition-colors hover:bg-black/[0.05] hover:text-[#a13d31] disabled:cursor-not-allowed disabled:opacity-30 disabled:hover:bg-transparent disabled:hover:text-[var(--haven-settings-muted-strong)] dark:hover:bg-white/[0.08] dark:hover:text-[#ff8178]"
                      >
                        <Trash2 className="h-[15px] w-[15px]" strokeWidth={1.8} />
                      </button>
                    }
                  />
                )
              })}
            </section>

            {familiesState.status === "error" && (
              <div className="flex items-center justify-between gap-3 border-t border-black/[0.04] px-6 py-3 dark:border-white/[0.08]">
                <p className="text-[12px] text-[#b42318] dark:text-[#ff8178]">本机字体枚举失败：{familiesState.error}</p>
                <button type="button" onClick={() => void familiesState.reload()} className="text-[12px] font-semibold text-[#62594b] underline-offset-2 hover:underline dark:text-white/80">重试</button>
              </div>
            )}
            {(familiesState.status === "loading" || familiesState.status === "idle") && (
              <p className="border-t border-black/[0.04] px-6 py-3 text-[12px] text-[var(--haven-settings-muted-strong)] dark:border-white/[0.08] dark:text-white/50">正在读取本机字体…</p>
            )}

            <section aria-label="本机字体" className="border-t border-black/[0.04] dark:border-white/[0.08]">
              <div className="flex items-center gap-2 px-6 pb-2 pt-4">
                <h4 className="text-[12px] font-semibold text-[#4c4942] dark:text-white/85">本机字体</h4>
                <span className="text-[11px] tabular-nums text-[var(--haven-settings-muted-strong)] dark:text-white/45">
                  {familiesState.status === "loading" || familiesState.status === "idle" ? "读取中" : familiesState.status === "error" ? "—" : familiesState.families.length}
                </span>
              </div>
              {familiesState.status === "ready" && visibleFamilies.length === 0 && query.trim().length === 0 && (
                <p className="px-6 pb-4 text-[12px] text-[var(--haven-settings-muted-strong)] dark:text-white/50">未检测到可用的本机字体</p>
              )}
              {visibleFamilies.map((entry) => (
                <InterfaceFontRow
                  key={entry.family}
                  label={entry.family}
                  detail={entry.localizedFamily}
                  previewFamily={entry.family}
                  selected={value.interfaceFontMode === "custom" && value.interfaceFontFamily === entry.family}
                  onSelect={() => selectFamily(entry.family)}
                />
              ))}
            </section>

            {queryHasNoResults && (
              <div className="px-6 py-5 text-[12px] text-[#6e6a62] dark:text-white/60">
                没有匹配「{query.trim()}」的字体。
              </div>
            )}
          </div>
        </>
      )}
      </div>
    </SettingsGroup>
  )
}

function InterfaceFontModeCard({ mode, label, detail, active, onSelect }: { mode: InterfaceFontModeWire; label: string; detail: string; active: boolean; onSelect: () => void }) {
  return (
    <button
      type="button"
      aria-pressed={active}
      aria-expanded={mode === "custom" ? active : undefined}
      aria-controls={mode === "custom" ? INTERFACE_FONT_SELECTION_PANEL_ID : undefined}
      onClick={onSelect}
      className={cn(
        "group rounded-2xl border p-[16px] text-left transition-all duration-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#a89c87]/40 dark:focus-visible:ring-white/25",
        active
          ? "border-[#817765]/55 bg-[#f1eee7] shadow-sm dark:border-white/40 dark:bg-white/[0.12]"
          : "border-black/[0.06] bg-black/[0.015] hover:border-black/[0.14] hover:bg-black/[0.035] dark:border-white/[0.08] dark:bg-white/[0.025] dark:hover:border-white/[0.16] dark:hover:bg-white/[0.06]",
      )}
    >
      <span className="flex items-center justify-between">
        <span className={cn("text-[15px] font-medium transition-colors", active ? "text-[#38342e] dark:text-[#f4f0e6]" : "text-[#1d1d1f] dark:text-white/90")}>{label}</span>
        <span className="flex items-center gap-1.5">
          {active && <Check aria-hidden="true" className="h-[16px] w-[16px] text-[#675d4f] dark:text-[#f4f0e6]" strokeWidth={2.6} />}
          {mode === "custom" && <ChevronDown aria-hidden="true" className={cn("h-[15px] w-[15px] text-[var(--haven-settings-muted-strong)] transition-transform", active && "rotate-180 dark:text-white/75")} strokeWidth={2} />}
        </span>
      </span>
      {/* 卡片自己用对应字体栈渲染示例，让「现代黑体 / 人文衬线」的差别可见。 */}
      <span style={interfaceFontModePreviewStyle(mode)} className="mt-2 block truncate text-[17px] leading-6 text-[#1d1d1f] dark:text-white/90">栖阅 Haven</span>
      <span className="mt-1 block text-[12px] leading-4 text-[var(--haven-settings-muted-strong)] dark:text-white/50">{detail}</span>
    </button>
  )
}

/** 卡片示例文字的字体栈：系统项使用真实默认栈，预设项明确区分字形。 */
function interfaceFontModePreviewStyle(mode: InterfaceFontModeWire): CSSProperties | undefined {
  if (mode === "sans") return { fontFamily: INTERFACE_FONT_PRESET_STACKS.sans }
  if (mode === "serif") return { fontFamily: INTERFACE_FONT_PRESET_STACKS.serif }
  if (mode === "system") return { fontFamily: "var(--ds-font-interface-fallback)" }
  return undefined
}

/**
 * 字体列表的一行。
 *
 * 行内容**用该字体自己渲染**（`fontFamilyPreviewStyle`），因此「名字写着黑体、
 * 实际长什么样」在列表里就能看到；族名不合法时该行回退默认字体，不会拼出坏 CSS。
 */
function InterfaceFontRow({ label, detail, previewFamily, selected, onSelect, action }: { label: string; detail?: string | null; previewFamily: string; selected: boolean; onSelect: () => void; action?: ReactNode }) {
  return (
    <div className="flex items-center border-b border-black/[0.04] px-4 last:border-b-0">
      <button
        type="button"
        aria-pressed={selected}
        onClick={onSelect}
        className={cn("my-1 flex min-w-0 flex-1 items-center gap-3 rounded-xl px-3 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#a89c87]/35 dark:focus-visible:ring-white/20", selected ? "bg-[#eeeae1] dark:bg-white/[0.10]" : "hover:bg-black/[0.03] dark:hover:bg-white/[0.05]")}
      >
        <span className="min-w-0 flex-1">
          <span style={fontFamilyPreviewStyle(previewFamily)} className="block truncate text-[15px] text-[#1d1d1f] dark:text-white/90">{label}</span>
          {detail && <span className="mt-0.5 block truncate text-[11px] text-[var(--haven-settings-muted-strong)] dark:text-white/45">{detail}</span>}
        </span>
        {selected && <Check aria-hidden="true" className="h-[16px] w-[16px] shrink-0 text-[#675d4f] dark:text-[#e8e2d6]" strokeWidth={2.4} />}
      </button>
      {action}
    </div>
  )
}

/** 导入字体字节数的展示格式。 */
function formatFontByteSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

/** 资产选择卡片：展示名称和预览，不把内部标识暴露给用户。 */
const APPEARANCE_SELECTED_CARD_CLASS = "border-[var(--haven-settings-primary)] bg-[var(--haven-settings-primary-06)]"

function AppearanceSelectionMark() {
  return (
    <span
      data-selection-mark="true"
      aria-hidden="true"
      className="absolute right-2 top-2 z-10 flex h-6 w-6 items-center justify-center rounded-full bg-[var(--haven-settings-primary)] text-[var(--haven-settings-primary-foreground)] shadow-[0_2px_6px_rgba(0,0,0,0.18)]"
    >
      <Check className="h-3.5 w-3.5" strokeWidth={2.5} />
    </span>
  )
}

function AppearanceAssetRow({
  asset,
  selected,
  onSelect,
  onDelete,
  deleteBlockedReason,
  preview,
  selectedStatus,
}: {
  asset: AppearanceAssetWire
  selected: boolean
  onSelect: () => void
  onDelete: () => void
  deleteBlockedReason: string | null
  preview?: ReactNode
  selectedStatus?: ReactNode
}) {
  const usable = isUsableAppearanceAsset(asset)
  const label = appearanceAssetLabel(asset)
  return (
    <article className={cn(
      "min-w-0 rounded-[14px] border bg-[var(--haven-settings-card)] p-3 transition-colors",
      selected
        ? APPEARANCE_SELECTED_CARD_CLASS
        : "border-[var(--haven-settings-border-subtle)] hover:bg-[var(--haven-settings-card-hover)]",
      !usable && "opacity-70",
    )}>
      <button
        type="button"
        disabled={!usable}
        onClick={onSelect}
        aria-pressed={selected}
        aria-label={`选择${label}`}
        className={cn(
          "block w-full min-w-0 rounded-[10px] text-left outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary-35)]",
          !usable && "cursor-not-allowed",
        )}
      >
        <div className="relative mb-3 flex h-[88px] items-center justify-center overflow-hidden rounded-[10px] bg-[var(--haven-settings-sidebar)]">
          {selected && <AppearanceSelectionMark />}
          {preview ?? (
            <div className="flex flex-col items-center gap-2 text-[var(--haven-settings-muted-strong)]">
              <FileText className="h-6 w-6" strokeWidth={1.7} />
              <span className="text-[11px]">界面字体</span>
            </div>
          )}
        </div>
        <div className="flex min-w-0 items-center justify-between gap-2">
          <span className={cn("min-w-0 truncate text-[13px]", selected ? "font-semibold text-[var(--haven-settings-primary)]" : "font-medium text-[var(--haven-settings-foreground)]")}>
            {label}
          </span>
          {selected && <span className="shrink-0 text-[10px] font-semibold text-[var(--haven-settings-primary)]">使用中</span>}
        </div>
        {!usable && <span className="mt-1 block text-[11px] text-[var(--haven-settings-muted)]">暂不可用</span>}
      </button>
      <div className="mt-2 flex min-h-[36px] items-center justify-between gap-2">
        <div className="min-w-0 flex-1">{selected ? selectedStatus : null}</div>
        <button
          type="button"
          onClick={onDelete}
          disabled={deleteBlockedReason !== null}
          title={deleteBlockedReason ?? "删除该项目"}
          aria-label={`删除${label}`}
          className="min-h-[36px] shrink-0 rounded-full px-3 text-[12px] font-semibold text-[var(--haven-settings-danger)] transition-colors hover:bg-[var(--haven-settings-danger-06)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-danger-20)] disabled:cursor-not-allowed disabled:opacity-40"
        >
          删除
        </button>
      </div>
    </article>
  )
}

/**
 * 资产操作结果的落点。
 *
 * 三类结果不能用同一副面孔：
 * - `cancelled`（用户自己取消了选择）**不是**故障：用失败的红字加 `role="alert"` 去播报
 *   它，等于把一次正常的放弃说成一次出错，屏幕阅读器还会把它当成需要立刻处理的告警；
 * - `blocked`（资产正在使用中所以不能删）也不是故障，但它要求用户先做点什么，因此按
 *   「提示」呈现而不是中性的静默；
 * - `failure` 才是真的失败：只有它配 `role="alert"` 与危险色。
 */
function AppearanceAssetAction({ action }: { action: AppearanceAssetActionResult | null }) {
  if (!action) return null
  const isFailure = action.kind === "failure"
  return (
    <div
      role={isFailure ? "alert" : "status"}
      className={cn(
        "px-6 py-3 text-[12px] leading-5",
        isFailure
          ? "text-[var(--haven-settings-danger)]"
          : "text-[var(--haven-settings-muted-strong)]",
      )}
    >
      {action.message}
    </div>
  )
}

const APPEARANCE_IMPORT_CARD_CLASS = "flex min-h-[156px] w-full flex-col items-center justify-center gap-3 rounded-[14px] border border-dashed border-[var(--haven-settings-border)] bg-[var(--haven-settings-card)] px-3 py-4 text-center transition-colors hover:border-[var(--haven-settings-primary-35)] hover:bg-[var(--haven-settings-card-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary-35)] disabled:cursor-not-allowed disabled:opacity-50"

/** 仅供壁纸宫格使用；界面字体不再按文件上传卡片展示。 */
function AppearanceAssetImportCard({
  title,
  description,
  pending,
  onClick,
}: {
  title: string
  description: string
  pending: boolean
  onClick: () => void
}) {
  return (
    <button type="button" onClick={onClick} disabled={pending} className={APPEARANCE_IMPORT_CARD_CLASS}>
      <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full bg-[var(--haven-settings-sidebar)] text-[var(--haven-settings-primary)]">
        <Upload className="h-[17px] w-[17px]" strokeWidth={1.9} />
      </span>
      <span className="min-w-0 w-full max-w-full text-center">
        <span className="block text-center text-[13px] font-semibold text-[var(--haven-settings-foreground)]">{pending ? "正在导入…" : title}</span>
        <span className="mt-1 block text-center text-[11px] leading-4 text-[var(--haven-settings-muted)]">{description}</span>
      </span>
    </button>
  )
}

type AppearanceFontPreviewChoice =
  | { kind: "preset"; preset: UiFontPresetWire }
  | { kind: "system"; family: string }
  | { kind: "asset"; assetId: string }

type AppearanceFontAssetPreview =
  | { status: "idle" }
  | { status: "loading"; assetId: string }
  | { status: "ready"; assetId: string; family: string }
  | { status: "error"; assetId: string }

const SYSTEM_FONT_RESULT_LIMIT = 80
const EMPTY_SYSTEM_FONTS: SystemFontEntry[] = []

export function AppearanceFontPicker({
  title,
  description,
  assets,
  loading,
  error,
  selectedAssetId,
  preset,
  family,
  pending,
  onImport,
  onSelectPreset,
  onSelectSystemFont,
  onSelectAsset,
  onDelete,
  blockedReasonFor,
  action,
}: {
  title: string
  description: string
  assets: AppearanceAssetWire[]
  loading: boolean
  error: string | null
  selectedAssetId: string | null
  preset: UiFontPresetWire
  family: string | null
  pending: boolean
  onImport: () => void
  onSelectPreset: (preset: UiFontPresetWire) => void
  onSelectSystemFont: (family: string) => void
  onSelectAsset: (assetId: string) => void
  onDelete: (assetId: string) => void
  blockedReasonFor: (assetId: string) => string | null
  action: AppearanceAssetActionResult | null
}) {
  const [catalog, setCatalog] = useState<SystemFontCatalogResult | null>(null)
  const [catalogLoading, setCatalogLoading] = useState(false)
  const [fontSearch, setFontSearch] = useState("")
  const [pickerOpen, setPickerOpen] = useState(false)
  const [activeFontIndex, setActiveFontIndex] = useState(0)
  const [previewChoice, setPreviewChoice] = useState<AppearanceFontPreviewChoice | null>(null)
  const [assetPreview, setAssetPreview] = useState<AppearanceFontAssetPreview>({ status: "idle" })
  const comboRef = useRef<HTMLDivElement>(null)
  const catalogRequestRef = useRef(0)
  const catalogLoadingRef = useRef(false)
  const inputId = useId()
  const listboxId = useId()

  const loadCatalog = (retry = false) => {
    if (catalogLoadingRef.current || (!retry && catalog !== null)) return
    catalogLoadingRef.current = true
    const requestId = ++catalogRequestRef.current
    setCatalogLoading(true)
    void querySystemFontCatalog()
      .then((result) => {
        if (requestId !== catalogRequestRef.current) return
        setCatalog(result)
        setActiveFontIndex(-1)
      })
      .catch(() => {
        if (requestId === catalogRequestRef.current) setCatalog({ status: "error" })
      })
      .finally(() => {
        if (requestId !== catalogRequestRef.current) return
        catalogLoadingRef.current = false
        setCatalogLoading(false)
      })
  }

  useEffect(() => () => {
    catalogRequestRef.current += 1
  }, [])

  const openPicker = () => {
    setPickerOpen(true)
    setFontSearch("")
    setActiveFontIndex(-1)
    if (catalog === null) loadCatalog()
  }

  useEffect(() => {
    if (!pickerOpen) return
    const closeOutside = (event: PointerEvent) => {
      if (event.target instanceof Node && !comboRef.current?.contains(event.target)) {
        setPickerOpen(false)
        setFontSearch("")
        setPreviewChoice(null)
      }
    }
    document.addEventListener("pointerdown", closeOutside)
    return () => document.removeEventListener("pointerdown", closeOutside)
  }, [pickerOpen])

  const catalogFonts = catalog?.status === "ready" ? catalog.fonts : EMPTY_SYSTEM_FONTS
  const matchingFonts = useMemo(
    () => searchSystemFonts(catalogFonts, fontSearch),
    [catalogFonts, fontSearch],
  )
  const visibleFonts = matchingFonts.slice(0, SYSTEM_FONT_RESULT_LIMIT)
  const selectedPreview: AppearanceFontPreviewChoice = selectedAssetId
    ? { kind: "asset", assetId: selectedAssetId }
    : preset === "custom_system" && family
      ? { kind: "system", family }
      : { kind: "preset", preset }
  const specimenChoice = previewChoice ?? selectedPreview
  const previewAssetId = specimenChoice.kind === "asset" ? specimenChoice.assetId : null

  useEffect(() => {
    if (!previewAssetId) {
      return
    }
    let active = true
    let dispose: (() => void) | null = null
    setAssetPreview({ status: "loading", assetId: previewAssetId })
    void installAppearanceFontPreview(previewAssetId)
      .then((result) => {
        if (!active) {
          result?.cleanup()
          return
        }
        if (!result) {
          setAssetPreview({ status: "error", assetId: previewAssetId })
          return
        }
        dispose = result.cleanup
        setAssetPreview({ status: "ready", assetId: previewAssetId, family: result.family })
      })
      .catch(() => {
        if (active) setAssetPreview({ status: "error", assetId: previewAssetId })
      })
    return () => {
      active = false
      dispose?.()
    }
  }, [previewAssetId])

  const specimenFontFamily = specimenChoice.kind === "asset"
    ? assetPreview.status === "ready" && assetPreview.assetId === specimenChoice.assetId
      ? `"${assetPreview.family}", var(--haven-font-ui-system)`
      : uiFontCssStack(preset, family)
    : specimenChoice.kind === "system"
      ? uiFontCssStack("custom_system", specimenChoice.family)
      : uiFontCssStack(specimenChoice.preset, null)

  const chooseSystemFont = (font: SystemFontEntry) => {
    onSelectSystemFont(font.family)
    setPickerOpen(false)
    setFontSearch("")
    setPreviewChoice(null)
  }

  const handleFontKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Escape") {
      event.preventDefault()
      setPickerOpen(false)
      setFontSearch("")
      setPreviewChoice(null)
      return
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault()
      if (!pickerOpen) {
        openPicker()
        return
      }
      if (visibleFonts.length === 0) return
      const nextIndex = activeFontIndex < 0
        ? event.key === "ArrowDown" ? 0 : visibleFonts.length - 1
        : event.key === "ArrowDown"
          ? Math.min(activeFontIndex + 1, visibleFonts.length - 1)
          : Math.max(activeFontIndex - 1, 0)
      const nextFont = visibleFonts[nextIndex]
      setActiveFontIndex(nextIndex)
      if (nextFont) setPreviewChoice({ kind: "system", family: nextFont.family })
      return
    }
    if (event.key === "Enter" && pickerOpen) {
      const activeFont = visibleFonts[activeFontIndex] ?? visibleFonts[0]
      if (activeFont) {
        event.preventDefault()
        chooseSystemFont(activeFont)
      }
    }
  }

  return (
    <SettingsGroup title={title} description={description}>
      <div className="px-6 py-5">
        <div role="group" aria-label="界面字体预设" className="grid grid-cols-2 gap-2 sm:grid-cols-4">
          {UI_FONT_PRESETS.map((option) => {
            const selected = selectedAssetId === null && preset === option.id
            return (
              <button
                key={option.id}
                type="button"
                aria-pressed={selected}
                aria-expanded={option.id === "custom_system" ? pickerOpen : undefined}
                onClick={() => {
                  if (option.id === "custom_system") {
                    openPicker()
                    return
                  }
                  setPickerOpen(false)
                  setPreviewChoice(null)
                  onSelectPreset(option.id)
                }}
                onMouseEnter={() => setPreviewChoice({ kind: "preset", preset: option.id })}
                onMouseLeave={() => setPreviewChoice(null)}
                onFocus={() => setPreviewChoice({ kind: "preset", preset: option.id })}
                onBlur={() => setPreviewChoice(null)}
                className={cn(
                  "flex min-h-[46px] min-w-0 flex-col items-start justify-center rounded-[10px] border px-3 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary-35)]",
                  selected
                    ? "border-[var(--haven-settings-primary-40)] bg-[var(--haven-settings-card-hover)]"
                    : "border-[var(--haven-settings-border-subtle)] hover:bg-[var(--haven-settings-card-hover)]",
                )}
              >
                <span className="block max-w-full truncate text-[12px] font-semibold text-[var(--haven-settings-foreground)]">{option.label}</span>
                <span className="mt-0.5 block max-w-full truncate text-[10px] text-[var(--haven-settings-muted)]">{option.sample}</span>
              </button>
            )
          })}
        </div>

        {(pickerOpen || preset === "custom_system" || selectedAssetId !== null) && (
          <div className="mt-4" ref={comboRef}>
            <div className="grid gap-2 sm:grid-cols-[minmax(0,1fr)_auto]">
              <div className="relative min-w-0">
                <label htmlFor={inputId} className="sr-only">搜索本机字体名称或拼音</label>
                <Search aria-hidden="true" className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-[var(--haven-settings-muted)]" />
                <input
                  id={inputId}
                  role="combobox"
                  aria-label="搜索本机字体名称或拼音"
                  aria-autocomplete="list"
                  aria-expanded={pickerOpen}
                  aria-controls={pickerOpen ? listboxId : undefined}
                  aria-activedescendant={pickerOpen && visibleFonts[activeFontIndex] ? `${listboxId}-option-${activeFontIndex}` : undefined}
                  autoComplete="off"
                  spellCheck={false}
                  value={pickerOpen ? fontSearch : selectedAssetId ? "" : family ?? ""}
                  placeholder="搜索字体名称或拼音"
                  onFocus={openPicker}
                  onChange={(event) => {
                    setFontSearch(event.target.value)
                    setActiveFontIndex(-1)
                    setPickerOpen(true)
                    setPreviewChoice(null)
                    if (catalog === null) loadCatalog()
                  }}
                  onKeyDown={handleFontKeyDown}
                  className="min-h-[44px] w-full rounded-[10px] border border-[var(--haven-settings-control-border)] bg-[var(--haven-settings-control)] pl-10 pr-3 text-[13px] text-[var(--haven-settings-foreground)] outline-none placeholder:text-[var(--haven-settings-muted)] focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary)]"
                />
              </div>
              <button
                type="button"
                disabled={pending}
                onClick={() => {
                  setPickerOpen(false)
                  onImport()
                }}
                className="inline-flex min-h-[44px] items-center justify-center gap-2 rounded-[10px] border border-[var(--haven-settings-control-border)] px-3 text-[12px] font-semibold text-[var(--haven-settings-muted-strong)] transition-colors hover:bg-[var(--haven-settings-card-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary)] disabled:cursor-not-allowed disabled:opacity-50"
              >
                <Plus aria-hidden="true" className="h-4 w-4" strokeWidth={1.9} />
                {pending ? "正在导入…" : "导入本地字体"}
              </button>
            </div>

            {pickerOpen && (
              <div className="mt-2 overflow-hidden rounded-[10px] border border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-card)]">
                {catalogLoading ? (
                  <p role="status" className="px-3 py-3 text-[12px] text-[var(--haven-settings-muted)]">正在读取本机字体…</p>
                ) : catalog?.status === "unsupported" ? (
                  <div className="flex flex-wrap items-center justify-between gap-2 px-3 py-3">
                    <p className="text-[12px] leading-5 text-[var(--haven-settings-muted-strong)]">当前环境暂不支持搜索本机字体；你仍可使用上方预设或导入字体文件。</p>
                    <button type="button" onClick={() => loadCatalog(true)} className="min-h-[36px] shrink-0 rounded-full px-3 text-[12px] font-semibold text-[var(--haven-settings-primary)] hover:bg-[var(--haven-settings-card-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary-35)]">重试</button>
                  </div>
                ) : catalog?.status === "denied" ? (
                  <div className="flex flex-wrap items-center justify-between gap-2 px-3 py-3">
                    <p className="text-[12px] leading-5 text-[var(--haven-settings-muted-strong)]">没有获得读取本机字体的权限；检查系统权限后可以重试。</p>
                    <button type="button" onClick={() => loadCatalog(true)} className="min-h-[36px] shrink-0 rounded-full px-3 text-[12px] font-semibold text-[var(--haven-settings-primary)] hover:bg-[var(--haven-settings-card-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary-35)]">重试</button>
                  </div>
                ) : catalog?.status === "error" ? (
                  <div className="flex flex-wrap items-center justify-between gap-2 px-3 py-3">
                    <p role="alert" className="text-[12px] leading-5 text-[var(--haven-settings-danger)]">读取本机字体失败，请稍后重试。</p>
                    <button type="button" onClick={() => loadCatalog(true)} className="min-h-[36px] shrink-0 rounded-full px-3 text-[12px] font-semibold text-[var(--haven-settings-primary)] hover:bg-[var(--haven-settings-card-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary-35)]">重试</button>
                  </div>
                ) : catalog?.status === "ready" && catalog.fonts.length === 0 ? (
                  <p className="px-3 py-3 text-[12px] text-[var(--haven-settings-muted-strong)]">没有检测到可用字体。</p>
                ) : visibleFonts.length === 0 ? (
                  <p className="px-3 py-3 text-[12px] text-[var(--haven-settings-muted-strong)]">没有找到匹配的字体，试试英文名或拼音。</p>
                ) : (
                  <>
                    <p className="px-3 py-2 text-[11px] text-[var(--haven-settings-muted)]">
                      {matchingFonts.length > SYSTEM_FONT_RESULT_LIMIT
                        ? `找到 ${matchingFonts.length} 款字体，仅显示前 ${SYSTEM_FONT_RESULT_LIMIT} 款，请继续搜索缩小范围。`
                        : `找到 ${matchingFonts.length} 款字体`}
                    </p>
                    <div
                      id={listboxId}
                      role="listbox"
                      aria-label="本机字体"
                      onMouseLeave={() => {
                        setActiveFontIndex(-1)
                        setPreviewChoice(null)
                      }}
                      className="max-h-56 overflow-y-auto border-t border-[var(--haven-settings-border-subtle)] py-1"
                    >
                      {visibleFonts.map((font, index) => (
                        <div
                          key={font.family}
                          id={`${listboxId}-option-${index}`}
                          role="option"
                          aria-selected={font.family === family && selectedAssetId === null}
                          onMouseDown={(event) => event.preventDefault()}
                          onMouseEnter={() => {
                            setActiveFontIndex(index)
                            setPreviewChoice({ kind: "system", family: font.family })
                          }}
                          onClick={() => chooseSystemFont(font)}
                          className={cn(
                            "cursor-pointer px-3 py-2 text-[13px] outline-none",
                            index === activeFontIndex
                              ? "bg-[var(--haven-settings-card-hover)] text-[var(--haven-settings-foreground)]"
                              : "text-[var(--haven-settings-muted-strong)] hover:bg-[var(--haven-settings-card-hover)]",
                          )}
                          style={{ fontFamily: uiFontCssStack("custom_system", font.family) }}
                        >
                          <span className="block truncate">{font.family}</span>
                          {font.fullName !== font.family && <span className="mt-0.5 block truncate text-[10px] text-[var(--haven-settings-muted)]">{font.fullName}</span>}
                        </div>
                      ))}
                    </div>
                  </>
                )}
              </div>
            )}
          </div>
        )}

        <div className="mt-4 rounded-[10px] border border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-sidebar)] px-4 py-3">
          <p className="text-[10px] font-semibold tracking-[0.08em] text-[var(--haven-settings-muted)]">排版预览</p>
          <p className="mt-2 text-[15px] font-semibold leading-6 text-[var(--haven-settings-foreground)]" style={{ fontFamily: specimenFontFamily }}>栖阅 · 沉浸阅读体验 0123 Aa</p>
          <p className="mt-1 text-[14px] leading-6 text-[var(--haven-settings-muted-strong)]" style={{ fontFamily: specimenFontFamily }}>白日依山尽，黄河入海流。</p>
          {specimenChoice.kind === "asset" && assetPreview.status === "loading" && assetPreview.assetId === specimenChoice.assetId && (
            <p role="status" className="mt-1 text-[11px] text-[var(--haven-settings-muted)]">正在载入字体预览…</p>
          )}
          {specimenChoice.kind === "asset" && assetPreview.status === "error" && assetPreview.assetId === specimenChoice.assetId && (
            <p role="status" className="mt-1 text-[11px] text-[var(--haven-settings-muted)]">暂时无法预览这款字体，保存后会按可用字体显示。</p>
          )}
        </div>

        <div className="mt-4 border-t border-[var(--haven-settings-border-subtle)] pt-3">
          <div className="mb-2 flex items-center justify-between gap-3">
            <h4 className="text-[12px] font-semibold text-[var(--haven-settings-foreground)]">已导入字体</h4>
            <span className="text-[11px] text-[var(--haven-settings-muted)]">{assets.length}</span>
          </div>
          {loading && <p role="status" className="py-2 text-[12px] text-[var(--haven-settings-muted)]">正在读取字体…</p>}
          {error && <p role="alert" className="py-2 text-[12px] text-[var(--haven-settings-danger)]">{error}</p>}
          {!loading && !error && assets.length === 0 && (
            <p className="rounded-[9px] border border-dashed border-[var(--haven-settings-border-subtle)] px-3 py-3 text-[12px] text-[var(--haven-settings-muted)]">尚未导入字体</p>
          )}
          {assets.length > 0 && (
            <ul aria-label="已导入字体列表" className="divide-y divide-[var(--haven-settings-border-subtle)] overflow-hidden rounded-[10px] border border-[var(--haven-settings-border-subtle)]">
              {assets.map((asset) => {
                const label = appearanceAssetLabel(asset)
                const selected = asset.assetId === selectedAssetId
                const usable = isUsableAppearanceAsset(asset)
                const deleteBlockedReason = blockedReasonFor(asset.assetId)
                return (
                  <li key={asset.assetId} className={cn("flex min-w-0 items-center gap-2 px-3 py-1.5", selected && "bg-[var(--haven-settings-card-hover)]")}>
                    <button
                      type="button"
                      disabled={!usable}
                      aria-pressed={selected}
                      aria-label={`选择${label}`}
                      onClick={() => onSelectAsset(asset.assetId)}
                      onMouseEnter={() => setPreviewChoice({ kind: "asset", assetId: asset.assetId })}
                      onMouseLeave={() => setPreviewChoice(null)}
                      onFocus={() => setPreviewChoice({ kind: "asset", assetId: asset.assetId })}
                      onBlur={() => setPreviewChoice(null)}
                      className="flex min-h-[42px] min-w-0 flex-1 items-center justify-between gap-3 rounded-md px-2 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary-35)] disabled:cursor-not-allowed disabled:opacity-50"
                    >
                      <span className="min-w-0 truncate text-[13px] font-medium text-[var(--haven-settings-foreground)]">{label}</span>
                      <span className="shrink-0 text-[11px] text-[var(--haven-settings-muted)]">{!usable ? "暂不可用" : selected ? "使用中" : "选择"}</span>
                    </button>
                    <button
                      type="button"
                      onClick={() => onDelete(asset.assetId)}
                      disabled={deleteBlockedReason !== null}
                      title={deleteBlockedReason ?? `删除${label}`}
                      aria-label={`删除${label}`}
                      className="min-h-[40px] shrink-0 rounded-full px-3 text-[12px] font-semibold text-[var(--haven-settings-danger)] transition-colors hover:bg-[var(--haven-settings-danger-06)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-danger-20)] disabled:cursor-not-allowed disabled:opacity-40"
                    >
                      删除
                    </button>
                  </li>
                )
              })}
            </ul>
          )}
        </div>
      </div>
      <AppearanceAssetAction action={action} />
    </SettingsGroup>
  )
}

function AppearanceWallpaperThumbnail({
  asset,
  reducedMotion,
  retryEpoch,
  selected,
  onPreviewError,
}: {
  asset: AppearanceAssetWire
  reducedMotion: boolean
  retryEpoch: number
  selected: boolean
  onPreviewError?: (uri: string) => void
}) {
  const uri = appearanceAssetRequestUri(asset.assetId)
  const [failedPreview, setFailedPreview] = useState<{ uri: string; epoch: number } | null>(null)
  const failed = uri !== null && failedPreview?.uri === uri && failedPreview.epoch === retryEpoch
  const dynamicSuppressed = selected && reducedMotion && asset.kind === "dynamic_wallpaper"
  const dynamicNotSelected = !selected && asset.kind === "dynamic_wallpaper"
  const usable = isUsableAppearanceAsset(asset)

  useEffect(() => {
    if (selected && failed && uri) onPreviewError?.(uri)
  }, [failed, onPreviewError, selected, uri])

  if (!uri || failed || dynamicSuppressed || dynamicNotSelected || !usable) {
    const copy = !usable
      ? "暂不可用"
      : dynamicSuppressed
        ? "减少动效已开启"
        : dynamicNotSelected
          ? "视频壁纸"
          : !uri
            ? "桌面端可预览"
            : "暂时无法预览"
    return (
      <div className="flex h-[88px] items-center justify-center rounded-[10px] bg-[var(--haven-settings-sidebar)] px-2 text-center text-[11px] text-[var(--haven-settings-muted)]">
        {dynamicNotSelected
          ? <span className="flex flex-col items-center gap-1"><PlaySquare className="h-5 w-5" strokeWidth={1.7} /><span>{copy}</span></span>
          : copy}
      </div>
    )
  }

  const handleError = () => setFailedPreview({ uri, epoch: retryEpoch })

  return asset.kind === "dynamic_wallpaper" ? (
    <video
      src={uri}
      aria-label={selected ? "当前动态壁纸预览" : `${appearanceAssetLabel(asset)}预览`}
      aria-hidden={!selected}
      autoPlay
      muted
      loop
      playsInline
      tabIndex={-1}
      onError={handleError}
      className="h-[88px] w-full object-cover"
    />
  ) : (
    <img
      src={uri}
      alt={selected ? "当前静态壁纸预览" : `${appearanceAssetLabel(asset)}预览`}
      onError={handleError}
      className="h-[88px] w-full object-cover"
    />
  )
}

/** 壁纸卡片直接承载预览和选择；预览地址只由受控资产 ID 生成。 */
export function AppearanceWallpaperGroup({
  assets,
  loading,
  error,
  selection,
  pending,
  onImport,
  onSelect,
  onDelete,
  blockedReasonFor,
  reducedMotion,
  action,
}: {
  assets: AppearanceAssetLists | null
  loading: boolean
  error: string | null
  selection: WallpaperSelection
  pending: boolean
  onImport: (kind: AppearanceAssetKindWire) => void
  onSelect: (selection: WallpaperSelection) => void
  onDelete: (assetId: string) => void
  blockedReasonFor: (assetId: string) => string | null
  reducedMotion: boolean
  action: AppearanceAssetActionResult | null
}) {
  const selectedId = selection.kind === "none" ? null : selection.assetId
  // 首页与设置页预览共享受控资源 ID；浏览器预览没有真实壁纸字节。
  const previewUri = selectedId ? appearanceAssetRequestUri(selectedId) : null
  const staticAssets = assets?.staticWallpapers ?? []
  const dynamicAssets = assets?.dynamicWallpapers ?? []
  // 失败记在当前选中的壁纸和重试代数上；其它卡片只在卡片内部回退。
  const retryEpoch = useWallpaperRetryEpoch()
  const homeWallpaperFailure = useHomeWallpaperFailure()
  const [failedPreview, setFailedPreview] = useState<{ uri: string; epoch: number } | null>(null)
  const previewUnavailable =
    previewUri !== null &&
    failedPreview !== null &&
    failedPreview.uri === previewUri &&
    failedPreview.epoch === retryEpoch
  const homeWallpaperUnavailable =
    previewUri !== null &&
    homeWallpaperFailure !== null &&
    homeWallpaperFailure.uri === previewUri &&
    homeWallpaperFailure.epoch === retryEpoch
  // 减少动效开启时不渲染动态视频。
  const dynamicPreviewSuppressed = reducedMotion && selection.kind === "dynamic"
  // 抑制态不是一次读取失败：那里没有可重试的东西，给一个重试按钮只会是假动作。
  const canRetryPreview = (previewUnavailable || homeWallpaperUnavailable) && !dynamicPreviewSuppressed
  const previewFailed = useCallback((uri: string) => {
    setFailedPreview((current) =>
      current?.uri === uri && current.epoch === retryEpoch
        ? current
        : { uri, epoch: retryEpoch },
    )
  }, [retryEpoch])
  const retryButton = canRetryPreview ? (
    <button
      type="button"
      onClick={requestWallpaperRetry}
      className="inline-flex shrink-0 items-center gap-[6px] rounded-full border border-[var(--haven-settings-control-border)] px-[11px] py-[6px] text-[12px] font-semibold text-[var(--haven-settings-primary)] transition-colors hover:bg-[var(--haven-settings-primary-06)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary)]"
    >
      <RefreshCw className="h-[13px] w-[13px]" strokeWidth={2} />
      重试预览
    </button>
  ) : null
  const empty = assets !== null && !loading && error === null && staticAssets.length === 0 && dynamicAssets.length === 0
  const importPending = pending === true

  const wallpaperDescription = empty
    ? "还没有首页壁纸，导入图片或视频后可从下方选择。减少动效开启时，动态壁纸会暂停。"
    : "选择图片或视频作为首页背景。减少动效开启时，动态壁纸会暂停。"

  return (
    <SettingsGroup title="首页壁纸" features={["appearance.wallpaper"]} description={wallpaperDescription}>
      {loading && <p role="status" className="px-6 pt-4 text-[12px] text-[var(--haven-settings-muted)]">正在读取壁纸…</p>}
      {error && <p role="alert" className="px-6 pt-4 text-[12px] text-[var(--haven-settings-danger)]">{error}</p>}
      <div className="px-6 py-5">
        <div role="group" aria-label="首页壁纸选择" className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
          <button
            type="button"
            onClick={() => onSelect({ kind: "none" })}
            aria-label="不使用壁纸"
            aria-pressed={selection.kind === "none"}
            className={cn(
              "relative min-h-[156px] min-w-0 rounded-[14px] border p-3 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary-35)]",
              selection.kind === "none"
                ? APPEARANCE_SELECTED_CARD_CLASS
                : "border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-card)] hover:bg-[var(--haven-settings-card-hover)]",
            )}
          >
            <span className="relative mb-3 flex h-[88px] items-center justify-center rounded-[10px] bg-[var(--haven-settings-sidebar)]">
              {selection.kind === "none" && <AppearanceSelectionMark />}
              <Home className="h-6 w-6 text-[var(--haven-settings-muted-strong)]" strokeWidth={1.7} />
            </span>
            <span className="block truncate text-[13px] font-semibold text-[var(--haven-settings-foreground)]">不使用壁纸</span>
            <span className="mt-1 block text-[11px] text-[var(--haven-settings-muted)]">{selection.kind === "none" ? "当前使用" : "保留首页原有背景"}</span>
          </button>
          <AppearanceAssetImportCard
            title="导入图片"
            description="支持 JPG、PNG、WebP 格式"
            pending={importPending}
            onClick={() => onImport("static_wallpaper")}
          />
          <AppearanceAssetImportCard
            title="导入视频"
            description="支持 MP4、WebM 格式"
            pending={importPending}
            onClick={() => onImport("dynamic_wallpaper")}
          />
          {staticAssets.map((asset) => {
            const selected = selection.kind === "static" && asset.assetId === selectedId
            const failed = selected && previewUnavailable
            const homeFailed = selected && homeWallpaperUnavailable
            const status = (failed || homeFailed) ? (
              <div className="flex flex-wrap items-center justify-between gap-2">
                <span className="text-[10px] leading-4 text-[var(--haven-settings-muted)]">
                  {failed ? "当前壁纸暂时不可用。" : "首页背景暂时无法显示。"}
                </span>
                {retryButton}
              </div>
            ) : null
            return (
              <AppearanceAssetRow
                key={asset.assetId}
                asset={asset}
                selected={selected}
                preview={<AppearanceWallpaperThumbnail asset={asset} reducedMotion={reducedMotion} retryEpoch={retryEpoch} selected={selected} onPreviewError={previewFailed} />}
                selectedStatus={status}
                onSelect={() => onSelect({ kind: "static", assetId: asset.assetId })}
                onDelete={() => onDelete(asset.assetId)}
                deleteBlockedReason={blockedReasonFor(asset.assetId)}
              />
            )
          })}
          {dynamicAssets.map((asset) => {
            const selected = selection.kind === "dynamic" && asset.assetId === selectedId
            const failed = selected && previewUnavailable
            const homeFailed = selected && homeWallpaperUnavailable
            const status = (failed || homeFailed) ? (
              <div className="flex flex-wrap items-center justify-between gap-2">
                <span className="text-[10px] leading-4 text-[var(--haven-settings-muted)]">
                  {failed ? "当前壁纸暂时不可用。" : "首页背景暂时无法显示。"}
                </span>
                {retryButton}
              </div>
            ) : null
            return (
              <AppearanceAssetRow
                key={asset.assetId}
                asset={asset}
                selected={selected}
                preview={<AppearanceWallpaperThumbnail asset={asset} reducedMotion={reducedMotion} retryEpoch={retryEpoch} selected={selected} onPreviewError={previewFailed} />}
                selectedStatus={status}
                onSelect={() => onSelect({ kind: "dynamic", assetId: asset.assetId })}
                onDelete={() => onDelete(asset.assetId)}
                deleteBlockedReason={blockedReasonFor(asset.assetId)}
              />
            )
          })}
        </div>
        <AppearanceAssetAction action={action} />
      </div>
    </SettingsGroup>
  )
}

type LayoutPreviewModule = { module: string; size: string; row: number; column: number }

function LayoutWireframePreview({
  ariaLabel,
  columns,
  placements,
  labelFor,
  spanFor,
}: {
  ariaLabel: string
  columns: number
  placements: readonly LayoutPreviewModule[]
  labelFor: (module: string) => string
  spanFor: (size: string) => number
}) {
  return (
    <div className="mx-6 my-4 rounded-[12px] border border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-sidebar)] p-4">
      <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1">
        <span className="text-[11px] font-semibold text-[var(--haven-settings-muted-strong)]">实时布局示意</span>
        <span className="text-[10px] text-[var(--haven-settings-muted)]">模块顺序与宽度会即时反映在这里</span>
      </div>
      {placements.length === 0 ? (
        <div role="img" aria-label={ariaLabel} className="mt-3 flex min-h-[72px] items-center justify-center rounded-[10px] border border-dashed border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-control)] p-3 text-center text-[11px] text-[var(--haven-settings-muted)]">
          目前没有显示的模块
        </div>
      ) : (
        <div
          role="img"
          aria-label={ariaLabel}
          className="mt-3 grid gap-2 rounded-[12px] bg-[var(--haven-settings-control)] p-3"
          style={{ gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))`, gridAutoRows: "28px" }}
        >
          {placements.map((placement) => (
            <span
              key={placement.module}
              data-layout-preview-module=""
              className="flex min-w-0 items-center justify-center overflow-hidden rounded-[4px] border border-[var(--haven-settings-primary-20)] bg-[var(--haven-settings-primary-06)] px-1.5 text-[9px] font-medium text-[var(--haven-settings-muted-strong)]"
              style={{
                gridColumn: `${placement.column + 1} / span ${Math.max(1, spanFor(placement.size))}`,
                gridRow: placement.row + 1,
              }}
            >
              <span className="truncate">{labelFor(placement.module)}</span>
            </span>
          ))}
        </div>
      )}
    </div>
  )
}

function LayoutWidthControl<T extends string>({
  value,
  options,
  columns,
  spanFor,
  moduleLabel,
  disabled,
  onChange,
}: {
  value: T
  options: ReadonlyArray<{ value: T; label: string }>
  columns: number
  spanFor: (size: T) => number
  moduleLabel: string
  disabled: boolean
  onChange: (value: T) => void
}) {
  return (
    <div role="group" aria-label={`${moduleLabel}显示宽度`} className="flex shrink-0 items-center gap-1 rounded-[10px] border border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-sidebar)] p-1">
      {options.map((option) => {
        const selected = value === option.value
        const gridColumns = Math.max(1, columns)
        const span = Math.min(gridColumns, Math.max(1, spanFor(option.value)))
        return (
          <button
            key={option.value}
            type="button"
            aria-label={`${moduleLabel}：${option.label}`}
            aria-pressed={selected}
            title={option.label}
            disabled={disabled}
            onClick={() => onChange(option.value)}
            className={cn(
              "flex min-h-[42px] min-w-[53px] flex-col items-center justify-center gap-1 rounded-[7px] px-1.5 text-[9px] font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary-35)] disabled:cursor-not-allowed disabled:opacity-40",
              selected
                ? "bg-[var(--haven-settings-card)] text-[var(--haven-settings-foreground)] shadow-[0_1px_3px_rgba(0,0,0,0.08)]"
                : "text-[var(--haven-settings-muted)] hover:bg-[var(--haven-settings-card-hover)] hover:text-[var(--haven-settings-foreground)]",
            )}
          >
            <span
              aria-hidden="true"
              data-layout-width-track=""
              className="relative block h-[9px] w-[32px] overflow-hidden rounded-[3px] border border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-control)]"
            >
              <span
                data-layout-width-fill=""
                className={cn(
                  "absolute inset-y-0 left-0 rounded-[2px] transition-[width,background-color]",
                  selected ? "bg-[var(--haven-settings-primary)]" : "bg-[var(--haven-settings-muted-strong)]",
                )}
                style={{ width: String((span / gridColumns) * 100) + "%" }}
              />
            </span>
            <span>{option.label}</span>
          </button>
        )
      })}
    </div>
  )
}

function LayoutEditorRow<TModule extends string, TSize extends string>({
  module,
  label,
  description,
  visible,
  size,
  sizeOptions,
  columns,
  spanFor,
  visibleIndex,
  canEdit,
  isSaving,
  isDragging,
  isDropTarget,
  onResize,
  onVisibilityChange,
  onReorder,
  onDragStart,
  onDragEnd,
  onDragEnter,
  onDrop,
}: {
  module: TModule
  label: string
  description: string
  visible: boolean
  size: TSize
  sizeOptions: ReadonlyArray<{ value: TSize; label: string }>
  columns: number
  spanFor: (size: TSize) => number
  visibleIndex: number
  canEdit: boolean
  isSaving: boolean
  isDragging: boolean
  isDropTarget: boolean
  onResize: (size: TSize) => void
  onVisibilityChange: (visible: boolean) => void
  onReorder: (targetVisibleIndex: number) => void
  onDragStart: (module: TModule) => void
  onDragEnd: () => void
  onDragEnter: (module: TModule) => void
  onDrop: (event: DragEvent<HTMLDivElement>) => void
}) {
  const canReorder = canEdit && !isSaving && visible
  const onHandleKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (!canReorder) return
    const delta = event.key === "ArrowUp" ? -1 : event.key === "ArrowDown" ? 1 : 0
    if (delta === 0) return
    event.preventDefault()
    onReorder(Math.max(0, visibleIndex + delta))
  }

  return (
    <div
      data-layout-module-row={module}
      onDragOver={(event) => {
        if (!canReorder || !isDragging) return
        event.preventDefault()
        event.dataTransfer.dropEffect = "move"
      }}
      onDragEnter={() => { if (canReorder && isDragging) onDragEnter(module) }}
      onDrop={(event) => {
        if (!canReorder || !isDragging) return
        event.preventDefault()
        onDrop(event)
      }}
      className={cn(
        "flex min-h-[76px] flex-wrap items-center gap-3 border-b border-black/[0.04] px-6 py-3 last:border-b-0",
        isDropTarget && "border-t-2 border-t-[var(--haven-settings-primary)] bg-[var(--haven-settings-primary-06)]",
        isDragging && "opacity-60",
      )}
    >
      <div
        data-layout-static-controls=""
        className={cn(
          "flex min-w-0 flex-1 flex-wrap items-center gap-3 transition-opacity",
          !visible && "pointer-events-none opacity-40",
        )}
      >
      <div
        role="button"
        tabIndex={canReorder ? 0 : -1}
        draggable={canReorder}
        aria-disabled={!canReorder}
        aria-label={`拖动或用方向键调整${label}顺序`}
        aria-description="可拖动此手柄排序；聚焦后按键盘上下方向键调整顺序。"
        aria-keyshortcuts="ArrowUp ArrowDown"
        title={canReorder ? "拖动排序，也可聚焦后按上下方向键调整" : "仅显示中的模块可以排序"}
        onKeyDown={onHandleKeyDown}
        onDragStart={(event) => {
          if (!canReorder) {
            event.preventDefault()
            return
          }
          event.dataTransfer.effectAllowed = "move"
          event.dataTransfer.setData("text/plain", module)
          onDragStart(module)
        }}
        onDragEnd={onDragEnd}
        className={cn(
          "flex h-11 w-11 shrink-0 cursor-grab items-center justify-center rounded-[9px] border border-transparent text-[var(--haven-settings-muted)] transition-colors hover:border-[var(--haven-settings-border-subtle)] hover:bg-[var(--haven-settings-sidebar)] hover:text-[var(--haven-settings-foreground)] active:cursor-grabbing focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary-35)]",
          !canReorder && "cursor-not-allowed",
          !canReorder && visible && "opacity-35",
        )}
      >
        <GripVertical aria-hidden="true" className="h-[18px] w-[18px]" strokeWidth={2} />
      </div>
      <div className="min-w-[150px] min-w-0 flex-1">
        <p className="truncate text-[13px] font-semibold text-[var(--haven-settings-foreground)]">{label}</p>
        <p className="mt-[2px] truncate text-[11px] text-[var(--haven-settings-muted)]">{description}</p>
      </div>
      <div className="shrink-0">
        <LayoutWidthControl
          value={size}
          options={sizeOptions}
          columns={columns}
          spanFor={spanFor}
          moduleLabel={label}
          disabled={!canEdit || isSaving || !visible}
          onChange={onResize}
        />
      </div>
      </div>
      <div className="flex shrink-0 items-center gap-2">
        <Toggle
          checked={visible}
          onChange={onVisibilityChange}
          label={`显示${label}`}
          disabled={!canEdit || isSaving}
        />
      </div>
    </div>
  )
}

function layoutDropIndex(
  modules: readonly { module: string; visible: boolean }[],
  sourceModule: string,
  targetModule: string,
  afterTarget: boolean,
): number | null {
  const visible = modules.filter((entry) => entry.visible)
  const sourceIndex = visible.findIndex((entry) => entry.module === sourceModule)
  const targetIndex = visible.findIndex((entry) => entry.module === targetModule)
  if (sourceIndex < 0 || targetIndex < 0) return null
  let targetPosition = targetIndex + (afterTarget ? 1 : 0)
  if (sourceIndex < targetPosition) targetPosition -= 1
  return Math.max(0, Math.min(targetPosition, visible.length - 1))
}

function LayoutEditorActions({
  sectionName,
  label,
  status,
  isDirty,
  isSaving,
  canEdit,
  layoutRevisionKnown,
  onSave,
  onReset,
  onReload,
}: {
  sectionName: string
  label: string
  status: string
  isDirty: boolean
  isSaving: boolean
  canEdit: boolean
  layoutRevisionKnown: boolean
  onSave: () => void
  onReset: () => void
  onReload: () => void
}) {
  const canSave = canEdit && isDirty && !isSaving
  const showReload = status === "conflict" || status === "error"
  return (
    <div className="flex flex-wrap items-center justify-between gap-3 border-b border-black/[0.04] px-6 py-3">
      <p role="status" className={cn(
        "text-[12px]",
        status === "conflict" || status === "error" || status === "save-error"
          ? "text-[var(--haven-settings-danger)]"
          : "text-[var(--haven-settings-muted-strong)]",
      )}>{label}</p>
      <div className="flex items-center gap-3">
        {showReload && (
          <button type="button" onClick={onReload} className="min-h-9 rounded-lg px-2 text-[12px] font-semibold text-[var(--haven-settings-primary)] hover:bg-[var(--haven-settings-primary-06)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary-35)]">重新加载</button>
        )}
        <button type="button" onClick={onReset} disabled={isSaving || !layoutRevisionKnown} className="min-h-9 rounded-lg px-2 text-[12px] font-medium text-[var(--haven-settings-muted)] transition-colors hover:bg-[var(--haven-settings-card-hover)] hover:text-[var(--haven-settings-foreground)] disabled:cursor-not-allowed disabled:opacity-40">恢复默认</button>
        <button
          type="button"
          aria-label={`保存${sectionName}布局`}
          title={canSave ? "保存当前布局" : isSaving ? "正在保存" : isDirty ? "请先重新读取布局后再保存" : "没有未保存的布局修改"}
          onClick={onSave}
          disabled={!canSave}
          className={cn(
            "min-h-9 rounded-full border px-4 text-[12px] font-semibold transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary-35)] disabled:cursor-not-allowed",
            canSave
              ? "border-[var(--haven-settings-primary)] bg-[var(--haven-settings-primary)] text-[var(--haven-settings-primary-foreground)] shadow-[0_2px_5px_rgba(0,0,0,0.12)] hover:bg-[var(--haven-settings-primary-hover)]"
              : "border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-sidebar)] text-[var(--haven-settings-muted)] opacity-65",
          )}
        >
          {isSaving ? "正在保存…" : "保存布局"}
        </button>
      </div>
    </div>
  )
}

function layoutStatusLabel(
  sectionLabel: string,
  status: string,
  message: string | null,
  isDirty: boolean,
  revision: string | null,
): string {
  if (status === "loading") return `正在读取${sectionLabel}布局…`
  if (status === "saving") return "正在保存…"
  if (status === "conflict" || status === "save-error" || status === "error") return message ?? `${sectionLabel}布局暂不可用`
  if (message) return message
  if (isDirty) return "有未保存的布局修改"
  return revision === null ? "当前使用默认布局" : "布局已保存"
}

/** 总览布局编辑器：使用同一套拖放与可视宽度控件，但依据总览自己的三列栅格。 */
export function OverviewLayoutEditor({ controller }: { controller: OverviewLayoutController }) {
  const { state, isDirty, isSaving } = controller
  const canEdit = state.status === "ready" || state.status === "save-error"
  const layoutRevisionKnown = canEdit || state.status === "saving"
  const [draggedModule, setDraggedModule] = useState<OverviewModuleId | null>(null)
  const [dropTarget, setDropTarget] = useState<OverviewModuleId | null>(null)
  const visibleModules = state.modules.filter((entry) => entry.visible)
  const preview = overviewModuleSettingsToLayout(state.modules)
  const onDrop = (event: DragEvent<HTMLDivElement>, targetModule: OverviewModuleId) => {
    const sourceValue = event.dataTransfer.getData("text/plain") || draggedModule
    const source = state.modules.find((entry) => entry.module === sourceValue && entry.visible)
    if (!source) return
    const rect = event.currentTarget.getBoundingClientRect()
    const targetIndex = layoutDropIndex(
      state.modules,
      source.module,
      targetModule,
      event.clientY >= rect.top + rect.height / 2,
    )
    if (targetIndex !== null) controller.reorder(source.module, targetIndex)
    setDraggedModule(null)
    setDropTarget(null)
  }

  return (
    <SettingsGroup title="总览布局" features={["appearance.overviewLayout"]} description="拖动卡片调整顺序，选择紧凑、标准或通栏宽度；标题与统计范围始终显示，窄屏下会自动排成单列。">
      <LayoutEditorActions
        sectionName="总览"
        label={layoutStatusLabel("总览", state.status, state.message, isDirty, state.revision)}
        status={state.status}
        isDirty={isDirty}
        isSaving={isSaving}
        canEdit={canEdit}
        layoutRevisionKnown={layoutRevisionKnown}
        onSave={controller.save}
        onReset={controller.reset}
        onReload={controller.reload}
      />
      <LayoutWireframePreview
        ariaLabel="总览布局实时预览"
        columns={overviewModuleColumnSpan("large")}
        placements={preview.modules}
        labelFor={(module) => overviewModuleLabel(module as OverviewModuleId)}
        spanFor={(size) => overviewModuleColumnSpan(size as OverviewModuleSize)}
      />
      {state.modules.map((entry) => {
        const visibleIndex = visibleModules.findIndex((item) => item.module === entry.module)
        return (
          <LayoutEditorRow
            key={entry.module}
            module={entry.module}
            label={overviewModuleLabel(entry.module)}
            description={OVERVIEW_MODULES.find((module) => module.id === entry.module)?.description ?? ""}
            visible={entry.visible}
            size={entry.size}
            sizeOptions={OVERVIEW_MODULE_SIZES}
            columns={overviewModuleColumnSpan("large")}
            spanFor={overviewModuleColumnSpan}
            visibleIndex={visibleIndex}
            canEdit={canEdit}
            isSaving={isSaving}
            isDragging={draggedModule !== null}
            isDropTarget={dropTarget === entry.module}
            onResize={(size) => controller.setSize(entry.module, size)}
            onVisibilityChange={(visible) => controller.setVisible(entry.module, visible)}
            onReorder={(targetIndex) => controller.reorder(entry.module, targetIndex)}
            onDragStart={setDraggedModule}
            onDragEnd={() => { setDraggedModule(null); setDropTarget(null) }}
            onDragEnter={setDropTarget}
            onDrop={(event) => onDrop(event, entry.module)}
          />
        )
      })}
    </SettingsGroup>
  )
}

/**
 * 播放分区。三个可配置项的取值与消费者都已由 Registry / Wire 证明：默认倍速、
 * 自动继续、自动下一集。截图不是设置项——快捷键与目录由播放器和后端固定，
 * 这里只如实说明，不给它配一个没有消费者的开关。
 *
 * 导出以便组件测试直接挂载它并验证控件真的写进表单草稿。
 */
export function PlaybackSettings({ form }: { form: SettingsFormController }) {
  const value = playbackDisplayValue(form)
  return (
    <>
      <SettingsIntro section="Playback" title="播放" description="让播放行为稳定、可恢复。字幕与音轨选择不属于当前产品范围；截图使用播放器固定快捷键。" />
      <SettingsFormStatusBar form={form} onReset={() => form.resetToDefaults()} />
      <SettingsFormError form={form} />
      <SettingsGroup title="默认播放偏好" features={["playback.defaultPlaybackRate"]}>
        <div className="px-6 py-5">
          <p className="text-[14px] font-semibold tracking-[-0.005em] text-[var(--haven-settings-foreground)]">默认倍速</p>
          <p className="mt-1 max-w-[520px] text-[12px] leading-[1.65] text-[var(--haven-settings-muted)]">新的视频播放会话默认使用此倍速；播放中的会话仍可在播放器内临时调整。</p>
          <div role="group" aria-label="默认倍速" className="mt-4 grid grid-cols-2 gap-2 sm:grid-cols-5">
            {PLAYBACK_RATE_OPTIONS.map((option) => {
              const active = value.defaultPlaybackRate === option.value
              return (
                <button
                  key={option.value}
                  type="button"
                  aria-pressed={active}
                  onClick={() => form.change({ section: "playback", defaultPlaybackRate: option.value })}
                  className={cn(
                    "min-h-[52px] min-w-0 rounded-[10px] border text-[13px] font-semibold tabular-nums transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary-35)]",
                    active
                      ? "border-[var(--haven-settings-primary)] bg-[var(--haven-settings-primary)] text-[var(--haven-settings-primary-foreground)]"
                      : "border-[var(--haven-settings-border-subtle)] text-[var(--haven-settings-foreground)] hover:bg-[var(--haven-settings-card-hover)]",
                  )}
                >
                  {option.label}
                </button>
              )
            })}
          </div>
        </div>
      </SettingsGroup>
      <SettingsGroup title="连续播放" features={["playback.autoNext", "playback.autoResume"]}>
        <SettingRow title="自动下一集" description="当前一集播放结束后继续下一集；播放列表结束时保持结束状态。">
          <Toggle checked={value.autoNext} onChange={(checked) => form.change({ section: "playback", autoNext: checked })} label="自动下一集" />
        </SettingRow>
        <SettingRow title="自动继续" description="从最近一次可靠保存的位置恢复，而不是只在正常退出时保存。">
          <Toggle checked={value.autoResume} onChange={(checked) => form.change({ section: "playback", autoResume: checked })} label="自动继续" />
        </SettingRow>
      </SettingsGroup>
      <SettingsGroup title="截图" description="使用固定快捷键保存当前画面，不会改变阅读或播放进度。">
        <SettingRow title="截图快捷键" description="仅在播放器页面响应；输入框、文本域、可编辑区域和重复按键不会触发。（播放器固定动作，不可配置）">
          <kbd className="rounded-lg border border-black/[0.08] bg-black/[0.03] px-3 py-2 font-mono text-xs font-semibold text-[var(--haven-settings-foreground)]">Ctrl+Shift+S</kbd>
        </SettingRow>
        <SettingRow title="默认保存位置" description="保存时仍由 Windows 系统对话框确认位置；对话框的初始目录为下载 / 栖阅 / 截图。">
          <span className="text-xs font-semibold text-[var(--haven-settings-muted-strong)]">下载 / 栖阅 / 截图</span>
        </SettingRow>
      </SettingsGroup>
    </>
  )
}

/**
 * 自定义字体族名输入。当前没有可验证的本机字体清单来源，也不做字体文件导入，所以这里
 * 只保存用户填写的族名本身：通过校验的族名才写入表单草稿（提交后由 Reader 包成单个
 * CSS 族名），会逃出 font-family 声明的字符只在行内提示，不进入草稿。
 */
function CustomFontFamilyRow({ value, onCommit }: { value: string | null; onCommit: (patch: ReadingPatchWire) => void }) {
  const [text, setText] = useState(value ?? "")
  const [invalid, setInvalid] = useState(false)
  /** 最近一次由本输入框提交的值：用于区分「外部改了值」和「自己刚提交的值」。 */
  const committedRef = useRef(value ?? "")
  // 表单重置或重新加载后跟随外部值，避免输入框停留在已经作废的草稿上；自己提交的值
  // 不回灌，否则输入中的首尾空格会被即时裁掉、光标跳到末尾。
  useEffect(() => {
    const next = value ?? ""
    if (next === committedRef.current) return
    committedRef.current = next
    setText(next)
    setInvalid(false)
  }, [value])
  return (
    <SettingRow title={READING_CUSTOM_FONT_LABEL} description={READING_CUSTOM_FONT_HINT}>
      <div className="flex flex-col items-end gap-1">
        <input
          type="text"
          value={text}
          spellCheck={false}
          autoComplete="off"
          placeholder={READING_CUSTOM_FONT_PLACEHOLDER}
          aria-label={READING_CUSTOM_FONT_LABEL}
          aria-invalid={invalid}
          onChange={(event) => {
            const next = event.target.value
            setText(next)
            const commit = customFontFamilyCommit(next)
            if (commit.status === "invalid") {
              setInvalid(true)
              return
            }
            setInvalid(false)
            committedRef.current = commit.patch.customFontFamily ?? ""
            onCommit(commit.patch)
          }}
          className={cn(
            "h-10 w-[260px] rounded-xl border bg-[var(--haven-settings-control)] px-3 text-sm text-[var(--haven-settings-foreground)] outline-none",
            invalid ? "border-[var(--haven-settings-danger)]" : "border-black/[0.08] focus:border-[var(--haven-settings-primary-50)]",
          )}
        />
        {invalid && <p className="max-w-[260px] text-right text-[11px] leading-4 text-[var(--haven-settings-danger)]">{READING_CUSTOM_FONT_INVALID_HINT}</p>}
      </div>
    </SettingRow>
  )
}

/** 预览样例：《山居秋暝》前四句。固定四行，取值变化只改排版，不改样例长度。 */
const READING_PREVIEW_SAMPLE_LINES = [
  "空山新雨后，天气晚来秋。",
  "明月松间照，清泉石上流。",
  "竹喧归浣女，莲动下渔舟。",
  "随意春芳歇，王孙自可留。",
] as const

/**
 * 阅读排版预览（卡片左栏）。
 *
 * 预览不自己算一套排版：它吃 `resolveReadingPresentation` 的结果，也就是两个文本阅读器
 * 真正用来渲染正文的那份取值（theme / fontSizePx / lineHeight / contentWidthPx /
 * customFontFamily）。所以字号、行高、字体、正文宽度一改，样例立刻跟着变，不会出现
 * 「预览好看、阅读器不是这样」。
 *
 * 正文栏按阅读器最宽档（820 px）等比缩放，只是示意：真实像素由阅读器按窗口宽度应用。
 * 预览框高度由内容决定（四行样例 + 内边距），不锁死行数以外的高度。
 */
function ReadingTypographyPreview({ settings }: { settings: ReadingSettingsValue }) {
  // 与 BookReaderPage / ArticleReaderPage 同一判定：system 主题按系统偏好落到暖纸或夜间。
  const prefersDark = useSystemPreference("(prefers-color-scheme: dark)")
  const presentation = resolveReadingPresentation(settings, prefersDark)
  const palette = readingPreviewPalette(presentation)
  // 自定义字体族名走阅读器同一套校验与回退；未填或未通过校验时不加这个属性。
  const customFontCss = resolveCustomFontFamilyCss(presentation.fontFamily, {
    background: presentation.customBackground,
    text: presentation.customText,
    fontFamily: presentation.customFontFamily,
  })
  const sampleClass = cn("min-w-0 break-words", READING_FONT_PREVIEW_CLASS[presentation.fontFamily])

  return (
    <div className="flex min-w-0 flex-col">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-[11px] font-semibold tracking-[0.12em] uppercase text-[var(--haven-settings-muted)]">阅读预览</p>
        <p data-testid="reading-typography-preview-meta" className="min-w-0 text-[11px] tabular-nums text-[var(--haven-settings-muted-strong)]">
          {`字号 ${presentation.fontSizePx} px · 行高 ${presentation.lineHeight} · 正文宽度 ${presentation.contentWidthPx} px · ${readingPreviewFontLabel(settings)}`}
        </p>
      </div>
      <div className="mt-3 flex min-h-[212px] flex-1 items-center justify-center overflow-hidden rounded-[12px] border border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-control)] px-4 py-6">
        <div
          data-testid="reading-typography-preview-measure"
          data-measure-px={presentation.contentWidthPx}
          className="min-w-0 max-w-full rounded-[10px] px-4 py-3 shadow-[0_1px_2px_rgba(0,0,0,0.06)]"
          style={{
            // presentation.theme 已经解析过（system 会落到暖纸或夜间），这里一定拿到 hex，
            // 所以只用 backgroundColor；渐变色只出现在上面的主题卡片色块里。
            backgroundColor: palette.background,
            color: palette.color,
            fontSize: `${presentation.fontSizePx}px`,
            lineHeight: presentation.lineHeight,
            width: `${Math.round(readingPreviewMeasureRatio(presentation) * 1000) / 10}%`,
            ...(customFontCss ? { fontFamily: customFontCss } : {}),
          }}
        >
          {READING_PREVIEW_SAMPLE_LINES.map((line, index) => (
            <p
              key={line}
              data-testid={index === 0 ? "reading-typography-preview-sample" : `reading-typography-preview-line-${index}`}
              className={sampleClass}
            >
              {line}
            </p>
          ))}
        </div>
      </div>
      <p className="mt-2 text-[11px] leading-5 text-[var(--haven-settings-muted)]">
        {`示例按最宽 ${READING_PREVIEW_FULL_MEASURE_PX} px 等比示意；真实像素由阅读器按窗口宽度应用。`}
      </p>
    </div>
  )
}

/**
 * 阅读字体选择器（全局「默认字体」与「本资源字体」共用同一份选项规则）。
 *
 * 选项来自 `readingFontPickerOptions`：渲染等价的取值（`fangsong` / `mianfei`）不再
 * 作为新选项提供，但旧快照里已经存了它们时仍会显示并回选当前值，所以打开设置不会看到
 * 空选择、保存也不会静默改掉旧设置。
 */
function ReadingFontSelect({
  value,
  onChange,
  ariaLabel,
  disabled = false,
}: {
  value: ReadingFontFamilyWire
  onChange: (value: ReadingFontFamilyWire) => void
  ariaLabel: string
  disabled?: boolean
}) {
  const options = readingFontPickerOptions(value)
  return (
    <SelectControl
      value={optionLabel(options, value)}
      options={options.map((option) => option.label)}
      onChange={(label) => onChange(optionValue(options, label))}
      ariaLabel={ariaLabel}
      disabled={disabled}
    />
  )
}

/**
 * 排版控件（卡片右栏）：标签列 + 控件列的对齐网格。
 *
 * 顺序按设计稿：默认字体 → 正文宽度 → 字号 → 行高。每一行都直接写回表单草稿，
 * 草稿一变左栏预览立刻重算，不需要先保存。
 */
function ReadingTypographyControls({
  value,
  onChange,
}: {
  value: ReadingSettingsValue
  onChange: (patch: ReadingPatchWire) => void
}) {
  return (
    <div className="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] content-start items-center gap-x-3 gap-y-4">
      <p className="whitespace-nowrap text-[12px] font-medium text-[var(--haven-settings-muted-strong)]">默认字体</p>
      <div className="min-w-0">
        <ReadingFontSelect
          value={value.fontFamily}
          onChange={(fontFamily) => onChange({ section: "reading", fontFamily })}
          ariaLabel="默认字体"
        />
      </div>
      <p className="whitespace-nowrap text-[12px] font-medium text-[var(--haven-settings-muted-strong)]">正文宽度</p>
      <div className="min-w-0">
        <SegmentedControl
          value={optionLabel(READING_WIDTH_OPTIONS, value.contentWidth)}
          options={READING_WIDTH_OPTIONS.map((option) => option.label)}
          onChange={(label) => onChange({ section: "reading", contentWidth: optionValue(READING_WIDTH_OPTIONS, label) })}
          ariaLabel="正文宽度"
        />
      </div>
      <p className="whitespace-nowrap text-[12px] font-medium text-[var(--haven-settings-muted-strong)]">字号</p>
      <div className="min-w-0">
        <SegmentedControl
          value={optionLabel(READING_FONT_SIZE_OPTIONS, value.fontSize)}
          options={READING_FONT_SIZE_OPTIONS.map((option) => option.label)}
          onChange={(label) => onChange({ section: "reading", fontSize: optionValue(READING_FONT_SIZE_OPTIONS, label) })}
          ariaLabel="字号"
        />
      </div>
      <p className="whitespace-nowrap text-[12px] font-medium text-[var(--haven-settings-muted-strong)]">行高</p>
      <div className="min-w-0">
        <SegmentedControl
          value={optionLabel(READING_LINE_HEIGHT_OPTIONS, value.lineHeight)}
          options={READING_LINE_HEIGHT_OPTIONS.map((option) => option.label)}
          onChange={(label) => onChange({ section: "reading", lineHeight: optionValue(READING_LINE_HEIGHT_OPTIONS, label) })}
          ariaLabel="行高"
        />
      </div>
    </div>
  )
}

/**
 * 阅读分区。导出以便组件测试直接挂载它并验证控件真的写进表单草稿。
 *
 * 结构是设计稿的两张主卡片，排版、底色和方式各自不再单独成卡：
 *   1. 「排版与预览」：左栏是吃 `resolveReadingPresentation` 的实时预览，右栏是
 *      默认字体 / 正文宽度 / 字号 / 行高的对齐网格；`fontFamily=custom` 时，自定义
 *      字体族名输入就在这张卡片里，不挪去底色分组。
 *   2. 「阅读底色与方式」：系统主题 + 六个内置预设 + 自定义主题；底下的取色器在
 *      非自定义主题下禁用；阅读方式是顶层「连续滚动 / 分页」，「单页 / 双页」只在
 *      分页时出现的子选择。
 *
 * 三处「不做假控件」的边界：
 *   1. 自定义颜色只在 `theme=custom` 下被阅读器消费，所以其余主题下取色器可见但禁用，
 *      并说明怎么解锁——不是画一个改了没效果的输入框。
 *   2. 自定义字体族名只做 CSS 族名写法校验：不查本机字体列表、也不导入字体文件，
 *      所以文案只说「请填本机已安装的族名」，并说明族名对不上时会退回系统字体栈。
 *   3. 阅读模式仍然只有一个 wire 字段 `reading.pagination`：顶层「滚动 / 分页」，
 *      「单页 / 双页」只在分页时可选的子选择，直接写回 paginated / double。
 */
export function ReadingSettings({
  form,
  resourceContext,
  showNotice,
}: {
  form: SettingsFormController
  resourceContext: ResourcePreferenceContext | null
  showNotice: (message: string) => void
}) {
  const value = readingDisplayValue(form)
  const pagination = value.pagination ?? "scroll"
  const paginationMode = readingPaginationMode(pagination)
  const pageSubMode = readingPageSubMode(pagination)
  const customColorsEnabled = value.theme === "custom"
  const selectPaginationMode = (next: ReadingPaginationMode) => {
    // 顶层模式没变就不写草稿：避免把「点了一下已经选中的项」变成一次空变更。
    if (next === paginationMode) return
    form.change({ section: "reading", pagination: readingPaginationWire(next, pageSubMode) })
  }
  const selectPageSubMode = (next: ReadingPageSubMode) => {
    form.change({ section: "reading", pagination: readingPaginationWire("paginated", next) })
  }

  return (
    <>
      <SettingsIntro section="Reading" title="阅读" description="统一图书、文章和部分报刊资料的阅读体验。全局默认值可保存，打开具体资源时也可以单独覆盖。" />
      <SettingsFormStatusBar form={form} onReset={() => form.resetToDefaults()} />
      <SettingsFormError form={form} />
      <SettingsGroup title="排版与预览" features={["reading.typography"]} description="示例会随字体、字号与行高实时变化；保存后用于文本阅读器。">
        <div className="grid min-w-0 gap-5 px-6 py-5 lg:grid-cols-[minmax(0,1fr)_minmax(0,320px)] lg:items-start">
          <ReadingTypographyPreview settings={value} />
          <ReadingTypographyControls value={value} onChange={(patch) => form.change(patch)} />
        </div>
        {/* 自定义字体族名属于排版，不放进底色卡片：选「自定义」后就在同一张卡片里填写。 */}
        {value.fontFamily === "custom" && (
          <div className="border-t border-[var(--haven-settings-border-subtle)]">
            <CustomFontFamilyRow value={value.customFontFamily} onCommit={(patch) => form.change(patch)} />
          </div>
        )}
      </SettingsGroup>
      <SettingsGroup title="阅读底色与方式" features={["reading.customAppearance", "reading.pagination"]} description={READING_CUSTOM_APPEARANCE_HINT}>
        <div className="border-b border-[var(--haven-settings-border-subtle)] px-6 py-5">
          <p className="text-[11px] font-semibold tracking-[0.12em] uppercase text-[var(--haven-settings-muted)]">阅读底色</p>
          <div role="group" aria-label="主题预设" className="mt-3 grid grid-cols-2 gap-2 sm:grid-cols-4">
            {READING_THEME_OPTIONS.map((option) => {
              const active = value.theme === option.value
              const swatch = readingThemeSwatch(option.value, value)
              return (
                <button
                  key={option.value}
                  type="button"
                  aria-pressed={active}
                  // 「跟随系统」不是一个纯 theme 取值：契约把它拆成 theme=system +
                  // systemAuto。patch 由 readingThemePatch 统一给出，点已选中的「跟随系统」
                  // 也会把历史快照里的 systemAuto=false 修回来。
                  onClick={() => form.change(readingThemePatch(option.value))}
                  className={cn(
                    "flex min-h-[58px] min-w-0 items-center gap-2.5 rounded-[10px] border px-3 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--haven-settings-primary)]",
                    active
                      ? "border-[var(--haven-settings-primary)] bg-[var(--haven-settings-card-hover)]"
                      : "border-[var(--haven-settings-control-border)] hover:bg-[var(--haven-settings-card-hover)]",
                  )}
                >
                  <span
                    aria-hidden="true"
                    className="flex h-[34px] w-[34px] shrink-0 items-center justify-center rounded-[9px] border border-black/[0.08] text-[13px] font-semibold"
                    style={{ background: swatch.background, color: swatch.color }}
                  >
                    文
                  </span>
                  <span className="min-w-0 flex-1 truncate text-[12px] font-semibold text-[var(--haven-settings-foreground)]">{option.label}</span>
                  {active && <Check aria-hidden="true" className="h-[15px] w-[15px] shrink-0 text-[var(--haven-settings-primary)]" strokeWidth={2.6} />}
                </button>
              )
            })}
          </div>
        </div>
        {!customColorsEnabled && (
          <p data-testid="reading-custom-color-disabled-hint" className="border-b border-[var(--haven-settings-border-subtle)] px-6 py-3 text-[12px] leading-5 text-[var(--haven-settings-muted)]">
            {READING_CUSTOM_COLOR_DISABLED_HINT}
          </p>
        )}
        <SettingRow title={READING_CUSTOM_BACKGROUND_LABEL}>
          <input
            type="color"
            value={customReadingColorInputValue(value.customBackground, READING_CUSTOM_BACKGROUND_FALLBACK)}
            aria-label={READING_CUSTOM_BACKGROUND_LABEL}
            disabled={!customColorsEnabled}
            onChange={(event) => {
              const patch = customReadingColorPatch("customBackground", event.target.value)
              if (patch) form.change(patch)
            }}
            className={cn(
              "h-10 w-[64px] rounded-xl border border-black/[0.08] bg-[var(--haven-settings-control)] p-1",
              customColorsEnabled ? "cursor-pointer" : "cursor-not-allowed opacity-50",
            )}
          />
        </SettingRow>
        <SettingRow title={READING_CUSTOM_TEXT_LABEL}>
          <input
            type="color"
            value={customReadingColorInputValue(value.customText, READING_CUSTOM_TEXT_FALLBACK)}
            aria-label={READING_CUSTOM_TEXT_LABEL}
            disabled={!customColorsEnabled}
            onChange={(event) => {
              const patch = customReadingColorPatch("customText", event.target.value)
              if (patch) form.change(patch)
            }}
            className={cn(
              "h-10 w-[64px] rounded-xl border border-black/[0.08] bg-[var(--haven-settings-control)] p-1",
              customColorsEnabled ? "cursor-pointer" : "cursor-not-allowed opacity-50",
            )}
          />
        </SettingRow>
        <div className="px-6 pb-1 pt-5">
          <p className="text-[11px] font-semibold tracking-[0.12em] uppercase text-[var(--haven-settings-muted)]">阅读方式</p>
        </div>
        <SettingRow title="阅读模式" description="仅对图书文本阅读器生效；文章、PDF 等格式继续使用各自阅读方式。切回连续滚动后会保存为连续滚动。">
          <SegmentedControl value={optionLabel(READING_PAGINATION_MODE_OPTIONS, paginationMode)} options={READING_PAGINATION_MODE_OPTIONS.map((option) => option.label)} onChange={(label) => selectPaginationMode(optionValue(READING_PAGINATION_MODE_OPTIONS, label))} ariaLabel="阅读模式" />
        </SettingRow>
        {paginationMode === "paginated" && (
          <SettingRow title="分页方式" description="单页一次显示一页；双页一次显示相邻的两页。">
            <SegmentedControl value={optionLabel(READING_PAGE_SUBMODE_OPTIONS, pageSubMode)} options={READING_PAGE_SUBMODE_OPTIONS.map((option) => option.label)} onChange={(label) => selectPageSubMode(optionValue(READING_PAGE_SUBMODE_OPTIONS, label))} ariaLabel="分页方式" />
          </SettingRow>
        )}
      </SettingsGroup>
      <ResourcePreferencePanel section="reading" context={resourceContext} showNotice={showNotice} />
    </>
  )
}

export function ComicSettings({
  form,
  resourceContext,
  showNotice,
}: {
  form: SettingsFormController
  resourceContext: ResourcePreferenceContext | null
  showNotice: (message: string) => void
}) {
  const value = comicDisplayValue(form)
  const loading = form.isLoading
  const saving = form.isSaving
  const statusLabel = loading
    ? "正在加载…"
    : saving
      ? "正在保存…"
      : form.hasError
        ? "设置暂不可用"
        : form.isDirty
          ? "有未保存的修改"
          : "所有设置已保存"
  const statusTone = loading || saving || form.hasError
    ? "bg-[#b7aa91]"
    : form.isDirty
      ? "bg-[#c58d4d]"
      : "bg-[#79a18a]"
  const disabled = loading || saving
  const viewModes = [
    { value: "single" as const, label: "单页", preview: "single" },
    { value: "double" as const, label: "双页", preview: "double" },
    { value: "strip" as const, label: "条漫", preview: "strip" },
  ]
  const pageGaps = [
    { value: "zero" as const, label: "0 px" },
    { value: "twelve" as const, label: "12 px" },
    { value: "twenty_four" as const, label: "24 px" },
  ]
  const preloadOptions = [
    { value: "one" as const, label: "1 页" },
    { value: "three" as const, label: "3 页" },
    { value: "five" as const, label: "5 页" },
    { value: "unlimited" as const, label: "不限制" },
  ]

  return (
    <div aria-label="Comic" className="pt-[8px]" data-settings-features="comic.layout" tabIndex={-1}>
      <header>
        <p className="h-[16px] text-[10px] font-light leading-[18px] tracking-[0.3px] text-[var(--haven-settings-muted-subtle)]">READING / COMIC LAYOUT</p>
        <h2 className="mt-[7px] h-[38px] text-[31px] font-bold leading-[38px] tracking-[-0.025em] text-[var(--haven-settings-foreground)]">漫画</h2>
        <p className="mt-[8px] h-[18px] text-[11px] leading-[18px] text-[var(--haven-settings-muted-strong)]">为漫画调整页面布局、阅读方向与预加载节奏。</p>
        <div className="mt-[5px] flex h-[30px] items-center justify-between gap-4">
          <div className="flex min-w-0 items-center gap-[8px] text-[10.5px] text-[var(--haven-settings-muted-strong)]" role="status" aria-live="polite">
            <span aria-hidden="true" className={cn("h-[8px] w-[8px] shrink-0 rounded-full", statusTone)} />
            <span className="truncate">{statusLabel}</span>
          </div>
          <div className="flex shrink-0 items-center gap-4">
            {form.isDirty && (
              <button
                type="button"
                onClick={() => { void form.save() }}
                disabled={disabled}
                className="text-[10.5px] font-medium text-[var(--haven-settings-foreground)] transition-colors hover:opacity-75 disabled:cursor-not-allowed disabled:opacity-50"
              >
                保存修改
              </button>
            )}
            <button
              type="button"
              onClick={() => form.resetToDefaults()}
              disabled={disabled}
              className="text-[10.5px] font-normal text-[var(--haven-settings-muted-strong)] transition-colors hover:text-[var(--haven-settings-foreground)] disabled:cursor-not-allowed disabled:opacity-50"
            >
              恢复默认
            </button>
          </div>
        </div>
      </header>
      <SettingsFormError form={form} />

      <div className="mt-[16px] flex flex-col gap-[20px]">
        <section aria-labelledby="comic-reading-mode-title" className="h-auto rounded-[20px] border border-[var(--haven-settings-border)] bg-[var(--haven-settings-sidebar)] px-[24px] py-[22px] min-[1232px]:min-h-[260px]">
          <h3 id="comic-reading-mode-title" className="h-[20px] text-[15px] font-bold leading-[20px] text-[var(--haven-settings-card-foreground)]">阅读方式</h3>
          <p className="mt-[10px] h-[18px] text-[11.5px] leading-[18px] text-[var(--haven-settings-muted-strong)]">为漫画选择合适的页面排列与翻页方向。</p>
          <div className="mt-[10px] flex flex-col gap-[24px] min-[1232px]:h-[140px] min-[1232px]:flex-row min-[1232px]:items-center">
            <div className="flex min-w-0 flex-col gap-[6px] min-[1232px]:w-[620px]">
              <p className="h-[16px] text-[11.5px] font-medium leading-[16px] text-[var(--haven-settings-foreground)]">阅读模式</p>
              <div className="grid w-full max-w-[572px] grid-cols-1 gap-[10px] sm:grid-cols-3" role="group" aria-label="阅读模式">
                {viewModes.map((mode) => {
                  const selected = value.viewMode === mode.value
                  return (
                    <button
                      key={mode.value}
                      type="button"
                      aria-pressed={selected}
                      aria-label={mode.label}
                      disabled={disabled}
                      onClick={() => form.change({ section: "comic", viewMode: mode.value })}
                      className="group flex w-full max-w-[184px] flex-col items-center gap-[6px] rounded-[8px] bg-transparent p-0 text-center outline-none focus-visible:ring-2 focus-visible:ring-[#8d8068] focus-visible:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-55"
                    >
                      <span
                        aria-hidden="true"
                        className={cn(
                          "relative h-[56px] w-full max-w-[172px] overflow-hidden rounded-[8px] border bg-[#f1eee7] dark:bg-[var(--haven-settings-control)]",
                          selected ? "border-[#b8aa90] dark:border-[var(--haven-settings-primary)]" : "border-[#ddd8ce] dark:border-[var(--haven-settings-border-subtle)]",
                        )}
                      >
                        {mode.preview === "single" && (
                          <span className="absolute left-1/2 top-[6px] h-[42px] w-[40px] -translate-x-1/2 rounded-[3px] border border-[#d7d0c3] bg-[#fffdfb]" />
                        )}
                        {mode.preview === "double" && (
                          <span className="absolute left-1/2 top-[6px] flex -translate-x-1/2 gap-[4px]">
                            <span className="h-[42px] w-[30px] rounded-[3px] border border-[#d7d0c3] bg-[#fffdfb]" />
                            <span className="h-[42px] w-[30px] rounded-[3px] border border-[#d7d0c3] bg-[#fffdfb]" />
                          </span>
                        )}
                        {mode.preview === "strip" && (
                          <span className="absolute left-1/2 top-[4px] flex -translate-x-1/2 flex-col gap-[3px]">
                            <span className="h-[13px] w-[44px] rounded-[2px] border border-[#d7d0c3] bg-[#fffdfb]" />
                            <span className="h-[13px] w-[44px] rounded-[2px] border border-[#d7d0c3] bg-[#ebe5d9]" />
                            <span className="h-[13px] w-[44px] rounded-[2px] border border-[#d7d0c3] bg-[#fffdfb]" />
                          </span>
                        )}
                      </span>
                      <span className={cn(
                        "flex h-[38px] w-[96px] items-center justify-center rounded-[12px] border text-[11.5px] leading-[18px] transition-colors",
                        selected ? "border-[#252320] bg-[#252320] text-[#f7f4ed] dark:border-[var(--haven-settings-primary)] dark:bg-[var(--haven-settings-primary)] dark:text-[var(--haven-settings-primary-foreground)]" : "border-[#ddd8ce] bg-[#f0ede6] text-[#252320] dark:border-[var(--haven-settings-border-subtle)] dark:bg-[var(--haven-settings-control)] dark:text-[var(--haven-settings-foreground)]",
                      )}>
                        {mode.label}
                      </span>
                    </button>
                  )
                })}
              </div>
            </div>

            <div className="flex min-w-0 flex-col justify-center gap-[8px] min-[1232px]:h-[140px] min-[1232px]:w-[348px]">
              <p className="h-[16px] text-[11.5px] font-medium leading-[16px] text-[var(--haven-settings-foreground)]">翻页方向</p>
              <p className="h-[16px] text-[10.5px] leading-[16px] text-[var(--haven-settings-muted-strong)]">页面按所选方向依次展开。</p>
              <div className="flex flex-wrap gap-[8px]" role="group" aria-label="翻页方向">
                {[
                  { value: "rtl" as const, label: "从右向左" },
                  { value: "ltr" as const, label: "从左向右" },
                ].map((direction) => {
                  const selected = value.direction === direction.value
                  return (
                    <button
                      key={direction.value}
                      type="button"
                      aria-pressed={selected}
                      disabled={disabled}
                      onClick={() => form.change({ section: "comic", direction: direction.value })}
                      className={cn(
                        "h-[38px] w-[96px] rounded-[12px] border text-[11.5px] transition-colors disabled:cursor-not-allowed disabled:opacity-55",
                        selected ? "border-[#252320] bg-[#252320] text-[#f7f4ed] dark:border-[var(--haven-settings-primary)] dark:bg-[var(--haven-settings-primary)] dark:text-[var(--haven-settings-primary-foreground)]" : "border-[#ddd8ce] bg-[#f0ede6] text-[#252320] hover:bg-[#eae6de] dark:border-[var(--haven-settings-border-subtle)] dark:bg-[var(--haven-settings-control)] dark:text-[var(--haven-settings-foreground)] dark:hover:bg-[var(--haven-settings-card-hover)]",
                      )}
                    >
                      {direction.label}
                    </button>
                  )
                })}
              </div>
            </div>
          </div>
        </section>

        <section aria-labelledby="comic-gap-preload-title" className="h-auto rounded-[20px] border border-[var(--haven-settings-border)] bg-[var(--haven-settings-sidebar)] px-[24px] py-[22px] min-[1232px]:h-[240px]">
          <h3 id="comic-gap-preload-title" className="h-[20px] text-[15px] font-bold leading-[20px] text-[var(--haven-settings-card-foreground)]">页面间距与预加载</h3>
          <p className="mt-[10px] h-[18px] text-[11.5px] leading-[18px] text-[var(--haven-settings-muted-strong)]">调整相邻页面的留白，以及阅读时提前准备的页数。</p>
          <div className="mt-[10px] flex flex-col gap-[24px] min-[1232px]:h-[136px] min-[1232px]:flex-row min-[1232px]:items-center">
            <div className="flex min-w-0 flex-col gap-[7px] min-[1232px]:w-[420px]">
              <p className="h-[16px] text-[11.5px] font-medium leading-[16px] text-[var(--haven-settings-foreground)]">页面间距</p>
              <p className="h-[16px] text-[10.5px] leading-[16px] text-[var(--haven-settings-muted-strong)]">页面留白更多，边界更清楚。</p>
              <div className="flex flex-wrap gap-[8px]" role="group" aria-label="页面间距">
                {pageGaps.map((gap) => {
                  const selected = value.pageGap === gap.value
                  return (
                    <button
                      key={gap.value}
                      type="button"
                      aria-pressed={selected}
                      disabled={disabled}
                      onClick={() => form.change({ section: "comic", pageGap: gap.value })}
                      className={cn(
                        "h-[38px] w-[96px] rounded-[12px] border text-[11.5px] transition-colors disabled:cursor-not-allowed disabled:opacity-55",
                        selected ? "border-[#252320] bg-[#252320] text-[#f7f4ed] dark:border-[var(--haven-settings-primary)] dark:bg-[var(--haven-settings-primary)] dark:text-[var(--haven-settings-primary-foreground)]" : "border-[#ddd8ce] bg-[#f0ede6] text-[#252320] hover:bg-[#eae6de] dark:border-[var(--haven-settings-border-subtle)] dark:bg-[var(--haven-settings-control)] dark:text-[var(--haven-settings-foreground)] dark:hover:bg-[var(--haven-settings-card-hover)]",
                      )}
                    >
                      {gap.label}
                    </button>
                  )
                })}
              </div>
            </div>

            <div className="flex min-w-0 flex-col gap-[6px] min-[1232px]:w-[548px]">
              <p className="h-[16px] text-[11.5px] font-medium leading-[16px] text-[var(--haven-settings-foreground)]">预加载页数</p>
              <p className="h-[16px] text-[10.5px] leading-[16px] text-[var(--haven-settings-muted-strong)]">提前准备后续页面，切页时更顺畅。</p>
              <div className="flex flex-wrap gap-[8px]" role="group" aria-label="预加载页数">
                {preloadOptions.map((option) => {
                  const selected = value.preloadPages === option.value
                  return (
                    <button
                      key={option.value}
                      type="button"
                      aria-pressed={selected}
                      disabled={disabled}
                      onClick={() => form.change({ section: "comic", preloadPages: option.value })}
                      className={cn(
                        "h-[38px] w-[96px] rounded-[12px] border text-[11.5px] transition-colors disabled:cursor-not-allowed disabled:opacity-55",
                        selected ? "border-[#252320] bg-[#252320] text-[#f7f4ed] dark:border-[var(--haven-settings-primary)] dark:bg-[var(--haven-settings-primary)] dark:text-[var(--haven-settings-primary-foreground)]" : "border-[#ddd8ce] bg-[#f0ede6] text-[#252320] hover:bg-[#eae6de] dark:border-[var(--haven-settings-border-subtle)] dark:bg-[var(--haven-settings-control)] dark:text-[var(--haven-settings-foreground)] dark:hover:bg-[var(--haven-settings-card-hover)]",
                      )}
                    >
                      {option.label}
                    </button>
                  )
                })}
              </div>
              <p className="h-[16px] text-[9.5px] leading-[16px] text-[var(--haven-settings-muted-strong)]">不限制时，阅读器仍会控制连续预读的页数。</p>
            </div>
          </div>
        </section>

        <ResourcePreferencePanel section="comic" context={resourceContext} showNotice={showNotice} />
      </div>
    </div>
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
    if (section === "comic") {
      return (
        <div data-testid="comic-resource-preferences-empty" className="flex min-h-[68px] w-full items-center gap-[12px] rounded-[14px] border border-[var(--haven-settings-border)] bg-[var(--haven-settings-background)] px-[18px] py-[10px]">
          <div className="min-w-0 flex-1">
            <p className="text-[12px] font-medium leading-[18px] text-[var(--haven-settings-foreground)]">漫画资源内设</p>
            <p className="text-[10.5px] leading-[16px] text-[var(--haven-settings-muted-strong)]">从漫画阅读器打开内容后，可为这部漫画单独调整阅读方式。</p>
          </div>
          <span className="w-[148px] shrink-0 text-right text-[10.5px] leading-[16px] text-[var(--haven-settings-muted-strong)]">请从漫画阅读器进入</span>
        </div>
      )
    }

    return (
      <SettingsGroup title="资源内设" description="从阅读器或漫画阅读器打开本资源设置后，可在不改变全局默认值的情况下覆盖单个资源。">
        <SettingRow title="当前资源" description="请从阅读器中打开本资源设置，再为这份内容单独调整偏好。">
          <span className="text-[12px] font-medium text-[var(--haven-settings-muted)]">请从阅读器进入</span>
        </SettingRow>
      </SettingsGroup>
    )
  }

  return (
    <SettingsGroup
      title="资源内设"
      description="优先级：本资源 → 版本 → 全局。资源身份由当前会话提供，保存使用版本校验。"
    >
      <div className="border-b border-black/[0.05] px-6 py-4">
        <div className="flex flex-wrap items-center gap-2 text-[11px] font-semibold text-[var(--haven-settings-muted-strong)]">
          <span className="rounded-md bg-black/[0.04] px-2 py-1">Work {workId ?? "未返回"}</span>
          <ChevronRight className="h-3.5 w-3.5" />
          <span className="rounded-md bg-black/[0.04] px-2 py-1">Edition {editionId}</span>
          <ChevronRight className="h-3.5 w-3.5" />
          <span className="rounded-md bg-[var(--haven-settings-primary-10)] px-2 py-1 text-[var(--haven-settings-primary)]">MediaItem {mediaItemId}</span>
        </div>
        {status === "ready" && result && (
          <p className="mt-2 text-[11px] text-[var(--haven-settings-muted)]">当前生效来源：{resourcePreferenceSource(result, section)}</p>
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
      {status === "loading" && <p className="px-6 py-5 text-sm text-[var(--haven-settings-muted)]">正在读取本资源配置…</p>}
      {status === "error" && (
        <div className="flex flex-wrap items-center justify-between gap-3 px-6 py-5">
          <p className="text-sm font-medium text-[var(--haven-settings-danger)]">{errorMessage ?? "本资源配置暂时不可用"}</p>
          <button type="button" onClick={() => { void load() }} className="rounded-full border border-[var(--haven-settings-primary-30)] px-3 py-1.5 text-xs font-semibold text-[var(--haven-settings-primary)]">重试</button>
        </div>
      )}
      {status === "ready" && result && draft?.section === "reading" && section === "reading" && (
        <>
          <SettingRow title="本资源字体" description="只覆盖当前 MediaItem，未设置的字段继续继承版本或全局值。">
            <ReadingFontSelect value={draft.fontFamily} onChange={(fontFamily) => updateDraft({ fontFamily })} ariaLabel="本资源字体" disabled={saving} />
          </SettingRow>
          <SettingRow title="本资源主题">
            <SelectControl value={optionLabel(READING_THEME_OPTIONS, draft.theme)} options={READING_THEME_OPTIONS.map((option) => option.label)} onChange={(label) => updateDraft({ theme: optionValue(READING_THEME_OPTIONS, label) })} ariaLabel="本资源主题" disabled={saving} />
          </SettingRow>
          <SettingRow title="本资源排版" description="字号、行高和正文宽度会在下次打开内容时保持；未调整的项继续继承版本或全局值。">
            <div className="flex flex-wrap justify-end gap-2">
              <SelectControl value={optionLabel(READING_FONT_SIZE_OPTIONS, draft.fontSize)} options={READING_FONT_SIZE_OPTIONS.map((option) => option.label)} onChange={(label) => updateDraft({ fontSize: optionValue(READING_FONT_SIZE_OPTIONS, label) })} ariaLabel="本资源字号" disabled={saving} />
              <SelectControl value={optionLabel(READING_LINE_HEIGHT_OPTIONS, draft.lineHeight)} options={READING_LINE_HEIGHT_OPTIONS.map((option) => option.label)} onChange={(label) => updateDraft({ lineHeight: optionValue(READING_LINE_HEIGHT_OPTIONS, label) })} ariaLabel="本资源行高" disabled={saving} />
              <SelectControl value={optionLabel(READING_WIDTH_OPTIONS, draft.contentWidth)} options={READING_WIDTH_OPTIONS.map((option) => option.label)} onChange={(label) => updateDraft({ contentWidth: optionValue(READING_WIDTH_OPTIONS, label) })} ariaLabel="本资源正文宽度" disabled={saving} />
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
        <div className="flex flex-wrap items-center justify-end gap-4 border-t border-black/[0.05] px-6 py-4">
          <button type="button" onClick={() => { void reset() }} disabled={saving} className="text-[13px] font-medium text-[var(--haven-settings-muted)] transition-colors hover:text-[var(--haven-settings-foreground)] disabled:cursor-not-allowed disabled:opacity-50">重置本资源</button>
          <button type="button" onClick={() => { void save() }} disabled={saving} className="rounded-full bg-[var(--haven-settings-primary)] px-4 py-2 text-[13px] font-semibold text-[var(--haven-settings-primary-foreground)] transition-colors hover:bg-[var(--haven-settings-primary-hover)] disabled:cursor-not-allowed disabled:opacity-50">{saving ? "保存中…" : "保存本资源设置"}</button>
        </div>
      )}
    </SettingsGroup>
  )
}



function agentBrokerStatusPresentation(status: AgentBrokerStatusDto | null, loading: boolean): { label: string; dot: string } {
  switch (status) {
    case "listening":
      return { label: "监听中", dot: "bg-[#34c759]" }
    case "busy":
      return { label: "端点被占用", dot: "bg-[#ff9500]" }
    case "unavailable":
      return { label: "不可用", dot: "bg-[#d70015]" }
    case "disabled":
      return { label: "已关闭", dot: "bg-[#c7ccd1]" }
    default:
      return { label: loading ? "读取中…" : "状态未知", dot: "bg-[#c7ccd1]" }
  }
}

/**
 * 外部 MCP 客户端「一键配置」的真实接线（MCP_EXTERNAL_AGENT_TRANSPORT.md §9.1.1）。
 *
 * 数据全部来自 `useMcpClientConfig`（→ gateway → Typed HavenClient → Tauri command →
 * Application service → 用户配置文件），组件不直接 invoke。
 *
 * 四条必须留在界面上的事实：
 * - **授权范围写在界面上**：只会写 `~/.codex/config.toml` 的 `[mcp_servers.haven]` 与
 *   `~/.claude.json` 的 `mcpServers.haven`；两个客户端之外没有按钮，也没有"自定义路径"。
 * - **只会新增/更新 Haven 这一条**：其它配置项原样保留，写入前还会留一份可恢复备份。
 * - **状态是后端读出来的**：`configured` 只表示"这一条与当前运行时一致"，不表示"客户端
 *   已经连上"；`blocked` / `malformed` 是"没有写入"，必须与"未配置"分开显示。
 * - **不显示任何文件内容**：这里只显示后端给出的状态、路径与原因。
 */
function McpClientAutoConfigRows({ showNotice }: { showNotice: (message: string) => void }) {
  const clients = useMcpClientConfig()

  const configure = async (entry: McpClientTargetStatusWire) => {
    const ok = await clients.configure(entry.target)
    // 只有权威写入成功才提示成功；失败由下面的错误行如实显示。
    if (ok) showNotice(`已写入 ${entry.label} 的 Haven 配置；其它配置项未改动`)
  }

  if (clients.status === null) {
    return (
      <SettingRow
        title="一键配置外部 MCP 客户端"
        description="只写 Codex 的 ~/.codex/config.toml 与 Claude Code 的 ~/.claude.json 里的 Haven 这一条，其它配置保持原样。"
      >
        <span className="text-[13px] text-[var(--haven-settings-muted-strong)]">{clients.error === null ? "读取中…" : "未能读取客户端配置状态"}</span>
      </SettingRow>
    )
  }

  return (
    <>
      <SettingRow
        title="一键配置外部 MCP 客户端"
        description="授权范围只有两个固定位置：Codex 的 ~/.codex/config.toml（[mcp_servers.haven]）与 Claude Code 的 ~/.claude.json（mcpServers.haven）。写入是结构化合并，其它配置项原样保留，并在写入前留一份可恢复备份。"
      >
        <span className="text-[13px] text-[var(--haven-settings-muted-strong)]">
          {clients.status.runtimeReady ? "随包分发的运行时已就绪" : "运行时未就绪"}
        </span>
      </SettingRow>
      {!clients.status.runtimeReady && clients.status.runtimeDetail.length > 0 && (
        <SettingRow title="运行时不可用" description={clients.status.runtimeDetail}>
          <button type="button" onClick={() => void clients.reload()} disabled={clients.busy} className="text-[13px] font-semibold text-[var(--haven-settings-primary)] disabled:cursor-not-allowed disabled:text-[var(--haven-settings-muted-strong)]">重新读取状态</button>
        </SettingRow>
      )}
      {clients.status.targets.map((entry) => (
        <SettingRow
          key={entry.target}
          title={`${entry.label} 的 Haven 配置`}
          description={entry.configPath === null ? entry.detail : `${entry.configPath} · ${entry.detail}`}
        >
          <div className="flex max-w-[420px] flex-col items-end gap-2">
            <div className="flex items-center gap-3">
              <span className="text-[13px] font-semibold text-[var(--haven-settings-foreground)]">{mcpClientStateLabel(entry.state)}</span>
              {entry.writable && (
                <button
                  type="button"
                  onClick={() => void configure(entry)}
                  disabled={clients.busy}
                  className="inline-flex h-[36px] items-center justify-center rounded-full bg-[var(--haven-settings-primary)] px-[16px] text-[12px] font-semibold leading-none text-[var(--haven-settings-primary-foreground)] transition-colors hover:bg-[var(--haven-settings-primary-hover)] disabled:cursor-not-allowed disabled:opacity-50"
                >
                  {clients.pendingTarget === entry.target ? "写入中…" : mcpClientActionLabel(entry.state)}
                </button>
              )}
            </div>
            <p className="text-right text-[11px] leading-5 text-[var(--haven-settings-muted-strong)] dark:text-[#8e8e93]">{mcpClientStatusNote(entry)}</p>
          </div>
        </SettingRow>
      ))}
      {clients.error !== null && (
        <SettingRow title="客户端配置操作失败" description={clients.error.message}>
          <div className="flex items-center gap-3">
            <button type="button" onClick={() => void clients.reload()} disabled={clients.busy} className="text-[13px] font-semibold text-[var(--haven-settings-primary)] disabled:cursor-not-allowed disabled:text-[var(--haven-settings-muted-strong)]">重新读取状态</button>
            <button type="button" onClick={clients.dismissError} className="text-[13px] font-medium text-[var(--haven-settings-muted-strong)]">关闭</button>
          </div>
        </SettingRow>
      )}
    </>
  )
}

/**
 * 设置页「外部 Agent 接入」分组（A5 接线切片）。
 *
 * 数据全部来自 `useAgentBrokerSettings`（→ gateway → Typed HavenClient → Tauri
 * command → Rust Broker），组件不直接 invoke，也不自己拼端点：
 * - 默认事实来自客户端：读到 disabled 之前显示「读取中」，绝不预置成已开启；
 * - 端点只有真实监听（Named Pipe / Unix socket）才可复制，浏览器 Mock 的演示标识
 *   只标记「没有真实本地监听」，也不提供任何模板；
 * - 这里没有 approve / apply / reject 按钮：Broker 的请求集只有 context /
 *   create_proposal，提案的批准与写入只能回到栖阅界面。
 */
function ExternalAgentAccessSettings({ showNotice, registerReloader }: { showNotice: (message: string) => void; registerReloader: RegisterSettingsSectionReloader }) {
  const broker = useAgentBrokerSettings()
  const reloadBroker = broker.reload
  const [templateId, setTemplateId] = useState<AgentBrokerClientTemplateId>("codex")
  const [copyError, setCopyError] = useState<string | null>(null)

  const presentation = agentBrokerStatusPresentation(broker.status, broker.loading)
  // 模板只在真实端点下生成：拿 mock:// 拼出来的配置会被粘进客户端然后连不上。
  const template = broker.endpointKind === "live" && broker.endpoint !== null
    ? buildAgentBrokerMcpTemplate(broker.endpoint)
    : null
  const selectedTemplate = AGENT_BROKER_CLIENT_TEMPLATES.find((item) => item.id === templateId)
    ?? AGENT_BROKER_CLIENT_TEMPLATES[0]
  const isListening = broker.status === "listening"

  useEffect(() => registerReloader("ai", () => { void reloadBroker() }), [registerReloader, reloadBroker])

  const copyText = async (text: string, done: string) => {
    setCopyError(null)
    try {
      const clipboard = navigator.clipboard
      if (!clipboard || typeof clipboard.writeText !== "function") {
        throw new Error("clipboard unavailable")
      }
      await clipboard.writeText(text)
      showNotice(done)
    } catch {
      setCopyError("复制失败：当前环境不允许写入剪贴板，请手动选中上面的文本复制。")
    }
  }

  return (
    <SettingsGroup
      title="外部 Agent 接入"
      features={["ai.agentBroker", "ai.clientAutoConfig"]}
      description="让本机已安装的外部 Agent 通过 MCP 接入栖阅。默认关闭，只有你显式开启后才会创建本地端点。"
    >
      <SettingRow
        title="接入状态"
        description="外部 Agent 只能读取脱敏信息并提交建议，批准、拒绝与应用改动均需你在栖阅中操作；不会获得本机文件、凭据或命令执行权限。"
      >
        <div className="flex items-center gap-3">
          <span className="flex items-center gap-1.5 text-[13px] font-semibold text-[var(--haven-settings-foreground)]">
            <span className={cn("h-[8px] w-[8px] rounded-full", presentation.dot)} />
            {presentation.label}
          </span>
          {isListening ? (
            <button type="button" onClick={() => void broker.disable()} disabled={broker.busy} className="inline-flex h-[36px] items-center justify-center rounded-full bg-black/[0.05] px-[16px] text-[12px] font-semibold leading-none text-[#1d1d1f] transition-colors hover:bg-black/[0.08] disabled:cursor-not-allowed disabled:opacity-50 dark:bg-white/[0.08] dark:text-[#f5f5f5] dark:hover:bg-white/[0.12]">停用</button>
          ) : (
            <button type="button" onClick={() => void broker.enable()} disabled={broker.busy} className="inline-flex h-[36px] items-center justify-center rounded-full bg-[var(--haven-settings-primary)] px-[16px] text-[12px] font-semibold leading-none text-[var(--haven-settings-primary-foreground)] transition-colors hover:bg-[var(--haven-settings-primary-hover)] disabled:cursor-not-allowed disabled:opacity-50">{broker.status === "busy" || broker.status === "unavailable" ? "重试启用" : "启用外部接入"}</button>
          )}
        </div>
      </SettingRow>
      {broker.reason !== null && (
        <SettingRow title="无法监听的原因" description={broker.reason}>
          <span className="text-[13px] text-[var(--haven-settings-muted-strong)]">由本机如实上报</span>
        </SettingRow>
      )}
      <details className="border-b border-[var(--haven-settings-border-subtle)] px-6 py-3 text-[12px] text-[var(--haven-settings-muted-strong)]">
        <summary className="cursor-pointer font-medium">权限与安全边界</summary>
        <p className="mt-2 leading-5">外部 Agent 没有 SQL、文件系统、Secret 或任意命令权限；只能读取脱敏上下文并创建待审批建议。</p>
      </details>
      {broker.endpointKind === "live" && (
        <SettingRow title="本地端点" description="由栖阅按当前用户解析的本地地址。它不是密钥，可以复制给本机 Agent 客户端。">
          <div className="flex max-w-[420px] items-center gap-2">
            <code className="truncate rounded-lg bg-[#f5f5f7] px-2 py-1 font-mono text-[11px] text-[#1d1d1f] dark:bg-[#2c2c2e] dark:text-[#f5f5f5]">{broker.endpoint}</code>
            <button type="button" onClick={() => void copyText(broker.endpoint ?? "", "已复制本地端点")} className="shrink-0 rounded-full px-[8px] py-[8px] text-xs font-semibold text-[var(--haven-settings-primary)]">复制</button>
          </div>
        </SettingRow>
      )}
      {broker.endpointKind === "preview" && (
        <SettingRow title="本地端点" description="这里没有可复制的地址，下面也不会给出连接模板。">
          <span className="text-[13px] font-semibold text-[var(--haven-settings-muted-strong)]">浏览器预览：没有真实本地监听</span>
        </SettingRow>
      )}
      {broker.endpointKind === "unknown" && (
        <SettingRow title="本地端点" description="端点形态不在本机允许的闭合集合内，因此不提供复制。">
          <span className="text-[13px] font-semibold text-[var(--haven-settings-muted-strong)]">未识别</span>
        </SettingRow>
      )}
      {template !== null && (
        <SettingRow
          title="MCP 连接配置"
          description="把这段 JSON 放进你所用客户端（Codex / Claude Code / DSH / Pi）的 MCP 服务器配置；四者内容完全相同，差别只在放进哪个配置文件。"
        >
          <div className="flex max-w-[440px] flex-col items-end gap-2">
            <div className="flex items-center gap-3">
              <span className="text-[12px] text-[var(--haven-settings-muted-strong)]">客户端</span>
              <SelectControl
                value={selectedTemplate.label}
                options={AGENT_BROKER_CLIENT_TEMPLATES.map((item) => item.label)}
                onChange={(value) => {
                  const next = AGENT_BROKER_CLIENT_TEMPLATES.find((item) => item.label === value)
                  if (next) setTemplateId(next.id)
                }}
                ariaLabel="配置模板"
              />
            </div>
            <p className="text-right text-[11px] leading-5 text-[var(--haven-settings-muted-strong)]">{selectedTemplate.hint}{AGENT_BROKER_TEMPLATE_NOTE}</p>
            <pre className="max-h-[220px] w-full overflow-auto rounded-xl bg-[#f5f5f7] p-3 text-left font-mono text-[11px] leading-5 text-[#1d1d1f] dark:bg-[#2c2c2e] dark:text-[#f5f5f5]">{template}</pre>
            <button type="button" onClick={() => void copyText(template, "已复制 MCP 连接配置")} className="rounded-full px-[8px] py-[8px] text-xs font-semibold text-[var(--haven-settings-primary)]">复制配置</button>
          </div>
        </SettingRow>
      )}
      {broker.error !== null && (
        <SettingRow title="接入操作失败" description={broker.error.message}>
          <div className="flex items-center gap-3">
            <button type="button" onClick={() => void broker.reload()} disabled={broker.busy} className="text-[13px] font-semibold text-[var(--haven-settings-primary)] disabled:cursor-not-allowed disabled:text-[var(--haven-settings-muted-strong)]">重新读取状态</button>
            <button type="button" onClick={broker.dismissError} className="text-[13px] font-medium text-[var(--haven-settings-muted-strong)]">关闭</button>
          </div>
        </SettingRow>
      )}
      <McpClientAutoConfigRows showNotice={showNotice} />
      {copyError !== null && (
        <SettingRow title="复制失败" description={copyError}>
          <button type="button" onClick={() => setCopyError(null)} className="text-[13px] font-medium text-[var(--haven-settings-muted-strong)]">关闭</button>
        </SettingRow>
      )}
    </SettingsGroup>
  )
}

/**
 * 设置页「内置技能」分组的真实接线（原生 Skill 运行时）。
 *
 * 数据全部来自 `useAgentSkills`（→ gateway → Typed HavenClient → Tauri command →
 * Application service → SQLite 权威状态），组件不直接 invoke，也不持有第二份状态。
 *
 * 三条必须留在界面上的事实：
 * - **技能正文不进前端**：这里只显示 id、frontmatter 描述与注入字数。正文由 Rust 编译期
 *   嵌入，只在模型请求内部拼装，因此界面没有"编辑技能 / 导入技能 / 按路径加载"入口。
 * - **三态分离**：`stale`（启用过，但随应用分发的内容已经变了）必须与「未启用」分开显示。
 *   把 stale 折叠成「已启用」会让用户以为旧内容还在起作用。
 * - **启用不等于授权**：技能只是模型请求里的说明性上下文，不新增工具、不改变能力清单，
 *   也不能写入设置——任何设置改动仍然停在 pending 提案上，批准只能由用户在栖阅界面完成。
 */
function SkillsSettings({ showNotice, registerReloader }: { showNotice: (message: string) => void; registerReloader: RegisterSettingsSectionReloader }) {
  const skills = useAgentSkills()
  const reloadSkills = skills.reload

  useEffect(() => registerReloader("ai", () => { void reloadSkills() }), [registerReloader, reloadSkills])

  const toggle = async (skillId: string, enabled: boolean) => {
    const ok = await skills.setEnabled(skillId, enabled)
    // 只有权威写入成功才提示；失败由下面的错误行如实显示，不在这里说"已启用"。
    if (ok) showNotice(enabled ? "已启用内置技能；栖伴后续请求会携带相应说明" : "已停用内置技能")
  }

  return (
    <>
      <div className="mb-7 flex gap-3 rounded-3xl border border-[var(--haven-settings-primary)]/15 bg-[var(--haven-settings-primary)]/[0.06] p-5">
        <LockKeyhole className="mt-0.5 h-5 w-5 shrink-0 text-[var(--haven-settings-primary)]" />
        <div>
          <p className="text-sm font-semibold">技能不是权限</p>
          <p className="mt-1 text-xs leading-5 text-[var(--haven-settings-muted-strong)]">技能为栖伴提供任务说明，不会增加权限；设置改动会形成待批准提案，只有你在栖阅中批准后才会应用。</p>
        </div>
      </div>
      <SettingsGroup
        title="本机内置技能"
        features={["skills.builtinRuntime"]}
        description="随栖阅提供的内置技能，可启用或停用；当前不支持导入或编辑自定义技能。"
        action={skills.error === null && (
          <button
            type="button"
            aria-label="重新读取内置技能"
            onClick={() => void skills.reload()}
            disabled={skills.busy || skills.loading}
            className="shrink-0 text-[11px] font-semibold text-[var(--haven-settings-primary)] transition-colors hover:underline disabled:cursor-not-allowed disabled:opacity-50"
          >
            重新读取
          </button>
        )}
      >
        {skills.loading && (
          <SettingRow title="内置技能目录" description="正在读取本机权威状态…">
            <span className="text-[13px] text-[var(--haven-settings-muted-strong)]">读取中</span>
          </SettingRow>
        )}
        {skills.error !== null && (
          <div role="alert">
            <SettingRow title="读取或写入失败" description={skills.error.message}>
              <div className="flex items-center gap-3">
                <button type="button" onClick={() => void skills.reload()} disabled={skills.busy} className="text-[13px] font-semibold text-[var(--haven-settings-primary)] disabled:cursor-not-allowed disabled:text-[var(--haven-settings-muted-strong)]">重新读取</button>
                <button type="button" onClick={skills.dismissError} className="text-[13px] font-medium text-[var(--haven-settings-muted-strong)]">关闭</button>
              </div>
            </SettingRow>
          </div>
        )}
        {!skills.loading && skills.error === null && skills.skills !== null && skills.skills.length === 0 && (
          <SettingRow title="内置技能目录为空" description={NO_BUILTIN_SKILLS}>
            <span className="text-[13px] font-semibold text-[var(--haven-settings-muted-strong)]">目录为空</span>
          </SettingRow>
        )}
        {(skills.skills ?? []).map((skill) => (
          <SettingRow
            key={skill.skillId}
            icon={<FileText className="h-[19px] w-[19px]" strokeWidth={1.8} />}
            title={skill.skillId}
            description={skill.description}
          >
            <div className="flex items-center gap-3">
              <span className="flex items-center gap-1.5 text-[13px] font-semibold text-[var(--haven-settings-foreground)]">
                <span className={cn("h-[8px] w-[8px] rounded-full", agentSkillIsEffective(skill.state) ? "bg-[#34c759]" : skill.state === "stale" ? "bg-[#ff9500]" : "bg-[#c7ccd1]")} />
                {agentSkillStateLabel(skill.state)}
              </span>
              <span className="text-[11px] text-[var(--haven-settings-muted-strong)]">{agentSkillCharsLabel(skill.instructionsChars)}</span>
              <button
                type="button"
                onClick={() => void toggle(skill.skillId, agentSkillToggleTarget(skill.state))}
                disabled={skills.pendingSkillId !== null || skills.error !== null}
                className="inline-flex h-[36px] items-center justify-center rounded-full bg-[var(--haven-settings-primary)] px-[16px] text-[12px] font-semibold leading-none text-[var(--haven-settings-primary-foreground)] transition-colors hover:bg-[var(--haven-settings-primary-hover)] disabled:cursor-not-allowed disabled:opacity-50"
              >
                {agentSkillActionLabel(skill.state)}
              </button>
            </div>
          </SettingRow>
        ))}
      </SettingsGroup>
    </>
  )
}

/**
 * 设置页 AI 分组的真实接线（A2 AI Provider 基础切片）。
 *
 * 数据全部来自 `useAiProviderSettings`（→ gateway → Typed HavenClient → Tauri
 * command → Application）。组件不直接 invoke，也不接触任何密钥原文：
 * - API Key 只单向提交，界面只显示「已配置 / 未配置」；
 * - 模型只能来自 Provider 的模型目录；没有可用模型时显示「无可用模型」，
 *   绝不写入或推断示例模型名；
 * - 目录状态与网络错误分开呈现，可重试错误带重试入口。
 */
type AiProviderProfileDraftState = {
  mode: "create" | "edit"
  profileId: string
  displayName: string
  endpoint: string
}

function AiSettings({ showNotice, registerReloader, aiSelection }: { showNotice: (message: string) => void; registerReloader: RegisterSettingsSectionReloader; aiSelection: AiProviderSelection }) {
  // 选择由分区之上持有：本组件会随分区切换被卸载重建，选中的 Provider 不能因此丢回列表第一项。
  const ai = useAiProviderSettings({
    initialProfileId: aiSelection.profileId,
    onSelectedProfileChange: aiSelection.onSelect,
  })
  const reloadAi = ai.reload
  const [draft, setDraft] = useState<AiProviderProfileDraftState | null>(null)
  const [apiKeyDraft, setApiKeyDraft] = useState("")
  const [isKeyEditorOpen, setIsKeyEditorOpen] = useState(false)
  /**
   * 浏览器 Mock / 开发预览。
   *
   * Mock 的凭据与模型目录都是**内存 + 契约 fixture**：说"已写入系统凭据管理器"或
   * "模型来自 Provider 的模型列表"在预览里是假的，而且是很具体的那种假——用户会以为
   * 本机真的存了密钥、真的探测到了模型。因此预览路径必须逐条改口径，不能复用生产文案。
   */
  const mockMode = getHavenClientMode() === "mock"

  useEffect(() => registerReloader("ai", () => { void reloadAi() }), [registerReloader, reloadAi])

  // Provider 列表读取失败时，保留的快照只供恢复后收敛，不能继续驱动任何动作。
  const selected = ai.state.loadError === null ? ai.selectedProfile : null
  const modelOptionStrings = ai.modelOptions.map((option) => (
    option.label === option.modelId ? option.modelId : `${option.label} (${option.modelId})`
  ))
  // 三个状态必须分开显示：无可用模型（目录为空）/ 未选择（有模型但还没选）/ 已选模型。
  // 把"还没选"显示成"无可用模型"会在目录非空时谎报没有模型。
  const selectedModelValue = !ai.hasAvailableModel
    ? ai.unavailableModelLabel
    : selected?.selectedModelId
      ? modelOptionStrings.find((option) => option === selected.selectedModelId
        || option.endsWith(`(${selected.selectedModelId})`)) ?? NO_MODEL_SELECTED
      : NO_MODEL_SELECTED

  const draftProfileId = draft?.profileId ?? selected?.profileId ?? ""
  const draftDisplayName = draft?.displayName ?? selected?.displayName ?? ""
  const draftEndpoint = draft?.endpoint ?? selected?.endpoint ?? ""
  const isDirty = draft !== null
  const isCreating = draft?.mode === "create"
  const profileOptions = ai.state.profiles.map((profile) => `${profile.displayName} (${profile.profileId})`)
  const selectedProfileOption = selected
    ? `${selected.displayName} (${selected.profileId})`
    : ""

  const beginCreateProfile = () => {
    const occupied = new Set(ai.state.profiles.map((profile) => profile.profileId))
    let index = 1
    while (occupied.has(`provider-${index}`)) index += 1
    setDraft({
      mode: "create",
      profileId: `provider-${index}`,
      displayName: "自建网关",
      endpoint: "https://",
    })
  }

  const updateDraft = (patch: Partial<Pick<AiProviderProfileDraftState, "profileId" | "displayName" | "endpoint">>) => {
    setDraft((current) => ({
      mode: current?.mode ?? "edit",
      profileId: current?.profileId ?? selected?.profileId ?? "",
      displayName: current?.displayName ?? selected?.displayName ?? "",
      endpoint: current?.endpoint ?? selected?.endpoint ?? "",
      ...patch,
    }))
  }

  const submitProfile = async () => {
    const ok = await ai.saveProfile({
      profileId: draftProfileId,
      displayName: draftDisplayName,
      endpoint: draftEndpoint,
      enabled: isCreating ? true : selected?.enabled ?? true,
      selectedModelId: isCreating ? null : selected?.selectedModelId ?? null,
      createOnly: isCreating,
    })
    if (ok) {
      setDraft(null)
      showNotice(mockMode
        ? "已保存到浏览器预览（Mock/预览）：没有写入本机数据库。"
        : "AI 服务配置已保存")
    }
  }

  const submitApiKey = async () => {
    const secret = apiKeyDraft
    // 先清空本地草稿，再提交：无论成功与否，密钥原文都不留在组件状态里。
    setApiKeyDraft("")
    setIsKeyEditorOpen(false)
    const ok = await ai.submitApiKey(secret)
    // Mock 只把它记在内存里：这里若照抄生产文案，用户会以为系统凭据管理器里真的有这份密钥。
    showNotice(ok
      ? mockMode
        ? "已保存到浏览器预览的内存状态（Mock/预览）：没有写入系统凭据管理器。"
        : "API Key 已写入系统凭据管理器"
      : "API Key 写入失败，请重试")
  }

  return (
    <>
      <SettingsGroup title="API 连接" features={["ai.providerProfile", "ai.providerCredential", "ai.providerModel"]} description="当前支持 OpenAI Compatible 协议。填写服务地址与 API Key 后，栖阅直接连接你的服务。">
        {ai.state.loadError && (
          <div role="alert" className="mx-5 mb-3 flex items-center justify-between gap-3 rounded-2xl border border-[#d70015]/15 bg-[#fff1f0] px-4 py-3 text-xs text-[#d70015]">
            <span>{ai.state.loadError.message}</span>
            {ai.state.loadError.retryable && <button type="button" onClick={() => void ai.reload()} className="shrink-0 font-semibold text-[var(--haven-settings-primary)]">重试</button>}
          </div>
        )}
        {!ai.state.loading && !ai.state.loadError && ai.state.profiles.length > 0 && (
          <SettingRow title="当前服务" description={`本机已配置 ${ai.state.profiles.length} 个 AI 服务，可分别保存凭据与默认模型。`}>
            <div className="flex items-center gap-3">
              <SelectControl
                value={selectedProfileOption}
                options={profileOptions}
                onChange={(value) => {
                  const profile = ai.state.profiles.find(
                    (item) => `${item.displayName} (${item.profileId})` === value,
                  )
                  if (!profile) return
                  setDraft(null)
                  setApiKeyDraft("")
                  setIsKeyEditorOpen(false)
                  ai.selectProfile(profile.profileId)
                }}
                ariaLabel="当前 AI 服务"
                disabled={isDirty || ai.state.saving}
              />
              <button
                type="button"
                onClick={beginCreateProfile}
                disabled={isDirty || ai.state.saving}
                className="shrink-0 rounded-full px-[8px] py-[8px] text-xs font-semibold text-[var(--haven-settings-primary)] disabled:cursor-not-allowed disabled:opacity-50"
              >
                新建配置
              </button>
            </div>
          </SettingRow>
        )}
        {ai.state.loading ? (
          <SettingRow title="AI 服务配置" description="正在读取本机配置…"><span className="text-[13px] text-[var(--haven-settings-muted-strong)]">读取中</span></SettingRow>
        ) : ai.state.loadError ? (
          <SettingRow title="AI 服务配置暂不可用" description="读取失败时不会显示为空配置，也不会允许覆盖本机服务设置。">
            {ai.state.loadError.retryable && <button type="button" onClick={() => void ai.reload()} className="text-[13px] font-semibold text-[var(--haven-settings-primary)]">重试读取</button>}
          </SettingRow>
        ) : isCreating ? (
          <>
            <SettingRow title="配置 ID" description="稳定标识，只允许字母、数字、连字符与下划线。">
              <input value={draftProfileId} onChange={(event) => updateDraft({ profileId: event.target.value })} className="h-10 w-[260px] rounded-xl border border-[var(--haven-settings-border)] bg-[var(--haven-settings-control)] px-3 text-sm text-[var(--haven-settings-foreground)] outline-none focus:border-[var(--haven-settings-primary)]" aria-label="配置 ID" />
            </SettingRow>
            <SettingRow title="配置名称">
              <input value={draftDisplayName} onChange={(event) => updateDraft({ displayName: event.target.value })} className="h-10 w-[260px] rounded-xl border border-[var(--haven-settings-border)] bg-[var(--haven-settings-control)] px-3 text-sm text-[var(--haven-settings-foreground)] outline-none focus:border-[var(--haven-settings-primary)]" aria-label="配置名称" />
            </SettingRow>
            <SettingRow title="API 地址" description="必须是 https 的公网服务地址；栖阅不会通过明文连接发送 API Key 或设置内容。">
              <input value={draftEndpoint} onChange={(event) => updateDraft({ endpoint: event.target.value })} className="h-10 w-[260px] rounded-xl border border-[var(--haven-settings-border)] bg-[var(--haven-settings-control)] px-3 text-sm text-[var(--haven-settings-foreground)] outline-none focus:border-[var(--haven-settings-primary)]" aria-label="API 地址" />
            </SettingRow>
            <SettingRow title="保存配置" description="新服务默认启用；API Key 将单独配置。">
              <div className="flex items-center gap-3">
                <button type="button" onClick={() => setDraft(null)} className="text-[13px] font-medium text-[var(--haven-settings-muted-strong)]">放弃</button>
                <button type="button" onClick={() => void submitProfile()} disabled={ai.state.saving} className="inline-flex h-[36px] items-center justify-center rounded-full bg-[var(--haven-settings-primary)] px-[16px] text-[12px] font-semibold leading-none text-[var(--haven-settings-primary-foreground)] transition-colors hover:bg-[var(--haven-settings-primary-hover)] disabled:cursor-not-allowed disabled:opacity-50">保存</button>
              </div>
            </SettingRow>
          </>
        ) : selected ? (
          <>
            <SettingRow title="配置名称" description="本机标识，只用于区分多个 Provider。">
              <input
                value={draftDisplayName}
                onChange={(event) => updateDraft({ displayName: event.target.value })}
                className="h-10 w-[260px] rounded-xl border border-[var(--haven-settings-border)] bg-[var(--haven-settings-control)] px-3 text-sm text-[var(--haven-settings-foreground)] outline-none focus:border-[var(--haven-settings-primary)]"
                aria-label="配置名称"
              />
            </SettingRow>
            <SettingRow title="启用智能功能" description={ai.selectedEndpointInsecure ? "该配置使用 http 明文地址，不能启用：启停会重写同一条配置，而明文地址不允许保存。请先把地址改成 https。" : "停用后不会向该 Provider 发起任何请求。"}>
              <Toggle
                checked={selected.enabled}
                onChange={(value) => void ai.saveProfile({
                  profileId: selected.profileId,
                  displayName: selected.displayName,
                  endpoint: selected.endpoint,
                  enabled: value,
                  selectedModelId: selected.selectedModelId,
                })}
                disabled={ai.selectedEndpointInsecure}
                label="启用智能功能"
              />
            </SettingRow>
            <SettingRow title="API 协议" description="决定请求与响应的编排协议，而不是具体服务商。">
              <span className="text-[13px] font-semibold text-[var(--haven-settings-foreground)]">OpenAI Compatible</span>
            </SettingRow>
            <SettingRow title="API 地址" description="协议服务根地址（https，公网主机）。已带 /v1 时不会重复拼接；http 明文地址不能保存，也不能用于发起请求。">
              <input
                value={draftEndpoint}
                onChange={(event) => updateDraft({ endpoint: event.target.value })}
                className="h-10 w-[260px] rounded-xl border border-[var(--haven-settings-border)] bg-[var(--haven-settings-control)] px-3 text-sm text-[var(--haven-settings-foreground)] outline-none focus:border-[var(--haven-settings-primary)]"
                aria-label="API 地址"
              />
            </SettingRow>
            {ai.selectedEndpointInsecure && (
              <div role="alert" className="mx-5 mb-3 rounded-2xl border border-[#ff9500]/25 bg-[#fff8e5] px-4 py-3 text-xs leading-5 text-[#7a5a1a] dark:border-[#ff9500]/30 dark:bg-[#3a2f12] dark:text-[#f0b429]">
                {`该配置保存的是 http 明文地址，因此栖阅不会向它发起任何请求（模型列表与设置建议都会以 ${INSECURE_ENDPOINT_CODE} 失败）。把地址改成 https 并保存后即可恢复；本次没有发送任何密钥或设置内容。`}
              </div>
            )}
            {isDirty && (
              <SettingRow title="保存配置" description="只有本机配置；不包含 API Key。">
                <div className="flex items-center gap-3">
                  <button type="button" onClick={() => setDraft(null)} className="text-[13px] font-medium text-[var(--haven-settings-muted-strong)]">放弃</button>
                  <button type="button" onClick={() => void submitProfile()} disabled={ai.state.saving} className="inline-flex h-[36px] items-center justify-center rounded-full bg-[var(--haven-settings-primary)] px-[16px] text-[12px] font-semibold leading-none text-[var(--haven-settings-primary-foreground)] transition-colors hover:bg-[var(--haven-settings-primary-hover)] disabled:cursor-not-allowed disabled:opacity-50">保存</button>
                </div>
              </SettingRow>
            )}
            <SettingRow title="API Key" description={mockMode ? "Mock/预览：密钥只记在浏览器内存里，刷新即失效，也不会写入任何系统凭据存储。" : "密钥只单向写入系统凭据管理器，设置界面不显示原文，也不会回填。"}>
              <div className="flex items-center gap-[8px]">
                <span className="flex items-center gap-1.5 text-[13px] font-semibold text-[var(--haven-settings-muted-strong)]">{ai.credentialConfigured ? "已配置" : "未配置"}{ai.credentialConfigured && <CircleCheck className="h-[15px] w-[15px] text-[#34c759]" strokeWidth={2.2} />}</span>
                {isKeyEditorOpen ? (
                  <>
                    <input
                      type="password"
                      value={apiKeyDraft}
                      onChange={(event) => setApiKeyDraft(event.target.value)}
                      placeholder="粘贴 API Key"
                      aria-label="API Key"
                      autoComplete="off"
                      className="h-10 w-[200px] rounded-xl border border-[var(--haven-settings-border)] bg-[var(--haven-settings-control)] px-3 text-sm text-[var(--haven-settings-foreground)] outline-none focus:border-[var(--haven-settings-primary)]"
                    />
                    <button type="button" onClick={() => void submitApiKey()} disabled={apiKeyDraft.length === 0 || ai.state.saving} className="rounded-full px-[8px] py-[8px] text-xs font-semibold text-[var(--haven-settings-primary)] disabled:cursor-not-allowed disabled:opacity-50">保存</button>
                    <button type="button" onClick={() => { setApiKeyDraft(""); setIsKeyEditorOpen(false) }} className="rounded-full px-[8px] py-[8px] text-xs font-medium text-[var(--haven-settings-muted-strong)]">取消</button>
                  </>
                ) : (
                  <>
                    {/*
                      写入与清除是两条互相覆盖的写操作：任一条在途时两条都禁用，避免"先点保存
                      再点清除"由完成顺序决定最终状态（Hook 里另有一道互斥闸门兜底）。
                    */}
                    <button type="button" onClick={() => setIsKeyEditorOpen(true)} disabled={ai.state.saving} className="rounded-full px-[8px] py-[8px] text-xs font-semibold text-[var(--haven-settings-primary)] disabled:cursor-not-allowed disabled:opacity-50">配置</button>
                    {ai.credentialConfigured && <button type="button" onClick={() => { void ai.clearApiKey().then((ok) => showNotice(ok ? mockMode ? "已在浏览器预览的内存状态中清除（Mock/预览）：没有改动系统凭据管理器。" : "已清除 API Key" : "清除失败，请重试")) }} disabled={ai.state.saving} className="rounded-full px-[8px] py-[8px] text-xs font-medium text-[var(--haven-settings-muted-strong)] disabled:cursor-not-allowed disabled:opacity-50">清除</button>}
                  </>
                )}
              </div>
            </SettingRow>
          </>
        ) : (
          <SettingRow title="AI 服务" description="尚未配置任何 Provider。无可用模型，也不会伪造模型列表。">
            <button type="button" onClick={beginCreateProfile} className="inline-flex h-[36px] items-center justify-center rounded-full bg-[var(--haven-settings-primary)] px-[16px] text-[12px] font-semibold leading-none text-[var(--haven-settings-primary-foreground)] transition-colors hover:bg-[var(--haven-settings-primary-hover)]">新建配置</button>
          </SettingRow>
        )}
        {!isCreating && !ai.state.loadError && <SettingRow title="默认模型" description={ai.hasAvailableModel
          ? (mockMode
            ? "Mock/预览：这份模型列表来自契约 fixture，不是真实 Provider 的 /models 响应。"
            : "模型与能力都来自 Provider 的模型列表。")
          : `${ai.unavailableModelLabel}：${ai.unavailableModelHint}`}>
          <div className="flex items-center gap-4">
            <SelectControl
              value={ai.hasAvailableModel ? selectedModelValue : ai.unavailableModelLabel}
              options={ai.hasAvailableModel ? modelOptionStrings : [ai.unavailableModelLabel]}
              onChange={(value) => {
                if (!selected) return
                const modelId = ai.modelOptions.find((option) => (
                  option.modelId === value || `${option.label} (${option.modelId})` === value
                ))?.modelId ?? null
                void ai.saveProfile({
                  profileId: selected.profileId,
                  displayName: selected.displayName,
                  endpoint: selected.endpoint,
                  enabled: selected.enabled,
                  selectedModelId: modelId,
                })
              }}
              ariaLabel="默认模型"
              disabled={!ai.hasAvailableModel || ai.state.saving}
            />
            <button type="button" onClick={() => void ai.refreshModels()} disabled={!selected || ai.state.catalogLoading} className="text-[13px] font-medium text-[var(--haven-settings-primary)] transition-colors hover:text-[#005bb5] hover:underline disabled:cursor-not-allowed disabled:text-[var(--haven-settings-muted-strong)]">{ai.state.catalogLoading ? "读取中…" : "拉取模型"}</button>
          </div>
        </SettingRow>}
        {!isCreating && selected && !ai.hasAvailableModel && ai.state.catalogError && (
          <SettingRow title="模型列表" description={ai.state.catalogError.message}>
            {ai.state.catalogError.retryable
              ? <button type="button" onClick={() => void ai.refreshModels()} className="text-[13px] font-semibold text-[var(--haven-settings-primary)]">重试</button>
              : <span className="text-[13px] text-[var(--haven-settings-muted-strong)]">请检查 API 地址与 API Key</span>}
          </SettingRow>
        )}
        {!isCreating && !ai.state.loadError && <SettingRow title="识图模型" description="能力只在 Provider 显式声明时才标注；未声明的模型显示「未声明」，不会按模型名推断。">
          <div className="flex max-w-[420px] flex-col items-end gap-1">
            {ai.hasAvailableModel ? ai.modelOptions.map((option) => (
              <span key={option.modelId} className="text-[11px] text-[var(--haven-settings-muted-strong)]">{`${option.modelId} · 识图：${option.visionLabel} · 向量：${option.embeddingLabel}`}</span>
            )) : <span className="text-[13px] font-semibold text-[var(--haven-settings-muted-strong)]">{ai.unavailableModelLabel}</span>}
          </div>
        </SettingRow>}
      </SettingsGroup>
      {ai.state.saveError && (
        <div role="alert" className="mb-4 flex items-center justify-between gap-3 rounded-2xl border border-[#d70015]/15 bg-[#fff1f0] px-4 py-3 text-xs text-[#d70015]">
          <span>{ai.state.saveError.message}</span>
          <div className="flex shrink-0 items-center gap-3">
            {/*
              重试只在"重试真的会重做那件事"时出现。这里曾经把它接到「拉取模型」上：
              用户点"重试"，看到的是一次网络请求和一份新目录，而真正失败的写入仍然没有
              发生。凭据写入不可重试（密钥原文按设计不留存），因此那种失败只给"关闭"。
            */}
            {ai.canRetrySaveError && <button type="button" onClick={() => void ai.retrySaveError()} disabled={ai.state.saving} className="font-semibold text-[var(--haven-settings-primary)] disabled:cursor-not-allowed disabled:opacity-50">重试</button>}
            <button type="button" onClick={ai.dismissSaveError} className="font-medium text-[var(--haven-settings-muted-strong)]">关闭</button>
          </div>
        </div>
      )}
      <div className="flex items-center justify-end gap-3">
        {selected && !isCreating && <button type="button" onClick={() => { void ai.deleteProfile().then((ok) => { if (ok) { setDraft(null); setApiKeyDraft(""); setIsKeyEditorOpen(false) } showNotice(ok ? mockMode ? "已从浏览器预览移除该配置（Mock/预览）：没有删除本机数据库记录，也没有触碰系统凭据管理器。" : "已删除配置并清除其 API Key" : "删除失败，请重试") }) }} disabled={ai.state.saving || isDirty || ai.state.loadError !== null} className="text-[13px] font-medium text-[#d70015] disabled:cursor-not-allowed disabled:opacity-50">删除配置</button>}
        {!isCreating && <button type="button" onClick={() => void ai.refreshModels()} disabled={!selected || ai.state.catalogLoading || ai.state.loadError !== null} className="inline-flex h-[36px] items-center justify-center rounded-full bg-[#1d1d1f] px-[16px] text-[12px] font-semibold leading-none text-white transition-colors hover:bg-[#2c2c2e] disabled:cursor-not-allowed disabled:opacity-50">测试连接</button>}
      </div>
      <ExternalAgentAccessSettings showNotice={showNotice} registerReloader={registerReloader} />
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
      <SettingsGroup title="本地行为" features={["privacy.searchHistory", "privacy.playbackHistory"]}>
        <SettingRow title="搜索历史" description="关闭后不再记录新的搜索词；已有记录不会自动删除。"><Toggle checked={value.searchHistory} onChange={(checked) => form.change({ section: "privacy", searchHistory: checked })} label="搜索历史" /></SettingRow>
        <SettingRow title="播放与阅读历史" description="关闭后不再记录新的播放或阅读打开记录；已有历史不会自动删除，清除操作仍单独生效。"><Toggle checked={value.playbackHistory} onChange={(checked) => form.change({ section: "privacy", playbackHistory: checked })} label="播放与阅读历史" /></SettingRow>
      </SettingsGroup>
      <SettingsGroup title="数据操作" description="只清理明确的技术缓存或搜索词，不会删除离线资源、原始媒体、进度、标记或收藏。">
        <SettingRow title="清除搜索历史" danger><button type="button" disabled={action !== null} onClick={() => void runClearSearchHistory()} className="rounded-full px-3 py-[8px] text-xs font-semibold text-[var(--haven-settings-danger)] hover:bg-[var(--haven-settings-danger-06)] disabled:cursor-not-allowed disabled:opacity-50">{action === "search-history" ? "清除中…" : "清除"}</button></SettingRow>
        <SettingRow title="清除 Artwork 缓存" description="只删除已登记的海报/Artwork 技术缓存；下次联网请求会自动重建。" danger><button type="button" disabled={action !== null} onClick={() => void runClearArtworkCache()} className="rounded-full px-3 py-[8px] text-xs font-semibold text-[var(--haven-settings-danger)] hover:bg-[var(--haven-settings-danger-06)] disabled:cursor-not-allowed disabled:opacity-50">{action === "artwork-cache" ? "清除中…" : "清除"}</button></SettingRow>
      </SettingsGroup>
      {actionError && <div role="alert" className="mx-2 rounded-2xl border border-[var(--haven-settings-danger-15)] bg-[var(--haven-settings-danger-surface)] px-4 py-3 text-[13px] text-[var(--haven-settings-danger)]">{actionError}</div>}
    </>
  )
}

export function ErrorReportSettings() {
  const report = useErrorReport()
  const { push } = useNotice()
  const mode = getHavenClientMode()
  const [confirming, setConfirming] = useState(false)
  const busy = report.loading || report.action !== null || confirming

  const generate = async () => {
    const success = await report.generate()
    if (!success && report.error) {
      push({ kind: "error", title: "诊断报告生成失败", message: report.error.dto.userMessage, code: report.error.code, retryable: report.error.retryable, dedupeKey: "error-report-generate" })
    }
  }

  const confirm = async () => {
    setConfirming(true)
    try {
      const success = await report.confirm()
      if (success) push({ kind: "success", title: "已确认诊断报告", message: "现在可以导出报告或打开 GitHub Issue 预填页面。", dedupeKey: "error-report-confirmed" })
    } finally {
      setConfirming(false)
    }
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
    <SystemSettingsSection feature="about.diagnostics" title="诊断与反馈" description="先预览脱敏报告，确认后再导出或前往 GitHub 反馈。">
      <SystemSettingsRow title="报告等级" description="基础包含版本与错误码；标准增加协议状态；详细增加脱敏后的诊断信息。">
        <select value={report.level} disabled={busy} onChange={(event) => report.setLevel(event.target.value as typeof report.level)} className="system-settings__select" aria-label="报告等级">
          {(Object.keys(ERROR_REPORT_LEVEL_LABELS) as Array<typeof report.level>).map((value) => <option key={value} value={value}>{ERROR_REPORT_LEVEL_LABELS[value]}</option>)}
        </select>
      </SystemSettingsRow>
      <SystemSettingsRow title="生成脱敏报告" description={mode === "mock" ? "浏览器中可预览报告流程；文件导出与问题反馈请在桌面应用中进行。" : "报告保存在本机，生成后不会自动上传。"}>
        <button type="button" onClick={() => void generate()} disabled={busy} className="system-settings__button system-settings__button--primary"><FileText size={15} aria-hidden="true" />{report.loading ? "生成中…" : "生成预览"}</button>
      </SystemSettingsRow>

      {report.preview && (
        <div className="system-settings__report" aria-busy={busy}>
          <div className="system-settings__report-heading">
            <div>
              <h4>脱敏检查：{report.preview.redaction.status === "passed" ? "通过" : "未通过"}</h4>
              <p className="system-settings__description system-settings__mono">报告 ID：{report.preview.reportId}</p>
            </div>
            <span>{report.confirmed ? "已确认报告" : "等待你检查并确认"}</span>
          </div>
          <dl className="system-settings__report-meta">
            <div><dt>Haven 版本</dt><dd>{report.preview.appVersion}</dd></div>
            <div><dt>系统</dt><dd>{report.preview.operatingSystem}</dd></div>
            <div><dt>运行模式</dt><dd>{report.preview.runtimeMode}</dd></div>
          </dl>
          <p className="system-settings__report-summary">{report.preview.errorSummary}</p>
          {report.preview.details?.diagnosticLines.length ? <ul className="system-settings__report-lines system-settings__mono">{report.preview.details.diagnosticLines.map((line) => <li key={line}>{line}</li>)}</ul> : null}
          <div className="system-settings__button-group">
            <button type="button" disabled={busy} onClick={() => void copySummary()} className="system-settings__button system-settings__button--secondary"><Clipboard size={14} aria-hidden="true" />复制错误摘要</button>
            {!report.confirmed ? <button type="button" onClick={() => void confirm()} disabled={busy || report.preview.redaction.status !== "passed"} className="system-settings__button system-settings__button--primary"><Check size={14} aria-hidden="true" />{confirming ? "确认中…" : "我已检查，允许使用"}</button> : <span className="system-settings__description">已确认，可以导出或反馈</span>}
          </div>
          {report.confirmed && <div className="system-settings__report-followup system-settings__button-group"><button type="button" onClick={() => void runExport()} disabled={busy} className="system-settings__button system-settings__button--primary">{report.action === "export" ? "导出中…" : "导出诊断报告"}</button><button type="button" onClick={() => void runIssue()} disabled={busy} className="system-settings__button system-settings__button--secondary">{report.action === "issue" ? "打开中…" : "打开 GitHub Issue"}</button></div>}
          {report.actionResult && <p role="status" className="system-settings__report-result">{report.actionResult.status === "exported" ? "报告导出成功，可在 Reports 文件夹中找到。" : "Issue 预填页面已打开，请在 GitHub 页面最终提交。"}</p>}
        </div>
      )}
      {report.error && <div role="alert" className="system-settings__error"><TriangleAlert size={17} aria-hidden="true" /><span>{report.error.dto.userMessage}</span>{report.error.retryable && <button type="button" onClick={retry} disabled={busy} className="system-settings__button system-settings__button--secondary">重试</button>}</div>}
    </SystemSettingsSection>
  )
}
