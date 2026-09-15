import { useCallback, useEffect, useRef, useState } from "react"
import type { ReactNode } from "react"
import { ArrowLeft, BookOpen, CircleAlert, FileText, Film, LoaderCircle, Play } from "lucide-react"
import { useNavigate, useParams } from "react-router"
import type { EditionDetailDto, MediaItemSummaryDto } from "@/lib/ipc/generated/wire"
import { HavenError } from "@/lib/ipc/errors"
import { getHavenClientMode } from "@/lib/ipc/runtime"
import {
  createDownloadForMediaItem,
  getMediaItemDownloadInfo,
  getMediaItemsDownloadInfo,
  subscribeDownloadEvents,
  type MediaItemDownloadInfo,
} from "@/features/downloads/ipc/download-gateway"
import { getEdition, normalizeEditionError } from "../ipc/edition-gateway"
import { primaryActionRoute } from "../lib/primary-action-route"
import {
  primaryActionPresentation,
  resolveMediaItemCapability,
  type MediaItemCapabilityPresentation,
} from "../lib/periodical-presentation"

type ItemCapability = {
  info: MediaItemDownloadInfo | null
  failed: boolean
}

function itemMeta(item: MediaItemSummaryDto): string {
  if (item.durationMs != null) return `${Math.max(1, Math.round(item.durationMs / 60_000))} 分钟`
  if (item.pageCount != null) return `${item.pageCount} 页`
  if (item.chapterCount != null) return `${item.chapterCount} 章`
  return item.indexLabel
}

function itemIcon(item: MediaItemSummaryDto) {
  if (item.mediaType === "movie" || item.mediaType === "series" || item.mediaType === "episode") return <Film className="h-5 w-5" />
  if (item.mediaType === "comic") return <BookOpen className="h-5 w-5" />
  if (item.mediaType === "article" || item.mediaType === "document") return <FileText className="h-5 w-5" />
  return <Play className="h-5 w-5" />
}

export function EditionDetailPage() {
  const { editionId } = useParams<{ editionId?: string }>()
  const navigate = useNavigate()
  const mode = getHavenClientMode()
  const [detail, setDetail] = useState<EditionDetailDto | null>(null)
  const [loading, setLoading] = useState(mode === "tauri")
  const [error, setError] = useState<HavenError | null>(null)
  const requestSequence = useRef(0)
  const capabilityGeneration = useRef(0)
  const capabilityRequests = useRef(new Map<string, number>())
  const capabilitiesRef = useRef<Record<string, ItemCapability>>({})
  const [capabilities, setCapabilities] = useState<Record<string, ItemCapability>>({})
  const [downloadPending, setDownloadPending] = useState<Record<string, boolean>>({})
  const [downloadErrors, setDownloadErrors] = useState<Record<string, string>>({})

  const load = useCallback(async () => {
    if (!editionId || mode !== "tauri") {
      setLoading(false)
      return
    }
    const requestId = ++requestSequence.current
    setLoading(true)
    setError(null)
    try {
      const next = await getEdition(editionId)
      if (requestSequence.current === requestId) setDetail(next)
    } catch (cause) {
      if (requestSequence.current === requestId) setError(normalizeEditionError(cause))
    } finally {
      if (requestSequence.current === requestId) setLoading(false)
    }
  }, [editionId, mode])

  useEffect(() => {
    void load()
    return () => { requestSequence.current += 1 }
  }, [load])

  useEffect(() => {
    capabilitiesRef.current = capabilities
  }, [capabilities])

  useEffect(() => {
    const generation = ++capabilityGeneration.current
    capabilityRequests.current.clear()
    setDownloadPending({})
    setDownloadErrors({})
    if (!detail || detail.items.length === 0) {
      setCapabilities({})
      return
    }

    const ids = detail.items.map((item) => item.mediaItemId)
    const initial: Record<string, ItemCapability> = {}
    for (const id of ids) initial[id] = { info: null, failed: false }
    capabilitiesRef.current = initial
    setCapabilities(initial)

    void getMediaItemsDownloadInfo(ids)
      .then((projected) => {
        if (capabilityGeneration.current !== generation) return
        const next: Record<string, ItemCapability> = {}
        for (const id of ids) {
          next[id] = { info: projected.get(id) ?? null, failed: false }
        }
        capabilitiesRef.current = next
        setCapabilities(next)
      })
      .catch(() => {
        if (capabilityGeneration.current !== generation) return
        const next: Record<string, ItemCapability> = {}
        for (const id of ids) next[id] = { info: null, failed: true }
        capabilitiesRef.current = next
        setCapabilities(next)
      })
  }, [detail])

  const refreshItemCapability = useCallback(async (mediaItemId: string, generation = capabilityGeneration.current) => {
    const requestId = (capabilityRequests.current.get(mediaItemId) ?? 0) + 1
    capabilityRequests.current.set(mediaItemId, requestId)
    setCapabilities((current) => ({ ...current, [mediaItemId]: { info: null, failed: false } }))
    try {
      const info = await getMediaItemDownloadInfo(mediaItemId)
      if (
        capabilityGeneration.current !== generation
        || capabilityRequests.current.get(mediaItemId) !== requestId
      ) return info
      setCapabilities((current) => ({ ...current, [mediaItemId]: { info, failed: false } }))
      return info
    } catch {
      if (
        capabilityGeneration.current !== generation
        || capabilityRequests.current.get(mediaItemId) !== requestId
      ) return null
      setCapabilities((current) => ({ ...current, [mediaItemId]: { info: null, failed: true } }))
      return null
    }
  }, [])

  useEffect(() => {
    if (!detail || detail.items.length === 0) return
    const generation = capabilityGeneration.current
    let mounted = true
    let cleanup: (() => Promise<void>) | null = null
    void subscribeDownloadEvents((event) => {
      if (!mounted || capabilityGeneration.current !== generation) return
      const item = detail.items.find((candidate) => (
        capabilitiesRef.current[candidate.mediaItemId]?.info?.taskId === event.data.taskId
      ))
      if (item) void refreshItemCapability(item.mediaItemId, generation)
    }).then((dispose) => {
      if (!mounted) void dispose().catch(() => undefined)
      else cleanup = dispose
    }).catch(() => undefined)
    return () => {
      mounted = false
      if (cleanup) void cleanup().catch(() => undefined)
    }
  }, [detail, refreshItemCapability])

  const handleDownload = useCallback(async (item: MediaItemSummaryDto) => {
    const generation = capabilityGeneration.current
    const current = capabilitiesRef.current[item.mediaItemId]?.info
    if (!current?.canDownload) {
      await refreshItemCapability(item.mediaItemId, generation)
      return
    }
    setDownloadErrors((previous) => {
      const next = { ...previous }
      delete next[item.mediaItemId]
      return next
    })
    setDownloadPending((previous) => ({ ...previous, [item.mediaItemId]: true }))
    try {
      await createDownloadForMediaItem(item.mediaItemId)
      await refreshItemCapability(item.mediaItemId, generation)
    } catch (cause) {
      if (capabilityGeneration.current === generation) {
        setDownloadErrors((previous) => ({
          ...previous,
          [item.mediaItemId]: cause instanceof HavenError ? cause.message : "创建下载任务失败",
        }))
      }
    } finally {
      if (capabilityGeneration.current === generation) {
        setDownloadPending((previous) => {
          const next = { ...previous }
          delete next[item.mediaItemId]
          return next
        })
      }
    }
  }, [refreshItemCapability])

  const handleRetryCapability = useCallback((mediaItemId: string) => {
    void refreshItemCapability(mediaItemId)
  }, [refreshItemCapability])

  if (mode !== "tauri") {
    return <StatePanel title="版本详情仅在桌面应用中可用" detail="浏览器预览不会伪造本地媒体条目。" onBack={() => navigate(-1)} />
  }
  if (!editionId) {
    return <StatePanel title="版本 ID 无效" detail="请从作品详情重新打开版本。" onBack={() => navigate("/library")} />
  }
  if (loading) {
    return <StatePanel title="正在加载版本" detail="正在读取真实媒体条目。" icon={<LoaderCircle className="h-6 w-6 animate-spin" />} onBack={() => navigate(-1)} />
  }
  if (error) {
    return <StatePanel title={error.code === "EDITION_NOT_FOUND" ? "版本不存在" : "版本加载失败"} detail={error.message} retry={error.retryable ? load : undefined} onBack={() => navigate(-1)} />
  }
  if (!detail) {
    return <StatePanel title="版本暂不可用" detail="没有收到有效的版本数据。" onBack={() => navigate(-1)} />
  }

  return (
    <main className="min-h-full bg-background px-6 py-8 text-foreground md:px-12">
      <button type="button" className="mb-8 inline-flex items-center gap-2 text-sm font-semibold text-muted-foreground hover:text-foreground" onClick={() => navigate(`/work/${detail.workId}`)}>
        <ArrowLeft className="h-4 w-4" /> 返回作品
      </button>
      <header className="mb-8 max-w-3xl">
        <p className="text-xs font-bold uppercase tracking-[0.18em] text-muted-foreground">版本详情</p>
        <h1 className="mt-2 text-3xl font-bold tracking-tight">{detail.title}</h1>
        {detail.subtitle && <p className="mt-2 text-base text-muted-foreground">{detail.subtitle}</p>}
        <div className="mt-4 flex flex-wrap gap-x-4 gap-y-2 text-sm text-muted-foreground">
          {detail.releaseDate && <span>{detail.releaseDate}</span>}
          {detail.language && <span>{detail.language}</span>}
          {detail.region && <span>{detail.region}</span>}
          {detail.publisherOrStudio && <span>{detail.publisherOrStudio}</span>}
        </div>
        {detail.description && <p className="mt-5 leading-7 text-foreground/80">{detail.description}</p>}
      </header>
      <section aria-labelledby="edition-items-heading" className="max-w-4xl">
        <div className="mb-3 flex items-center justify-between">
          <h2 id="edition-items-heading" className="text-lg font-bold">可消费内容</h2>
          <span className="text-sm text-muted-foreground">{detail.items.length} 项</span>
        </div>
        {detail.items.length === 0 ? (
          <div className="rounded-xl border border-dashed border-border px-5 py-10 text-center text-sm text-muted-foreground">该版本暂无已登记的媒体条目</div>
        ) : (
          <div className="divide-y divide-border rounded-xl border border-border">
            {detail.items.map((item) => (
              <MediaItemRow
                key={item.mediaItemId}
                item={item}
                capability={capabilities[item.mediaItemId]}
                downloadPending={downloadPending[item.mediaItemId] === true}
                downloadError={downloadErrors[item.mediaItemId]}
                onOpen={(route) => navigate(route)}
                onDownload={() => void handleDownload(item)}
                onRetry={() => handleRetryCapability(item.mediaItemId)}
              />
            ))}
          </div>
        )}
      </section>
    </main>
  )
}

function MediaItemRow({
  item,
  capability,
  downloadPending,
  downloadError,
  onOpen,
  onDownload,
  onRetry,
}: {
  item: MediaItemSummaryDto
  capability?: ItemCapability
  downloadPending: boolean
  downloadError?: string
  onOpen: (route: string) => void
  onDownload: () => void
  onRetry: () => void
}) {
  const action = primaryActionPresentation(item.primaryAction)
  const route = primaryActionRoute(item.primaryAction)
  const capabilityPresentation: MediaItemCapabilityPresentation = resolveMediaItemCapability(
    capability?.info,
    capability?.failed ?? false,
  )
  const selfEditionAction = item.primaryAction?.kind === "open_edition"
    && route === `/edition/${item.editionId}`
  const canOpen = !selfEditionAction
    && route !== null
    && (capabilityPresentation.state === "online" || capabilityPresentation.state === "offline")
  const canDownload = !canOpen && capabilityPresentation.state === "download_only"
  const disabled = downloadPending
    || (!canOpen && !canDownload && capabilityPresentation.state !== "error")
  const buttonLabel = downloadPending
    ? "准备下载"
    : canOpen
      ? item.primaryAction?.labelHint === "continue" ? "继续" : "打开"
      : canDownload
        ? "下载后阅读"
        : capabilityPresentation.state === "queued"
          ? "下载中"
          : capabilityPresentation.state === "loading"
            ? "读取能力"
            : capabilityPresentation.state === "error"
              ? "重试"
              : "不可用"
  const statusLabel = downloadError ?? (
    selfEditionAction ? "当前内容暂不可用" : capabilityPresentation.label
  )
  return (
    <div className="flex items-center gap-4 px-5 py-4">
      <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">{itemIcon(item)}</span>
      <div className="min-w-0 flex-1">
        <p className="truncate font-semibold">{item.title}</p>
        <p className="mt-1 text-sm text-muted-foreground">{item.indexLabel} · {itemMeta(item)} · {statusLabel}</p>
      </div>
      <button
        type="button"
        disabled={disabled}
        onClick={() => {
          if (canOpen && route) onOpen(route)
          else if (canDownload) onDownload()
          else if (capabilityPresentation.state === "error") onRetry()
        }}
        aria-label={buttonLabel}
        title={action.label}
        className="shrink-0 rounded-lg border border-border px-4 py-2 text-sm font-semibold transition-colors hover:bg-muted disabled:cursor-not-allowed disabled:opacity-50"
      >
        {buttonLabel}
      </button>
    </div>
  )
}

function StatePanel({ title, detail, icon = <CircleAlert className="h-6 w-6" />, retry, onBack }: { title: string; detail: string; icon?: ReactNode; retry?: () => Promise<void>; onBack: () => void }) {
  return (
    <main className="flex min-h-full items-center justify-center bg-background px-6 py-12 text-foreground">
      <section className="w-full max-w-md rounded-xl border border-border p-8 text-center">
        <div className="mx-auto flex h-12 w-12 items-center justify-center rounded-full bg-muted text-muted-foreground">{icon}</div>
        <h1 className="mt-5 text-xl font-bold">{title}</h1>
        <p className="mt-2 text-sm leading-6 text-muted-foreground">{detail}</p>
        <div className="mt-6 flex justify-center gap-3">
          <button type="button" className="rounded-lg border border-border px-4 py-2 text-sm font-semibold hover:bg-muted" onClick={onBack}>返回</button>
          {retry && <button type="button" className="rounded-lg bg-foreground px-4 py-2 text-sm font-semibold text-background hover:opacity-90" onClick={() => void retry()}>重试</button>}
        </div>
      </section>
    </main>
  )
}
