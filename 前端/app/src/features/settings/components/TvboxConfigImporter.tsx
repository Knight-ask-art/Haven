// TVBox / FongMi 配置导入（Film/TV Provider 基础切片的前端交互）。
//
// 边界与后端一致，页面只做三件事：取回配置 → 展示形态摘要 → 登记来源。
// - 预览成功之前，"保存并导入"不可用；地址一改，上一次摘要立即失效，必须重新测试。
// - 摘要只渲染聚合计数与形态分类，不渲染配置地址、原始 JSON、站点端点或第三方脚本内容。
// - 登记后的来源默认停用，且本版本后端拒绝启用（没有已实现的影视搜索/播放路径），
//   所以这里不提供启用开关，只如实说明当前不可用。

import { useEffect, useRef, useState } from "react"
import type { ReactNode } from "react"
import { Clapperboard, CircleCheck, TriangleAlert } from "lucide-react"
import { cn } from "@/lib/utils"
import { toHavenError } from "@/lib/ipc/errors"
import type {
  SourceCategoryDto,
  SourceRegistryDto,
  TvboxConfigImplementationKindDto,
  TvboxConfigPreviewDto,
  TvboxConfigSaveResult,
} from "@/lib/ipc/generated/wire"
import {
  isTvboxSourceId,
  previewTvboxConfig,
  saveTvboxConfig,
  type SourceDescriptorWire,
} from "../ipc/sources-gateway"
import { sourceMatchesCategory } from "../lib/source-catalog"
import { SettingsDialog } from "./SettingsDialog"

/** 第三方实现种类的展示名（只标记，不下载、不加载、不执行）。 */
const TVBOX_IMPLEMENTATION_LABELS: Record<TvboxConfigImplementationKindDto, string> = {
  jar: "JAR 包",
  javascript: "JavaScript",
  python: "Python",
  json_manifest: "JSON 清单",
  unidentified: "未识别",
}

type SummaryRow = { label: string; value: string }

/**
 * 形态摘要 → 展示行。
 *
 * 只消费 `TvboxConfigPreviewDto` 里已有的聚合字段；零值与空集合不占版面，避免把
 * "没有这一项" 渲染成一屏 0。
 */
function tvboxSummaryRows(facts: TvboxConfigPreviewDto): SummaryRow[] {
  const rows: SummaryRow[] = [
    {
      label: "站点",
      value: `${facts.siteCount} 个（HTTP 端点 ${facts.httpEndpointSiteCount} · spider ${facts.spiderSiteCount} · 未归类 ${facts.unclassifiedSiteCount}）`,
    },
    { label: "直播源", value: `${facts.liveCount} 个` },
    { label: "解析器", value: `${facts.parserCount} 个` },
    {
      label: "第三方实现",
      value:
        facts.spiderConfigured && facts.spiderKind !== null
          ? `${TVBOX_IMPLEMENTATION_LABELS[facts.spiderKind]}${facts.spiderHasIntegrityDigest ? "（带完整性摘要）" : ""} · 仅标记，不下载也不执行`
          : "配置未声明 spider",
    },
  ]
  if (facts.skippedSiteRows + facts.skippedLiveRows + facts.skippedParserRows > 0) {
    rows.push({
      label: "跳过行",
      value: `站点 ${facts.skippedSiteRows} · 直播 ${facts.skippedLiveRows} · 解析器 ${facts.skippedParserRows}`,
    })
  }
  if (facts.opaqueTopLevelFieldNames.length > 0) {
    rows.push({
      label: "已识别但未结构化",
      value: facts.opaqueTopLevelFieldNames.join("、"),
    })
  }
  if (facts.unrecognizedTopLevelFieldCount > 0) {
    rows.push({
      label: "未识别顶层字段",
      value: `${facts.unrecognizedTopLevelFieldCount} 项（名称与取值都不展示）`,
    })
  }
  if (facts.withheldTopLevelFieldCount > 0) {
    rows.push({
      label: "因数量上限未保留",
      value: `${facts.withheldTopLevelFieldCount} 个顶层字段`,
    })
  }
  return rows
}

function TvboxPreviewSummary({ facts }: { facts: TvboxConfigPreviewDto }) {
  return (
    <div className="rounded-2xl bg-[var(--haven-settings-control)] p-[16px]" aria-label="配置摘要">
      <p className="text-xs font-semibold text-[var(--haven-settings-foreground)]">
        配置摘要（只统计形态，不展示配置内容）
      </p>
      <dl className="mt-3 grid gap-2.5 sm:grid-cols-2">
        {tvboxSummaryRows(facts).map((row) => (
          <div key={row.label} className="min-w-0">
            <dt className="text-[11px] text-[var(--haven-settings-muted)]">{row.label}</dt>
            <dd className="mt-0.5 text-xs font-semibold text-[var(--haven-settings-foreground)]">{row.value}</dd>
          </div>
        ))}
      </dl>
    </div>
  )
}

/** 已导入的 TVBox 来源：只读一行，没有启用开关（后端拒绝启用这类来源）。 */
function TvboxSourceRow({ source }: { source: SourceDescriptorWire }) {
  return (
    <div className="relative mb-3 overflow-hidden rounded-[16px] border border-[var(--haven-settings-border)] bg-[var(--haven-settings-card)]">
      <span className="absolute inset-y-0 left-0 w-0.5 bg-[#af52de] opacity-90" aria-hidden="true" />
      <div className="flex items-start gap-3 px-[16px] py-3.5">
        <span className="mt-0.5 flex h-[32px] w-[32px] shrink-0 items-center justify-center rounded-xl border border-[var(--haven-settings-border)] bg-[var(--haven-settings-card)] text-[#af52de]">
          <Clapperboard className="h-[16px] w-[16px]" strokeWidth={1.8} />
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 flex-wrap items-center gap-1.5">
            <p className="min-w-0 truncate text-[13px] font-semibold tracking-[-0.01em] text-[var(--haven-settings-foreground)]">{source.displayName}</p>
            <span className="rounded-md bg-black/[0.05] px-1.5 py-0.5 text-[9px] font-semibold text-[var(--haven-settings-muted-strong)]">影视</span>
            <span className="rounded-md bg-black/[0.05] px-1.5 py-0.5 text-[9px] font-semibold text-[var(--haven-settings-muted-strong)]">单一来源</span>
          </div>
          <p className="mt-1 text-[11px] leading-5 text-[var(--haven-settings-muted)]">
            影视 · 单一来源 · TVBox / FongMi 配置
          </p>
          <p className="mt-1 text-[11px] leading-5 text-[var(--haven-settings-muted)]">
            {source.enabled
              ? "已启用（与当前版本的约定不符，请反馈）"
              : "已停用。本版本尚未提供影视搜索与播放能力，暂不支持启用，因此没有启用开关。"}
          </p>
        </div>
      </div>
    </div>
  )
}

export type TvboxConfigImporterProps = {
  registry: SourceRegistryDto | null
  category: SourceCategoryDto | "all"
  filterQuery?: string
  /** 递增时由来源页顶部的统一添加菜单打开导入表单。 */
  openRequest?: number
  hideAddTrigger?: boolean
  showEmptyState?: boolean
  /** 让生产目录复用统一表格行；独立预览/测试仍可用默认行。 */
  renderSourceRow?: (source: SourceDescriptorWire) => ReactNode
  showNotice: (message: string) => void
  /** 保存成功后调用，用来刷新来源注册表（新来源要出现在列表里）。 */
  onChanged: () => void
}

export function TvboxConfigImporter({
  registry,
  category,
  filterQuery = "",
  openRequest,
  hideAddTrigger = false,
  showEmptyState = true,
  renderSourceRow,
  showNotice,
  onChanged,
}: TvboxConfigImporterProps) {
  const [open, setOpen] = useState(false)
  const [displayName, setDisplayName] = useState("")
  const [url, setUrl] = useState("")
  // 摘要与它对应的地址一起存：地址改了，摘要就作废，保存只能建立在"当前地址预览成功"之上。
  const [preview, setPreview] = useState<{ url: string; facts: TvboxConfigPreviewDto } | null>(null)
  const [saved, setSaved] = useState<{ displayName: string; result: TvboxConfigSaveResult } | null>(null)
  const [busy, setBusy] = useState<"idle" | "previewing" | "saving">("idle")
  const [error, setError] = useState<string | null>(null)
  const lastOpenRequest = useRef(openRequest ?? 0)

  const trimmedUrl = url.trim()
  const previewReady = preview !== null && preview.url === trimmedUrl && trimmedUrl.length > 0
  const canPreview = trimmedUrl.length > 0 && busy === "idle"
  const canSave = previewReady && displayName.trim().length > 0 && busy === "idle"

  useEffect(() => {
    if (openRequest === undefined || openRequest <= lastOpenRequest.current) return
    lastOpenRequest.current = openRequest
    setDisplayName("")
    setUrl("")
    setPreview(null)
    setSaved(null)
    setError(null)
    setOpen(true)
  }, [openRequest])

  const normalizedFilter = filterQuery.trim().toLocaleLowerCase()
  const tvboxSources = (registry?.sources ?? [])
    .filter((source) => isTvboxSourceId(source.sourceId))
    .filter((source) => sourceMatchesCategory(source, category))
    .filter((source) => normalizedFilter.length === 0 || source.displayName.toLocaleLowerCase().includes(normalizedFilter))

  const resetForm = () => {
    setOpen(false)
    setDisplayName("")
    setUrl("")
    setPreview(null)
    setError(null)
  }

  const submitPreview = async () => {
    if (!canPreview) return
    setBusy("previewing")
    setError(null)
    try {
      const facts = await previewTvboxConfig({ url: trimmedUrl })
      setPreview({ url: trimmedUrl, facts })
    } catch (caught) {
      // 失败即清空上一次摘要：留着它会让"保存"看起来仍然可用。
      setPreview(null)
      setError(toHavenError(caught).dto.userMessage)
    } finally {
      setBusy("idle")
    }
  }

  const submitSave = async () => {
    if (!canSave || preview === null) return
    const name = displayName.trim()
    setBusy("saving")
    setError(null)
    try {
      const result = await saveTvboxConfig({ displayName: name, url: preview.url })
      setSaved({ displayName: name, result })
      setPreview(null)
      setDisplayName("")
      setUrl("")
      setOpen(false)
      showNotice("TVBox 配置已导入；来源保持停用，搜索与播放尚不可用")
      onChanged()
    } catch (caught) {
      setError(toHavenError(caught).dto.userMessage)
    } finally {
      setBusy("idle")
    }
  }

  return (
    <>
      {tvboxSources.map((source) => renderSourceRow
        ? <div key={source.sourceId} className="border-b border-[var(--haven-settings-border-subtle)] last:border-b-0">{renderSourceRow(source)}</div>
        : <TvboxSourceRow key={source.sourceId} source={source} />)}
      {showEmptyState && registry && tvboxSources.length === 0 && (category === "all" || category === "video") && (
        <p className="mb-3 rounded-2xl bg-black/[0.025] px-[16px] py-3 text-xs leading-5 text-[var(--haven-settings-muted)]">
          还没有导入 TVBox / FongMi 配置。
        </p>
      )}

      {saved && !hideAddTrigger && (
        <div className="mb-3 rounded-3xl border border-[var(--haven-settings-border)] bg-[var(--haven-settings-card)] px-5 py-5">
          <div className="flex items-start gap-3">
            <span className="mt-0.5 flex h-[32px] w-[32px] shrink-0 items-center justify-center rounded-xl bg-[#34a853]/[0.12] text-[#216e32]">
              <CircleCheck className="h-[16px] w-[16px]" />
            </span>
            <div className="min-w-0">
              <p className="text-sm font-semibold text-[#216e32]">配置已保存，来源保持停用</p>
              <p className="mt-1 text-xs leading-5 text-[#4f7659]">
                {saved.displayName} 已登记（{saved.result.sourceId}）。本版本尚未提供影视搜索与播放能力，该来源不会参与搜索，也不提供启用开关。
              </p>
            </div>
          </div>
          <div className="mt-[16px]">
            <TvboxPreviewSummary facts={saved.result.preview} />
          </div>
        </div>
      )}

      {open ? (
        <SettingsDialog title="导入 TVBox / FongMi 配置" description="先测试配置地址，确认摘要后再保存。只导入配置，不下载或执行第三方脚本。" onClose={resetForm} busy={busy !== "idle"}>
          <div className="mt-5 flex flex-col gap-[16px]">
            <label className="flex flex-col gap-1.5">
              <span className="text-xs font-semibold">来源名称</span>
              <span className="text-[11px] text-[var(--haven-settings-muted)]">给自己看的名字，例如“客厅电视源”。</span>
              <input
                type="text"
                disabled={busy !== "idle"}
                aria-label="来源名称"
                value={displayName}
                onChange={(event) => { setDisplayName(event.target.value); setError(null) }}
                maxLength={100}
                placeholder="客厅电视源"
                autoComplete="off"
                className="rounded-2xl border border-[var(--haven-settings-border)] bg-[var(--haven-settings-card)] px-[16px] py-2.5 text-sm outline-none focus:border-[var(--haven-settings-primary)]"
              />
            </label>
            <label className="flex flex-col gap-1.5">
              <span className="text-xs font-semibold">配置地址</span>
              <span className="text-[11px] leading-5 text-[var(--haven-settings-muted)]">
                TVBox / FongMi 的 JSON 配置地址。地址仅用于配置请求，不在摘要或来源列表回显。
              </span>
              <input
                type="url"
                disabled={busy !== "idle"}
                aria-label="配置地址"
                value={url}
                onChange={(event) => {
                  setUrl(event.target.value)
                  // 地址一变，上一次的摘要不再代表这份配置。
                  setPreview(null)
                  setError(null)
                }}
                maxLength={2048}
                placeholder="https://example.org/tvbox.json"
                autoComplete="url"
                spellCheck={false}
                className="rounded-2xl border border-[var(--haven-settings-border)] bg-[var(--haven-settings-card)] px-[16px] py-2.5 text-sm outline-none focus:border-[var(--haven-settings-primary)]"
              />
            </label>
            {error && (
              <p role="alert" className="flex items-start gap-[8px] rounded-xl bg-[var(--haven-settings-danger-surface)] px-3 py-[8px] text-xs text-[var(--haven-settings-danger)]">
                <TriangleAlert className="mt-0.5 h-[14px] w-[14px] shrink-0" />
                {error}
              </p>
            )}
            {previewReady && preview && <TvboxPreviewSummary facts={preview.facts} />}
            <div className="flex flex-col items-end gap-[8px]">
              <div className="flex justify-end gap-[8px] text-xs font-semibold">
                <button
                  type="button"
                  disabled={!canPreview}
                  onClick={() => { void submitPreview() }}
                  className={cn("rounded-full border border-[var(--haven-settings-border)] px-[16px] py-[8px]", busy === "idle" ? "text-[var(--haven-settings-foreground)]" : "text-[var(--haven-settings-muted)]", "disabled:opacity-50")}
                >
                  {busy === "previewing" ? "测试中…" : "测试配置"}
                </button>
                <button
                  type="button"
                  disabled={!canSave}
                  onClick={() => { void submitSave() }}
                  className="rounded-full bg-[var(--haven-settings-primary)] px-[16px] py-[8px] text-[var(--haven-settings-primary-foreground)] disabled:opacity-50"
                >
                  {busy === "saving" ? "保存中…" : "保存并导入"}
                </button>
              </div>
              {!previewReady && (
                <p className="text-[11px] text-[var(--haven-settings-muted)]">需要先通过一次成功的测试，才能保存。</p>
              )}
            </div>
          </div>
        </SettingsDialog>
      ) : !hideAddTrigger ? (
        <div className="flex items-center justify-between rounded-3xl border border-dashed border-[var(--haven-settings-border)] bg-[var(--haven-settings-card)] px-5 py-[16px]">
          <div>
            <p className="text-sm font-semibold text-[var(--haven-settings-foreground)]">导入 TVBox / FongMi 配置</p>
            <p className="mt-1 text-xs text-[var(--haven-settings-muted)]">填入显示名与配置地址，先测试再保存；导入后的来源保持停用。</p>
          </div>
          <button
            type="button"
            onClick={() => { setOpen(true); setSaved(null) }}
            className="rounded-full bg-[var(--haven-settings-primary)] px-[16px] py-[8px] text-xs font-semibold text-[var(--haven-settings-primary-foreground)]"
          >
            导入配置
          </button>
        </div>
      ) : null}
    </>
  )
}
