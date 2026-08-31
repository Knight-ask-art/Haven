import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from "react"
import { useNavigate, useParams } from "react-router"
import { ArrowLeft, Bookmark, ChevronLeft, ChevronRight, ListTree, Minus, Plus, Search, Trash2, X } from "lucide-react"
import { cn } from "@/lib/utils"
import ReactMarkdown from "react-markdown"
import { recordHistory } from "@/lib/havenState"
import { getHavenClientMode, type HavenClientMode } from "@/lib/ipc/runtime"
import { HavenError, toHavenError, type HavenError as HavenErrorDto } from "@/lib/ipc/errors"
import { useMediaSession } from "@/features/session/useMediaSession"
import { fetchSessionResource } from "@/features/session/ipc/resource-fetch"
import {
  loadDemoBookReaderBookmarks,
  recordDemoBookReaderHistory,
  resolveBookReaderRuntimeState,
} from "../lib/book-reader-runtime-state"
import { selectReaderSessionView } from "../lib/reader-session-view"
import { createBookProgressController, restoreBookProgress, type BookProgressController } from "../lib/book-progress-controller"
import { decodeBookText, parseBookText, type BookChapter, type BookContentFormat } from "../lib/book-content"
import { parseEpubBook } from "../lib/epub-content"
import { findBookBookmark } from "../lib/book-marker-match"
import { isPdfMimeType } from "../lib/pdf-reader-state"
import { PDF_RANGE_CHUNK_SIZE } from "../lib/pdf-document"
import {
  createPdfProgressController,
  restorePdfProgress,
  type PdfProgressController,
} from "../lib/pdf-progress-controller"
import { PdfReader } from "../components/PdfReader"
import type { PdfDocumentSource } from "../lib/pdf-document"
import { useReadingSettings } from "../lib/use-reading-settings"
import { resolveReadingPresentation } from "../lib/reading-settings-mapping"
import { bookMarkerLocator, createMarker, deleteMarker, listMarkers } from "@/features/markers/ipc/marker-gateway"
import type { MarkerDto, TocItemDto } from "@/lib/ipc/generated/wire"
import { getReaderToc } from "../ipc/reader-toc-gateway"
import { buildBookSearchIndex, searchBook, type BookSearchHit } from "../lib/book-search"
import { searchReaderContent } from "../ipc/reader-search-gateway"
import {
  alignBookOffsetToPage,
  bookOffsetForPageDelta,
  bookOffsetForProgression,
  getBookPaginationMetrics,
  setBookPaginationOffsetInstant,
  type BookPaginationMode,
  type BookPaginationViewport,
} from "../lib/book-pagination"

type ReaderTheme = "paper" | "warm" | "slate" | "dark" | "sepia" | "eyeCare" | "custom"
type FontFamily = "sans" | "serif" | "kai" | "heiti" | "fangsong" | "mianfei" | "custom"
type ColumnWidth = "narrow" | "medium" | "wide"
type ReaderLineHeight = "compact" | "comfortable" | "airy"

const PAGINATION_OPTIONS: Array<{ id: BookPaginationMode; label: string }> = [
  { id: "scroll", label: "连续" },
  { id: "paginated", label: "分页" },
  { id: "double", label: "双页" },
]

interface BookmarkType {
  id: string
  progress: number
  scrollTop: number
  /** 新增分页模式的水平位置；旧演示书签缺失时按 progress 恢复。 */
  scrollLeft?: number
  chapterId: string
  chapterTitle: string
  timestamp: string
  /** 后端标记 ID（Tauri 环境创建成功后回填；浏览器演示环境为 undefined）。 */
  markerId?: string
}

type BookContentState =
  | { status: "idle" }
  | { status: "loading"; contentUri: string }
  | { status: "ready"; contentUri: string; chapters: BookChapter[]; format: BookContentFormat; title: string | null }
  | { status: "pdf_ready"; contentUri: string; source: PdfDocumentSource }
  | { status: "empty"; contentUri: string }
  | { status: "retryable_error"; contentUri: string; error: HavenErrorDto }
  | { status: "terminal_error"; contentUri: string; error: HavenErrorDto }

const BOOK_CHAPTERS: BookChapter[] = [
  {
    id: "ch1",
    kicker: "Chapter 1",
    title: "序言 · 重新定义个人内容空间",
    paragraphs: [
      "离开苹果之后，乔布斯并没有离开技术世界。他带着一群相信未来的人重新开始，试图证明一台计算机可以不仅仅是一台工具，而是一种关于工作与思考方式的独特表达。",
      "在这个信息过载的时代，我们每天被无数的碎片信息淹没。内容消费变得前所未有的容易，但真正属于个人的、能够沉淀下来的思考却变得稀缺。",
    ],
  },
  {
    id: "ch2",
    kicker: "Chapter 2",
    title: "第一章 · 硅谷的王者归来与 NeXT 时代",
    paragraphs: [
      "NeXT 的故事并不顺利。产品昂贵、市场狭窄，公司的方向经历了几次剧烈的摆动。但正是在这些巨大的不确定中，新的软件架构逐渐成形，后来成为苹果重新获得生命力的基石。",
      "很多时候，创新并不是在一片坦途上发生的。它往往诞生于限制和困境之中。当时的人们很难想象，这样一个被视为失败的创业项目，最终会孕育出现代操作系统的心脏。",
    ],
  },
  {
    id: "ch3",
    kicker: "Chapter 3",
    title: "第二章 · 软件架构的复兴之路",
    paragraphs: [
      "当我们将目光转向软件架构，会发现它并非冰冷的代码堆砌，而是一种关于如何组织思想和信息的艺术。优秀的架构能够经受住时间的考验，像经典的建筑一样历久弥新。",
      "好的架构不会把内部的复杂性直接展示给使用者。它将选择整理成清晰的路径，让人可以把注意力留给真正重要的问题。",
    ],
    quote: "真正重要的不是你拥有多少资源，而是你是否足够清楚什么东西才值得被创造出来。",
  },
  {
    id: "ch4",
    kicker: "Chapter 4",
    title: "第三章 · 跨媒介协同与信息流净化",
    paragraphs: [
      "好的产品不会把内部的复杂性直接剥开展示给用户。它把无数种可能性精心整理成清晰、优雅的路径，让人可以彻底专注于内容本身。",
      "文章、图书与其他媒体最终都应该回到同一个作品上下文中。用户不需要记住内容来自哪个入口，只需要知道下一次从哪里继续。",
    ],
  },
  {
    id: "ch5",
    kicker: "Chapter 5",
    title: "第四章 · 留给未来的记忆锚点",
    paragraphs: [
      "当我们阅读时，我们也是在时间的河流中为自己抛下锚点。一个高亮，一句批注，都在试图捕捉那一瞬间思维的火花。",
      "阅读进度不只是一个百分比。它是章节、文字锚点、最近一次阅读位置和用户留下的标记共同组成的记忆。",
    ],
  },
  {
    id: "ch6",
    kicker: "Epilogue",
    title: "尾声 · 寻找值得被做出来的造物",
    paragraphs: [
      "在技术狂飙突进的时代，我们更需要慢下来思考。创造并不是盲目的增加，而是克制的选择。",
      "真正值得留下的工具，会在用户需要时出现，在用户开始思考时安静退后。",
    ],
  },
]
const EMPTY_BOOK_CHAPTERS: BookChapter[] = []

const THEME_OPTIONS: Array<{ id: ReaderTheme; label: string; color: string }> = [
  { id: "paper", label: "纸白", color: "#fcfcfc" },
  { id: "warm", label: "暖纸", color: "#f5efe3" },
  { id: "slate", label: "石板", color: "#292a2d" },
  { id: "dark", label: "墨黑", color: "#0f0f11" },
  { id: "sepia", label: "复古", color: "#f4ecd8" },
  { id: "eyeCare", label: "护眼", color: "#cce8cc" },
  { id: "custom", label: "自定义", color: "#e8e0d0" },
]

const FONT_OPTIONS: Array<{ id: FontFamily; label: string; className: string }> = [
  { id: "sans", label: "黑体", className: "font-sans" },
  { id: "serif", label: "宋体", className: "font-serif" },
  { id: "kai", label: "楷体", className: "font-serif italic" },
  { id: "heiti", label: "黑体", className: "font-sans font-bold" },
  { id: "fangsong", label: "仿宋", className: "font-serif" },
  { id: "mianfei", label: "免费", className: "font-sans" },
  { id: "custom", label: "自定义", className: "font-sans" },
]

type ActiveBookReaderMode = Exclude<HavenClientMode, "unavailable">

export function BookReaderPage() {
  const clientMode = getHavenClientMode()

  if (clientMode === "unavailable") return <BookReaderUnavailable />

  return <BookReaderExperience clientMode={clientMode} />
}

function BookReaderUnavailable() {
  const navigate = useNavigate()

  return (
    <div className="flex min-h-[100dvh] flex-col bg-[#f5efe3] text-[#3c332b]">
      <header className="flex h-[68px] items-center gap-3 border-b border-black/[0.07] px-5 sm:px-8">
        <button type="button" onClick={() => navigate(-1)} aria-label="返回" className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full transition-colors hover:bg-black/[0.06]">
          <ArrowLeft className="h-4 w-4" />
        </button>
        <p className="text-sm font-semibold">图书阅读</p>
      </header>
      <main className="flex flex-1 items-center justify-center px-6 text-center">
        <div className="max-w-sm space-y-2">
          <h1 className="text-lg font-semibold">当前无法打开图书</h1>
          <p className="text-sm opacity-60">当前环境未连接栖阅本地阅读服务，请在桌面应用中重新打开此内容。</p>
        </div>
      </main>
    </div>
  )
}

function BookReaderExperience({ clientMode }: { clientMode: ActiveBookReaderMode }) {
  const navigate = useNavigate()
  const { mediaItemId } = useParams<{ mediaItemId?: string }>()
  const runtimeState = resolveBookReaderRuntimeState(clientMode)
  const tauriRuntime = runtimeState === "production"
  const demoRuntime = runtimeState === "demo"
  const storageId = mediaItemId || "book-jobs"
  const bookmarkStorageKey = `haven:bookmarks:${storageId}`

  // Session + 进度：Tauri 环境接真实 useMediaSession（engine=reader）；
  // 浏览器演示环境保持既有静态演示内容。
  const { state, retry, registerReleaseBarrier } = useMediaSession(mediaItemId, "reader")
  const sessionView = selectReaderSessionView(state, mediaItemId)
  const sessionContentUri = sessionView.contentUri
  const preferenceEditionId = state.status === "ready" && state.session.mediaItemId === mediaItemId
    ? state.session.editionId
    : undefined
  const { settings: readingSettings, status: readingSettingsStatus, scopeKey: readingSettingsScopeKey } = useReadingSettings(mediaItemId, preferenceEditionId)

  const [theme, setTheme] = useState<ReaderTheme>("warm")
  const [fontFamily, setFontFamily] = useState<FontFamily>("serif")
  const [fontSize, setFontSize] = useState(18)
  const [columnWidth, setColumnWidth] = useState<ColumnWidth>("medium")
  const [lineHeight, setLineHeight] = useState<ReaderLineHeight>("comfortable")
  const [paginationMode, setPaginationMode] = useState<BookPaginationMode>("scroll")
  const [paginationPageCount, setPaginationPageCount] = useState(1)
  const [paginationPageIndex, setPaginationPageIndex] = useState(0)
  const [paginationViewportWidth, setPaginationViewportWidth] = useState(0)
  const [showTools, setShowTools] = useState(true)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [drawerOpen, setDrawerOpen] = useState(false)
  const [drawerTab, setDrawerTab] = useState<"toc" | "bookmarks" | "search">("toc")
  const [searchQuery, setSearchQuery] = useState("")
  const [searchHits, setSearchHits] = useState<BookSearchHit[]>([])
  const [searchStatus, setSearchStatus] = useState<"idle" | "searching" | "done">("idle")
  const [highlightedHitKey, setHighlightedHitKey] = useState<string | null>(null)
  const [readingProgress, setReadingProgress] = useState(0)
  const [activeChapterId, setActiveChapterId] = useState(() => demoRuntime ? BOOK_CHAPTERS[0].id : "")
  const [bookmarks, setBookmarks] = useState<BookmarkType[]>(() => (
    loadDemoBookReaderBookmarks(clientMode, () => readBookmarks(bookmarkStorageKey))
  ))
  const [sessionMarkers, setSessionMarkers] = useState<MarkerDto[]>([])
  const [isBookmarkPending, setIsBookmarkPending] = useState(false)
  const [markersLoaded, setMarkersLoaded] = useState(false)
  const [contentState, setContentState] = useState<BookContentState>({ status: "idle" })
  const [contentRetryNonce, setContentRetryNonce] = useState(0)
  const [readerTocItems, setReaderTocItems] = useState<TocItemDto[] | null>(null)
  const searchAbortRef = useRef<AbortController | null>(null)
  const readerScrollRef = useRef<HTMLDivElement>(null)
  const articleRef = useRef<HTMLElement>(null)
  const progressControllerRef = useRef<BookProgressController | null>(null)
  const pdfProgressControllerRef = useRef<PdfProgressController | null>(null)
  const restoredProgressRef = useRef<string | null>(null)
  const restoredPdfProgressRef = useRef<string | null>(null)
  const resourceRequestRef = useRef(0)
  const markerListRequestRef = useRef(0)
  const tocRequestRef = useRef(0)
  const navigationOperationRef = useRef(0)
  const bookmarkOperationRef = useRef(0)
  const readingSettingsAppliedRef = useRef(false)
  const readingSettingsTouchedRef = useRef(false)
  const previousPaginationModeRef = useRef<BookPaginationMode | null>(null)
  const latestBookProgressionRef = useRef(0)
  // A newly opened Session must restore the server locator before the first
  // layout measurement is allowed to report progress.  Otherwise the initial
  // horizontal offset (always 0) can be flushed as a real progress update
  // before the persisted position has been mapped to the current columns.
  const progressRestoreSettledRef = useRef(!tauriRuntime)
  // A mode change rerenders the columns before the new layout effect can read
  // the old axis. Capture the format-independent progression in the click
  // handler so the first measurement of the new layout cannot overwrite it.
  const pendingPaginationProgressionRef = useRef<number | null>(null)

  const markReadingSettingsTouched = () => {
    readingSettingsTouchedRef.current = true
  }

  useEffect(() => {
    if (readingSettingsStatus === "loading" || readingSettingsAppliedRef.current || readingSettingsTouchedRef.current) return
    readingSettingsAppliedRef.current = true
    const prefersDark = readingSettings.theme === "system"
      && typeof window.matchMedia === "function"
      && window.matchMedia("(prefers-color-scheme: dark)").matches
    const presentation = resolveReadingPresentation(readingSettings, prefersDark)
    setTheme(presentation.theme)
    setFontFamily(presentation.fontFamily)
    setFontSize(presentation.fontSizePx)
    setLineHeight(readingSettings.lineHeight)
    setColumnWidth(readingSettings.contentWidth)
    pendingPaginationProgressionRef.current = null
    setPaginationMode(presentation.pagination)
  }, [readingSettings, readingSettingsScopeKey, readingSettingsStatus])

  const changePaginationMode = (next: BookPaginationMode) => {
    if (next === paginationMode) return
    const scrollContainer = readerScrollRef.current
    if (scrollContainer) {
      const viewport: BookPaginationViewport = {
        scrollLeft: scrollContainer.scrollLeft,
        scrollTop: scrollContainer.scrollTop,
        scrollWidth: scrollContainer.scrollWidth,
        scrollHeight: scrollContainer.scrollHeight,
        clientWidth: scrollContainer.clientWidth,
        clientHeight: scrollContainer.clientHeight,
      }
      const progression = getBookPaginationMetrics(viewport, paginationMode).progression
      latestBookProgressionRef.current = progression
      pendingPaginationProgressionRef.current = progression
    }
    setPaginationMode(next)
  }

  const contentMatchesSession = contentState.status !== "idle"
    && contentState.contentUri === sessionContentUri
  const chapters = demoRuntime
    ? BOOK_CHAPTERS
    : (contentState.status === "ready" && contentMatchesSession ? contentState.chapters : EMPTY_BOOK_CHAPTERS)
  const currentChapter = chapters.find((chapter) => chapter.id === activeChapterId) || chapters[0]
  const parsedBookTitle = contentState.status === "ready" && contentMatchesSession ? contentState.title : null
  const bookTitle = demoRuntime ? "史蒂夫·乔布斯传" : parsedBookTitle || "本地图书"
  const contentReady = demoRuntime
    || (sessionView.status === "ready" && (contentState.status === "ready" || contentState.status === "pdf_ready") && contentMatchesSession)

  const themeClass = {
    paper: "bg-[#fcfcfc] text-[#1d1d1f]",
    warm: "bg-[#f5efe3] text-[#3c332b]",
    slate: "bg-[#292a2d] text-[#e3e3e8]",
    dark: "bg-[#0f0f11] text-[#d4d4d8]",
    sepia: "bg-[#f4ecd8] text-[#5b4636]",
    eyeCare: "bg-[#cce8cc] text-[#2e4a2e]",
    custom: "bg-[#f5efe3] text-[#3c332b]",
  }[theme]
  const isDark = theme === "slate" || theme === "dark"
  // 会话内工具栏调整优先于全局设置；全局 Reading 只负责首次加载时的初始值。
  const contentWidthPx = columnWidth === "narrow" ? 620 : columnWidth === "wide" ? 820 : 700
  const contentWidth = `${contentWidthPx}px`
  const contentLineHeight = lineHeight === "compact" ? 1.65 : lineHeight === "airy" ? 2.05 : 1.85
  const sessionBookmark = tauriRuntime && mediaItemId
    ? findBookBookmark(sessionMarkers, mediaItemId, Math.max(0, Math.min(1, readingProgress / 100)))
    : null
  const isBookmarked = tauriRuntime
    ? sessionBookmark !== null
    : bookmarks.some((bookmark) => Math.abs(bookmark.progress - readingProgress) < 1)
  const bookFormat: BookContentFormat = demoRuntime
    ? "text"
    : (contentState.status === "ready" && contentMatchesSession ? contentState.format : "text")
  const pdfSource = contentState.status === "pdf_ready" && contentMatchesSession ? contentState.source : null
  const isTextPagination = !pdfSource && paginationMode !== "scroll"
  const paginationColumnGapPx = paginationMode === "double" ? 28 : 48
  // The existing article uses 24px padding below the sm breakpoint and 40px
  // above it. Read the breakpoint synchronously so the first layout already
  // uses the same frame width; ResizeObserver then only recalculates columns.
  const paginationHorizontalPadding = typeof window !== "undefined" && window.innerWidth >= 640 ? 80 : 48
  const paginationContentWidth = Math.max(280, paginationViewportWidth - paginationHorizontalPadding)
  const paginationColumnWidth = paginationMode === "double"
    ? Math.max(120, Math.floor((paginationContentWidth - paginationColumnGapPx) / 2))
    : paginationContentWidth
  const readerFrameStyle: CSSProperties | undefined = isTextPagination
    ? { maxWidth: `${contentWidthPx + paginationHorizontalPadding}px`, marginInline: "auto" }
    : undefined
  const articleStyle: CSSProperties = isTextPagination
    ? {
      boxSizing: "border-box",
      height: "100%",
      maxWidth: "none",
      width: "100%",
      columnWidth: `${paginationColumnWidth}px`,
      columnGap: `${paginationColumnGapPx}px`,
      columnFill: "auto",
      fontSize: `${fontSize}px`,
      lineHeight: contentLineHeight,
    }
    : {
      maxWidth: contentWidth,
      fontSize: `${fontSize}px`,
      lineHeight: contentLineHeight,
    }

  useEffect(() => {
    recordDemoBookReaderHistory(clientMode, storageId, recordHistory)
  }, [clientMode, storageId])

  useLayoutEffect(() => {
    markerListRequestRef.current += 1
    bookmarkOperationRef.current += 1
    readingSettingsAppliedRef.current = false
    readingSettingsTouchedRef.current = false
    restoredProgressRef.current = null
    latestBookProgressionRef.current = 0
    progressRestoreSettledRef.current = !tauriRuntime
    previousPaginationModeRef.current = null
    pendingPaginationProgressionRef.current = null
    setSessionMarkers([])
    setIsBookmarkPending(false)
    setMarkersLoaded(false)
  }, [mediaItemId, sessionContentUri, tauriRuntime])

  useEffect(() => {
    const requestId = ++markerListRequestRef.current
    if (!tauriRuntime || !mediaItemId || sessionView.status !== "ready") return

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
  }, [mediaItemId, sessionContentUri, sessionView.status, tauriRuntime])

  // 真实 EPUB 目录：经 reader_toc_get 由后端从归档抽取（nav/ncx/spine 兜底）。
  // 条目携带 progression 和可选 fragment；前端优先使用 EPUB 正文中保存的
  // anchorMap 精确落到段落，找不到锚点时才回退到章节起点。
  useEffect(() => {
    const requestId = ++tocRequestRef.current
    setReaderTocItems(null)
    if (!tauriRuntime || sessionView.status !== "ready" || state.status !== "ready") return
    if (!contentReady || !contentMatchesSession || bookFormat !== "epub") return
    void getReaderToc(state.session)
      .then((result) => {
        if (tocRequestRef.current === requestId) setReaderTocItems(result.items)
      })
      .catch(() => {
        if (tocRequestRef.current === requestId) setReaderTocItems(null)
      })
  }, [bookFormat, contentMatchesSession, contentReady, sessionContentUri, sessionView.status, state, tauriRuntime])

  const searchIndex = useMemo(() => buildBookSearchIndex(chapters), [chapters])

  useEffect(() => {
    const query = searchQuery.trim()
    if (query.length === 0) {
      setSearchHits([])
      setSearchStatus("idle")
      return
    }
    if (query.length < 2) {
      setSearchHits([])
      setSearchStatus("idle")
      return
    }
    setSearchStatus("searching")
    searchAbortRef.current?.abort()
    const controller = new AbortController()
    searchAbortRef.current = controller
    if (tauriRuntime && state.status === "ready" && mediaItemId) {
      const timer = window.setTimeout(() => {
        void searchReaderContent(state.session, query)
          .then((result) => {
            if (controller.signal.aborted) return
            const hits: BookSearchHit[] = result.hits.map((hit) => ({
              chapterId: hit.chapterId,
              chapterTitle: hit.chapterTitle,
              chapterIndex: hit.chapterIndex,
              paragraphIndex: hit.paragraphIndex,
              progressionInChapter: hit.progressionInChapter,
              exact: hit.textAnchor.exact ?? query,
              prefix: hit.textAnchor.prefix ?? null,
              suffix: hit.textAnchor.suffix ?? null,
              score: hit.score,
            }))
            setSearchHits(hits)
            setSearchStatus("done")
          })
          .catch(() => {
            if (controller.signal.aborted) return
            setSearchHits([])
            setSearchStatus("done")
          })
      }, 280)
      return () => {
        window.clearTimeout(timer)
        controller.abort()
      }
    }
    const timer = window.setTimeout(() => {
      try {
        const hits = searchBook(chapters, searchIndex, { query, signal: controller.signal })
        if (controller.signal.aborted) return
        setSearchHits(hits)
        setSearchStatus("done")
      } catch (error) {
        if ((error as DOMException)?.name === "AbortError") return
        setSearchHits([])
        setSearchStatus("done")
      }
    }, 280)
    return () => {
      window.clearTimeout(timer)
      controller.abort()
    }
  }, [chapters, mediaItemId, searchIndex, searchQuery, state, tauriRuntime])

  // Tauri 只读取后端签发的受控 URI；请求序号与 Abort 同时阻止旧 Session 覆盖新内容。
  useEffect(() => {
    const requestId = ++resourceRequestRef.current
    if (!tauriRuntime) {
      setContentState({ status: "idle" })
      return
    }
    if (sessionView.status !== "ready" || !sessionContentUri) {
      setContentState({ status: "idle" })
      return
    }

    const abortController = new AbortController()
    setContentState({ status: "loading", contentUri: sessionContentUri })
    // Probe only a small prefix first.  This is enough to identify the MIME
    // and, for PDFs, obtain Content-Range.total without materialising the
    // document in the WebView.
    void fetchSessionResource(sessionContentUri, {
      range: `bytes=0-${PDF_RANGE_CHUNK_SIZE - 1}`,
      signal: abortController.signal,
    })
      .then(async (resource) => {
        if (resourceRequestRef.current !== requestId || abortController.signal.aborted) return
        if (resource.kind === "empty") {
          setContentState({ status: "empty", contentUri: sessionContentUri })
          return
        }
        if (isPdfMimeType(resource.contentType)) {
          // PDF.js must not receive a full remote response just to discover the
          // document length.  A bounded range probe establishes the total size;
          // all subsequent page reads go through SessionPdfRangeTransport.
          if (!resource.partial || !resource.contentRange || resource.contentRange.start !== 0) {
            throw new HavenError({
              code: "SOURCE_RANGE_UNSUPPORTED",
              userMessage: "该 PDF 来源不支持分段读取，请先下载到本地",
              retryable: false,
            })
          }
          setContentState({
            status: "pdf_ready",
            contentUri: sessionContentUri,
            source: {
              contentUri: sessionContentUri,
              totalBytes: resource.contentRange.total,
              initialData: resource.bytes,
            },
          })
          return
        }
        // Non-PDF readers still need the complete bounded payload.  The probe
        // only tells us the MIME and avoids ever making a PDF one-shot request;
        // EPUB/TXT/Markdown then perform one normal session fetch.
        const completeResource = resource.partial
          ? await fetchSessionResource(sessionContentUri, { signal: abortController.signal })
          : resource
        if (resourceRequestRef.current !== requestId || abortController.signal.aborted) return
        if (completeResource.contentType === "application/epub+zip") {
          const publication = await parseEpubBook(completeResource.bytes, abortController.signal)
          if (resourceRequestRef.current !== requestId || abortController.signal.aborted) return
          setContentState(publication.chapters.length === 0
            ? { status: "empty", contentUri: sessionContentUri }
            : {
              status: "ready",
              contentUri: sessionContentUri,
              chapters: publication.chapters,
              format: "epub",
              title: publication.title,
            })
          return
        }
        const text = decodeBookText(completeResource.bytes, completeResource.contentType)
        const format: BookContentFormat = completeResource.contentType === "text/markdown" ? "markdown" : "text"
        const parsed = parseBookText(text, format)
        setContentState(parsed.length === 0
          ? { status: "empty", contentUri: sessionContentUri }
          : { status: "ready", contentUri: sessionContentUri, chapters: parsed, format, title: null })
      })
      .catch((error: unknown) => {
        if (resourceRequestRef.current !== requestId || abortController.signal.aborted) return
        const havenError = toHavenError(error)
        setContentState(havenError.retryable
          ? { status: "retryable_error", contentUri: sessionContentUri, error: havenError }
          : { status: "terminal_error", contentUri: sessionContentUri, error: havenError })
      })

    return () => abortController.abort()
  }, [contentRetryNonce, sessionContentUri, sessionView.status, tauriRuntime])

  useEffect(() => {
    if (!contentReady || chapters.length === 0) return
    setActiveChapterId(chapters[0].id)
  }, [chapters, contentReady, sessionContentUri])

  // 文本与 PDF 使用互斥控制器；同一 Session 只保存与实际渲染格式一致的 Locator。
  useEffect(() => {
    progressControllerRef.current = null
    pdfProgressControllerRef.current = null
    if (!tauriRuntime || sessionView.status !== "ready" || state.status !== "ready" || !contentMatchesSession) return
    const controller = contentState.status === "pdf_ready"
      ? createPdfProgressController({ session: state.session, retry })
      : contentState.status === "ready"
        ? createBookProgressController({ session: state.session, retry })
        : null
    if (!controller) return
    if (contentState.status === "pdf_ready") pdfProgressControllerRef.current = controller as PdfProgressController
    else progressControllerRef.current = controller as BookProgressController
    registerReleaseBarrier(() => controller.cleanup())
    return () => {
      progressControllerRef.current = null
      pdfProgressControllerRef.current = null
      registerReleaseBarrier(null)
      void controller.cleanup()
    }
  }, [contentMatchesSession, contentState.status, sessionContentUri, sessionView.status, state, retry, registerReleaseBarrier, tauriRuntime])

  // 恢复进度：session ready 且 scrollContainer 可用时，恢复至上次阅读位置。
  // 分页模式沿用同一个 progression，但把它映射到当前水平 page/spread。
  // 等待两帧让 CSS columns 完成布局，并在恢复完成前禁止首个 0 位置
  // 写回 Progress；这对“首次打开即为分页模式”的 Session 尤其重要。
  useEffect(() => {
    if (sessionView.status !== "ready" || state.status !== "ready") return
    if (tauriRuntime && (contentState.status !== "ready" || !contentMatchesSession)) return
    if (paginationMode !== "scroll" && paginationViewportWidth <= 0) return
    const scrollContainer = readerScrollRef.current
    if (!scrollContainer) return
    let cancelled = false
    let frame: number | null = null
    let attempts = 0
    const restore = () => {
      if (cancelled) return
      const progress = state.session.progress?.locator.kind === "book"
        ? state.session.progress.locator.data.progression
        : null
      const safeProgress = typeof progress === "number" && Number.isFinite(progress)
        ? Math.min(1, Math.max(0, progress))
        : 0
      const layoutReady = paginationMode === "scroll"
        ? scrollContainer.clientHeight > 0 && (safeProgress <= 0 || scrollContainer.scrollHeight > scrollContainer.clientHeight)
        : scrollContainer.clientWidth > 0 && (safeProgress <= 0 || scrollContainer.scrollWidth > scrollContainer.clientWidth)
      if (!layoutReady && attempts < 12) {
        attempts += 1
        frame = requestAnimationFrame(restore)
        return
      }

      restoreBookProgress(scrollContainer, state.session, restoredProgressRef, paginationMode)
      latestBookProgressionRef.current = safeProgress
      setReadingProgress(Math.round(safeProgress * 100))
      // Keep the controller's latest value aligned with the restored locator;
      // this prevents its close barrier from flushing the pre-restore zero.
      if (tauriRuntime && safeProgress > 0) progressControllerRef.current?.scroll(safeProgress)
      progressRestoreSettledRef.current = true
    }
    frame = requestAnimationFrame(() => {
      frame = requestAnimationFrame(restore)
    })
    return () => {
      cancelled = true
      if (frame !== null) cancelAnimationFrame(frame)
    }
  }, [contentMatchesSession, contentState.status, paginationMode, paginationViewportWidth, sessionView.status, state, sessionContentUri, tauriRuntime])

  useEffect(() => {
    if (demoRuntime) localStorage.setItem(bookmarkStorageKey, JSON.stringify(bookmarks))
  }, [bookmarkStorageKey, bookmarks, demoRuntime])

  useEffect(() => {
    const scrollContainer = readerScrollRef.current
    if (!scrollContainer) return

    const updateReadingState = () => {
      const viewport: BookPaginationViewport = {
        scrollLeft: scrollContainer.scrollLeft,
        scrollTop: scrollContainer.scrollTop,
        scrollWidth: scrollContainer.scrollWidth,
        scrollHeight: scrollContainer.scrollHeight,
        clientWidth: scrollContainer.clientWidth,
        clientHeight: scrollContainer.clientHeight,
      }
      const metrics = getBookPaginationMetrics(viewport, paginationMode)
      if (!tauriRuntime || progressRestoreSettledRef.current) {
        latestBookProgressionRef.current = metrics.progression
        setReadingProgress(Math.round(metrics.progression * 100))
      }
      setPaginationPageCount((current) => current === metrics.pageCount ? current : metrics.pageCount)
      setPaginationPageIndex((current) => current === metrics.pageIndex ? current : metrics.pageIndex)
      if (paginationMode !== "scroll") {
        setPaginationViewportWidth((current) => current === scrollContainer.clientWidth ? current : scrollContainer.clientWidth)
      }

      // Tauri 环境向进度控制器报告 progression（0..1）。
      if (tauriRuntime && progressRestoreSettledRef.current && metrics.maxOffset > 0) {
        progressControllerRef.current?.scroll(metrics.progression)
      }

      const bounds = scrollContainer.getBoundingClientRect()
      const target = paginationMode === "scroll" ? bounds.top + 96 : bounds.left + 24
      let nextChapter = chapters[0]?.id || ""
      articleRef.current?.querySelectorAll<HTMLElement>(".chapter-section").forEach((section) => {
        const sectionBounds = section.getBoundingClientRect()
        if ((paginationMode === "scroll" ? sectionBounds.top : sectionBounds.left) <= target) nextChapter = section.id
      })
      setActiveChapterId(nextChapter)
      setShowTools(metrics.offset < 48)
    }

    let frame: number | null = null
    const onScroll = () => {
      if (frame !== null) cancelAnimationFrame(frame)
      frame = requestAnimationFrame(() => {
        frame = null
        updateReadingState()
      })
    }
    scrollContainer.addEventListener("scroll", onScroll, { passive: true })
    const resizeObserver = new ResizeObserver(updateReadingState)
    resizeObserver.observe(scrollContainer)
    resizeObserver.observe(articleRef.current || scrollContainer)
    updateReadingState()

    return () => {
      scrollContainer.removeEventListener("scroll", onScroll)
      resizeObserver.disconnect()
      if (frame !== null) cancelAnimationFrame(frame)
    }
  }, [chapters, columnWidth, contentReady, fontFamily, fontSize, lineHeight, paginationMode, tauriRuntime, theme])

  // 切换 scroll / pagination / double 时保留当前的格式无关 progression。
  // 首次挂载不执行映射，避免覆盖后端恢复的位置。
  useEffect(() => {
    const previous = previousPaginationModeRef.current
    previousPaginationModeRef.current = paginationMode
    const pendingProgression = pendingPaginationProgressionRef.current
    pendingPaginationProgressionRef.current = null
    if (previous === null || previous === paginationMode) return
    const scrollContainer = readerScrollRef.current
    if (!scrollContainer) return
    const frame = requestAnimationFrame(() => {
      const viewport: BookPaginationViewport = {
        scrollLeft: scrollContainer.scrollLeft,
        scrollTop: scrollContainer.scrollTop,
        scrollWidth: scrollContainer.scrollWidth,
        scrollHeight: scrollContainer.scrollHeight,
        clientWidth: scrollContainer.clientWidth,
        clientHeight: scrollContainer.clientHeight,
      }
      const targetProgression = pendingProgression ?? latestBookProgressionRef.current
      const target = bookOffsetForProgression(viewport, targetProgression, paginationMode)
      setBookPaginationOffsetInstant(scrollContainer, target, paginationMode)
      // scrollTo({ behavior: "auto" }) is not required to dispatch a scroll
      // event. Keep the visible progress in sync even when the browser does
      // not emit one for an unchanged/clamped offset.
      latestBookProgressionRef.current = targetProgression
      setReadingProgress(Math.round(targetProgression * 100))
    })
    return () => cancelAnimationFrame(frame)
  }, [paginationMode])

  const scrollToChapter = (id: string) => {
    const section = articleRef.current?.querySelector<HTMLElement>(`#${CSS.escape(id)}`)
    const scrollContainer = readerScrollRef.current
    if (section && scrollContainer) {
      if (paginationMode === "scroll") {
        const top = section.getBoundingClientRect().top - scrollContainer.getBoundingClientRect().top + scrollContainer.scrollTop
        scrollContainer.scrollTo({ top: Math.max(0, top - 24), behavior: "smooth" })
      } else {
        const viewport: BookPaginationViewport = {
          scrollLeft: scrollContainer.scrollLeft,
          scrollTop: scrollContainer.scrollTop,
          scrollWidth: scrollContainer.scrollWidth,
          scrollHeight: scrollContainer.scrollHeight,
          clientWidth: scrollContainer.clientWidth,
          clientHeight: scrollContainer.clientHeight,
        }
        const left = section.getBoundingClientRect().left - scrollContainer.getBoundingClientRect().left + scrollContainer.scrollLeft
        scrollContainer.scrollTo({ left: alignBookOffsetToPage(viewport, left, paginationMode), top: 0, behavior: "smooth" })
      }
    }
    setDrawerOpen(false)
  }

  // 目录条目跳转：progression 在滚动模式映射到纵向位置，在分页模式映射到
  // 水平 page/spread；TXT/Markdown 回退章节 id 跳转。
  const scrollToTocItem = (item: TocItemDto) => {
    const scrollContainer = readerScrollRef.current
    if (!scrollContainer) return
    const operation = ++navigationOperationRef.current
    const expectedIndex = chapters.length === 0
      ? 0
      : Math.min(chapters.length - 1, Math.max(0, Math.floor(item.progression * chapters.length)))
    const fragmentCandidates = item.fragment
      ? chapters
        .map((chapter, index) => ({ chapter, index }))
        .filter(({ chapter }) => chapter.anchorMap?.[item.fragment!] !== undefined)
      : []
    const candidates = fragmentCandidates.length > 0
      ? fragmentCandidates
      : chapters
        .map((chapter, index) => ({ chapter, index }))
        .filter(({ chapter }) => chapter.title.trim() === item.title.trim())
    const targetChapter = (candidates.length > 0
      ? candidates.slice().sort((left, right) => Math.abs(left.index - expectedIndex) - Math.abs(right.index - expectedIndex))[0]
      : { chapter: chapters[expectedIndex], index: expectedIndex })?.chapter
    if (!targetChapter) {
      setDrawerOpen(false)
      return
    }
    const paragraphIndex = item.fragment ? targetChapter.anchorMap?.[item.fragment] : undefined
    const targetId = paragraphIndex === undefined
      ? targetChapter.id
      : `${targetChapter.id}-p${paragraphIndex}`
    setActiveChapterId(targetChapter.id)
    // CSS columns are laid out asynchronously. Two animation frames ensure the
    // target's horizontal offset is measured after a mode/resize transition.
    requestAnimationFrame(() => requestAnimationFrame(() => {
      if (navigationOperationRef.current !== operation) return
      const targetElement = articleRef.current?.querySelector<HTMLElement>(`#${CSS.escape(targetId)}`)
        ?? articleRef.current?.querySelector<HTMLElement>(`#${CSS.escape(targetChapter.id)}`)
      if (!targetElement) return
      const viewport: BookPaginationViewport = {
        scrollLeft: scrollContainer.scrollLeft,
        scrollTop: scrollContainer.scrollTop,
        scrollWidth: scrollContainer.scrollWidth,
        scrollHeight: scrollContainer.scrollHeight,
        clientWidth: scrollContainer.clientWidth,
        clientHeight: scrollContainer.clientHeight,
      }
      if (paginationMode === "scroll") {
        const top = targetElement.getBoundingClientRect().top - scrollContainer.getBoundingClientRect().top + scrollContainer.scrollTop
        scrollContainer.scrollTo({ top: Math.max(0, top - 24), behavior: "smooth" })
      } else {
        const left = targetElement.getBoundingClientRect().left - scrollContainer.getBoundingClientRect().left + scrollContainer.scrollLeft
        scrollContainer.scrollTo({ left: alignBookOffsetToPage(viewport, left, paginationMode), top: 0, behavior: "smooth" })
      }
    }))
    setDrawerOpen(false)
  }

  const tocItems = tauriRuntime && bookFormat === "epub" ? (readerTocItems ?? []) : []

  const scrollToSearchHit = (hit: BookSearchHit) => {
    const scrollContainer = readerScrollRef.current
    if (!scrollContainer) return
    const operation = ++navigationOperationRef.current
    const targetId = `${hit.chapterId}-p${hit.paragraphIndex}`
    setActiveChapterId(hit.chapterId)
    const key = targetId
    setHighlightedHitKey(key)
    window.setTimeout(() => setHighlightedHitKey((current) => (current === key ? null : current)), 2200)
    requestAnimationFrame(() => requestAnimationFrame(() => {
      if (navigationOperationRef.current !== operation) return
      const section = articleRef.current?.querySelector<HTMLElement>(`#${CSS.escape(hit.chapterId)}`)
      const paragraph = articleRef.current?.querySelector<HTMLElement>(`#${CSS.escape(targetId)}`)
      const target = paragraph ?? section
      if (!target) return
      if (paginationMode === "scroll") {
        const top = target.getBoundingClientRect().top - scrollContainer.getBoundingClientRect().top + scrollContainer.scrollTop
        scrollContainer.scrollTo({ top: Math.max(0, top - 24), behavior: "smooth" })
      } else {
        const viewport: BookPaginationViewport = {
          scrollLeft: scrollContainer.scrollLeft,
          scrollTop: scrollContainer.scrollTop,
          scrollWidth: scrollContainer.scrollWidth,
          scrollHeight: scrollContainer.scrollHeight,
          clientWidth: scrollContainer.clientWidth,
          clientHeight: scrollContainer.clientHeight,
        }
        const left = target.getBoundingClientRect().left - scrollContainer.getBoundingClientRect().left + scrollContainer.scrollLeft
        scrollContainer.scrollTo({ left: alignBookOffsetToPage(viewport, left, paginationMode), top: 0, behavior: "smooth" })
      }
    }))
    setDrawerOpen(false)
  }

  const changeBookPage = useCallback((delta: -1 | 1) => {
    const scrollContainer = readerScrollRef.current
    if (!scrollContainer) return
    const viewport: BookPaginationViewport = {
      scrollLeft: scrollContainer.scrollLeft,
      scrollTop: scrollContainer.scrollTop,
      scrollWidth: scrollContainer.scrollWidth,
      scrollHeight: scrollContainer.scrollHeight,
      clientWidth: scrollContainer.clientWidth,
      clientHeight: scrollContainer.clientHeight,
    }
    const target = bookOffsetForPageDelta(viewport, delta, paginationMode)
    if (paginationMode === "scroll") scrollContainer.scrollTo({ top: target, behavior: "smooth" })
    else scrollContainer.scrollTo({ left: target, top: 0, behavior: "smooth" })
  }, [paginationMode])

  useEffect(() => {
    if (!isTextPagination) return
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return
      const target = event.target as HTMLElement | null
      if (target?.tagName === "INPUT" || target?.tagName === "TEXTAREA" || target?.isContentEditable) return
      if (event.key === "ArrowLeft") {
        event.preventDefault()
        changeBookPage(-1)
      } else if (event.key === "ArrowRight") {
        event.preventDefault()
        changeBookPage(1)
      }
    }
    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [changeBookPage, isTextPagination])

  const toggleBookmark = () => {
    if (tauriRuntime) {
      if (!mediaItemId || isBookmarkPending || !markersLoaded) return
      markerListRequestRef.current += 1
      const operation = ++bookmarkOperationRef.current
      setIsBookmarkPending(true)
      if (sessionBookmark) {
        setSessionMarkers((current) => current.filter((marker) => marker.markerId !== sessionBookmark.markerId))
        void deleteMarker(sessionBookmark.markerId)
          .catch(() => {
            if (bookmarkOperationRef.current !== operation) return
            setSessionMarkers((current) => [...current, sessionBookmark])
          })
          .finally(() => {
            if (bookmarkOperationRef.current === operation) setIsBookmarkPending(false)
          })
        return
      }

      const chapter = chapters.find((item) => item.id === activeChapterId) || chapters[0]
      void createMarker({
        mediaItemId,
        locator: bookMarkerLocator(mediaItemId, Math.max(0, Math.min(1, readingProgress / 100))),
        markerType: "bookmark",
        title: chapter?.title ?? null,
        excerpt: null,
        note: null,
      })
        .then((marker) => {
          if (bookmarkOperationRef.current !== operation) return
          setSessionMarkers((current) => [...current.filter((item) => item.markerId !== marker.markerId), marker])
        })
        .catch(() => undefined)
        .finally(() => {
          if (bookmarkOperationRef.current === operation) setIsBookmarkPending(false)
        })
      return
    }

    if (isBookmarked) {
      setBookmarks((current) => current.filter((bookmark) => Math.abs(bookmark.progress - readingProgress) >= 1))
      return
    }
    const chapter = chapters.find((item) => item.id === activeChapterId) || chapters[0]
    if (!chapter) return
    const localId = Date.now().toString()
    setBookmarks((current) => [
      ...current,
      {
        id: localId,
        progress: readingProgress,
        scrollTop: readerScrollRef.current?.scrollTop || 0,
        scrollLeft: readerScrollRef.current?.scrollLeft || 0,
        chapterId: chapter.id,
        chapterTitle: chapter.title,
        timestamp: new Date().toLocaleDateString(),
      },
    ])
  }

  const restoreBookmark = (bookmark: BookmarkType) => {
    const scrollContainer = readerScrollRef.current
    if (!scrollContainer) return
    const viewport: BookPaginationViewport = {
      scrollLeft: scrollContainer.scrollLeft,
      scrollTop: scrollContainer.scrollTop,
      scrollWidth: scrollContainer.scrollWidth,
      scrollHeight: scrollContainer.scrollHeight,
      clientWidth: scrollContainer.clientWidth,
      clientHeight: scrollContainer.clientHeight,
    }
    if (paginationMode === "scroll") {
      const maxScroll = Math.max(0, scrollContainer.scrollHeight - scrollContainer.clientHeight)
      const fallbackTop = (bookmark.progress / 100) * maxScroll
      scrollContainer.scrollTo({ top: Math.min(Math.max(0, bookmark.scrollTop || fallbackTop), maxScroll), behavior: "smooth" })
    } else {
      const fallbackLeft = bookOffsetForProgression(viewport, bookmark.progress / 100, paginationMode)
      const left = bookmark.scrollLeft === undefined
        ? fallbackLeft
        : alignBookOffsetToPage(viewport, bookmark.scrollLeft, paginationMode)
      scrollContainer.scrollTo({ left, top: 0, behavior: "smooth" })
    }
    setDrawerOpen(false)
  }

  return (
    <div className={cn("relative h-[100dvh] min-h-screen w-full overflow-hidden transition-colors duration-500", themeClass)}>
      <header className={cn(
        "fixed inset-x-0 top-0 z-50 flex h-[68px] items-center justify-between border-b px-5 backdrop-blur-2xl transition-all duration-500 sm:px-[32px]",
        isDark ? "border-white/10 bg-[#18181b]/80" : "border-black/[0.07] bg-white/80",
        showTools || drawerOpen || settingsOpen ? "translate-y-0 opacity-100" : "-translate-y-[8px] opacity-30 hover:opacity-100",
      )}>
        <div className="flex min-w-0 items-center gap-3">
          <button type="button" onClick={() => navigate(-1)} aria-label="返回" className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full transition-colors hover:bg-black/[0.06] dark:hover:bg-white/[0.08]"><ArrowLeft className="h-[16px] w-[16px]" /></button>
          <div className="min-w-0">
            <p className="truncate text-sm font-semibold">{bookTitle}</p>
            <p className="truncate text-[11px] opacity-55">{currentChapter?.title || "正在准备内容"} · {mediaItemId || "d3"}{isTextPagination && ` · 第 ${paginationPageIndex + 1}/${paginationPageCount} 页`}</p>
          </div>
        </div>
        <div className="flex items-center gap-1">
          <button type="button" onClick={() => setDrawerOpen((open) => !open)} disabled={Boolean(pdfSource)} aria-label="打开章节目录与书签" className={cn("flex h-9 w-9 items-center justify-center rounded-full transition-colors hover:bg-black/[0.06] disabled:cursor-not-allowed disabled:opacity-35 dark:hover:bg-white/[0.08]", drawerOpen && "bg-primary/10 text-primary")}><ListTree className="h-[17px] w-[17px]" /></button>
          <button type="button" onClick={() => setSettingsOpen((open) => !open)} aria-label="打开阅读排版设置" className={cn("flex h-9 w-9 items-center justify-center rounded-full text-sm font-bold transition-colors hover:bg-black/[0.06] dark:hover:bg-white/[0.08]", settingsOpen && "bg-primary/10 text-primary")}>aA</button>
          <button type="button" onClick={toggleBookmark} disabled={!contentReady || Boolean(pdfSource) || isBookmarkPending || (tauriRuntime && !markersLoaded)} aria-label={isBookmarked ? "取消书签" : "添加书签"} className={cn("flex h-9 w-9 items-center justify-center rounded-full transition-colors hover:bg-black/[0.06] disabled:cursor-not-allowed disabled:opacity-35 dark:hover:bg-white/[0.08]", isBookmarked && "bg-primary/10 text-primary")}><Bookmark className={cn("h-[17px] w-[17px]", isBookmarked && "fill-current")} /></button>
        </div>
      </header>

      <main className="relative h-full w-full pt-[68px]">
        {(sessionView.status === "opening" || sessionView.status === "idle") && tauriRuntime && (
          <div className="flex h-full items-center justify-center">
            <p className="text-sm opacity-55">正在准备阅读…</p>
          </div>
        )}
        {(sessionView.status === "retryable_error" || sessionView.status === "terminal_error") && tauriRuntime && (
          <div className="flex h-full items-center justify-center px-6 text-center">
            <div className="max-w-sm space-y-3">
              <p className="text-base font-semibold">{sessionView.message}</p>
              {sessionView.retryable && (
                <button type="button" onClick={retry} className="rounded-full border border-current/20 px-4 py-2 text-sm font-semibold transition-colors hover:bg-current/5">
                  重试
                </button>
              )}
            </div>
          </div>
        )}
        {sessionView.status === "ready" && tauriRuntime && (!contentMatchesSession || contentState.status === "loading") && (
          <div className="flex h-full items-center justify-center">
            <p className="text-sm opacity-55">正在读取文本…</p>
          </div>
        )}
        {sessionView.status === "ready" && tauriRuntime && contentMatchesSession && contentState.status === "empty" && (
          <div className="flex h-full items-center justify-center px-6 text-center">
            <div className="max-w-sm space-y-2">
              <p className="text-base font-semibold">图书内容为空</p>
              <p className="text-sm opacity-55">这个本地文件没有可显示的正文。</p>
            </div>
          </div>
        )}
        {sessionView.status === "ready" && tauriRuntime && contentMatchesSession && (contentState.status === "retryable_error" || contentState.status === "terminal_error") && (
          <div className="flex h-full items-center justify-center px-6 text-center">
            <div className="max-w-sm space-y-3">
              <p className="text-base font-semibold">{contentState.error.message}</p>
              {contentState.status === "retryable_error" && (
                <button type="button" onClick={() => setContentRetryNonce((value) => value + 1)} className="rounded-full border border-current/20 px-4 py-2 text-sm font-semibold transition-colors hover:bg-current/5">
                  重试
                </button>
              )}
            </div>
          </div>
        )}
        {isTextPagination && contentReady && (
          <>
            <button
              type="button"
              onClick={() => changeBookPage(-1)}
              aria-label="上一页"
              title="上一页（←）"
              className="group absolute bottom-0 left-0 top-0 z-20 flex w-[18%] cursor-pointer items-center justify-start pl-3 transition-colors hover:bg-black/[0.025] sm:pl-6 dark:hover:bg-white/[0.025]"
            >
              <span className="rounded-full border border-current/10 bg-black/40 p-2 text-white opacity-0 shadow-lg backdrop-blur-md transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100">
                <ChevronLeft className="h-5 w-5" />
              </span>
            </button>
            <button
              type="button"
              onClick={() => changeBookPage(1)}
              aria-label="下一页"
              title="下一页（→）"
              className="group absolute bottom-0 right-0 top-0 z-20 flex w-[18%] cursor-pointer items-center justify-end pr-3 transition-colors hover:bg-black/[0.025] sm:pr-6 dark:hover:bg-white/[0.025]"
            >
              <span className="rounded-full border border-current/10 bg-black/40 p-2 text-white opacity-0 shadow-lg backdrop-blur-md transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100">
                <ChevronRight className="h-5 w-5" />
              </span>
            </button>
          </>
        )}
        {contentReady && pdfSource && state.status === "ready" && <PdfReader key={state.session.sessionId} source={pdfSource} restoreLocator={(pageCount) => (
          restorePdfProgress(pageCount, state.session, restoredPdfProgressRef)
        )} onLocatorChange={(locator) => {
          const percentage = locator.pageCount <= 1 ? 0 : Math.round((locator.pageIndex / (locator.pageCount - 1)) * 100)
          setReadingProgress(percentage)
          pdfProgressControllerRef.current?.locatorChange(locator)
        }} className="h-full overflow-y-auto" />}
        {contentReady && !pdfSource && <div ref={readerScrollRef} style={readerFrameStyle} className={cn("h-full overscroll-contain scroll-smooth [scrollbar-width:none] [&::-webkit-scrollbar]:hidden", isTextPagination ? "overflow-x-auto overflow-y-hidden" : "overflow-x-hidden overflow-y-auto")} aria-label={isTextPagination ? "图书分页阅读区" : "图书纵向阅读区"}>
          <article ref={articleRef} className={cn("mx-auto w-full select-text px-6 pb-[128px] pt-14 sm:px-10 sm:pt-[80px]", FONT_CLASSES[fontFamily], isTextPagination && "break-inside-avoid")} style={articleStyle}>
            <header className="break-inside-avoid border-b border-current/15 pb-[48px]">
              <p className="text-[10px] font-semibold uppercase tracking-[0.22em] text-primary">BOOK · LOCAL READER</p>
              <h1 className="mt-6 text-4xl font-semibold leading-[1.08] tracking-[-0.055em] sm:text-6xl">{bookTitle}</h1>
              {demoRuntime && <p className="mt-[16px] text-[0.9em] opacity-55">Steve Jobs · Walter Isaacson</p>}
              <div className="mt-7 flex flex-wrap gap-x-3 gap-y-1 text-[11px] font-medium opacity-45"><span>{demoRuntime || bookFormat === "epub" ? "EPUB" : bookFormat === "markdown" ? "MARKDOWN" : "TXT"}</span><span>·</span><span>{chapters.length} 章</span><span>·</span><span>阅读进度 {readingProgress}%</span></div>
            </header>

            <div className="mt-14 space-y-[80px]">
              {chapters.map((chapter, chapterIndex) => (
                <section key={chapter.id} id={chapter.id} className="chapter-section scroll-mt-[96px]" style={isTextPagination ? { breakInside: "avoid" } : undefined}>
                  <div className="mb-[32px] border-b border-current/12 pb-6"><p className="text-[10px] font-semibold uppercase tracking-[0.22em] text-primary">{chapter.kicker}</p><h2 className="mt-3 text-3xl font-semibold leading-tight tracking-[-0.04em] sm:text-4xl">{chapter.title}</h2></div>
                  <div className="space-y-7">
                    {chapter.paragraphs.map((paragraph, paragraphIndex) => {
                      const paragraphId = `${chapter.id}-p${paragraphIndex}`
                      const isHighlighted = highlightedHitKey === `${chapter.id}-p${paragraphIndex}`
                      return bookFormat === "markdown" ? (
                        <div
                          key={paragraphId}
                          id={paragraphId}
                          className={cn("scroll-mt-[96px] rounded-xl px-2 py-1 transition-colors", isHighlighted && "bg-primary/10 ring-1 ring-primary/20")}
                        >
                          <ReactMarkdown
                            components={{
                              a: ({ children }) => <span className="underline decoration-current/40 underline-offset-4">{children}</span>,
                              img: () => <span className="opacity-55">[图片已隐藏]</span>,
                            }}
                          >
                            {paragraph}
                          </ReactMarkdown>
                        </div>
                      ) : (
                        <p
                          key={paragraphId}
                          id={paragraphId}
                          className={cn(
                            paragraphIndex === 0 && "first-letter:float-left first-letter:mr-[8px] first-letter:mt-1 first-letter:text-5xl first-letter:font-semibold first-letter:leading-[0.75]",
                            "scroll-mt-[96px] rounded-xl px-2 py-1 leading-[inherit] transition-colors",
                            isHighlighted && "bg-primary/10 ring-1 ring-primary/20",
                          )}
                        >
                          {paragraph}
                        </p>
                      )
                    })}
                    {chapter.quote && <blockquote className="my-10 border-l-2 border-primary/65 py-[8px] pl-6 text-[1.08em] leading-[1.75] opacity-80">“{chapter.quote}”</blockquote>}
                  </div>
                  {chapterIndex < chapters.length - 1 && <div className="mt-[64px] h-px w-[48px] bg-primary/45" aria-hidden="true" />}
                </section>
              ))}
            </div>
          </article>
        </div>}
      </main>

      {settingsOpen && (
        <div className="fixed right-[16px] top-[80px] z-[65] w-[min(330px,calc(100vw-2rem))] rounded-2xl border border-black/[0.08] bg-white/95 p-5 text-[#1d1d1f] shadow-2xl backdrop-blur-2xl dark:border-white/10 dark:bg-[#17181b]/95 dark:text-white">
          <div className="flex items-center justify-between border-b border-current/10 pb-[16px]"><div><p className="text-[10px] font-semibold uppercase tracking-[0.2em] text-primary">Reading Settings</p><h2 className="mt-1 text-base font-semibold">阅读排版</h2></div><button type="button" onClick={() => setSettingsOpen(false)} aria-label="关闭排版设置" className="rounded-full p-1.5 opacity-55 hover:bg-black/[0.06] hover:opacity-100 dark:hover:bg-white/[0.08]"><X className="h-[16px] w-[16px]" /></button></div>
          <div className="mt-5 space-y-5 text-sm">
            <div><p className="mb-[8px] text-xs font-semibold opacity-55">字体</p><div className="grid grid-cols-3 gap-[8px]">{FONT_OPTIONS.map((option) => <button key={option.id} type="button" onClick={() => { markReadingSettingsTouched(); setFontFamily(option.id) }} className={cn("inline-flex h-[36px] items-center justify-center rounded-xl border px-[8px] text-xs font-semibold", option.className, fontFamily === option.id ? "border-primary bg-primary/10 text-primary" : "border-current/10")}>{option.label}</button>)}</div></div>
            <div><div className="mb-[8px] flex items-center justify-between text-xs font-semibold opacity-55"><span>字号</span><span>{fontSize}px</span></div><div className="flex items-center gap-3"><button type="button" onClick={() => { markReadingSettingsTouched(); setFontSize((size) => Math.max(14, size - 1)) }} aria-label="减小字号" className="flex h-[32px] w-[32px] items-center justify-center rounded-full bg-black/[0.05] hover:bg-black/[0.1] dark:bg-white/[0.08]"><Minus className="h-[16px] w-[16px]" /></button><input type="range" min="14" max="26" value={fontSize} onChange={(event) => { markReadingSettingsTouched(); setFontSize(Number(event.target.value)) }} className="h-1 flex-1 accent-primary" aria-label="字号大小" /><button type="button" onClick={() => { markReadingSettingsTouched(); setFontSize((size) => Math.min(26, size + 1)) }} aria-label="增大字号" className="flex h-[32px] w-[32px] items-center justify-center rounded-full bg-black/[0.05] hover:bg-black/[0.1] dark:bg-white/[0.08]"><Plus className="h-[16px] w-[16px]" /></button></div></div>
            <div><p className="mb-[8px] text-xs font-semibold opacity-55">正文宽度</p><div className="grid grid-cols-3 gap-[8px]">{([["narrow", "窄"], ["medium", "标准"], ["wide", "宽"]] as const).map(([id, label]) => <button key={id} type="button" onClick={() => { markReadingSettingsTouched(); setColumnWidth(id) }} className={cn("inline-flex h-[36px] items-center justify-center rounded-full px-[8px] text-xs font-semibold", columnWidth === id ? "bg-foreground text-background" : "bg-black/[0.05] dark:bg-white/[0.08]")}>{label}</button>)}</div></div>
            <div><p className="mb-[8px] text-xs font-semibold opacity-55">阅读模式</p><div className="grid grid-cols-3 gap-[8px]">{PAGINATION_OPTIONS.map((option) => <button key={option.id} type="button" onClick={() => { markReadingSettingsTouched(); changePaginationMode(option.id) }} className={cn("inline-flex h-[36px] items-center justify-center rounded-full px-[8px] text-xs font-semibold", paginationMode === option.id ? "bg-foreground text-background" : "bg-black/[0.05] dark:bg-white/[0.08]")}>{option.label}</button>)}</div><p className="mt-2 text-[11px] opacity-45">分页只应用于 EPUB、TXT 和 Markdown；PDF 保持原生页码。</p></div>
            <div><p className="mb-[8px] text-xs font-semibold opacity-55">纸张</p><div className="flex gap-3">{THEME_OPTIONS.map((option) => <button key={option.id} type="button" onClick={() => { markReadingSettingsTouched(); setTheme(option.id) }} aria-label={option.label} className={cn("h-[20px] w-[20px] rounded-full border-2 transition-transform", theme === option.id ? "scale-110 ring-2 ring-primary ring-offset-1" : "border-black/15 dark:border-white/20")} style={{ backgroundColor: option.color }} />)}</div></div>
          </div>
        </div>
      )}

      {drawerOpen && (
        <div className="fixed inset-0 z-[60]" onClick={() => setDrawerOpen(false)}>
          <aside onClick={(event) => event.stopPropagation()} className={cn("absolute right-0 top-0 flex h-full w-[min(380px,100vw)] flex-col border-l pt-[68px] shadow-2xl backdrop-blur-2xl", isDark ? "border-white/10 bg-[#18181b]/95" : "border-black/[0.08] bg-white/95")}>
            <div className="flex items-center justify-between border-b border-current/10 px-5 py-[16px]"><div className="flex gap-1 rounded-xl bg-black/[0.05] p-1 dark:bg-white/[0.08]"><button type="button" onClick={() => setDrawerTab("toc")} className={cn("rounded-lg px-3 py-1.5 text-xs font-semibold", drawerTab === "toc" ? "bg-background shadow-sm" : "opacity-55")}>章节目录</button><button type="button" onClick={() => setDrawerTab("search")} className={cn("flex items-center gap-1 rounded-lg px-3 py-1.5 text-xs font-semibold", drawerTab === "search" ? "bg-background shadow-sm" : "opacity-55")}><Search className="h-3 w-3" />搜索</button><button type="button" onClick={() => setDrawerTab("bookmarks")} className={cn("rounded-lg px-3 py-1.5 text-xs font-semibold", drawerTab === "bookmarks" ? "bg-background shadow-sm" : "opacity-55")}>书签 {bookmarks.length > 0 && `(${bookmarks.length})`}</button></div><button type="button" onClick={() => setDrawerOpen(false)} aria-label="关闭目录" className="rounded-full p-[8px] opacity-55 hover:bg-black/[0.06] hover:opacity-100 dark:hover:bg-white/[0.08]"><X className="h-[16px] w-[16px]" /></button></div>
            {drawerTab === "toc" ? <nav className="flex-1 space-y-1 overflow-y-auto p-[16px] [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">{tocItems.length > 0 ? tocItems.map((item) => <button key={item.id} type="button" onClick={() => scrollToTocItem(item)} className="flex w-full items-center justify-between rounded-xl px-3 py-3 text-left text-sm transition-colors hover:bg-black/[0.05] dark:hover:bg-white/[0.08]" style={{ paddingLeft: `${16 + Math.min(item.depth, 8) * 16}px` }}><span className="truncate">{item.title}</span><span className="ml-3 shrink-0 text-[10px] opacity-45">{Math.round(item.progression * 100)}%</span></button>) : chapters.map((chapter, index) => <button key={chapter.id} type="button" onClick={() => scrollToChapter(chapter.id)} className={cn("flex w-full items-center justify-between rounded-xl px-3 py-3 text-left text-sm transition-colors hover:bg-black/[0.05] dark:hover:bg-white/[0.08]", activeChapterId === chapter.id && "bg-primary/10 font-semibold text-primary")}><span className="truncate">{chapter.title}</span><span className="ml-3 shrink-0 text-[10px] opacity-45">{index + 1}</span></button>)}</nav> : drawerTab === "search" ? <div className="flex flex-1 flex-col overflow-hidden"><div className="border-b border-current/10 p-3"><div className="flex items-center gap-2 rounded-xl border border-current/10 bg-black/[0.03] px-3 py-2 focus-within:border-primary/40 dark:bg-white/[0.06]"><Search className="h-4 w-4 shrink-0 opacity-40" /><input value={searchQuery} onChange={(event) => setSearchQuery(event.target.value)} placeholder="搜索本书内容…" aria-label="搜索本书" className="w-full bg-transparent text-sm outline-none placeholder:text-current/40" /><button type="button" onClick={() => setSearchQuery("")} aria-label="清空搜索" className={cn("rounded-full p-1 opacity-40 hover:opacity-80", !searchQuery && "invisible")}><X className="h-3.5 w-3.5" /></button></div><p className="mt-2 px-1 text-[11px] opacity-45">{searchStatus === "searching" ? "正在搜索…" : searchQuery.trim().length >= 2 ? `找到 ${searchHits.length} 条结果` : "输入至少 2 个字符开始搜索"}</p></div><div className="flex-1 space-y-1 overflow-y-auto p-3 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">{searchHits.length === 0 ? <div className="flex h-[120px] flex-col items-center justify-center gap-2 text-xs opacity-40"><Search className="h-5 w-5" /><span>{searchQuery.trim().length >= 2 ? "未找到匹配内容" : "输入关键词搜索正文"}</span></div> : searchHits.map((hit, index) => <button key={`${hit.chapterId}:${hit.paragraphIndex}:${index}:${hit.exact.slice(0, 8)}`} type="button" onClick={() => scrollToSearchHit(hit)} className="w-full rounded-xl border border-transparent px-3 py-3 text-left transition-colors hover:border-current/10 hover:bg-black/[0.04] dark:hover:bg-white/[0.06]"><p className="truncate text-xs font-semibold text-primary">{hit.chapterTitle}</p><p className="mt-1 line-clamp-2 text-sm leading-relaxed opacity-80"><span className="opacity-40">{hit.prefix?.slice(-24)}</span><mark className="rounded bg-primary/15 px-0.5 font-medium text-primary">{hit.exact.slice(0, 80)}</mark><span className="opacity-40">{hit.suffix?.slice(0, 24)}</span></p><p className="mt-1 text-[10px] opacity-35">第 {hit.chapterIndex + 1} 章 · 段 {hit.paragraphIndex + 1}</p></button>)}</div></div> : <div className="flex-1 space-y-3 overflow-y-auto p-[16px] [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">{bookmarks.length === 0 ? <div className="flex h-[160px] flex-col items-center justify-center gap-[8px] text-xs opacity-50"><Bookmark className="h-6 w-6" /><span>暂无书签</span></div> : bookmarks.map((bookmark) => <div key={bookmark.id} className="group rounded-2xl border border-current/10 bg-black/[0.04] p-[16px] dark:bg-white/[0.06]"><button type="button" onClick={() => restoreBookmark(bookmark)} className="w-full text-left"><div className="flex items-center justify-between gap-[8px]"><p className="truncate text-xs font-semibold text-primary">{bookmark.chapterTitle}</p><span className="text-[10px] opacity-55">{bookmark.progress}%</span></div><p className="mt-[8px] text-xs opacity-55">保存于 {bookmark.timestamp}</p></button><button type="button" onClick={() => setBookmarks((current) => current.filter((item) => item.id !== bookmark.id))} aria-label="删除书签" className="mt-3 flex items-center gap-1 text-[11px] text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100 hover:text-destructive"><Trash2 className="h-[14px] w-[14px]" />删除</button></div>)}</div>}
          </aside>
        </div>
      )}

      <div className="fixed inset-x-0 bottom-0 z-50 h-[2px] bg-current/10" aria-label={`阅读进度 ${readingProgress}%`}><div className="h-full bg-primary transition-[width] duration-200" style={{ width: `${readingProgress}%` }} /></div>
    </div>
  )
}

const FONT_CLASSES: Record<FontFamily, string> = {
  sans: "font-sans",
  serif: "font-serif",
  kai: "font-serif italic",
  heiti: "font-sans font-bold",
  fangsong: "font-serif",
  mianfei: "font-sans",
  custom: "font-sans",
}

function readBookmarks(key: string): BookmarkType[] {
  try {
    const saved = localStorage.getItem(key)
    return saved ? JSON.parse(saved) as BookmarkType[] : []
  } catch {
    return []
  }
}
