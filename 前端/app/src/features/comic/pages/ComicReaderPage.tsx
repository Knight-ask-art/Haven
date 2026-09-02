import { useState, useEffect, useLayoutEffect, useRef, useCallback, useMemo } from "react"
import { useNavigate, useParams } from "react-router"
import {
  ArrowLeft,
  Bookmark,
  ChevronLeft,
  ChevronRight,
  Grid,
  Maximize2,
  Minimize2,
  X,
  Check
} from "lucide-react"
import { cn } from "@/lib/utils"
import { getHavenClientMode, isTauriRuntime } from "@/lib/ipc/runtime"
import { toHavenError } from "@/lib/ipc/errors"
import { useMediaSession } from "@/features/session/useMediaSession"
import { getComicPageManifest } from "../ipc/comic-page-manifest-gateway"
import { selectComicSessionView } from "../lib/comic-session-view"
import { createComicProgressController, restoreComicProgress, type ComicProgressController } from "../lib/comic-progress-controller"
import { findComicBookmark } from "../lib/comic-marker-match"
import { createDemoComicPageSequence, mapComicPageManifest, pageAt, pageNumbersAround, resolveComicReaderDefaults, type ComicPageModel, type ComicPageSequence } from "../lib/comic-reader-model"
import { ComicPageResourcePool, type ComicPageResource } from "../lib/comic-page-resource-pool"
import { resolveComicReaderRuntimeState } from "../lib/comic-reader-runtime-state"
import { useComicSettings } from "../lib/useComicSettings"
import { comicMarkerLocator, createMarker, deleteMarker, listMarkers } from "@/features/markers/ipc/marker-gateway"
import type { MarkerDto } from "@/lib/ipc/generated/wire"

type ViewMode = "single" | "double" | "strip"
type ReadDirection = "rtl" | "ltr" // rtl: 日漫从右往左, ltr: 国漫/美漫从左往右
type FitMode = "height" | "width" | "contain"

type ComicManifestState =
  | { status: "idle" | "loading" }
  | { status: "ready"; sequence: ComicPageSequence; sessionKey: string }
  | { status: "error"; message: string; retryable: boolean }

// Browser-only demo content. The Tauri branch never reads these values.
const DEMO_CHAPTERS = [
  { id: "c139", title: "第 139 话：自由的彼岸（最终话）", pagesCount: 45 },
  { id: "c138", title: "第 138 话：长久的梦", pagesCount: 42 },
  { id: "c137", title: "第 137 话：巨人", pagesCount: 40 },
  { id: "c136", title: "第 136 话：献出你的心脏", pagesCount: 38 },
]

const DEMO_COMIC_PAGE_URLS = [
  "https://images.unsplash.com/photo-1607604276583-eef5d076aa5f?q=80&w=1200&auto=format&fit=crop",
  "https://images.unsplash.com/photo-1578632767115-351597cf2477?q=80&w=1200&auto=format&fit=crop",
  "https://images.unsplash.com/photo-1541562232579-512a21360020?q=80&w=1200&auto=format&fit=crop",
  "https://images.unsplash.com/photo-1534447677768-be436bb09401?q=80&w=1200&auto=format&fit=crop",
  "https://images.unsplash.com/photo-1579783902614-a3fb3927b675?q=80&w=1200&auto=format&fit=crop",
  "https://images.unsplash.com/photo-1618005182384-a83a8bd57fbe?q=80&w=1200&auto=format&fit=crop",
  "https://images.unsplash.com/photo-1614036417651-efe5912149d8?q=80&w=1200&auto=format&fit=crop",
]

export function ComicReaderPage() {
  const runtimeState = resolveComicReaderRuntimeState(getHavenClientMode())

  if (runtimeState === "unavailable") {
    return <ComicReaderUnavailable variant="browser" />
  }

  return <ComicReaderExperience demoMode={runtimeState === "demo"} />
}

function ComicReaderUnavailable({ variant }: { variant: "production" | "browser" }) {
  const navigate = useNavigate()
  const productionBlocked = variant === "production"

  return (
    <div className="flex min-h-[100dvh] flex-col bg-[#0a0a0c] text-white">
      <header className="flex h-[68px] items-center gap-3 border-b border-white/10 px-5 sm:px-8">
        <button
          type="button"
          onClick={() => navigate(-1)}
          aria-label="返回"
          className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full transition-colors hover:bg-white/10"
        >
          <ArrowLeft className="h-4 w-4" />
        </button>
        <p className="text-sm font-semibold">漫画阅读</p>
      </header>
      <main className="flex flex-1 items-center justify-center px-6 text-center">
        <div className="max-w-sm space-y-2">
          <h1 className="text-lg font-semibold">
            {productionBlocked ? "本地漫画暂不可打开" : "当前无法打开漫画"}
          </h1>
          <p className="text-sm text-white/55">
            {productionBlocked
              ? "当前版本还不能读取这部漫画的页面，请返回作品页选择其他可用内容。"
              : "当前环境未连接栖阅本地阅读服务，请在桌面应用中重新打开此内容。"}
          </p>
        </div>
      </main>
    </div>
  )
}

function ComicPageView({
  page,
  src,
  alt,
  className,
  style,
  onLoad,
  onError,
  resourcePool,
}: {
  page: ComicPageModel | null
  src: string | null
  alt: string
  className?: string
  style?: React.CSSProperties
  onLoad?: React.ReactEventHandler<HTMLImageElement>
  onError?: React.ReactEventHandler<HTMLImageElement>
  resourcePool?: ComicPageResourcePool | null
}) {
  const [managedResource, setManagedResource] = useState<ComicPageResource | null>(null)
  const [managedError, setManagedError] = useState(false)
  const managed = Boolean(resourcePool && page?.availability === "ready")

  useEffect(() => {
    if (!managed || !resourcePool || !page || page.availability !== "ready") {
      setManagedResource(null)
      setManagedError(false)
      return
    }
    let disposed = false
    setManagedResource(null)
    setManagedError(false)
    const request = resourcePool.load(page.pageNumber)
    void request.then((result) => {
      if (disposed) {
        // A request can resolve between unmount and its cleanup. The resource
        // lease is consumer-specific and idempotent, so late completion cannot
        // release the next page's permit.
        if (result.status === "loaded") result.resource.release()
        return
      }
      if (result.status === "loaded") {
        setManagedResource(result.resource)
      } else if (result.status === "error") {
        setManagedError(true)
      }
    })
    return () => {
      disposed = true
      request.cancel()
    }
  }, [managed, page, resourcePool])

  const imageSrc = managed ? managedResource?.src ?? null : src
  if (!page || page.availability !== "ready" || !imageSrc) {
    return (
      <div className={cn("flex min-h-[180px] items-center justify-center rounded-md border border-white/10 bg-white/[0.03] px-5 text-center text-xs text-white/50", className)}>
        {managedError ? `第 ${page?.pageNumber ?? "?"} 页加载失败` : `第 ${page?.pageNumber ?? "?"} 页暂不可用`}
      </div>
    )
  }
  return (
    <img
      key={page.pageNumber}
      src={imageSrc}
      alt={alt}
      className={className}
      style={style}
      onLoad={(event) => {
        onLoad?.(event)
        if (managed) managedResource?.release()
      }}
      onError={(event) => {
        if (managed) {
          setManagedError(true)
          setManagedResource(null)
        }
        onError?.(event)
        if (managed) managedResource?.release()
      }}
      loading={managed ? "eager" : "lazy"}
    />
  )
}

function ComicManifestStatus({
  state,
  onRetry,
  onBack,
}: {
  state: ComicManifestState
  onRetry: () => void
  onBack: () => void
}) {
  const loading = state.status === "idle" || state.status === "loading"
  return (
    <div className="flex min-h-[100dvh] flex-col bg-[#0a0a0c] text-white">
      <header className="flex h-[68px] items-center gap-3 border-b border-white/10 px-5 sm:px-8">
        <button type="button" onClick={onBack} aria-label="返回" className="flex h-9 w-9 items-center justify-center rounded-full hover:bg-white/10">
          <ArrowLeft className="h-4 w-4" />
        </button>
        <p className="text-sm font-semibold">漫画阅读</p>
      </header>
      <main className="flex flex-1 items-center justify-center px-6 text-center">
        <div className="max-w-sm space-y-3">
          <h1 className="text-lg font-semibold">{loading ? "正在读取漫画页面" : "漫画页面不可用"}</h1>
          <p className="text-sm text-white/55">
            {loading ? "正在获取真实页面清单，请稍候。" : state.status === "error" ? state.message : ""}
          </p>
          {!loading && state.status === "error" && state.retryable && (
            <button type="button" onClick={onRetry} className="rounded-full bg-white px-4 py-2 text-xs font-semibold text-black hover:bg-white/85">
              重试
            </button>
          )}
        </div>
      </main>
    </div>
  )
}

function ComicReaderExperience({ demoMode }: { demoMode: boolean }) {
  const navigate = useNavigate()
  const { mediaItemId } = useParams<{ mediaItemId?: string }>()

  // Session + 进度：Tauri 环境接真实 useMediaSession（engine=comic）；
  // 浏览器演示环境保持既有静态演示内容。
  const { state, retry, registerReleaseBarrier } = useMediaSession(mediaItemId, "comic")
  const sessionView = selectComicSessionView(state, mediaItemId)
  const sessionId = state.status === "ready" ? state.session.sessionId : null
  const sessionIdentity = sessionId ? `${mediaItemId ?? ""}:${sessionId}` : null
  const preferenceEditionId = state.status === "ready" && state.session.mediaItemId === mediaItemId
    ? state.session.editionId
    : undefined
  const comicSettingsState = useComicSettings(mediaItemId, preferenceEditionId)
  const readerSessionIdentity = demoMode ? "demo" : sessionIdentity
  // Demo sequence construction must remain inside the browser Mock branch;
  // Tauri production only consumes the session-issued page manifest.
  const demoSequence = useMemo(
    () => demoMode ? createDemoComicPageSequence(DEMO_COMIC_PAGE_URLS) : null,
    [demoMode],
  )
  const [manifestState, setManifestState] = useState<ComicManifestState>(() => {
    if (demoSequence) return { status: "ready", sequence: demoSequence, sessionKey: "demo" }
    return { status: "idle" }
  })
  const [manifestRetry, setManifestRetry] = useState(0)
  const manifestGenerationRef = useRef(0)

  useEffect(() => {
    const generation = ++manifestGenerationRef.current
    let active = true
    if (demoMode && demoSequence) {
      setManifestState({ status: "ready", sequence: demoSequence, sessionKey: "demo" })
      return () => { active = false }
    }
    if (state.status !== "ready") {
      setManifestState(state.status === "opening" || state.status === "idle" ? { status: "loading" } : {
        status: "error",
        message: sessionView.message ?? "漫画会话不可用",
        retryable: sessionView.retryable,
      })
      return () => { active = false }
    }
    setManifestState({ status: "loading" })
    void getComicPageManifest(state.session)
      .then((manifest) => {
        if (active && manifestGenerationRef.current === generation) {
          setManifestState({ status: "ready", sequence: mapComicPageManifest(manifest), sessionKey: `${mediaItemId ?? ""}:${state.session.sessionId}` })
        }
      })
      .catch((error: unknown) => {
        if (!active || manifestGenerationRef.current !== generation) return
        const normalized = toHavenError(error)
        setManifestState({ status: "error", message: normalized.dto.userMessage, retryable: normalized.retryable })
      })
    return () => { active = false }
  }, [demoMode, demoSequence, manifestRetry, mediaItemId, sessionId, sessionView.message, sessionView.retryable, state])

  const currentSessionKey = sessionId ? `${mediaItemId ?? ""}:${sessionId}` : null
  const sequence = manifestState.status === "ready"
    && (demoMode ? manifestState.sessionKey === "demo" : manifestState.sessionKey === currentSessionKey)
    ? manifestState.sequence
    : null

  // 阅读器状态
  const [currentPage, setCurrentPage] = useState(1)
  const totalPages = sequence?.pageCount ?? 0
  const [viewMode, setViewMode] = useState<ViewMode>("single")
  const [direction, setDirection] = useState<ReadDirection>("rtl")
  const [pageGapPx, setPageGapPx] = useState<0 | 12 | 24>(12)
  const [preloadRadius, setPreloadRadius] = useState(3)
  const [fitMode] = useState<FitMode>("height")

  // Settings 只在每个新 Session 建立后应用一次。阅读器内的临时模式/方向
  // 切换不回写全局设置，也不能被稍晚返回的 settingsGet 覆盖。
  const sessionSettingsAppliedRef = useRef<string | null>(null)
  const sessionSettingsTouchedRef = useRef(false)

  const [showTools, setShowTools] = useState(true)
  const [isDrawerOpen, setIsDrawerOpen] = useState(false)
  const [activeTab, setActiveTab] = useState<"pages" | "chapters">("pages")
  useEffect(() => {
    if (!demoMode && activeTab !== "pages") setActiveTab("pages")
  }, [activeTab, demoMode])
  const visibleTab = demoMode ? activeTab : "pages"
  const [isBookmarked, setIsBookmarked] = useState(false)
  /** Tauri 环境创建成功后的后端标记 ID（供取消书签时软删除）。 */
  const [comicMarkerId, setComicMarkerId] = useState<string | null>(null)
  const [isBookmarkPending, setIsBookmarkPending] = useState(false)
  const [markersLoaded, setMarkersLoaded] = useState(false)
  const [sessionMarkers, setSessionMarkers] = useState<MarkerDto[]>([])
  const [isFullscreen, setIsFullscreen] = useState(false)
  const [zoomScale, setZoomScale] = useState(1)
  const [failedPages, setFailedPages] = useState<Set<number>>(new Set())
  const [, setResourcePoolRevision] = useState(0)

  const autoHideTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const stripScrollRef = useRef<HTMLDivElement>(null)
  const resourcePoolRef = useRef<ComicPageResourcePool | null>(null)
  const resourcePoolGenerationRef = useRef(0)
  const suppressClickRef = useRef(false)
  const touchRef = useRef<{
    x: number
    y: number
    startedAt: number
    pinchDistance: number | null
    pinchZoom: number
  } | null>(null)

  // 条漫模式只保留视口附近的页面节点，避免几百张图片同时进入 DOM。
  const [stripWindow, setStripWindow] = useState({ start: 1, end: 8 })
  const [stripViewportWidth, setStripViewportWidth] = useState(760)
  const [stripHeights, setStripHeights] = useState<Record<number, number>>({})
  const stripModeInitializedRef = useRef(false)
  const previousStripOffsetsRef = useRef<readonly number[] | null>(null)
  const previousStripAnchorPageRef = useRef<number | null>(null)

  // 图片尺寸缓存，用于跨页检测
  const [imageDimensions, setImageDimensions] = useState<Record<number, { width: number, height: number }>>({})
  const progressControllerRef = useRef<ComicProgressController | null>(null)
  const restoredProgressRef = useRef<string | null>(null)
  const markerListRequestRef = useRef(0)
  const bookmarkOperationRef = useRef(0)

  useLayoutEffect(() => {
    sessionSettingsAppliedRef.current = null
    sessionSettingsTouchedRef.current = false
    markerListRequestRef.current += 1
    bookmarkOperationRef.current += 1
    setSessionMarkers([])
    setIsBookmarked(false)
    setComicMarkerId(null)
    setIsBookmarkPending(false)
    setMarkersLoaded(false)
    restoredProgressRef.current = null
    setCurrentPage(1)
    setViewMode("single")
    setDirection("rtl")
    setPageGapPx(12)
    setPreloadRadius(3)
    setFailedPages(new Set())
    setImageDimensions({})
    setStripWindow({ start: 1, end: 8 })
    setStripHeights({})
    stripModeInitializedRef.current = false
    previousStripOffsetsRef.current = null
    previousStripAnchorPageRef.current = null
  }, [readerSessionIdentity])

  useEffect(() => {
    if (!readerSessionIdentity || comicSettingsState.status === "loading") return
    const expectedScopeKey = !demoMode && mediaItemId && preferenceEditionId
      ? `resource:${mediaItemId}:${preferenceEditionId}`
      : "global"
    if (comicSettingsState.scopeKey !== expectedScopeKey) return
    if (sessionSettingsAppliedRef.current === readerSessionIdentity) return

    // 用户已在设置加载完成前主动切换过会话模式/方向时，保留该临时选择，
    // 但仍应用不影响交互的页距和预加载窗口默认值。
    const defaults = resolveComicReaderDefaults(comicSettingsState.value)
    if (!sessionSettingsTouchedRef.current) {
      setViewMode(defaults.viewMode)
      setDirection(defaults.direction)
    }
    setPageGapPx(defaults.pageGapPx)
    setPreloadRadius(defaults.preloadRadius)
    sessionSettingsAppliedRef.current = readerSessionIdentity
  }, [comicSettingsState, demoMode, mediaItemId, preferenceEditionId, readerSessionIdentity])

  const changeViewMode = useCallback((next: ViewMode) => {
    sessionSettingsTouchedRef.current = true
    setViewMode(next)
  }, [])

  const changeDirection = useCallback((next: ReadDirection) => {
    sessionSettingsTouchedRef.current = true
    setDirection(next)
  }, [])

  useEffect(() => {
    const requestId = ++markerListRequestRef.current
    if (!isTauriRuntime() || !mediaItemId || sessionView.status !== "ready" || manifestState.status !== "ready") return

    void listMarkers(mediaItemId)
      .then((markers) => {
        if (markerListRequestRef.current === requestId) {
          setSessionMarkers(markers)
          setMarkersLoaded(true)
        }
      })
      .catch(() => {
        if (markerListRequestRef.current === requestId) {
          setSessionMarkers([])
          setMarkersLoaded(true)
        }
      })
  }, [mediaItemId, manifestState.status, sessionId, sessionView.status])

  useEffect(() => {
    if (!isTauriRuntime() || !mediaItemId || totalPages <= 0 || sessionView.status !== "ready" || manifestState.status !== "ready" || isBookmarkPending || !markersLoaded) return
    const marker = findComicBookmark(sessionMarkers, mediaItemId, Math.max(0, currentPage - 1))
    setIsBookmarked(marker !== null)
    setComicMarkerId(marker?.markerId ?? null)
  }, [currentPage, isBookmarkPending, manifestState.status, markersLoaded, mediaItemId, sessionMarkers, sessionView.status, totalPages])

  // 进度控制器生命周期：仅 Tauri 环境 session ready 时创建。
  useEffect(() => {
    progressControllerRef.current = null
    if (totalPages <= 0 || sessionView.status !== "ready" || state.status !== "ready" || manifestState.status !== "ready") return
    const controller = createComicProgressController({ session: state.session, totalPages, retry })
    progressControllerRef.current = controller
    registerReleaseBarrier(async () => {
      await controller.cleanup()
      resourcePoolRef.current?.dispose()
    })
    return () => { progressControllerRef.current = null; registerReleaseBarrier(null) }
  }, [manifestState.status, sessionIdentity, sessionView.status, state, retry, registerReleaseBarrier, totalPages])

  // 恢复进度：session ready 时恢复至上次阅读页。
  useEffect(() => {
    if (totalPages <= 0 || sessionView.status !== "ready" || state.status !== "ready" || manifestState.status !== "ready") return
    const restored = restoreComicProgress(totalPages, state.session, restoredProgressRef)
    if (restored) setCurrentPage(restored.pageIndex)
  }, [manifestState.status, sessionIdentity, sessionView.status, state, totalPages])

  // currentPage 变化时向进度控制器上报（Tauri 环境节流）。
  useEffect(() => {
    if (isTauriRuntime() && sessionView.status === "ready" && manifestState.status === "ready") {
      progressControllerRef.current?.pageChange(currentPage)
    }
  }, [currentPage, manifestState.status, sessionView.status])

  useEffect(() => {
    if (totalPages <= 0) return
    setCurrentPage((page) => Math.max(1, Math.min(totalPages, page)))
  }, [totalPages])

  /** 漫画书签：Tauri 环境创建/软删除真实 marker（comic Locator，pageIndex 转 0-based）。 */
  const toggleComicBookmark = () => {
    if (isBookmarkPending || (isTauriRuntime() && !markersLoaded)) return
    if (isTauriRuntime() && isBookmarked && comicMarkerId === null) return
    const next = !isBookmarked
    const removingMarkerId = comicMarkerId
    setIsBookmarked(next)
    if (!isTauriRuntime() || !mediaItemId) return
    markerListRequestRef.current += 1
    const operation = ++bookmarkOperationRef.current
    setIsBookmarkPending(true)
    if (next) {
      void createMarker({
        mediaItemId,
        locator: comicMarkerLocator(mediaItemId, Math.max(0, currentPage - 1)),
        markerType: "bookmark",
        title: null,
        excerpt: null,
        note: null,
      })
        .then((marker) => {
          if (bookmarkOperationRef.current !== operation) return
          setSessionMarkers((current) => [...current.filter((item) => item.markerId !== marker.markerId), marker])
          setComicMarkerId(marker.markerId)
        })
        .catch(() => {
          if (bookmarkOperationRef.current !== operation) return
          setIsBookmarked(false)
          setComicMarkerId(null)
        })
        .finally(() => {
          if (bookmarkOperationRef.current === operation) setIsBookmarkPending(false)
        })
      return
    }
    if (removingMarkerId) {
      const removedMarker = sessionMarkers.find((marker) => marker.markerId === removingMarkerId) ?? null
      setComicMarkerId(null)
      setSessionMarkers((current) => current.filter((marker) => marker.markerId !== removingMarkerId))
      void deleteMarker(removingMarkerId).catch(() => {
        if (bookmarkOperationRef.current !== operation) return
        if (removedMarker) setSessionMarkers((current) => [...current, removedMarker])
        setIsBookmarked(true)
        setComicMarkerId(removingMarkerId)
      }).finally(() => {
        if (bookmarkOperationRef.current === operation) setIsBookmarkPending(false)
      })
      return
    }
    setIsBookmarked(true)
    setIsBookmarkPending(false)
  }

  const getPage = useCallback((pageNumber: number) => pageAt(sequence, pageNumber), [sequence])
  const getImageSrc = useCallback((pageNumber: number) => {
    const page = getPage(pageNumber)
    if (!page || page.availability !== "ready" || failedPages.has(pageNumber)) return null
    return demoMode ? page.contentUri : null
  }, [demoMode, failedPages, getPage])

  const markPageFailed = useCallback((pageNumber: number, generation: number) => {
    if (resourcePoolGenerationRef.current !== generation) return
    setFailedPages((previous) => {
      if (previous.has(pageNumber)) return previous
      const next = new Set(previous)
      next.add(pageNumber)
      return next
    })
  }, [])

  // Each manifest/session gets a new bounded pool. Disposing it drops pending
  // handlers so a late image callback cannot update the next session.
  useEffect(() => {
    const generation = ++resourcePoolGenerationRef.current
    resourcePoolRef.current?.dispose()
    resourcePoolRef.current = null
    if (!sequence || demoMode) return
    const pool = new ComicPageResourcePool(sequence.pages, {
      maxConcurrent: 4,
      onChange: () => setResourcePoolRevision((value) => value + 1),
    })
    resourcePoolRef.current = pool
    setResourcePoolRevision((value) => value + 1)
    return () => {
      pool.dispose()
      if (resourcePoolRef.current === pool) resourcePoolRef.current = null
      if (resourcePoolGenerationRef.current === generation) resourcePoolGenerationRef.current += 1
    }
  }, [demoMode, sequence, sessionIdentity])

  const markPageLoaded = useCallback((pageNumber: number, event: React.SyntheticEvent<HTMLImageElement>, generation: number) => {
    if (resourcePoolGenerationRef.current !== generation) return
    const { naturalWidth: width, naturalHeight: height } = event.currentTarget
    if (!width || !height) return
    setImageDimensions((previous) => {
      const current = previous[pageNumber]
      if (current?.width === width && current.height === height) return previous
      return { ...previous, [pageNumber]: { width, height } }
    })
  }, [])

  const handlePageLoad = useCallback((pageNumber: number, event: React.SyntheticEvent<HTMLImageElement>, generation: number) => {
    markPageLoaded(pageNumber, event, generation)
  }, [markPageLoaded])

  const handlePageError = useCallback((pageNumber: number, generation: number) => {
    markPageFailed(pageNumber, generation)
  }, [markPageFailed])

  const isWidePage = useCallback((pageNum: number) => {
    const dim = imageDimensions[pageNum]
    return dim ? (dim.width / dim.height > 1.2) : false
  }, [imageDimensions])

  const thumbnailStart = totalPages > 0 ? Math.max(1, Math.min(totalPages, currentPage - 24)) : 1
  const thumbnailEnd = totalPages > 0 ? Math.min(totalPages, thumbnailStart + 48) : 0
  const thumbnailPages = useMemo(
    () => Array.from({ length: Math.max(0, thumbnailEnd - thumbnailStart + 1) }, (_, index) => thumbnailStart + index),
    [thumbnailEnd, thumbnailStart],
  )

  // 将页面编排成真正的 spread：宽幅页独占一屏，普通页才会和下一张组成跨页。
  // 当前页即使是跨页中的第二张，也会被归一到该 spread，避免跳页后出现错位。
  const spreads = useMemo(() => {
    const nextSpreads: number[][] = []
    let page = 1
    while (page <= totalPages) {
      const current = getPage(page)
      const following = getPage(page + 1)
      if (!current || current.availability !== "ready" || isWidePage(page)) {
        nextSpreads.push([page])
        page += 1
        continue
      }

      if (following?.availability === "ready" && !isWidePage(page + 1)) {
        nextSpreads.push([page, page + 1])
        page += 2
      } else {
        nextSpreads.push([page])
        page += 1
      }
    }
    return nextSpreads
  }, [getPage, isWidePage, totalPages])

  const getSpreadForPage = useCallback((page: number) => {
    return spreads.find((spread) => spread.includes(page)) || [page]
  }, [spreads])

  const retainedPageNumbers = useMemo(() => {
    const nearby = pageNumbersAround(currentPage, totalPages, preloadRadius)
    if (viewMode === "strip") {
      const mounted = Array.from(
        { length: Math.max(0, stripWindow.end - stripWindow.start + 1) },
        (_, index) => stripWindow.start + index,
      )
      const mountedAndNearby = [...new Set([...mounted, ...nearby])]
      return isDrawerOpen ? [...new Set([...mountedAndNearby, ...thumbnailPages])] : mountedAndNearby
    }
    if (viewMode === "double") {
      const spreadAndNearby = [...new Set([...getSpreadForPage(currentPage), ...nearby])]
      return isDrawerOpen ? [...new Set([...spreadAndNearby, ...thumbnailPages])] : spreadAndNearby
    }
    return isDrawerOpen ? [...new Set([...nearby, ...thumbnailPages])] : nearby
  }, [currentPage, getSpreadForPage, isDrawerOpen, preloadRadius, stripWindow, thumbnailPages, totalPages, viewMode])

  useEffect(() => {
    if (demoMode || !sequence || totalPages <= 0) return
    resourcePoolRef.current?.retain(retainedPageNumbers)
  }, [demoMode, retainedPageNumbers, sequence, totalPages])

  // 自动隐藏顶底工具栏
  const resetAutoHideTimer = useCallback(() => {
    setShowTools(true)
    if (autoHideTimeoutRef.current) clearTimeout(autoHideTimeoutRef.current)
    autoHideTimeoutRef.current = setTimeout(() => {
      if (!isDrawerOpen) {
        setShowTools(false)
      }
    }, 4500)
  }, [isDrawerOpen])

  useEffect(() => {
    resetAutoHideTimer()
    return () => {
      if (autoHideTimeoutRef.current) clearTimeout(autoHideTimeoutRef.current)
    }
  }, [resetAutoHideTimer])

  // 全屏状态同步
  useEffect(() => {
    const handleFullscreenChange = () => {
      setIsFullscreen(!!document.fullscreenElement)
    }
    document.addEventListener("fullscreenchange", handleFullscreenChange)
    return () => document.removeEventListener("fullscreenchange", handleFullscreenChange)
  }, [])

  // 翻页控制
  const nextPage = useCallback(() => {
    setCurrentPage((prev) => {
      if (viewMode !== "double") return Math.min(totalPages, prev + 1)
      const currentSpread = getSpreadForPage(prev)
      const currentIndex = spreads.findIndex((spread) => spread === currentSpread)
      const nextSpread = spreads[currentIndex + 1]
      return nextSpread ? nextSpread[0] : totalPages
    })
    resetAutoHideTimer()
  }, [getSpreadForPage, resetAutoHideTimer, spreads, totalPages, viewMode])

  const prevPage = useCallback(() => {
    setCurrentPage((prev) => {
      if (viewMode !== "double") return Math.max(1, prev - 1)
      const currentSpread = getSpreadForPage(prev)
      const currentIndex = spreads.findIndex((spread) => spread === currentSpread)
      return currentIndex > 0 ? spreads[currentIndex - 1][0] : 1
    })
    resetAutoHideTimer()
  }, [getSpreadForPage, resetAutoHideTimer, spreads, viewMode])

  // 按阅读方向处理左右点击翻页
  const handleLeftClick = () => {
    if (direction === "rtl") {
      nextPage() // 日漫: 点击左侧前进
    } else {
      prevPage() // 国漫: 点击左侧后退
    }
  }

  const handleRightClick = () => {
    if (direction === "rtl") {
      prevPage() // 日漫: 点击右侧后退
    } else {
      nextPage() // 国漫: 点击右侧前进
    }
  }

  // 键盘快捷键监听
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "ArrowLeft") {
        if (direction === "rtl") nextPage()
        else prevPage()
      } else if (e.key === "ArrowRight") {
        if (direction === "rtl") prevPage()
        else nextPage()
      } else if (e.key === " ") {
        e.preventDefault()
        nextPage()
      } else if (e.key === "Escape") {
        setIsDrawerOpen(false)
      } else if (e.key === "f" || e.key === "F") {
        toggleFullscreen()
      }
    }

    window.addEventListener("keydown", handleKeyDown)
    return () => window.removeEventListener("keydown", handleKeyDown)
  }, [direction, nextPage, prevPage])

  // 全屏切换
  const toggleFullscreen = () => {
    if (!document.fullscreenElement) {
      document.documentElement.requestFullscreen().catch(() => {})
      setIsFullscreen(true)
    } else {
      if (document.exitFullscreen) {
        document.exitFullscreen().catch(() => {})
      }
      setIsFullscreen(false)
    }
  }

  // 条漫滚动监听，同步进度
  const getStripPageHeight = useCallback((pageNum: number) => {
    // A bad page is rendered as the fixed 180px placeholder below. Using the
    // normal image fallback for it would make the virtualized offset table
    // reserve 420px for a 180px node, so the scroll position would drift every
    // time a broken page entered the window.
    const page = getPage(pageNum)
    if (!page || page.availability === "unavailable" || failedPages.has(pageNum)) return 180
    const dimensions = imageDimensions[pageNum]
    if (dimensions?.width && dimensions.height) {
      return stripViewportWidth * (dimensions.height / dimensions.width)
    }
    // Natural dimensions are populated before the measured DOM height. Use
    // the ratio first so a viewport resize cannot keep an old pixel height.
    if (stripHeights[pageNum]) return stripHeights[pageNum]
    return Math.max(420, stripViewportWidth * 1.35)
  }, [failedPages, getPage, imageDimensions, stripHeights, stripViewportWidth])

  const stripPageOffsets = useMemo(() => {
    const offsets = Array<number>(totalPages + 2).fill(0)
    for (let page = 1; page <= totalPages; page += 1) {
      const gapAfterPage = page < totalPages ? pageGapPx : 0
      offsets[page + 1] = offsets[page] + getStripPageHeight(page) + gapAfterPage
    }
    return offsets
  }, [getStripPageHeight, pageGapPx, totalPages])

  // Actual image dimensions arrive after the initial layout and a window
  // resize changes every page's pixel height. Preserve the current page's
  // viewport anchor when those offsets change; otherwise the same scrollTop
  // would make progress jump to an unrelated page.
  useLayoutEffect(() => {
    if (viewMode !== "strip") {
      previousStripOffsetsRef.current = null
      previousStripAnchorPageRef.current = null
      return
    }
    const previousOffsets = previousStripOffsetsRef.current
    const previousPage = previousStripAnchorPageRef.current
    const container = stripScrollRef.current
    if (container && previousOffsets && previousPage === currentPage) {
      const previousOffset = previousOffsets[currentPage] ?? 0
      const nextOffset = stripPageOffsets[currentPage] ?? 0
      const delta = nextOffset - previousOffset
      if (Number.isFinite(delta) && delta !== 0) container.scrollTop += delta
    }
    previousStripOffsetsRef.current = stripPageOffsets
    previousStripAnchorPageRef.current = currentPage
  }, [currentPage, stripPageOffsets, viewMode])

  const getStripPageOffset = useCallback((pageNum: number) => {
    return stripPageOffsets[Math.max(1, Math.min(totalPages + 1, pageNum))] || 0
  }, [stripPageOffsets, totalPages])

  const getStripWindowForPage = useCallback((page: number) => {
    const before = Math.max(4, preloadRadius)
    const after = Math.max(7, preloadRadius + 3)
    const nextStart = Math.max(1, page - before)
    const nextEnd = Math.min(totalPages, nextStart + before + after)
    return { start: nextStart, end: nextEnd }
  }, [preloadRadius, totalPages])

  const handleStripScroll = useCallback((e: React.UIEvent<HTMLDivElement>) => {
    const container = e.currentTarget
    const scrollTop = container.scrollTop
    const containerCenter = scrollTop + container.clientHeight / 2
    let low = 1
    let high = totalPages
    let newPage = totalPages
    while (low <= high) {
      const middle = Math.floor((low + high) / 2)
      if (containerCenter <= getStripPageOffset(middle + 1)) {
        newPage = middle
        high = middle - 1
      } else {
        low = middle + 1
      }
    }

    if (newPage !== currentPage) {
      setCurrentPage(newPage)
    }

    const { start: nextStart, end: nextEnd } = getStripWindowForPage(newPage)
    setStripWindow((prev) => {
      if (prev.start === nextStart && prev.end === nextEnd) return prev
      return { start: nextStart, end: nextEnd }
    })
  }, [currentPage, getStripPageOffset, getStripWindowForPage, totalPages])

  // 进度条拖动响应
  const handleSliderChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const page = Number(e.target.value)
    setCurrentPage(page)
    if (viewMode === "strip" && stripScrollRef.current) {
      setStripWindow(getStripWindowForPage(page))
      window.setTimeout(() => {
        const container = stripScrollRef.current
        if (container) {
          container.scrollTo({ top: getStripPageOffset(page) - container.clientHeight / 2 + getStripPageHeight(page) / 2, behavior: "smooth" })
        }
      }, 0)
    }
  }

  // 触屏：中心轻点显示工具栏，边缘轻点和左右滑动翻页，双指手势缩放。
  const getTouchDistance = (touches: React.TouchList) => {
    if (touches.length < 2) return null
    const first = touches[0]
    const second = touches[1]
    return Math.hypot(first.clientX - second.clientX, first.clientY - second.clientY)
  }

  const handleTouchStart = (e: React.TouchEvent<HTMLElement>) => {
    resetAutoHideTimer()
    const firstTouch = e.touches[0]
    const pinchDistance = getTouchDistance(e.touches)
    touchRef.current = {
      x: firstTouch.clientX,
      y: firstTouch.clientY,
      startedAt: Date.now(),
      pinchDistance,
      pinchZoom: zoomScale,
    }
  }

  const handleTouchMove = (e: React.TouchEvent<HTMLElement>) => {
    const gesture = touchRef.current
    if (!gesture) return
    const pinchDistance = getTouchDistance(e.touches)
    if (pinchDistance && gesture.pinchDistance) {
      e.preventDefault()
      setZoomScale(Math.min(2.5, Math.max(1, gesture.pinchZoom * (pinchDistance / gesture.pinchDistance))))
    }
  }

  const handleTouchEnd = (e: React.TouchEvent<HTMLElement>) => {
    const gesture = touchRef.current
    touchRef.current = null
    if (!gesture) return

    const lastTouch = e.changedTouches[0]
    const dx = lastTouch.clientX - gesture.x
    const dy = lastTouch.clientY - gesture.y
    const elapsed = Date.now() - gesture.startedAt
    const isSwipe = Math.abs(dx) > 48 && Math.abs(dx) > Math.abs(dy) * 1.15

    if (isSwipe) {
      suppressClickRef.current = true
      if (dx < 0) {
        if (direction === "rtl") nextPage()
        else prevPage()
      } else {
        if (direction === "rtl") prevPage()
        else nextPage()
      }
    } else if (elapsed < 450 && Math.abs(dx) < 18 && Math.abs(dy) < 18) {
      const viewportWidth = window.innerWidth
      if (lastTouch.clientX < viewportWidth * 0.28) {
        suppressClickRef.current = true
        handleLeftClick()
      } else if (lastTouch.clientX > viewportWidth * 0.72) {
        suppressClickRef.current = true
        handleRightClick()
      } else {
        setShowTools((prev) => !prev)
        suppressClickRef.current = true
      }
    }

    if (suppressClickRef.current) {
      window.setTimeout(() => { suppressClickRef.current = false }, 350)
    }
  }

  useEffect(() => {
    if (viewMode !== "strip") return
    const container = stripScrollRef.current
    if (!container) return

    const updateViewportWidth = () => {
      setStripViewportWidth(Math.max(320, Math.min(760, container.clientWidth - 32)))
    }
    updateViewportWidth()
    const resizeObserver = new ResizeObserver(updateViewportWidth)
    resizeObserver.observe(container)
    return () => resizeObserver.disconnect()
  }, [viewMode])

  useEffect(() => {
    if (viewMode !== "strip") {
      stripModeInitializedRef.current = false
      return
    }
    if (stripModeInitializedRef.current) return
    stripModeInitializedRef.current = true
    setStripWindow(getStripWindowForPage(currentPage))
    const frame = requestAnimationFrame(() => {
      const container = stripScrollRef.current
      if (!container) return
      container.scrollTo({
        top: getStripPageOffset(currentPage) - container.clientHeight / 2 + getStripPageHeight(currentPage) / 2,
        behavior: "auto",
      })
    })
    return () => cancelAnimationFrame(frame)
  }, [currentPage, getStripPageHeight, getStripPageOffset, getStripWindowForPage, totalPages, viewMode])

  if (!demoMode && (!sequence || manifestState.status !== "ready")) {
    return (
      <ComicManifestStatus
        state={manifestState.status === "ready" ? { status: "loading" } : manifestState}
        onRetry={() => {
          setManifestState({ status: "loading" })
          if (sessionView.status === "retryable_error") retry()
          else setManifestRetry((value) => value + 1)
        }}
        onBack={() => navigate(-1)}
      />
    )
  }

  if (!demoMode && sequence && totalPages === 0) {
    return <ComicManifestStatus state={{ status: "error", message: "这部漫画没有可阅读页面", retryable: false }} onRetry={() => undefined} onBack={() => navigate(-1)} />
  }

  const renderResourcePoolGeneration = resourcePoolGenerationRef.current
  const renderResourcePool = demoMode ? null : resourcePoolRef.current

  return (
    <div
      className="relative h-[100dvh] min-h-screen w-full overflow-hidden bg-[#0a0a0c] font-sans text-white select-none"
      onMouseMove={resetAutoHideTimer}
    >
      {/* 顶部毛玻璃导航栏 */}
      <header
        className={cn(
          "fixed left-0 right-0 top-0 z-40 flex h-[calc(4rem+env(safe-area-inset-top))] items-center justify-between border-b border-white/10 bg-black/70 px-[16px] pt-[env(safe-area-inset-top)] backdrop-blur-2xl transition-all duration-300 sm:px-6",
          showTools || isDrawerOpen ? "translate-y-0 opacity-100" : "-translate-y-full opacity-0"
        )}
      >
        <div className="flex items-center gap-[16px]">
          <button
            type="button"
            onClick={() => navigate(-1)}
            className="flex h-9 w-9 items-center justify-center rounded-full bg-white/10 hover:bg-white/20 transition-colors text-white cursor-pointer"
            title="返回作品页"
          >
            <ArrowLeft size={18} className="shrink-0" />
          </button>

          <div>
            <h1 className="text-sm font-bold tracking-tight text-white flex items-center gap-[8px]">
              <span>{demoMode ? "进击的巨人" : "漫画阅读"}</span>
              <span className="rounded-full bg-white/15 px-[8px] py-0.5 text-[10px] font-semibold text-white/80">漫画</span>
            </h1>
            <p className="text-xs text-white/50 font-medium">{demoMode ? "第 139 话：自由的彼岸（最终话） · " : "本地页面 · "}{mediaItemId || "comic"}</p>
          </div>
        </div>

        <div className="flex items-center gap-[8px]">
          {/* 打开抽屉：缩略图 / 章节 */}
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation()
              setIsDrawerOpen(!isDrawerOpen)
            }}
            className={cn(
              "flex items-center gap-1.5 rounded-full px-3 py-1.5 text-xs font-semibold transition-colors cursor-pointer",
              isDrawerOpen ? "bg-white text-black font-bold" : "bg-white/10 hover:bg-white/20 text-white"
            )}
          >
            <Grid size={15} className="shrink-0" />
            <span>页面与章节</span>
          </button>

          {/* 标记/书签 */}
          <button
            type="button"
            onClick={toggleComicBookmark}
            disabled={isBookmarkPending || (isTauriRuntime() && !markersLoaded)}
            className={cn(
              "flex h-9 w-9 items-center justify-center rounded-full transition-colors cursor-pointer disabled:cursor-not-allowed disabled:opacity-50",
              isBookmarked ? "bg-primary text-white" : "bg-white/10 hover:bg-white/20 text-white"
            )}
            title={isBookmarked ? "取消书签" : "添加书签"}
          >
            <Bookmark size={16} className={cn("shrink-0", isBookmarked ? "fill-current" : "")} />
          </button>

          {/* 全屏 */}
          <button
            type="button"
            onClick={toggleFullscreen}
            className="flex h-9 w-9 items-center justify-center rounded-full bg-white/10 hover:bg-white/20 transition-colors text-white cursor-pointer"
            title={isFullscreen ? "退出全屏" : "全屏模式"}
          >
            {isFullscreen ? <Minimize2 size={16} className="shrink-0" /> : <Maximize2 size={16} className="shrink-0" />}
          </button>
        </div>
      </header>

      {/* 主阅读区域 */}
      <main
        className="relative flex h-[100dvh] w-full items-center justify-center pt-[calc(4rem+env(safe-area-inset-top))] pb-[calc(5rem+env(safe-area-inset-bottom))]"
        onClick={() => {
          if (suppressClickRef.current) return
          setShowTools(prev => !prev)
        }}
        onTouchStart={handleTouchStart}
        onTouchMove={handleTouchMove}
        onTouchEnd={handleTouchEnd}
        style={{ touchAction: viewMode === "strip" ? "pan-y" : "none" }}
      >
        {/* 左右两侧交互翻页热区 */}
        {viewMode !== "strip" && (
          <>
            <div
              onClick={(e) => { e.stopPropagation(); handleLeftClick(); }}
              className="absolute bottom-[calc(5rem+env(safe-area-inset-bottom))] left-0 top-[calc(4rem+env(safe-area-inset-top))] z-20 flex w-[30%] cursor-pointer items-center justify-start pl-[16px] transition-colors group hover:bg-white/[0.02] sm:pl-6"
              title={direction === "rtl" ? "下一页 (日漫)" : "上一页"}
            >
              <div className="opacity-0 group-hover:opacity-100 transition-opacity p-3 rounded-full bg-black/60 backdrop-blur-md border border-white/10 text-white">
                <ChevronLeft size={24} className="shrink-0" />
              </div>
            </div>

            <div
              onClick={(e) => { e.stopPropagation(); handleRightClick(); }}
              className="absolute bottom-[calc(5rem+env(safe-area-inset-bottom))] right-0 top-[calc(4rem+env(safe-area-inset-top))] z-20 flex w-[30%] cursor-pointer items-center justify-end pr-[16px] transition-colors group hover:bg-white/[0.02] sm:pr-6"
              title={direction === "rtl" ? "上一页 (日漫)" : "下一页"}
            >
              <div className="opacity-0 group-hover:opacity-100 transition-opacity p-3 rounded-full bg-black/60 backdrop-blur-md border border-white/10 text-white">
                <ChevronRight size={24} className="shrink-0" />
              </div>
            </div>
          </>
        )}

        {/* 视图展现 1: 单页模式 */}
        {viewMode === "single" && (
          <div className="relative flex h-full w-full items-center justify-center overflow-hidden p-3 sm:p-[16px]">
            <ComicPageView
              page={getPage(currentPage)}
              src={getImageSrc(currentPage)}
              onLoad={(e) => { handlePageLoad(currentPage, e, renderResourcePoolGeneration); void e.currentTarget.decode?.() }}
              onError={() => handlePageError(currentPage, renderResourcePoolGeneration)}
              alt={`第 ${currentPage} 页`}
              className={cn(
                "max-h-full max-w-full rounded-md object-contain shadow-2xl transition-transform duration-200",
                fitMode === "height" && "h-full w-auto",
                fitMode === "width" && "w-full h-auto max-h-full",
                fitMode === "contain" && "max-h-full max-w-full"
              )}
              style={{ transform: `scale(${zoomScale})` }}
              resourcePool={renderResourcePool}
            />
          </div>
        )}

        {/* 视图展现 2: 双页/跨页模式 (跨页看日漫体验极佳) */}
        {viewMode === "double" && (
          <div
            className="relative flex h-full w-full items-center justify-center overflow-hidden p-3 sm:p-[16px]"
            style={{ gap: `${pageGapPx}px` }}
          >
            {(() => {
              const spread = getSpreadForPage(currentPage)
              const showTwo = spread.length === 2;
              if (showTwo) {
                return direction === "rtl" ? (
                  <>
                    <ComicPageView
                      page={getPage(spread[1])}
                      src={getImageSrc(spread[1])}
                      onError={() => handlePageError(spread[1], renderResourcePoolGeneration)}
                      onLoad={(e) => handlePageLoad(spread[1], e, renderResourcePoolGeneration)}
                      alt={`第 ${spread[1]} 页`}
                      className="h-full w-auto max-w-[48%] rounded-r-md object-contain shadow-2xl"
                      style={{ transform: `scale(${zoomScale})` }}
                      resourcePool={renderResourcePool}
                    />
                    <ComicPageView
                      page={getPage(spread[0])}
                      src={getImageSrc(spread[0])}
                      onError={() => handlePageError(spread[0], renderResourcePoolGeneration)}
                      onLoad={(e) => handlePageLoad(spread[0], e, renderResourcePoolGeneration)}
                      alt={`第 ${spread[0]} 页`}
                      className="h-full w-auto max-w-[48%] rounded-l-md object-contain shadow-2xl"
                      style={{ transform: `scale(${zoomScale})` }}
                      resourcePool={renderResourcePool}
                    />
                  </>
                ) : (
                  <>
                    <ComicPageView
                      page={getPage(spread[0])}
                      src={getImageSrc(spread[0])}
                      onError={() => handlePageError(spread[0], renderResourcePoolGeneration)}
                      onLoad={(e) => handlePageLoad(spread[0], e, renderResourcePoolGeneration)}
                      alt={`第 ${spread[0]} 页`}
                      className="h-full w-auto max-w-[48%] rounded-l-md object-contain shadow-2xl"
                      style={{ transform: `scale(${zoomScale})` }}
                      resourcePool={renderResourcePool}
                    />
                    <ComicPageView
                      page={getPage(spread[1])}
                      src={getImageSrc(spread[1])}
                      onError={() => handlePageError(spread[1], renderResourcePoolGeneration)}
                      onLoad={(e) => handlePageLoad(spread[1], e, renderResourcePoolGeneration)}
                      alt={`第 ${spread[1]} 页`}
                      className="h-full w-auto max-w-[48%] rounded-r-md object-contain shadow-2xl"
                      style={{ transform: `scale(${zoomScale})` }}
                      resourcePool={renderResourcePool}
                    />
                  </>
                )
              } else {
                return (
                  <ComicPageView
                    page={getPage(spread[0])}
                    src={getImageSrc(spread[0])}
                    onError={() => handlePageError(spread[0], renderResourcePoolGeneration)}
                    onLoad={(e) => handlePageLoad(spread[0], e, renderResourcePoolGeneration)}
                    alt={`第 ${spread[0]} 页`}
                    className="h-full w-full rounded-md object-contain shadow-2xl"
                    style={{ transform: `scale(${zoomScale})` }}
                    resourcePool={renderResourcePool}
                  />
                )
              }
            })()}
          </div>
        )}

        {/* 视图展现 3: 连续条漫模式 (Webtoon Vertical Scroll) */}
        {viewMode === "strip" && (
          <div
            ref={stripScrollRef}
            onScroll={handleStripScroll}
            className="relative h-full w-full overflow-y-auto px-[16px] py-[32px] flex flex-col items-center custom-scrollbar [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
            // Keep virtual offsets authoritative. A flex `gap` also applies
            // between the top/bottom spacer and its neighbour, which creates
            // phantom spacing not represented by stripPageOffsets. Page gaps
            // are attached to the page wrappers instead.
            style={{ gap: 0 }}
          >
            <div
              aria-hidden="true"
              style={{ height: getStripPageOffset(stripWindow.start) }}
            />
            {Array.from({ length: stripWindow.end - stripWindow.start + 1 }).map((_, index) => {
              const pageNum = stripWindow.start + index
              return (
                <div
                  key={pageNum}
                  data-page={pageNum}
                  className="relative w-full max-w-[760px]"
                  style={{ marginBottom: pageNum < totalPages ? pageGapPx : 0 }}
                >
                  <ComicPageView
                    page={getPage(pageNum)}
                    src={getImageSrc(pageNum)}
                    onError={() => handlePageError(pageNum, renderResourcePoolGeneration)}
                    alt={`第 ${pageNum} 页`}
                    className="w-full h-auto object-contain block"
                    resourcePool={renderResourcePool}
                    onLoad={(e) => {
                      handlePageLoad(pageNum, e, renderResourcePoolGeneration)
                      // React 的事件对象在异步 state updater 执行前可能已经释放 currentTarget。
                      // 先同步读取高度，再把纯数字交给 updater，避免窗口化节点切换时出现空引用。
                      const height = e.currentTarget.getBoundingClientRect().height
                      setStripHeights((prev) => ({ ...prev, [pageNum]: height }))
                    }}
                  />
                  <span className="absolute bottom-3 right-[16px] rounded-full bg-black/70 px-[10px] py-1 text-[10px] font-bold text-white/80 backdrop-blur-md">
                    {pageNum} / {totalPages}
                  </span>
                </div>
              )
            })}
            <div
              aria-hidden="true"
              style={{ height: Math.max(0, getStripPageOffset(totalPages + 1) - getStripPageOffset(stripWindow.end + 1)) }}
            />
          </div>
        )}
      </main>

      {/* 底部悬浮控制 Bar (Apple Glassmorphism Pill) */}
      <footer
        onClick={(e) => e.stopPropagation()}
        className={cn(
          "fixed bottom-[calc(1rem+env(safe-area-inset-bottom))] left-1/2 z-40 flex max-w-[calc(100vw-1rem)] -translate-x-1/2 items-center gap-3 overflow-x-auto rounded-full border border-white/15 bg-black/75 px-3 py-[10px] shadow-2xl backdrop-blur-2xl transition-all duration-300 sm:bottom-[calc(1.5rem+env(safe-area-inset-bottom))] sm:gap-[16px] sm:px-6 sm:py-3",
          showTools || isDrawerOpen ? "translate-y-0 opacity-100" : "translate-y-[96px] opacity-0 pointer-events-none"
        )}
      >
        {/* 上一页 / (RTL时为下一页) */}
        <button
          type="button"
          onClick={direction === "rtl" ? nextPage : prevPage}
          disabled={direction === "rtl" ? currentPage >= totalPages : currentPage <= 1}
          className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full bg-white/10 hover:bg-white/20 disabled:opacity-30 disabled:pointer-events-none transition-colors cursor-pointer"
          title={direction === "rtl" ? "下一页" : "上一页"}
        >
          <ChevronLeft size={18} className="shrink-0" />
        </button>

        {/* 进度条与数字 */}
        <div className="flex items-center gap-3">
          <span className="text-xs font-extrabold tracking-widest text-white/90 min-w-[50px] text-center">
            {currentPage} / {totalPages}
          </span>

          <input
            type="range"
            min={1}
            max={totalPages}
            value={currentPage}
            onChange={handleSliderChange}
            className="w-28 sm:w-44 h-1.5 bg-white/20 rounded-lg appearance-none cursor-pointer accent-primary"
          />
        </div>

        {/* 下一页 / (RTL时为上一页) */}
        <button
          type="button"
          onClick={direction === "rtl" ? prevPage : nextPage}
          disabled={direction === "rtl" ? currentPage <= 1 : currentPage >= totalPages}
          className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full bg-white/10 hover:bg-white/20 disabled:opacity-30 disabled:pointer-events-none transition-colors cursor-pointer"
          title={direction === "rtl" ? "上一页" : "下一页"}
        >
          <ChevronRight size={18} className="shrink-0" />
        </button>

        <div className="h-[16px] w-px bg-white/20 shrink-0" />

        {/* 视图模式切换 */}
        <div className="flex items-center gap-1 bg-white/10 p-1 rounded-full">
          <button
            type="button"
            onClick={() => changeViewMode("single")}
            className={cn(
              "px-3 py-1 rounded-full text-xs font-bold transition-all cursor-pointer",
              viewMode === "single" ? "bg-white text-black shadow-sm" : "text-white/70 hover:text-white"
            )}
          >
            单页
          </button>
          <button
            type="button"
            onClick={() => changeViewMode("double")}
            className={cn(
              "px-3 py-1 rounded-full text-xs font-bold transition-all cursor-pointer",
              viewMode === "double" ? "bg-white text-black shadow-sm" : "text-white/70 hover:text-white"
            )}
          >
            双页
          </button>
          <button
            type="button"
            onClick={() => changeViewMode("strip")}
            className={cn(
              "px-3 py-1 rounded-full text-xs font-bold transition-all cursor-pointer",
              viewMode === "strip" ? "bg-white text-black shadow-sm" : "text-white/70 hover:text-white"
            )}
          >
            条漫
          </button>
        </div>

        {/* 阅读方向切换 (RTL 日漫 vs LTR 国漫) */}
        <button
          type="button"
          onClick={() => changeDirection(direction === "rtl" ? "ltr" : "rtl")}
          className="flex items-center gap-1.5 px-3 py-1.5 rounded-full bg-white/10 hover:bg-white/20 text-xs font-bold transition-colors cursor-pointer"
          title="切换日漫/国漫翻页方向"
        >
          <span>{direction === "rtl" ? "日漫 (RTL)" : "国漫 (LTR)"}</span>
        </button>
      </footer>

      {/* 缩略图 / 章节滑动侧栏抽屉 (Drawer) FULL REDESIGN */}
      <aside
        onClick={(e) => e.stopPropagation()}
        className={cn(
          "fixed top-0 right-0 bottom-0 z-50 w-full max-w-[360px] bg-[#0f1014]/95 backdrop-blur-3xl border-l border-white/10 shadow-2xl flex flex-col transition-transform duration-300 ease-out",
          isDrawerOpen ? "translate-x-0" : "translate-x-full"
        )}
      >
        {/* 抽屉头部 (Apple Style) */}
        <div className="flex flex-col pt-3 pb-[8px] px-6 border-b border-white/10">
          <div className="w-10 h-1 bg-white/20 rounded-full mx-auto mb-[16px]" /> {/* Drag indicator */}
          <div className="flex items-center justify-between">
            <span className="text-sm font-bold text-white">{demoMode ? "页面与章节" : "漫画页面"}</span>
            <button
              onClick={() => setIsDrawerOpen(false)}
              className="flex items-center gap-1 bg-white/10 hover:bg-white/20 px-3 py-1.5 rounded-full text-xs font-medium text-white transition-colors cursor-pointer"
            >
              <X size={14} className="shrink-0" />
              关闭
            </button>
          </div>
        </div>

        {/* Tab Switcher Segmented Control */}
        <div className="px-6 py-[16px] shrink-0">
          <div className="flex p-1 bg-black/40 rounded-xl relative">
            <button
              onClick={() => setActiveTab("pages")}
              className={cn(
                "flex-1 py-1.5 text-xs font-semibold rounded-lg z-10 transition-colors cursor-pointer",
                visibleTab === "pages" ? "text-black" : "text-white/70 hover:text-white"
              )}
            >
              全页缩略图
            </button>
            {demoMode && (
              <button
                onClick={() => setActiveTab("chapters")}
                className={cn(
                  "flex-1 py-1.5 text-xs font-semibold rounded-lg z-10 transition-colors cursor-pointer",
                  visibleTab === "chapters" ? "text-black" : "text-white/70 hover:text-white"
                )}
              >
                话数列表
              </button>
            )}
            {/* Animated background pill */}
            <div className={cn(
               "absolute top-1 bottom-1 bg-white rounded-lg transition-transform duration-300 ease-out",
               demoMode ? "w-[calc(50%-4px)]" : "left-1 right-1",
               visibleTab === "pages" ? "translate-x-0" : "translate-x-full"
            )} />
          </div>
        </div>

        {/* Tab 1: 全页缩略图 (3-column grid) */}
        {visibleTab === "pages" && (
          <div className="flex-1 overflow-y-auto px-6 pb-6 grid grid-cols-3 gap-3 custom-scrollbar">
            {thumbnailStart > 1 && <div aria-hidden="true" className="col-span-3" style={{ height: `${Math.floor((thumbnailStart - 1) / 3) * 132}px` }} />}
            {thumbnailPages.map((pageNum) => {
              const isSelected = currentPage === pageNum
              return (
                <button
                  key={pageNum}
                  type="button"
                  onClick={() => {
                    setCurrentPage(pageNum)
                    setIsDrawerOpen(false)
                  }}
                  className={cn(
                    "group relative aspect-[3/4] overflow-hidden rounded-xl border transition-all text-left bg-black/40 cursor-pointer shadow-sm",
                    isSelected
                      ? "border-primary ring-2 ring-primary/50 scale-105"
                      : "border-white/10 hover:border-white/40"
                  )}
                >
                  <ComicPageView
                    page={getPage(pageNum)}
                    src={demoMode ? getImageSrc(pageNum) : null}
                    onError={() => handlePageError(pageNum, renderResourcePoolGeneration)}
                    onLoad={(e) => handlePageLoad(pageNum, e, renderResourcePoolGeneration)}
                    alt={`Page ${pageNum}`}
                    className="h-full w-full object-cover transition-transform duration-300 group-hover:scale-105"
                    resourcePool={renderResourcePool}
                  />
                  <div className="absolute inset-x-0 bottom-0 bg-gradient-to-t from-black/80 to-transparent p-[8px] pt-6">
                    <span className="text-[11px] font-bold text-white drop-shadow-md">
                      {pageNum}
                    </span>
                  </div>
                  {isSelected && (
                    <div className="absolute top-1.5 right-1.5 bg-primary rounded-full p-0.5 shadow-md">
                      <Check size={12} className="text-white shrink-0" />
                    </div>
                  )}
                </button>
              )
            })}
            {thumbnailEnd < totalPages && <div aria-hidden="true" className="col-span-3" style={{ height: `${Math.floor((totalPages - thumbnailEnd) / 3) * 132}px` }} />}
          </div>
        )}

        {/* Tab 2: 章节列表 (Elegant Cards) */}
        {demoMode && visibleTab === "chapters" && (
          <div className="flex-1 overflow-y-auto px-6 pb-6 space-y-3 custom-scrollbar">
            {DEMO_CHAPTERS.map((chapter) => {
              const isActive = chapter.id === "c139"
              return (
                <button
                  key={chapter.id}
                  type="button"
                  onClick={() => {
                    setCurrentPage(1)
                    setIsDrawerOpen(false)
                  }}
                  className={cn(
                    "w-full rounded-2xl p-[16px] text-left transition-all border relative overflow-hidden group cursor-pointer",
                    isActive
                      ? "bg-white/10 border-white/20 text-white shadow-lg"
                      : "bg-white/5 border-transparent text-white/70 hover:bg-white/10 hover:text-white"
                  )}
                >
                  {isActive && (
                    <div className="absolute left-0 top-0 bottom-0 w-1 bg-primary" />
                  )}
                  <div className="flex justify-between items-center mb-1">
                    <p className={cn("text-sm", isActive ? "font-bold" : "font-semibold")}>
                      {chapter.title}
                    </p>
                  </div>
                  <div className="flex items-center justify-between mt-[8px]">
                    <p className="text-xs text-white/40">{chapter.pagesCount} 页</p>
                    {isActive && (
                      <span className="text-[10px] text-primary font-bold bg-primary/10 px-[8px] py-0.5 rounded-full">
                        当前阅读
                      </span>
                    )}
                  </div>
                  {/* Subtle progress bar inside card */}
                  <div className="mt-3 w-full h-1 bg-white/5 rounded-full overflow-hidden">
                     <div
                       className={cn("h-full rounded-full transition-all duration-300", isActive ? "bg-primary" : "bg-white/20")}
                       style={{ width: isActive ? `${(currentPage / totalPages) * 100}%` : '0%' }}
                     />
                  </div>
                </button>
              )
            })}
          </div>
        )}
      </aside>
    </div>
  )
}
