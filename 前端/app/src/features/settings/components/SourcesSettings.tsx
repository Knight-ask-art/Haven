import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import type { ReactNode } from "react"
import { Check, Info, LockKeyhole, MoreHorizontal, Plus, Search, Settings2, TriangleAlert } from "lucide-react"
import { cn } from "@/lib/utils"
import { toHavenError, type HavenError } from "@/lib/ipc/errors"
import { useNotice } from "@/app/notice-center/notice-context"
import { addSource, isComicLibrarySourceId, isCustomSourceId, isFeedSourceId, isKavitaSourceId, isTvboxSourceId, listSources, removeSource, setSourceCredential, setSourceEnabled, setSourceEndpoint, updateSource, SOURCE_HEALTH_LABELS, type SourceAddKind, type SourceDescriptorWire } from "../ipc/sources-gateway"
import type { SourceCategoryDto, SourceKindDto, SourceRegistryDto } from "@/lib/ipc/generated/wire"
import { SOURCE_CATEGORY_LABELS, SOURCE_CATEGORY_ORDER, SOURCE_MODE_LABELS, sourceUsesConfiguredEndpoint } from "../lib/source-catalog"
import { TvboxConfigImporter } from "./TvboxConfigImporter"
import { SettingsDialog } from "./SettingsDialog"
import "./sources-settings.css"

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
    <div className="flex flex-col gap-3 border-t border-[var(--haven-settings-border-subtle)] px-5 py-[16px]">
      <div className="flex flex-col gap-[8px] border-t border-[var(--haven-settings-border-subtle)] pt-3 sm:flex-row sm:items-center">
        <label className="min-w-0 flex-1 text-xs text-[var(--haven-settings-muted)]" htmlFor={`endpoint-${source.sourceId}`}>
          用户配置采集接口地址（http/https，例如 https://host/api.php/provide/vod）
          <input
            id={`endpoint-${source.sourceId}`}
            value={endpoint}
            onChange={(event) => setEndpoint(event.target.value)}
            placeholder="https://…/api.php/provide/vod"
            className="mt-1 w-full rounded-xl border border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-card)] px-3 py-[8px] font-mono text-xs text-[var(--haven-settings-foreground)] outline-none focus:border-[var(--haven-settings-primary)]"
            autoComplete="off"
            spellCheck={false}
          />
        </label>
        <button
          type="button"
          disabled={saving}
          onClick={() => { void save() }}
          className="shrink-0 self-start rounded-full bg-[var(--haven-settings-primary)] px-[16px] py-[8px] text-xs font-semibold text-[var(--haven-settings-primary-foreground)] disabled:opacity-50 sm:self-center"
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
    <div className="flex flex-col gap-[8px] border-t border-[var(--haven-settings-border-subtle)] px-5 py-[16px]">
      <p className="text-xs font-semibold text-[var(--haven-settings-foreground)]">M3U 播放列表地址</p>
      <p className="text-[11px] leading-5 text-[var(--haven-settings-muted)]">填入以 http:// 或 https:// 开头的播放列表地址。保存后可按频道名称搜索；播放地址不会显示在搜索结果中。</p>
      <div className="flex flex-col gap-[8px] sm:flex-row sm:items-center">
        <input
          value={endpoint}
          onChange={(event) => { setEndpoint(event.target.value) }}
          placeholder="https://example.org/playlist.m3u"
          maxLength={500}
          autoComplete="off"
          spellCheck={false}
          className="min-w-0 flex-1 rounded-xl border border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-card)] px-3 py-[8px] font-mono text-xs text-[var(--haven-settings-foreground)] outline-none focus:border-[var(--haven-settings-primary)]"
          aria-label="M3U 播放列表地址"
        />
        <button
          type="button"
          disabled={saving}
          onClick={() => { void save() }}
          className="shrink-0 self-start rounded-full bg-[var(--haven-settings-primary)] px-[16px] py-[8px] text-xs font-semibold text-[var(--haven-settings-primary-foreground)] disabled:opacity-50 sm:self-auto"
        >
          {saving ? "保存中…" : "保存地址"}
        </button>
      </div>
    </div>
  )
}

export function SourcesSettings({ showNotice }: { showNotice: (message: string) => void }) {
  const [registry, setRegistry] = useState<SourceRegistryDto | null>(null)
  const [isLoading, setIsLoading] = useState(true)
  const [loadError, setLoadError] = useState<HavenError | null>(null)
  const [togglingId, setTogglingId] = useState<string | null>(null)
  const [category, setCategory] = useState<SourceCategoryDto | "all">("all")
  const [sourceQuery, setSourceQuery] = useState("")
  const [addMenuOpen, setAddMenuOpen] = useState(false)
  const [customAddRequest, setCustomAddRequest] = useState<SourceAddKind | null>(null)
  const [tvboxOpenRequest, setTvboxOpenRequest] = useState(0)
  const addMenuRef = useRef<HTMLDivElement>(null)
  const loadEpochRef = useRef(0)

  const load = useCallback(async () => {
    const epoch = ++loadEpochRef.current
    setIsLoading(true)
    setLoadError(null)
    try {
      const next = await listSources()
      if (epoch === loadEpochRef.current) setRegistry(next)
    } catch (error) {
      if (epoch === loadEpochRef.current) setLoadError(toHavenError(error))
    } finally {
      if (epoch === loadEpochRef.current) setIsLoading(false)
    }
  }, [])

  useEffect(() => {
    void load()
    return () => { loadEpochRef.current += 1 }
  }, [load])

  useEffect(() => {
    if (!addMenuOpen) return undefined
    const close = (event: PointerEvent) => {
      if (!addMenuRef.current?.contains(event.target as Node)) setAddMenuOpen(false)
    }
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setAddMenuOpen(false)
    }
    document.addEventListener("pointerdown", close)
    document.addEventListener("keydown", closeOnEscape)
    return () => {
      document.removeEventListener("pointerdown", close)
      document.removeEventListener("keydown", closeOnEscape)
    }
  }, [addMenuOpen])

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

  const normalizedQuery = sourceQuery.trim().toLocaleLowerCase()
  const visibleSources = useMemo(() => (registry?.sources ?? []).filter((source) => (
    (category === "all" || source.categories.includes(category))
    && (normalizedQuery.length === 0 || source.displayName.toLocaleLowerCase().includes(normalizedQuery))
  )), [category, normalizedQuery, registry])
  const builtinSources = visibleSources.filter((source) => !isCustomSourceId(source.sourceId))

  const beginAdding = (kind: SourceAddKind | "tvbox") => {
    setAddMenuOpen(false)
    // 菜单项马上卸载；先回到稳定触发器，窗口关闭后才有可恢复的焦点。
    addMenuRef.current?.querySelector<HTMLButtonElement>("button")?.focus()
    if (kind === "tvbox") {
      setTvboxOpenRequest((request) => request + 1)
      return
    }
    setCustomAddRequest(kind)
  }

  return (
    <div className="sources-settings" data-settings-features="sources.registry sources.feedSubscription sources.comicLibrary" tabIndex={-1}>
      <header className="mb-6 flex flex-wrap items-end justify-between gap-[16px]">
        <div>
          <h1 className="text-[28px] font-bold tracking-[-0.025em] text-[var(--haven-settings-foreground)]">来源</h1>
          <p className="mt-1 text-[12px] leading-5 text-[var(--haven-settings-muted-strong)]">管理已接入的书库、订阅、漫画库与影视服务。</p>
        </div>
        <div ref={addMenuRef} className="relative">
          <button
            type="button"
            disabled={registry === null || loadError !== null}
            aria-haspopup="menu"
            aria-expanded={addMenuOpen}
            onClick={() => setAddMenuOpen((open) => !open)}
            className="inline-flex h-[40px] items-center gap-[8px] rounded-[12px] bg-[var(--haven-settings-primary)] px-5 text-[13px] font-semibold text-[var(--haven-settings-primary-foreground)] transition-colors enabled:hover:bg-[var(--haven-settings-primary-hover)]"
          >
            <Plus className="h-[15px] w-[15px]" strokeWidth={2} />
            添加来源
          </button>
          {addMenuOpen && (
            <div role="menu" aria-label="选择来源类型" className="absolute right-0 top-12 z-40 w-[240px] overflow-hidden rounded-[14px] border border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-card)] p-1.5 shadow-[0_12px_32px_rgba(0,0,0,0.14)]">
              <button type="button" role="menuitem" onClick={() => beginAdding("tvbox")} className="flex w-full flex-col rounded-[10px] px-3 py-2.5 text-left hover:bg-[var(--haven-settings-card-hover)]">
                <span className="text-[12px] font-semibold text-[var(--haven-settings-foreground)]">TVBox / FongMi</span>
                <span className="mt-0.5 text-[10px] text-[var(--haven-settings-muted)]">影视配置 URL · 测试后导入</span>
              </button>
              <div className="my-1 h-px bg-[var(--haven-settings-control)]" />
              <button type="button" role="menuitem" onClick={() => beginAdding("opds")} className="flex w-full flex-col rounded-[10px] px-3 py-2.5 text-left hover:bg-[var(--haven-settings-card-hover)]">
                <span className="text-[12px] font-semibold text-[var(--haven-settings-foreground)]">OPDS 书库</span>
                <span className="mt-0.5 text-[10px] text-[var(--haven-settings-muted)]">图书目录与自托管书库</span>
              </button>
              <button type="button" role="menuitem" onClick={() => beginAdding("komga")} className="flex w-full flex-col rounded-[10px] px-3 py-2.5 text-left hover:bg-[var(--haven-settings-card-hover)]">
                <span className="text-[12px] font-semibold text-[var(--haven-settings-foreground)]">Komga 漫画库</span>
                <span className="mt-0.5 text-[10px] text-[var(--haven-settings-muted)]">自托管漫画服务</span>
              </button>
              <button type="button" role="menuitem" onClick={() => beginAdding("kavita")} className="flex w-full flex-col rounded-[10px] px-3 py-2.5 text-left hover:bg-[var(--haven-settings-card-hover)]">
                <span className="text-[12px] font-semibold text-[var(--haven-settings-foreground)]">Kavita 漫画库</span>
                <span className="mt-0.5 text-[10px] text-[var(--haven-settings-muted)]">自托管漫画服务</span>
              </button>
              <button type="button" role="menuitem" onClick={() => beginAdding("feed")} className="flex w-full flex-col rounded-[10px] px-3 py-2.5 text-left hover:bg-[var(--haven-settings-card-hover)]">
                <span className="text-[12px] font-semibold text-[var(--haven-settings-foreground)]">RSS / Atom 订阅</span>
                <span className="mt-0.5 text-[10px] text-[var(--haven-settings-muted)]">报刊与文章来源</span>
              </button>
            </div>
          )}
        </div>
      </header>

      <div className="mb-3 flex flex-col gap-3 xl:flex-row xl:items-center xl:justify-between">
        <div id="source-category-tabs" role="tablist" aria-label="来源类型" className="flex flex-wrap items-center gap-[8px]">
          <button
            type="button"
            role="tab"
            aria-selected={category === "all"}
            aria-controls="source-catalog"
            onClick={() => setCategory("all")}
            className={cn("h-[36px] min-w-[56px] rounded-[12px] border px-[16px] text-[12px] font-semibold transition-colors", category === "all" ? "border-[#292720] bg-[var(--haven-settings-primary)] text-[var(--haven-settings-primary-foreground)]" : "border-[var(--haven-settings-control-border)] bg-[var(--haven-settings-card)] text-[var(--haven-settings-foreground)] hover:bg-[var(--haven-settings-card-hover)]")}
          >全部</button>
          {SOURCE_CATEGORY_ORDER.map((item) => (
            <button
              key={item}
              type="button"
              role="tab"
              aria-selected={category === item}
              aria-controls="source-catalog"
              onClick={() => setCategory(item)}
              className={cn("h-[36px] min-w-[72px] rounded-[12px] border px-[16px] text-[12px] font-semibold transition-colors", category === item ? "border-[#292720] bg-[var(--haven-settings-primary)] text-[var(--haven-settings-primary-foreground)]" : "border-[var(--haven-settings-control-border)] bg-[var(--haven-settings-card)] text-[var(--haven-settings-foreground)] hover:bg-[var(--haven-settings-card-hover)]")}
            >{SOURCE_CATEGORY_LABELS[item]}</button>
          ))}
        </div>
        <label className="flex h-[40px] w-full items-center gap-2.5 rounded-[12px] border border-[var(--haven-settings-control-border)] bg-[var(--haven-settings-card)] px-3.5 focus-within:border-[var(--haven-settings-primary)] focus-within:ring-4 focus-within:ring-[var(--haven-settings-primary-10)] xl:max-w-[280px]">
          <Search className="h-[15px] w-[15px] shrink-0 text-[var(--haven-settings-muted)]" strokeWidth={1.8} />
          <input
            type="search"
            aria-label="搜索来源"
            placeholder="搜索来源名称"
            value={sourceQuery}
            onChange={(event) => setSourceQuery(event.target.value)}
            className="min-w-0 flex-1 bg-transparent text-[12px] text-[var(--haven-settings-foreground)] outline-none placeholder:text-[var(--haven-settings-muted)]"
          />
        </label>
      </div>

      <div id="source-catalog" className="overflow-visible rounded-[16px] border border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-card)] shadow-[0_3px_12px_rgba(0,0,0,0.025)]">
        <div className="source-catalog-header hidden grid-cols-[minmax(0,2fr)_minmax(88px,0.65fr)_minmax(110px,1.4fr)_minmax(88px,0.85fr)_76px] items-center gap-[16px] border-b border-[var(--haven-settings-border-subtle)] px-[32px] py-3 text-[10px] font-medium text-[var(--haven-settings-muted-strong)] md:grid">
          <span>来源</span><span>内容类型</span><span>可用方式</span><span>状态</span><span className="text-right">操作</span>
        </div>
        {isLoading && <div role="status" className="flex items-center gap-3 px-6 py-[16px] text-[12px] text-[var(--haven-settings-muted)]"><span className="h-[7px] w-[7px] animate-pulse rounded-full bg-[var(--haven-settings-primary)]" />{registry ? "正在更新来源目录…" : "正在读取来源目录…"}</div>}
        {loadError && <div className="flex flex-col gap-3 px-6 py-6"><p role="alert" className="text-[12px] font-medium text-[var(--haven-settings-danger)]">{loadError.dto.userMessage}</p><button type="button" onClick={() => { void load() }} className="self-start rounded-[10px] border border-[var(--haven-settings-border-subtle)] px-3 py-[8px] text-[11px] font-semibold text-[var(--haven-settings-foreground)]">重新读取来源</button></div>}
        {registry && (
          <>
            {builtinSources.map((source) => (
              <BuiltinSourceRow
                key={source.sourceId}
                source={source}
                toggling={isLoading || loadError !== null || togglingId === source.sourceId}
                onToggle={() => toggleSource(source)}
                showNotice={showNotice}
                onChanged={() => { void load() }}
              />
            ))}
            <CustomSourceManager
              registry={registry}
              category={category}
              searchQuery={sourceQuery}
              requestedAddKind={customAddRequest}
              unavailable={isLoading || loadError !== null}
              onAddRequestConsumed={() => setCustomAddRequest(null)}
              showEmptyState={false}
              showNotice={showNotice}
              onChanged={() => { void load() }}
            />
            <TvboxConfigImporter
              registry={registry}
              category={category}
              filterQuery={sourceQuery}
              openRequest={tvboxOpenRequest}
              showEmptyState={false}
              hideAddTrigger
              renderSourceRow={(source) => (
                <SourceCatalogRow
                  source={source}
                  origin="外部 · TVBox / FongMi"
                  capabilityLabel="已导入配置 · 影视搜索与播放未接入"
                  statusLabel="暂不可用"
                  statusHint="当前版本不支持启用"
                  controls={<span className="rounded-full bg-[var(--haven-settings-control)] px-2.5 py-1 text-[10px] font-medium text-[var(--haven-settings-muted)]">只读</span>}
                />
              )}
              showNotice={showNotice}
              onChanged={() => { void load() }}
            />
            {visibleSources.length === 0 && (
              <p className="px-6 py-[32px] text-center text-[12px] text-[var(--haven-settings-muted)]">当前筛选下没有匹配的来源。</p>
            )}
          </>
        )}
      </div>

      {registry && !isLoading && !loadError && (
        <footer className="mt-3 flex items-center justify-between gap-[16px] px-1">
          <span className="text-[10px] text-[var(--haven-settings-muted)]">{visibleSources.length} 个来源</span>
          <button
            type="button"
            onClick={() => {
              setCategory("all")
              setSourceQuery("")
              document.querySelector<HTMLElement>("[data-source-origin='builtin']")?.scrollIntoView?.({ behavior: "smooth", block: "center" })
            }}
            className="rounded-[12px] border border-[var(--haven-settings-control-border)] bg-[var(--haven-settings-card)] px-[16px] py-[8px] text-[11px] font-semibold text-[var(--haven-settings-foreground)] transition-colors hover:bg-[var(--haven-settings-card-hover)]"
          >浏览内置来源</button>
        </footer>
      )}
    </div>
  )
}

const SOURCE_METHOD_LABELS: Record<SourceKindDto, string> = {
  search: "搜索",
  online_read: "阅读",
  offline_download: "下载",
}

function SourceEnableSwitch({ source, busy, onToggle }: { source: SourceDescriptorWire; busy: boolean; onToggle: () => void }) {
  return (
    <button
      type="button"
      role="switch"
      aria-label={`${source.displayName} 启用`}
      aria-checked={source.enabled}
      disabled={busy}
      onClick={onToggle}
      className={cn("relative h-[24px] w-[42px] shrink-0 rounded-full border p-[2px] transition-colors disabled:cursor-not-allowed disabled:opacity-60", source.enabled ? "border-[var(--haven-settings-primary)] bg-[var(--haven-settings-primary)]" : "border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-toggle-off)] enabled:hover:bg-[var(--haven-settings-toggle-off-hover)]")}
    >
      <span className={cn("block h-[18px] w-[18px] rounded-full shadow-sm transition-transform", source.enabled ? "translate-x-[18px] bg-[var(--haven-settings-toggle-thumb-on)]" : "translate-x-0 bg-[var(--haven-settings-toggle-thumb)]")} />
    </button>
  )
}

function SourceCatalogRow({
  source,
  origin,
  capabilityLabel,
  statusLabel,
  statusHint,
  controls,
}: {
  source: SourceDescriptorWire
  origin: string
  capabilityLabel?: string
  statusLabel: string
  statusHint: string
  controls: ReactNode
}) {
  const sourceMark = source.categories[0] === "video" ? "影"
    : source.categories[0] === "comic" ? "漫"
      : source.categories[0] === "periodical" ? "文"
        : source.categories.length > 1 ? "源" : "书"
  const methodLabels = source.kinds.map((kind) => (
    kind === "online_read" && source.categories.includes("video") ? "播放" : SOURCE_METHOD_LABELS[kind]
  ))
  return (
    <div
      data-source-row="true"
      data-source-origin={origin.startsWith("内置") ? "builtin" : "external"}
      data-source-id={source.sourceId}
      className="grid min-h-[72px] grid-cols-1 items-center gap-[8px] px-5 py-3 md:grid-cols-[minmax(0,2fr)_minmax(88px,0.65fr)_minmax(110px,1.4fr)_minmax(88px,0.85fr)_76px] md:gap-[16px] md:px-[32px]"
    >
      <div className="flex min-w-0 items-center gap-3">
        <span className="flex h-[36px] w-[36px] shrink-0 items-center justify-center rounded-[12px] bg-[var(--haven-settings-control)] text-[12px] font-semibold text-[var(--haven-settings-primary)]">{sourceMark}</span>
        <div className="min-w-0">
          <p className="truncate text-[13px] font-semibold leading-[22px] text-[var(--haven-settings-foreground)]">{source.displayName}</p>
          <p className="truncate text-[11px] leading-[18px] text-[var(--haven-settings-muted)]">{origin} · {SOURCE_MODE_LABELS[source.mode]}</p>
        </div>
      </div>
      <div className="flex items-center gap-[8px] md:block">
        <span className="text-[10px] text-[var(--haven-settings-muted)] md:hidden">内容类型</span>
        <span className="text-[12px] font-medium text-[var(--haven-settings-foreground)]">{source.categories.map((item) => SOURCE_CATEGORY_LABELS[item]).join("、") || "未分类"}</span>
      </div>
      <div className="flex items-center gap-[8px] md:block">
        <span className="text-[10px] text-[var(--haven-settings-muted)] md:hidden">可用方式</span>
        <span className="text-[11px] leading-5 text-[var(--haven-settings-muted-strong)]">{capabilityLabel ?? (methodLabels.length > 0 ? methodLabels.join(" · ") : "当前未接入")}</span>
      </div>
      <div className="flex items-center gap-[8px] md:block">
        <span className="text-[10px] text-[var(--haven-settings-muted)] md:hidden">状态</span>
        <p className="text-[12px] font-semibold leading-[18px] text-[var(--haven-settings-foreground)]">{statusLabel}</p>
        <p className="text-[10px] leading-[16px] text-[var(--haven-settings-muted)]">{statusHint}</p>
      </div>
      <div className="flex items-center justify-end gap-[8px] md:gap-1">{controls}</div>
    </div>
  )
}

function BuiltinSourceRow({
  source,
  toggling,
  onToggle,
  showNotice,
  onChanged,
}: {
  source: SourceDescriptorWire
  toggling: boolean
  onToggle: () => void
  showNotice: (message: string) => void
  onChanged: () => void
}) {
  const healthLabel = SOURCE_HEALTH_LABELS[source.health] ?? "未检测"
  const [detailsOpen, setDetailsOpen] = useState(false)
  const [endpointOpen, setEndpointOpen] = useState(false)
  const supportsEndpoint = sourceUsesConfiguredEndpoint(source.sourceId)
  return (
    <div className="border-b border-[var(--haven-settings-border-subtle)] last:border-b-0">
      <SourceCatalogRow
        source={source}
        origin={`内置 · ${source.displayName}`}
        statusLabel={source.enabled ? "已启用" : "已停用"}
        statusHint={supportsEndpoint && !source.endpointConfigured ? "待配置接口" : source.health === "unknown" ? "未检测" : healthLabel}
        controls={(
          <>
            <SourceEnableSwitch source={source} busy={toggling} onToggle={onToggle} />
            <SourceActionMenu
              source={source}
              detailsOpen={detailsOpen}
              endpointOpen={endpointOpen}
              busy={toggling}
              allowToggle={false}
              onToggle={onToggle}
              onToggleDetails={() => setDetailsOpen((current) => !current)}
              onToggleEndpoint={() => setEndpointOpen((current) => !current)}
            />
          </>
        )}
      />
      {detailsOpen && <p className="px-5 pb-3 pl-[68px] text-[11px] leading-5 text-[var(--haven-settings-muted)] md:px-[32px] md:pl-[80px]">维护备注：{source.notes}</p>}
      {supportsEndpoint && (source.sourceId === "cms10" || source.sourceId === "m3u") && endpointOpen && (
        <div className="px-5 pb-[16px] md:px-[32px]">
          {source.sourceId === "cms10"
            ? <Cms10EndpointRow source={source} showNotice={showNotice} onChanged={onChanged} />
            : <M3uEndpointRow source={source} showNotice={showNotice} onChanged={onChanged} />}
        </div>
      )}
    </div>
  )
}

function SourceActionMenu({
  source,
  detailsOpen,
  endpointOpen,
  variant = "builtin",
  busy = false,
  allowToggle = true,
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
  allowToggle?: boolean
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
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setOpen(false)
        menuRef.current?.querySelector<HTMLButtonElement>("button")?.focus()
      }
    }
    document.addEventListener("pointerdown", close)
    document.addEventListener("keydown", escape)
    return () => {
      document.removeEventListener("pointerdown", close)
      document.removeEventListener("keydown", escape)
    }
  }, [open])

  const select = (action: () => void) => {
    menuRef.current?.querySelector<HTMLButtonElement>("button")?.focus()
    action()
    setOpen(false)
  }

  return (
    <div ref={menuRef} className="relative">
      <button type="button" disabled={busy} aria-label={`${source.displayName} 更多操作`} aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen((current) => !current)} className={cn("flex h-[28px] w-[28px] items-center justify-center rounded-lg text-[var(--haven-settings-muted)] transition-colors hover:bg-[var(--haven-settings-card-hover)] hover:text-[var(--haven-settings-foreground)] disabled:opacity-50", open && "bg-[var(--haven-settings-control)] text-[var(--haven-settings-foreground)]")}>
        <MoreHorizontal className="h-[16px] w-[16px]" strokeWidth={2} />
      </button>
      {open && (
        <div role="menu" className="absolute right-0 top-9 z-30 min-w-[164px] overflow-hidden rounded-xl border border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-card)] p-1 shadow-[0_10px_28px_rgba(0,0,0,0.14)]">
          {allowToggle && <button type="button" role="menuitem" disabled={busy} onClick={() => select(onToggle)} className="flex w-full items-center gap-[8px] rounded-lg px-2.5 py-[8px] text-left text-xs text-[var(--haven-settings-foreground)] hover:bg-[var(--haven-settings-card-hover)] disabled:cursor-not-allowed disabled:opacity-50"><Check className="h-[14px] w-[14px] shrink-0 text-[var(--haven-settings-primary)]" />{busy ? "处理中…" : source.enabled ? "停用来源" : "启用来源"}</button>}
          {variant === "builtin" ? (
            <>
              <button type="button" role="menuitem" onClick={() => select(() => onToggleDetails?.())} className="flex w-full items-center gap-[8px] rounded-lg px-2.5 py-[8px] text-left text-xs text-[var(--haven-settings-foreground)] hover:bg-[var(--haven-settings-card-hover)]"><Info className="h-[14px] w-[14px] text-[var(--haven-settings-primary)]" />{detailsOpen ? "收起维护说明" : "查看维护说明"}</button>
              {(source.sourceId === "cms10" || source.sourceId === "m3u") && <button type="button" role="menuitem" onClick={() => select(() => onToggleEndpoint?.())} className="flex w-full items-center gap-[8px] rounded-lg px-2.5 py-[8px] text-left text-xs text-[var(--haven-settings-foreground)] hover:bg-[var(--haven-settings-card-hover)]"><Settings2 className="h-[14px] w-[14px] text-[var(--haven-settings-primary)]" />{endpointOpen ? "收起接口配置" : "配置接口"}</button>}
            </>
          ) : (
            <>
              {onEdit && <button type="button" role="menuitem" onClick={() => select(onEdit)} className="flex w-full items-center gap-[8px] rounded-lg px-2.5 py-[8px] text-left text-xs text-[var(--haven-settings-foreground)] hover:bg-[var(--haven-settings-card-hover)]"><Settings2 className="h-[14px] w-[14px] text-[var(--haven-settings-primary)]" />编辑来源</button>}
              {onCredential && <button type="button" role="menuitem" onClick={() => select(onCredential)} className="flex w-full items-center gap-[8px] rounded-lg px-2.5 py-[8px] text-left text-xs text-[var(--haven-settings-foreground)] hover:bg-[var(--haven-settings-card-hover)]"><LockKeyhole className="h-[14px] w-[14px] text-[var(--haven-settings-primary)]" />配置访问凭据</button>}
              {onRemove && <>
                <div className="my-1 h-px bg-[var(--haven-settings-control)]" aria-hidden="true" />
                <button type="button" role="menuitem" onClick={() => select(onRemove)} className="flex w-full items-center gap-[8px] rounded-lg px-2.5 py-[8px] text-left text-xs text-[var(--haven-settings-danger)] hover:bg-[var(--haven-settings-danger)]/[0.08]"><TriangleAlert className="h-[14px] w-[14px]" />删除来源</button>
              </>}
            </>
          )}
        </div>
      )}
    </div>
  )
}

/**
 * 自定义来源管理（V2-H 收尾批次；Task 1 增补 RSS/Atom 订阅源）：
 * 添加 / 编辑 / 删除，以及 OPDS 书源的凭据录入。
 *
 * 订阅源（`custom_feed_`）没有受控的 HTTP 认证路径，因此隐藏凭据入口；
 * 端点必须是 HTTPS 且不能带查询串（后端会拒绝，见 用户配置参考.md）。
 */
function CustomSourceManager({
  registry,
  category,
  searchQuery,
  requestedAddKind,
  unavailable = false,
  onAddRequestConsumed,
  showEmptyState = true,
  showNotice,
  onChanged,
}: {
  registry: SourceRegistryDto | null
  category: SourceCategoryDto | "all"
  searchQuery: string
  requestedAddKind: SourceAddKind | null
  unavailable?: boolean
  onAddRequestConsumed: () => void
  showEmptyState?: boolean
  showNotice: (message: string) => void
  onChanged: () => void
}) {
  const { confirm } = useNotice()
  const [adding, setAdding] = useState(false)
  const [addStep, setAddStep] = useState<"details" | "credential" | "done">("details")
  const [addKind, setAddKind] = useState<SourceAddKind>("opds")
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
  const [credentialError, setCredentialError] = useState<string | null>(null)
  const [customTogglingId, setCustomTogglingId] = useState<string | null>(null)

  useEffect(() => {
    if (requestedAddKind === null) return
    setAddKind(requestedAddKind)
    setAddStep("details")
    setDisplayName("")
    setEndpoint("")
    setAddError(null)
    setCreatedSourceId(null)
    setCredentialValue("")
    setCredentialConfigured(false)
    setCredentialError(null)
    setAdding(true)
    onAddRequestConsumed()
  }, [onAddRequestConsumed, requestedAddKind])

  const normalizedSearch = searchQuery.trim().toLocaleLowerCase()
  const customSources = (registry?.sources ?? [])
    .filter((s) => isCustomSourceId(s.sourceId))
    // TVBox 配置来源同属 `custom_` 家族，但它默认停用且后端拒绝启用，因此不归这里管：
    // 它由下方的 TvboxConfigImporter 只读展示（没有启用开关、没有编辑/凭据入口）。
    .filter((s) => !isTvboxSourceId(s.sourceId))
    .filter((s) => category === "all" || s.categories.includes(category))
    .filter((s) => normalizedSearch.length === 0 || s.displayName.toLocaleLowerCase().includes(normalizedSearch))

  /** 添加流程的每一步文案随种类变化：订阅源没有凭据步骤。 */
  const addSteps = addKind === "feed" ? ["填写信息"] : ["填写信息", "配置凭据"]

  /** 自托管漫画库：端点必须 HTTPS 且不带查询串，凭据是 API key。 */
  const comicLibraryKind = addKind === "komga" || addKind === "kavita"
  const comicLibraryLabel = addKind === "kavita" ? "Kavita" : "Komga"
  const nameHint = addKind === "feed"
    ? "给自己看的名字，例如“科技新闻订阅”。"
    : comicLibraryKind
      ? `给自己看的名字，例如“家里的 ${comicLibraryLabel}”。`
      : "给自己看的名字，例如“我的 Calibre 书库”。"
  const namePlaceholder = addKind === "feed" ? "科技新闻订阅" : comicLibraryKind ? `我的 ${comicLibraryLabel}` : "我的 Calibre 书库"
  const endpointLabel = addKind === "feed" ? "订阅源地址" : comicLibraryKind ? `${comicLibraryLabel} 地址` : "OPDS 目录地址"
  const endpointHint = addKind === "feed"
    ? "这是订阅源自身的 RSS/Atom 地址，不是文章页面；必须是 https:// 开头，且不能带 ?查询串（带访问令牌的地址当前不受支持）。"
    : comicLibraryKind
      ? `这是 ${comicLibraryLabel} 的访问地址（例如 https://comics.example.org），不是浏览器里某一本书的页面；必须使用 https://，且不能带 ?查询串。API key 会通过请求头发送，不需要写进地址。`
      : "这是书库的 OPDS/目录地址，不是浏览器首页；必须以 http:// 或 https:// 开头。"
  const endpointPlaceholder = addKind === "feed" ? "https://example.org/feed.xml" : comicLibraryKind ? "https://comics.example.org" : "https://example.org/opds/"

  const resetAddForm = () => {
    setAdding(false)
    setAddStep("details")
    setAddKind("opds")
    setDisplayName("")
    setEndpoint("")
    setCreatedSourceId(null)
    setAddError(null)
    setCredentialValue("")
    setCredentialConfigured(false)
    setCredentialError(null)
  }

  const submitAdd = async () => {
    if (submitting) return
    setSubmitting(true)
    try {
      const result = await addSource({ displayName, endpoint, kind: addKind })
      setCreatedSourceId(result.sourceId)
      setAddError(null)
      if (addKind === "feed") {
        // 订阅源不使用系统凭据；直接给出完成态，不展示一个不会被消费的凭据表单。
        setAddStep("done")
        showNotice("订阅源已添加，确认无误后可在列表中启用")
      } else {
        setAddStep("credential")
        showNotice("来源已添加，可以现在配置凭据，也可以稍后再配")
      }
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

  const submitCredential = async (sourceId: string, secret = credentialValue) => {
    if (submitting) return
    setSubmitting(true)
    setCredentialError(null)
    try {
      await setSourceCredential({
        sourceId,
        secret: secret.length > 0 ? secret : null,
      })
      setCredentialConfigured(secret.length > 0)
      setCredentialValue("")
      showNotice(secret.length > 0 ? "凭据已保存到系统凭据管理器" : "凭据已清除")
      if (adding) setAddStep("done")
      onChanged()
    } catch (error) {
      const message = toHavenError(error).dto.userMessage
      setCredentialError(message)
      showNotice(message)
    } finally {
      setSubmitting(false)
    }
  }

  if (adding) {
    return (
      <SettingsDialog title="添加来源" description="新来源默认停用。可以添加多个书库、订阅或漫画服务，确认配置后再启用。" onClose={resetAddForm} busy={submitting}>
        <div className="mt-[16px] flex items-center gap-[8px]" aria-label="添加来源步骤">
          {addSteps.map((label, index) => {
            const stepIndex = addStep === "details" ? 0 : 1
            return <span key={label} className={cn("rounded-full px-3 py-1 text-[11px] font-semibold", index <= stepIndex ? "bg-[var(--haven-settings-primary-12)] text-[var(--haven-settings-primary)]" : "bg-[var(--haven-settings-control)] text-[var(--haven-settings-muted)]")}>{index + 1}. {label}</span>
          })}
        </div>
        {addStep === "details" && (
          <div className="mt-5 flex flex-col gap-[16px]">
            <div className="flex flex-col gap-1.5">
              <span className="text-xs font-semibold">来源种类</span>
              <div role="group" aria-label="来源种类" className="grid grid-cols-1 gap-[8px] sm:grid-cols-2">
                {([
                  { kind: "opds" as const, label: "OPDS 书库", hint: "图书目录，可配置访问凭据" },
                  { kind: "feed" as const, label: "RSS/Atom 订阅源", hint: "订阅文章，只读取订阅源自身内容" },
                  { kind: "komga" as const, label: "Komga 漫画库", hint: "自托管漫画库，用 API key 访问" },
                  { kind: "kavita" as const, label: "Kavita 漫画库", hint: "自托管漫画库，用 API key 访问" },
                ]).map((option) => (
                  <button
                    key={option.kind}
                    type="button"
                    disabled={submitting}
                    aria-pressed={addKind === option.kind}
                    onClick={() => { setAddKind(option.kind); setAddError(null) }}
                    className={cn(
                      "flex-1 rounded-2xl border px-[16px] py-2.5 text-left text-xs",
                      addKind === option.kind
                        ? "border-[var(--haven-settings-primary)] bg-[var(--haven-settings-primary-12)] text-[var(--haven-settings-foreground)]"
                        : "border-[var(--haven-settings-control-border)] bg-[var(--haven-settings-card)] text-[var(--haven-settings-muted-strong)]",
                    )}
                  >
                    <span className="block font-semibold">{option.label}</span>
                    <span className="mt-0.5 block text-[11px] leading-4 text-[var(--haven-settings-muted)]">{option.hint}</span>
                  </button>
                ))}
              </div>
            </div>
            <label className="flex flex-col gap-1.5">
              <span className="text-xs font-semibold">来源名称</span>
              <span className="text-[11px] text-[var(--haven-settings-muted)]">{nameHint}</span>
              <input type="text" disabled={submitting} value={displayName} onChange={(e) => { setDisplayName(e.target.value); setAddError(null) }} maxLength={100} placeholder={namePlaceholder} autoComplete="off" className="rounded-2xl border border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-card)] px-[16px] py-2.5 text-sm outline-none focus:border-[var(--haven-settings-primary)]" />
            </label>
            <label className="flex flex-col gap-1.5">
              <span className="text-xs font-semibold">{endpointLabel}</span>
              <span className="text-[11px] leading-5 text-[var(--haven-settings-muted)]">
                {endpointHint}
              </span>
              <input type="url" disabled={submitting} value={endpoint} onChange={(e) => { setEndpoint(e.target.value); setAddError(null) }} maxLength={500} placeholder={endpointPlaceholder} autoComplete="url" spellCheck={false} className="rounded-2xl border border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-card)] px-[16px] py-2.5 text-sm outline-none focus:border-[var(--haven-settings-primary)]" />
            </label>
            {addError && <p role="alert" className="rounded-xl bg-[var(--haven-settings-danger-surface)] px-3 py-[8px] text-xs text-[var(--haven-settings-danger)]">{addError}</p>}
            <div className="flex justify-end gap-[8px] text-xs font-semibold">
              <button type="button" disabled={submitting || displayName.trim().length === 0 || endpoint.trim().length === 0} onClick={() => { void submitAdd() }} className="rounded-full bg-[var(--haven-settings-primary)] px-[16px] py-[8px] text-[var(--haven-settings-primary-foreground)] disabled:opacity-50">{submitting ? "添加中…" : "继续"}</button>
            </div>
          </div>
        )}
        {addStep === "credential" && createdSourceId && (
          <div className="mt-5 flex flex-col gap-[16px]">
            <div className="rounded-2xl bg-[var(--haven-settings-control)] p-[16px]">
              <p className="text-sm font-semibold">{comicLibraryKind ? "需要 API key 吗？" : "需要登录吗？"}</p>
              <p className="mt-1 text-xs leading-5 text-[var(--haven-settings-muted-strong)]">
                {comicLibraryKind
                  ? `如果 ${comicLibraryLabel} 打开了认证，可以现在填写 API key；它只保存到系统凭据管理器，并作为请求头发送，不会出现在地址、来源列表或日志中。`
                  : "如果书库需要密码，可以现在填写；凭据只保存到系统凭据管理器，不会出现在来源列表或日志中。"}
              </p>
            </div>
            <label className="flex flex-col gap-1.5">
              <span className="text-xs font-semibold">{comicLibraryKind ? "API key（可选）" : "访问凭据（可选）"}</span>
              <input type="password" disabled={submitting} value={credentialValue} onChange={(e) => { setCredentialValue(e.target.value); setCredentialError(null) }} autoComplete="new-password" placeholder={comicLibraryKind ? "不需要认证可留空" : "不需要登录可留空"} className="rounded-2xl border border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-card)] px-[16px] py-2.5 text-sm outline-none focus:border-[var(--haven-settings-primary)]" />
            </label>
            {credentialError && <p role="alert" className="text-xs text-[var(--haven-settings-danger)]">{credentialError}</p>}
            <div className="flex justify-end gap-[8px] text-xs font-semibold">
              <button type="button" disabled={submitting} onClick={() => { setCredentialValue(""); setAddStep("done") }} className="rounded-full border border-[var(--haven-settings-border-subtle)] px-[16px] py-[8px]">稍后配置</button>
              <button type="button" disabled={submitting || credentialValue.length === 0} onClick={() => { void submitCredential(createdSourceId) }} className="rounded-full bg-[var(--haven-settings-primary)] px-[16px] py-[8px] text-[var(--haven-settings-primary-foreground)] disabled:opacity-50">{submitting ? "保存中…" : "保存并完成"}</button>
            </div>
          </div>
        )}
        {addStep === "done" && (
          <div className="mt-5 flex flex-col gap-[16px]">
            <div className="rounded-2xl border border-[#34a853]/20 bg-[var(--haven-settings-card)] p-[16px]">
              <p className="text-sm font-semibold text-[var(--haven-settings-primary)]">来源已准备好</p>
              <p className="mt-1 text-xs leading-5 text-[var(--haven-settings-primary)]">{displayName} · 单一来源 · {addKind === "feed" ? "RSS/Atom 订阅源（不使用访问凭据）" : comicLibraryKind ? `${comicLibraryLabel} 漫画库${credentialConfigured ? " · API key 已配置" : " · API key 稍后配置"}` : `OPDS 目录${credentialConfigured ? " · 凭据已配置" : " · 凭据稍后配置"}`}</p>
            </div>
            <div className="flex justify-end"><button type="button" onClick={resetAddForm} className="rounded-full bg-[var(--haven-settings-primary)] px-[16px] py-[8px] text-xs font-semibold text-[var(--haven-settings-primary-foreground)]">返回来源列表</button></div>
          </div>
        )}
      </SettingsDialog>
    )
  }

  return (
    <>
      {showEmptyState && registry && customSources.length === 0 && (
        <p className="mb-3 rounded-2xl bg-[var(--haven-settings-control)] px-[16px] py-3 text-xs leading-5 text-[var(--haven-settings-muted)]">
          {category === "all"
            ? "还没有添加自定义来源；可以添加 OPDS 书库、RSS/Atom 订阅源或 Komga/Kavita 漫画库。"
            : category === "periodical"
              ? "还没有添加 RSS/Atom 订阅源。"
              : category === "comic"
                ? "还没有添加 Komga/Kavita 漫画库。"
                : "当前筛选下没有自定义来源；自定义 OPDS 书库归入图书，订阅源归入报刊文章，漫画库归入漫画。"}
        </p>
      )}
      {customSources.map((source) => {
        const editing = editingId === source.sourceId
        const feed = isFeedSourceId(source.sourceId)
        const comic = isComicLibrarySourceId(source.sourceId)
        const comicLabel = isKavitaSourceId(source.sourceId) ? "Kavita" : "Komga"
        const origin = feed ? "自定义 · RSS / Atom" : comic ? `自定义 · ${comicLabel}` : "自定义 · OPDS"
        const statusHint = source.health === "unknown"
          ? comic
            ? `API key ${source.credentialConfigured ? "已配置" : "未配置"}`
            : !feed && !source.credentialConfigured ? "访问凭据未配置" : "未检测"
          : SOURCE_HEALTH_LABELS[source.health] ?? "未检测"
        return (
          <div key={source.sourceId} className="border-b border-[var(--haven-settings-border-subtle)] last:border-b-0">
            <SourceCatalogRow
              source={source}
              origin={origin}
              statusLabel={source.enabled ? "已启用" : "已停用"}
              statusHint={statusHint}
              controls={(
                <>
                  <SourceEnableSwitch
                    source={source}
                    busy={unavailable || submitting || customTogglingId === source.sourceId}
                    onToggle={() => { void toggleCustomSource(source) }}
                  />
                  <SourceActionMenu
                    source={source}
                    variant="custom"
                    detailsOpen={false}
                    endpointOpen={false}
                    busy={unavailable || submitting || customTogglingId === source.sourceId}
                    allowToggle={false}
                    onToggle={() => { void toggleCustomSource(source) }}
                    onEdit={() => {
                      setEditingId(source.sourceId)
                      setEditName(source.displayName)
                      setEditEndpoint("")
                    }}
                    onCredential={feed ? undefined : () => {
                      setCredentialFor(source.sourceId)
                      setCredentialValue("")
                      setCredentialConfigured(source.credentialConfigured)
                      setCredentialError(null)
                    }}
                    onRemove={() => { void submitRemove(source.sourceId) }}
                  />
                </>
              )}
            />
            {(editing || credentialFor === source.sourceId) && (
              <div className="space-y-3 border-t border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-control)] px-5 py-[16px] md:px-[32px] md:pl-[80px]">
                {editing && (
                  <div className="flex flex-col gap-3">
                    <input
                      type="text"
                      disabled={submitting}
                      value={editName}
                      onChange={(e) => { setEditName(e.target.value) }}
                      maxLength={100}
                      aria-label="显示名"
                      className="rounded-2xl border border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-card)] px-[16px] py-[8px] text-sm text-[var(--haven-settings-foreground)] outline-none focus:border-[var(--haven-settings-primary)]"
                    />
                    <input
                      type="url"
                      disabled={submitting}
                      value={editEndpoint}
                      onChange={(e) => { setEditEndpoint(e.target.value) }}
                      maxLength={500}
                      placeholder={feed ? "留空表示地址不变；填入新的 HTTPS 地址覆盖（不能带查询串）" : comic ? "留空表示地址不变；填入新的 HTTPS 地址覆盖（不能带查询串）" : "留空表示端点不变；填入新地址覆盖"}
                      aria-label={feed ? "订阅源地址" : comic ? `${comicLabel} 地址` : "OPDS 端点地址"}
                      className="rounded-2xl border border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-card)] px-[16px] py-[8px] text-sm text-[var(--haven-settings-foreground)] outline-none focus:border-[var(--haven-settings-primary)]"
                    />
                    <div className="flex justify-end gap-[8px] text-xs font-semibold">
                      <button type="button" disabled={submitting} onClick={() => { setEditingId(null) }} className="rounded-full border border-[var(--haven-settings-border-subtle)] px-[16px] py-[8px] text-[var(--haven-settings-foreground)]">取消</button>
                      <button
                        type="button"
                        disabled={submitting || (editName.trim().length === 0 && editEndpoint.trim().length === 0)}
                        onClick={() => { void submitEdit(source.sourceId) }}
                        className="rounded-full bg-[var(--haven-settings-primary)] px-[16px] py-[8px] text-[var(--haven-settings-primary-foreground)] disabled:opacity-50"
                      >
                        保存
                      </button>
                    </div>
                  </div>
                )}
                {credentialFor === source.sourceId ? (
                  <div className="flex flex-col gap-[8px]">
                    <label className="flex flex-col gap-1">
                      <span className="text-xs text-[var(--haven-settings-muted)]">
                        {comic
                          ? "API key（仅写入系统凭据管理器，不回显、不落库；作为请求头发送，不进入地址）"
                          : "访问密码（仅写入系统凭据管理器，不回显、不落库）"}
                      </span>
                      <input
                        type="password"
                        disabled={submitting}
                        value={credentialValue}
                        onChange={(e) => { setCredentialValue(e.target.value); setCredentialError(null) }}
                        autoComplete="new-password"
                        className="rounded-2xl border border-[var(--haven-settings-border-subtle)] bg-[var(--haven-settings-card)] px-[16px] py-[8px] text-sm text-[var(--haven-settings-foreground)] outline-none focus:border-[var(--haven-settings-primary)]"
                      />
                    </label>
                    {credentialError && <p role="alert" className="text-xs text-[var(--haven-settings-danger)]">{credentialError}</p>}
                    <div className="flex justify-end gap-[8px] text-xs font-semibold">
                      <button
                        type="button"
                        disabled={submitting}
                        onClick={() => {
                          setCredentialFor(null)
                          setCredentialValue("")
                          setCredentialConfigured(false)
                          setCredentialError(null)
                        }}
                        className="rounded-full border border-[var(--haven-settings-border-subtle)] px-[14px] py-[6px] text-[var(--haven-settings-foreground)]"
                      >
                        收起
                      </button>
                      <button
                        type="button"
                        disabled={submitting || credentialValue.length === 0}
                        onClick={() => { void submitCredential(source.sourceId) }}
                        className="rounded-full bg-[var(--haven-settings-primary)] px-[14px] py-[6px] text-[var(--haven-settings-primary-foreground)] disabled:opacity-50"
                      >
                        保存凭据
                      </button>
                      {credentialConfigured && (
                        <button
                          type="button"
                          disabled={submitting}
                          onClick={() => {
                            setCredentialValue("")
                            void submitCredential(source.sourceId, "")
                          }}
                          className="rounded-full px-[14px] py-[6px] text-[var(--haven-settings-danger)]"
                        >
                          清除凭据
                        </button>
                      )}
                    </div>
                  </div>
                ) : null}
              </div>
            )}
          </div>
        )
      })}
    </>
  )
}
