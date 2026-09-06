import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { Link, useNavigate, useSearchParams } from "react-router"
import { HavenStage } from "@/features/home/components/HavenStage"
import { ContentShelf } from "@/features/home/components/ContentShelf"
import { HavenIcon } from "@/components/ui/haven/HavenIcon"
import { ArtworkImage } from "@/components/ui/haven/ArtworkImage"
import { defaultCoverCategoryForMediaType } from "@/lib/default-cover"
import { ArrowRight, Bookmark, Heart, Trash2 } from "lucide-react"
import type { MediaCardProps } from "@/components/ui/haven/MediaCard"
import { primaryActionRoute } from "@/features/media/lib/primary-action-route"
import { setFavorite } from "@/features/media/ipc/favorite-gateway"
import { createDownloadForMediaItem, deleteOfflineDownload, getMediaItemDownloadInfo, revealOfflineDownload, subscribeDownloadEvents, type MediaItemDownloadInfo } from "@/features/downloads/ipc/download-gateway"
import type { DownloadEvent } from "@/lib/ipc/generated/wire"
import { getCatalogItem, getStoredMarkers, removeStoredMarker } from "@/lib/havenState"
import { getHavenClientMode } from "@/lib/ipc/runtime"
import { onFavoriteChanged } from "@/lib/ipc/events"
import { HavenError, toHavenError, type HavenError as HavenErrorType } from "@/lib/ipc/errors"
import { deriveLibrarySliceState } from "@/lib/slice-state"
import { getFavoriteFootprintItems, getContinueFootprintItems, getRecentActivityFootprintItems, getMarkerFootprintCards, deleteFootprintMarker, resetFootprintProgress, type FootprintActionCard, type HistoryCardProps } from "../ipc/footprints-gateway"
import {
  canLoadFootprintsData,
  loadDemoFootprintMarkers,
  resolveDemoFootprintsEmptyState,
  resolveFootprintsRuntimeState,
} from "../lib/footprints-runtime-state"
import { selectLatestUnfinished } from "../lib/select-latest-continue"

const HIDDEN_MARKERS_KEY = "haven:hidden-markers"

// ==========================================
// MOCK DATA (临时假数据，满足 UI 开发)
// ==========================================
const mockHavenStageData = {
  id: "2",
  title: "沙丘2",
  originalTitle: "Dune: Part Two",
  metadata: "2024 · 2h 46m · 科幻 / 史诗 · 4K HDR",
  backdropUrl: "https://images.unsplash.com/photo-1534447677768-be436bb09401?q=80&w=2663&auto=format&fit=crop", 
  description: "保罗·厄崔迪与契妮和弗雷曼人会合，展开一场针对毁灭他家族的阴谋者的报复之旅。在面对一生挚爱与已知宇宙命运的两难选择时，他必须努力阻止唯有他能预见的可怕未来。",
  primaryActionLabel: "继续播放 (45%)"
}

const mockContinueShelf: MediaCardProps[] = [
  {
    id: "4",
    title: "怪奇物语：1985故事集 第一季",
    subtitle: "S1E4 · 你真正的自己",
    typeBadge: "1080P",
    layout: "landscape",
    progress: 89,
    imageUrl: "https://images.unsplash.com/photo-1497366216548-37526070297c?q=80&w=1200&auto=format&fit=crop",
  },
  {
    id: "10",
    title: "星际穿越",
    subtitle: "已看 1h 23m",
    typeBadge: "4K",
    layout: "landscape",
    progress: 55,
    imageUrl: "https://images.unsplash.com/photo-1462331940025-496dfbfc7564?q=80&w=1200&auto=format&fit=crop",
  }
]

const mockFavoritesShelf: MediaCardProps[] = [
  {
    id: "c3",
    title: "葬送的芙莉莲 (第 1-13 卷)",
    subtitle: "已收藏 · Manga",
    typeBadge: "Manga",
    imageUrl: "https://images.unsplash.com/photo-1578632767115-351597cf2477?q=80&w=800&auto=format&fit=crop",
  },
  {
    id: "5",
    title: "程序员修炼之道 (第2版)",
    subtitle: "已收藏 · PDF",
    typeBadge: "PDF",
    imageUrl: "https://images.unsplash.com/photo-1518770660439-4636190af475?q=80&w=800&auto=format&fit=crop",
  },
  {
    id: "c2",
    title: "迷宫饭 (单行本 1-14 卷全集)",
    subtitle: "已收藏 · Manga",
    typeBadge: "Manga",
    imageUrl: "https://images.unsplash.com/photo-1607604276583-eef5d076aa5f?q=80&w=800&auto=format&fit=crop",
  },
  {
    id: "6",
    title: "奥本海默 Oppenheimer",
    subtitle: "已收藏 · 4K HDR",
    typeBadge: "4K HDR",
    imageUrl: "https://images.unsplash.com/photo-1446776811953-b23d57bd21aa?q=80&w=800&auto=format&fit=crop",
  }
]

const mockRecentActivity: MediaCardProps[] = [
  {
    id: "2",
    title: "沙丘2",
    subtitle: "昨天 · 已播放 45%",
    typeBadge: "4K HDR",
    layout: "landscape",
    progress: 45,
    imageUrl: "https://images.unsplash.com/photo-1534447677768-be436bb09401?q=80&w=1200&auto=format&fit=crop",
  },
  {
    id: "article-agentic-ai",
    title: "The Agentic AI Era",
    subtitle: "周一 · 阅读 6 分钟",
    typeBadge: "HTML",
    layout: "landscape",
    progress: 30,
    imageUrl: "https://images.unsplash.com/photo-1451187580459-43490279c0fa?q=80&w=1200&auto=format&fit=crop",
    onClick: () => window.location.assign("/article/article-agentic-ai"),
  }
]

interface MarkerItem {
  id: string
  mode: string
  mediaItemId: string
  title: string
  subtitle: string
  type: string
  imageUrl: string
  artworkCategory?: "video" | "book" | "comic" | "article"
}

const mockMarkers: MarkerItem[] = [
  { id: "marker-jobs", mode: "book", mediaItemId: "3", title: "史蒂夫·乔布斯传", subtitle: "第 14 章 · 硅谷的王者归来与 NeXT 时代", type: "阅读", imageUrl: "https://images.unsplash.com/photo-1518770660439-4636190af475?q=80&w=800&auto=format&fit=crop" },
  { id: "marker-aot", mode: "comic", mediaItemId: "v1", title: "进击的巨人", subtitle: "最终卷 · 人类真正的敌人", type: "漫画", imageUrl: "https://images.unsplash.com/photo-1542451313056-b7c8e626645f?q=80&w=800&auto=format&fit=crop" },
]

function readHiddenMarkerIds(): string[] {
  try {
    const stored = localStorage.getItem(HIDDEN_MARKERS_KEY)
    const parsed: unknown = stored ? JSON.parse(stored) : []
    return Array.isArray(parsed) && parsed.every((id) => typeof id === "string") ? parsed : []
  } catch {
    return []
  }
}

function buildMarkerItems(): MarkerItem[] {
  const hidden = readHiddenMarkerIds()
  const mockVisible = mockMarkers.filter((marker) => !hidden.includes(marker.id))
  const realMarkers = getStoredMarkers().map((marker) => {
    const catalogItem = getCatalogItem(marker.mediaItemId)
    return {
      id: `real-${marker.mode}-${marker.mediaItemId}`,
      mode: marker.mode,
      mediaItemId: marker.mediaItemId,
      title: catalogItem?.title || `书签 · ${marker.mediaItemId}`,
      subtitle: "在阅读 / 播放时标记的位置",
      type: marker.mode === "comic" ? "漫画" : marker.mode === "article" ? "文章" : "阅读",
      imageUrl: catalogItem?.imageUrl || mockMarkers[0].imageUrl,
    }
  })
  return [...mockVisible, ...realMarkers]
}

function getMarkerRoute(mode: string, mediaItemId: string): string {
  if (mode === "comic") return `/comic/${mediaItemId}`
  if (mode === "article") return `/article/${mediaItemId}`
  return `/reader/${mediaItemId}`
}

// ==========================================
// COMPONENT
// ==========================================

export function FootprintsPage() {
  const navigate = useNavigate()
  const [searchParams] = useSearchParams()
  const clientMode = getHavenClientMode()
  const runtimeState = resolveFootprintsRuntimeState(clientMode)
  const demoMode = runtimeState === "demo"
  const productionMode = runtimeState === "production"
  const unavailableMode = runtimeState === "unavailable"
  const dataAccessEnabled = canLoadFootprintsData(clientMode)
  const [activeLikesTab, setActiveLikesTab] = useState<"favorites" | "markers">("favorites")
  // IPC-MOCK-001：收藏改经 gateway 拉真实数据（Tauri=library_list 过滤 favorite）；
  // 浏览器演示环境保持既有演示目录兜底（零 localStorage 依赖差异可见化）。
  const [favoriteItems, setFavoriteItems] = useState<FootprintActionCard[]>([])
  const [favoritesLoading, setFavoritesLoading] = useState(productionMode)
  const [favoritesError, setFavoritesError] = useState<HavenErrorType | null>(null)
  const favoritesRequestRef = useRef(0)
  const loadFavorites = useCallback(async () => {
    const requestId = ++favoritesRequestRef.current
    setFavoritesLoading(true)
    setFavoritesError(null)
    try {
      const items = await getFavoriteFootprintItems()
      if (favoritesRequestRef.current === requestId) setFavoriteItems(items)
    } catch (error) {
      if (favoritesRequestRef.current === requestId) setFavoritesError(toHavenError(error))
    } finally {
      if (favoritesRequestRef.current === requestId) setFavoritesLoading(false)
    }
  }, [])

  // 继续观看/阅读：progress_recent 联查 WorkCard 投影（浏览器演示环境返回空，由页面兜底 mock）。
  const [continueItems, setContinueItems] = useState<FootprintActionCard[]>([])
  const [continueLoading, setContinueLoading] = useState(productionMode)
  const [continueError, setContinueError] = useState<HavenErrorType | null>(null)
  const continueRequestRef = useRef(0)
  const loadContinue = useCallback(async () => {
    const requestId = ++continueRequestRef.current
    setContinueLoading(true)
    setContinueError(null)
    try {
      const items = await getContinueFootprintItems()
      if (continueRequestRef.current === requestId) setContinueItems(items)
    } catch (error) {
      if (continueRequestRef.current === requestId) setContinueError(toHavenError(error))
    } finally {
      if (continueRequestRef.current === requestId) setContinueLoading(false)
    }
  }, [])

  // 最近活动：history_list 联查 WorkCard 投影（浏览器演示环境返回空，由页面兜底 mock）。
  const [recentItems, setRecentItems] = useState<HistoryCardProps[]>([])
  const [recentLoading, setRecentLoading] = useState(productionMode)
  const [recentError, setRecentError] = useState<HavenErrorType | null>(null)
  const recentRequestRef = useRef(0)
  const [heroDownloadInfo, setHeroDownloadInfo] = useState<MediaItemDownloadInfo | null>(null)
  const [heroDownloadMediaItemId, setHeroDownloadMediaItemId] = useState<string | null>(null)
  const [heroActionPending, setHeroActionPending] = useState(false)
  const loadRecent = useCallback(async () => {
    const requestId = ++recentRequestRef.current
    setRecentLoading(true)
    setRecentError(null)
    try {
      const items = await getRecentActivityFootprintItems()
      if (recentRequestRef.current === requestId) setRecentItems(items)
    } catch (error) {
      if (recentRequestRef.current === requestId) setRecentError(toHavenError(error))
    } finally {
      if (recentRequestRef.current === requestId) setRecentLoading(false)
    }
  }, [])
  useEffect(() => {
    if (!dataAccessEnabled) return
    void loadFavorites()
    return () => {
      favoritesRequestRef.current += 1
    }
  }, [dataAccessEnabled, loadFavorites])

  // 继续观看/阅读：progress_recent（仅 Tauri 环境；浏览器演示环境由页面 mock 兜底）。
  useEffect(() => {
    if (!productionMode) return
    void loadContinue()
    return () => {
      continueRequestRef.current += 1
    }
  }, [loadContinue, productionMode])

  // 最近活动：history_list（仅 Tauri 环境；浏览器演示环境由页面 mock 兜底）。
  useEffect(() => {
    if (!productionMode) return
    void loadRecent()
    return () => {
      recentRequestRef.current += 1
    }
  }, [loadRecent, productionMode])

  // favorite-changed → 重拉收藏投影（SLICE-FAVORITE-001：跨入口一致；仅 Tauri 环境）。
  useEffect(() => {
    if (!productionMode) return
    let unlisten: (() => void) | null = null
    let disposed = false
    onFavoriteChanged(() => {
      if (!disposed) void loadFavorites()
    })
      .then((fn) => {
        if (disposed) fn()
        else unlisten = fn
      })
      .catch(() => {})
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [loadFavorites, productionMode])
  const favoriteDisplay =
    demoMode && favoriteItems.length === 0 ? mockFavoritesShelf : favoriteItems
  const favoriteSliceState = deriveLibrarySliceState({
    loading: favoritesLoading,
    itemCount: favoriteDisplay.length,
    error: favoritesError,
  })
  // 书签：Tauri 环境用真实 marker_list_all 联查 WorkCard；浏览器演示用 localStorage 兜底 mock。
  const [markerItems, setMarkerItems] = useState<MarkerItem[]>(() => loadDemoFootprintMarkers(clientMode, buildMarkerItems))
  const [markersLoading, setMarkersLoading] = useState(productionMode)
  const [markersError, setMarkersError] = useState<HavenErrorType | null>(null)
  const markersRequestRef = useRef(0)
  const loadMarkers = useCallback(async () => {
    if (!productionMode) return
    const requestId = ++markersRequestRef.current
    setMarkersLoading(true)
    setMarkersError(null)
    try {
      const cards = await getMarkerFootprintCards()
      if (markersRequestRef.current === requestId) {
        setMarkerItems(cards)
      }
    } catch (error) {
      if (markersRequestRef.current === requestId) setMarkersError(toHavenError(error))
    } finally {
      if (markersRequestRef.current === requestId) setMarkersLoading(false)
    }
  }, [productionMode])
  const isEmptyState = resolveDemoFootprintsEmptyState(clientMode, searchParams.get("state"))

  // 书签：仅 Tauri 环境异步加载；浏览器演示环境由 buildMarkerItems() 兜底。
  useEffect(() => {
    if (!productionMode) return
    void loadMarkers()
    return () => {
      markersRequestRef.current += 1
    }
  }, [loadMarkers, productionMode])

  // 继续/最近：Tauri 环境用真实 IPC 结果；浏览器演示环境保持既有 mock 兜底。
  const continueDisplay = productionMode
    ? continueItems.map((item) => ({ ...item, onClick: () => void openFootprintAction(item) }))
    : demoMode
      ? isEmptyState
        ? []
        : mockContinueShelf.map((item) => ({ ...item, onClick: () => navigate(`/player/${item.id}`) }))
      : []
  const recentDisplay = productionMode
    ? recentItems.map((item) => ({ ...item, onClick: () => void openFootprintAction(item) }))
    : demoMode ? (isEmptyState ? [] : mockRecentActivity) : []
  // 生产：最新未看完的置顶为 Hero（严格 0<progress<100），剩下的按时间依次排序；Demo 保持既有沙丘2 Hero 不动
  const productionHeroSplit = useMemo(
    () =>
      productionMode
        ? selectLatestUnfinished(continueItems)
        : { hero: null as (typeof continueItems)[number] | null, rest: [] as typeof continueItems },
    [continueItems, productionMode],
  )
  const productionHero = productionHeroSplit.hero
  const productionContinueRest = productionHeroSplit.rest.map((item) => ({
    ...item,
    onClick: () => void openFootprintAction(item),
  }))
  const heroStageData = useMemo(() => {
    if (!productionMode || isEmptyState || !productionHero) return null
    const metaParts = [
      productionHero.releaseYear ? String(productionHero.releaseYear) : null,
      productionHero.typeBadge ?? null,
    ].filter(Boolean) as string[]
    return {
      id: productionHero.id,
      workId: productionHero.workId,
      mediaItemId: productionHero.mediaItemId,
      primaryAction: productionHero.primaryAction,
      isFavorite: productionHero.favorite,
      title: productionHero.title,
      originalTitle: undefined as string | undefined,
      metadata: metaParts.length > 0 ? metaParts.join(" · ") : (productionHero.subtitle ?? ""),
      description: productionHero.description ?? "",
      backdropUrl: productionHero.imageUrl,
      primaryActionLabel:
        `${productionHero.primaryAction?.kind === "playback" ? "继续播放" : "继续阅读"}${productionHero.progress !== undefined ? ` (${productionHero.progress}%)` : ""}`,
    }
  }, [isEmptyState, productionHero, productionMode])
  useEffect(() => {
    let cancelled = false
    const mediaItemId = productionHero?.mediaItemId
    setHeroDownloadInfo(null)
    setHeroDownloadMediaItemId(null)
    if (!productionMode || !mediaItemId) {
      return
    }
    void getMediaItemDownloadInfo(mediaItemId).then((info) => {
      if (!cancelled) {
        setHeroDownloadInfo(info)
        setHeroDownloadMediaItemId(mediaItemId)
      }
    }).catch(() => {
      if (!cancelled) setHeroDownloadInfo(null)
    })
    return () => { cancelled = true }
  }, [productionHero, productionMode])
  useEffect(() => {
    if (!productionMode || !productionHero?.mediaItemId) return
    let mounted = true
    let dispose: (() => Promise<void>) | null = null
    const onEvent = (_event: DownloadEvent) => {
      void getMediaItemDownloadInfo(productionHero.mediaItemId!).then((info) => {
        if (mounted) {
          setHeroDownloadInfo(info)
          setHeroDownloadMediaItemId(productionHero.mediaItemId!)
        }
      }).catch(() => {})
    }
    void subscribeDownloadEvents(onEvent).then((cleanup) => {
      if (!mounted) void cleanup().catch(() => undefined)
      else dispose = cleanup
    }).catch(() => {})
    return () => {
      mounted = false
      if (dispose) void dispose().catch(() => undefined)
    }
  }, [productionHero, productionMode])
  const continueShelfItems = productionMode ? productionContinueRest : continueDisplay
  const continueSliceState = deriveLibrarySliceState({
    loading: continueLoading,
    itemCount: continueDisplay.length,
    error: continueError,
  })
  const recentSliceState = deriveLibrarySliceState({
    loading: recentLoading,
    itemCount: recentDisplay.length,
    error: recentError,
  })
  const stageData = isEmptyState ? { title: "", backdropUrl: "", metadata: "", description: "", primaryActionLabel: "", id: "" } : mockHavenStageData

  function showMessage(message: string) {
    // Keep the existing page-level visual surface; action errors are rendered as a live region.
    setBatchActionMessage(message)
  }
  const [batchActionMessage, setBatchActionMessage] = useState<string | null>(null)
  async function openFootprintAction(card: Pick<FootprintActionCard, "primaryAction" | "mediaItemId">) {
    const target = primaryActionRoute(card.primaryAction)
    if (!target) {
      showMessage("当前内容暂不可打开")
      return
    }
    if (card.primaryAction?.kind === "open_edition") {
      navigate(target)
      return
    }
    const mediaItemId = card.primaryAction?.mediaItemId ?? card.mediaItemId
    if (!mediaItemId) {
      showMessage("当前内容缺少可用版本")
      return
    }
    try {
      const info = await getMediaItemDownloadInfo(mediaItemId)
      if (info.canOnlineRead || info.hasOfflineResource) navigate(target)
      else showMessage(info.canDownload ? "该内容需要下载后阅读" : "当前内容暂不可用")
    } catch (error) {
      showMessage(error instanceof HavenError ? error.dto.userMessage : "读取内容能力失败，请重试")
    }
  }
  const handleHeroAction = async (action: string) => {
    if (!productionHero || heroActionPending) return
    setHeroActionPending(true)
    try {
      if (action === "heart") {
        const favorite = !productionHero.favorite
        await setFavorite({ workId: productionHero.workId, favorite })
        setContinueItems((items) => items.map((item) => item.id === productionHero.id ? { ...item, favorite } : item))
        await loadFavorites()
      } else if (action === "download") {
        const mediaItemId = productionHero.mediaItemId
        if (!mediaItemId) throw new Error("当前内容没有可下载的媒体版本")
        const info = await getMediaItemDownloadInfo(mediaItemId)
        if (info.hasOfflineResource && info.taskId) await revealOfflineDownload(info.taskId)
        else if (!info.hasOfflineResource) await createDownloadForMediaItem(mediaItemId)
        setHeroDownloadInfo(await getMediaItemDownloadInfo(mediaItemId))
        showMessage(info.hasOfflineResource ? "已打开本地文件夹" : "已加入下载队列")
      } else if (action === "reset") {
        const mediaItemId = productionHero.mediaItemId
        if (!mediaItemId) throw new Error("当前内容没有可重置的媒体版本")
        await resetFootprintProgress(mediaItemId)
        await Promise.all([loadContinue(), loadRecent()])
        showMessage("已重置进度")
      } else if (action === "folder") {
        if (heroDownloadMediaItemId !== productionHero.mediaItemId || !heroDownloadInfo?.hasOfflineResource || !heroDownloadInfo.taskId) throw new Error("当前没有可定位的离线文件")
        await revealOfflineDownload(heroDownloadInfo.taskId)
        showMessage("已打开离线文件夹")
      } else if (action === "delete") {
        if (heroDownloadMediaItemId !== productionHero.mediaItemId || !heroDownloadInfo?.hasOfflineResource || !heroDownloadInfo.taskId) throw new Error("当前没有可删除的离线内容")
        if (!window.confirm("确定删除这个作品的离线内容吗？此操作不会删除媒体库记录。")) return
        await deleteOfflineDownload(heroDownloadInfo.taskId)
        const info = await getMediaItemDownloadInfo(productionHero.mediaItemId!)
        setHeroDownloadInfo(info)
        setHeroDownloadMediaItemId(productionHero.mediaItemId)
        showMessage("已删除离线内容")
      }
    } catch (error) {
      showMessage(error instanceof HavenError ? error.dto.userMessage : "操作失败，请重试")
    } finally {
      setHeroActionPending(false)
    }
  }

  const deleteMarker = (id: string) => {
    setMarkerItems((current) => current.filter((marker) => marker.id !== id))
    // Tauri 环境软删除走真实 IPC；浏览器演示环境只改本地 state。
    if (productionMode) {
      void deleteFootprintMarker(id).catch(() => {
        // 删除失败：重拉书签恢复真实状态（与 favorite 乐观回滚一致）。
        void loadMarkers()
      })
    } else if (demoMode) {
      // 浏览器演示兜底：localStorage 兼容旧 real-/mock- 标记 id。
      const target = markerItems.find((marker) => marker.id === id)
      if (target?.id.startsWith("real-")) {
        removeStoredMarker(target.mode, target.mediaItemId)
      } else {
        localStorage.setItem(HIDDEN_MARKERS_KEY, JSON.stringify([...readHiddenMarkerIds(), id]))
      }
    }
  }
  const markersSliceState = deriveLibrarySliceState({
    loading: markersLoading,
    itemCount: markerItems.length,
    error: markersError,
  })

  return (
    <div className="w-full flex flex-col min-h-full bg-background">
      {!isEmptyState && heroStageData && (
        <HavenStage
          {...heroStageData}
          onPrimaryAction={() => {
            void openFootprintAction(heroStageData)
          }}
          isFavorite={heroStageData.isFavorite}
          isDownloaded={heroDownloadInfo?.hasOfflineResource ?? false}
          canManageOffline={heroDownloadMediaItemId === productionHero?.mediaItemId && Boolean(heroDownloadInfo?.hasOfflineResource && heroDownloadInfo.taskId)}
          isActionPending={heroActionPending}
          onAction={(action) => void handleHeroAction(action)}
        />
      )}
      {!isEmptyState && !heroStageData && demoMode && (
        <HavenStage
          {...stageData}
          onPrimaryAction={() => navigate(`/player/${stageData.id || "2"}`)}
          onAction={(action) => console.log("Action clicked:", action)}
        />
      )}
      {batchActionMessage && <div role="status" className="fixed top-6 left-1/2 z-[100] -translate-x-1/2 rounded-full bg-zinc-950/90 px-5 py-3 text-sm font-semibold text-white shadow-xl">{batchActionMessage}</div>}
      {unavailableMode ? (
        <UnavailableFootprintsState />
      ) : (
        <div className="flex flex-col gap-[48px] mt-[16px] md:mt-[32px] relative z-20">
        {isEmptyState ? (
          <div className="flex flex-col items-center justify-center py-[64px] text-center text-muted-foreground">
            <div className="w-[64px] h-[64px] rounded-full bg-muted/40 flex items-center justify-center mb-[16px]">
              <HavenIcon symbol="history" size={32} className="opacity-50" />
            </div>
            <h3 className="text-lg font-bold text-foreground mb-1">暂无足迹记录</h3>
            <p className="text-sm max-w-sm">您看过的媒体内容和进度会自动保存在这里。</p>
          </div>
        ) : (
          <>
            <div data-slice-state={continueSliceState.kind} aria-busy={continueSliceState.kind === "loading"}>
              <ContentShelf
                title="继续"
                items={continueShelfItems}
              />
            </div>
            <div data-slice-state={recentSliceState.kind} aria-busy={recentSliceState.kind === "loading"}>
              <ContentShelf
                title="最近活动"
                items={recentDisplay}
                actionRight={
                  <Link to="/footprints/history" className="flex items-center gap-1 text-sm font-semibold text-foreground hover:underline transition-colors">
                    查看我的浏览记录 <ArrowRight className="w-[1.2em] h-[1.2em]" strokeWidth={2.5} />
                  </Link>
                }
              />
            </div>

            <section className="flex flex-col gap-5">
              <div className="flex flex-col gap-3 px-[48px] md:px-[64px] lg:px-[96px] sm:flex-row sm:items-end sm:justify-between">
                <div>
                  <p className="text-xs font-semibold uppercase tracking-[0.18em] text-muted-foreground">Footprints</p>
                  <h2 className="mt-1 text-xl font-bold tracking-tight md:text-2xl">我的喜爱</h2>
                </div>
                <div className="flex items-center gap-0.5 rounded-full bg-black/5 p-0.5 dark:bg-white/10" role="tablist" aria-label="收藏与书签">
                  <button
                    type="button"
                    role="tab"
                    aria-selected={activeLikesTab === "favorites"}
                    onClick={() => setActiveLikesTab("favorites")}
                    className={`flex items-center gap-1.5 rounded-full px-3 py-1.5 text-xs font-semibold transition-colors ${activeLikesTab === "favorites" ? "bg-background text-foreground shadow-sm" : "text-muted-foreground hover:text-foreground"}`}
                  >
                    <Heart className="h-3.5 w-3.5" />
                    收藏
                  </button>
                  <button
                    type="button"
                    role="tab"
                    aria-selected={activeLikesTab === "markers"}
                    onClick={() => setActiveLikesTab("markers")}
                    className={`flex items-center gap-1.5 rounded-full px-3 py-1.5 text-xs font-semibold transition-colors ${activeLikesTab === "markers" ? "bg-background text-foreground shadow-sm" : "text-muted-foreground hover:text-foreground"}`}
                  >
                    <Bookmark className="h-3.5 w-3.5" />
                    书签
                  </button>
                </div>
              </div>

              {activeLikesTab === "favorites" ? (
                <div data-slice-state={favoriteSliceState.kind} aria-busy={favoriteSliceState.kind === "loading"}>
                  {favoriteSliceState.kind === "loading" ? (
                    <p className="px-[48px] text-sm text-muted-foreground md:px-[64px] lg:px-[96px]">正在加载收藏…</p>
                  ) : favoriteSliceState.kind === "retryable_error" || favoriteSliceState.kind === "terminal_error" ? (
                    <div className="px-[48px] text-sm text-muted-foreground md:px-[64px] lg:px-[96px]">
                      <p>{favoriteSliceState.message || "收藏加载失败"}</p>
                      {favoriteSliceState.canRetry && (
                        <button type="button" onClick={() => void loadFavorites()} className="mt-3 font-semibold text-foreground underline">
                          重试加载
                        </button>
                      )}
                    </div>
                  ) : favoriteSliceState.kind === "empty" || isEmptyState ? (
                    <p className="px-[48px] text-sm text-muted-foreground md:px-[64px] lg:px-[96px]">暂无收藏</p>
                  ) : (
                    <ContentShelf title="" items={favoriteDisplay} className="pt-0" />
                  )}
                </div>
              ) : (
                <div data-slice-state={markersSliceState.kind} aria-busy={markersSliceState.kind === "loading"}>
                  {markersSliceState.kind === "loading" ? (
                    <p className="px-[48px] text-sm text-muted-foreground md:px-[64px] lg:px-[96px]">正在加载书签…</p>
                  ) : markersSliceState.kind === "retryable_error" || markersSliceState.kind === "terminal_error" ? (
                    <div className="px-[48px] text-sm text-muted-foreground md:px-[64px] lg:px-[96px]">
                      <p>{markersSliceState.message || "书签加载失败"}</p>
                      {markersSliceState.canRetry && (
                        <button type="button" onClick={() => void loadMarkers()} className="mt-3 font-semibold text-foreground underline">
                          重试加载
                        </button>
                      )}
                    </div>
                  ) : markersSliceState.kind === "empty" ? (
                    <p className="px-[48px] text-sm text-muted-foreground md:px-[64px] lg:px-[96px]">暂无书签</p>
                  ) : (
                    <MarkerList
                      markers={isEmptyState ? [] : markerItems}
                      onOpen={(mode, mediaItemId) => navigate(getMarkerRoute(mode, mediaItemId))}
                      onDelete={deleteMarker}
                    />
                  )}
                </div>
              )}
            </section>
          </>
        )}
        </div>
      )}
    </div>
  )
}

function UnavailableFootprintsState() {
  return (
    <section className="flex min-h-[420px] flex-1 flex-col items-center justify-center px-6 py-16 text-center">
      <HavenIcon symbol="history" size={32} className="mb-4 text-muted-foreground/60" />
      <h1 className="text-xl font-semibold text-foreground">足迹功能未启用</h1>
      <p className="mt-2 max-w-md text-sm text-muted-foreground">
        当前浏览器不支持应用数据访问，请在 Haven 应用中打开足迹。
      </p>
    </section>
  )
}

function MarkerList({
  markers,
  onOpen,
  onDelete,
}: {
  markers: MarkerItem[]
  onOpen: (mode: string, mediaItemId: string) => void
  onDelete: (id: string) => void
}) {
  if (markers.length === 0) {
    return (
      <div className="mx-[48px] rounded-2xl border border-dashed border-border bg-muted/20 px-5 py-10 text-center md:mx-[64px] lg:mx-[96px]">
        <Bookmark className="mx-auto h-7 w-7 text-muted-foreground/60" />
        <p className="mt-3 text-sm font-semibold">还没有书签</p>
        <p className="mt-1 text-xs text-muted-foreground">在播放或阅读时保存时间点、章节或页码。</p>
      </div>
    )
  }

  return (
    <div className="mx-[48px] max-h-[60vh] overflow-y-auto overscroll-contain rounded-2xl border border-border/60 bg-background/70 scrollbar-none md:mx-[64px] lg:mx-[96px]">
      {markers.map((marker) => (
        <div key={marker.id} className="group/marker flex min-h-[88px] w-full items-center gap-5 border-b border-border/50 px-5 py-[16px] text-left transition-colors last:border-b-0 hover:bg-muted/50">
          <button type="button" onClick={() => onOpen(marker.mode, marker.mediaItemId)} className="flex min-w-0 flex-1 items-center gap-5 text-left">
            <span className="relative flex h-[64px] w-[64px] shrink-0 items-center justify-center overflow-hidden rounded-2xl border border-border/60 bg-muted">
              <ArtworkImage
                src={marker.imageUrl}
                alt=""
                allowExternal={getHavenClientMode() !== "tauri"}
                fallbackCategory={marker.artworkCategory ?? defaultCoverCategoryForMediaType(marker.mode)}
                fallbackSeed={marker.mediaItemId || marker.id}
                className="h-full w-full object-cover"
                loading="lazy"
              />
              <span className="absolute bottom-1 right-1 flex h-5 w-5 items-center justify-center rounded-full bg-background/90 text-primary shadow-sm backdrop-blur-sm">
                <Bookmark className="h-3 w-3 fill-current" />
              </span>
            </span>
            <span className="min-w-0 flex-1">
              <span className="flex items-center gap-[8px]"><span className="truncate text-sm font-semibold transition-colors group-hover/marker:text-primary">{marker.title}</span><span className="shrink-0 rounded-full bg-muted px-[8px] py-0.5 text-[10px] font-semibold text-muted-foreground">{marker.type}</span></span>
              <span className="mt-1 block truncate text-xs text-muted-foreground">{marker.subtitle}</span>
            </span>
          </button>
          <div className="flex shrink-0 items-center gap-1">
            <button
              type="button"
              aria-label={`删除书签 ${marker.title}`}
              onClick={() => onDelete(marker.id)}
              className="flex h-9 w-9 items-center justify-center rounded-full text-muted-foreground/60 transition-colors hover:bg-destructive/10 hover:text-destructive focus-visible:ring-2 focus-visible:ring-primary"
            >
              <Trash2 className="h-[16px] w-[16px]" />
            </button>
            <span className="flex h-9 w-9 items-center justify-center rounded-full text-muted-foreground/60 transition-colors group-hover/marker:bg-primary/10 group-hover/marker:text-primary">
              <ArrowRight className="h-[16px] w-[16px] transition-transform group-hover/marker:translate-x-0.5" />
            </span>
          </div>
        </div>
      ))}
    </div>
  )
}
