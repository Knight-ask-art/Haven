import { useCallback, useEffect, useRef, useState } from "react"
import type { ReactNode } from "react"
import { ArrowLeft, BookOpen, CircleAlert, FileText, Film, LoaderCircle, Play } from "lucide-react"
import { useNavigate, useParams } from "react-router"
import type { EditionDetailDto, MediaItemSummaryDto } from "@/lib/ipc/generated/wire"
import { HavenError } from "@/lib/ipc/errors"
import { getHavenClientMode } from "@/lib/ipc/runtime"
import { getEdition, normalizeEditionError } from "../ipc/edition-gateway"
import { primaryActionRoute } from "../lib/primary-action-route"

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
            {detail.items.map((item) => <MediaItemRow key={item.mediaItemId} item={item} onOpen={(route) => navigate(route)} />)}
          </div>
        )}
      </section>
    </main>
  )
}

function MediaItemRow({ item, onOpen }: { item: MediaItemSummaryDto; onOpen: (route: string) => void }) {
  const route = primaryActionRoute(item.primaryAction)
  const disabled = route === null
  return (
    <div className="flex items-center gap-4 px-5 py-4">
      <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">{itemIcon(item)}</span>
      <div className="min-w-0 flex-1">
        <p className="truncate font-semibold">{item.title}</p>
        <p className="mt-1 text-sm text-muted-foreground">{item.indexLabel} · {itemMeta(item)} · {item.availableResourceCount > 0 ? "本地可用" : "资源不可用"}</p>
      </div>
      <button type="button" disabled={disabled} onClick={() => { if (route) onOpen(route) }} className="shrink-0 rounded-lg border border-border px-4 py-2 text-sm font-semibold transition-colors hover:bg-muted disabled:cursor-not-allowed disabled:opacity-50">
        {disabled ? "不可用" : item.primaryAction?.labelHint === "continue" ? "继续" : "打开"}
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
