import { Children, createElement, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"
import type { KeyboardEvent, ReactNode } from "react"
import { useNavigate, useParams } from "react-router"
import {
  ArrowLeft,
  Bookmark,
  Languages,
  ChevronRight,
  Highlighter,
  ListTree,
  Minus,
  Plus,
  Sparkles,
  X,
} from "lucide-react"
import { cn } from "@/lib/utils"
import ReactMarkdown from "react-markdown"
import { recordHistory } from "@/lib/havenState"
import { getHavenClientMode, type HavenClientMode } from "@/lib/ipc/runtime"
import { HavenError, toHavenError, type HavenError as HavenErrorType } from "@/lib/ipc/errors"
import { useMediaSession } from "@/features/session/useMediaSession"
import { fetchSessionResource } from "@/features/session/ipc/resource-fetch"
import {
  canUseDemoArticleReaderTools,
  loadDemoArticleReaderValue,
  recordDemoArticleReaderHistory,
  resolveArticleReaderRuntimeState,
} from "../lib/article-reader-runtime-state"
import { selectReaderSessionView } from "../lib/reader-session-view"
import {
  createArticleProgressController,
  restoreArticleProgress,
  type ArticleProgressController,
} from "../lib/article-progress-controller"
import {
  articleOutline,
  decodeArticleText,
  parseArticleContent,
  type ArticleDocument,
  type ArticleSection,
} from "../lib/article-content"
import { findArticleBookmark } from "../lib/article-marker-match"
import { isPdfMimeType } from "../lib/pdf-reader-state"
import {
  createPdfProgressController,
  restorePdfProgress,
  type PdfProgressController,
} from "../lib/pdf-progress-controller"
import { PdfReader } from "../components/PdfReader"
import { PDF_RANGE_CHUNK_SIZE, type PdfSessionSource } from "../lib/pdf-document"
import { useReadingSettings } from "../lib/use-reading-settings"
import { resolveReadingPresentation } from "../lib/reading-settings-mapping"
import { articleMarkerLocator, createMarker, deleteMarker, listMarkers } from "@/features/markers/ipc/marker-gateway"
import type { MarkerDto } from "@/lib/ipc/generated/wire"
import { buildArticleTextAnchor, findParagraphText } from "../lib/article-text-anchor"

type ReaderTheme = "paper" | "warm" | "slate" | "dark" | "sepia" | "eyeCare" | "custom"
type ReaderFont = "serif" | "sans" | "kai" | "heiti" | "fangsong" | "mianfei" | "custom"
type LineHeight = "compact" | "comfortable" | "airy"

interface ArticleNote {
  id: string
  blockId: string
  text: string
  note?: string
  time: string
}

interface ArticleHighlight {
  blockId: string
  text: string
}

type ArticleContentState =
  | { status: "idle" }
  | { status: "loading"; storageId: string; contentUri: string }
  | { status: "ready"; storageId: string; contentUri: string; document: ArticleDocument }
  | { status: "pdf_ready"; storageId: string; contentUri: string; source: PdfSessionSource }
  | { status: "empty"; storageId: string; contentUri: string }
  | { status: "retryable_error"; storageId: string; contentUri: string; error: HavenErrorType }
  | { status: "terminal_error"; storageId: string; contentUri: string; error: HavenErrorType }

const ARTICLE_SECTIONS: ArticleSection[] = [
  {
    id: "h1-intro",
    level: 1,
    title: "引言 · Agentic AI 的崛起",
    dek: "探讨从传统控制台到 Local-First 助读的范式变革。",
    paragraphs: [
      {
        id: "p1",
        translation: "AI Agents are no longer just simple chat windows waiting for user input. They have begun to understand complex intent, decompose multi-stage steps, invoke underlying tool APIs concurrently, and deliver complete, verifiable work outputs within bounded sandboxes.",
        text: "AI Agent（智能体）不再只是一个等待键盘输入的简单对话框。它开始主动理解复杂意图、拆解多阶段步骤、并发调用底层接口，并在受控的沙盒边界内交出完整的交付成果。",
      },
      {
        id: "p2",
        translation: "For personal content spaces and reading experiences, the true revolution lies not in how many extra buttons appear on the interface, but in dramatically shortening the distance between content, knowledge, and user intent.",
        text: "对于个人媒体空间与阅读体验而言，真正的革命不在于界面上多出了多少个按钮，而在于内容、知识与用户意图之间的距离被彻底缩短。",
      },
    ],
  },
  {
    id: "h1-arch",
    level: 1,
    title: "架构变革 · 本地优先与数据主权",
    dek: "当工具开始理解上下文，内容空间也需要重新获得边界。",
    paragraphs: [
      {
        id: "p3",
        translation: "True software tools should bring people closer to their core questions, rather than pushing them further away from answers through repetitive, tedious operations.",
        text: "真正的工具应当让人更加接近自己的核心问题，而不是让人在繁杂的操作中离答案越来越远。",
      },
      {
        id: "p4",
        translation: "In the architectural design of Haven, articles, books, notes, and highlights eventually converge back into the unified context of the same work. The native renderer delivers high-fidelity presentation, while local storage ensures data sovereignty.",
        text: "在栖阅的架构设计中，文章、图书、笔记与书签标记最终都会收拢回到同一个作品的统一上下文中。原生渲染器负责高保真还原，而本地存储则确保了用户的数据主权。",
      },
    ],
    quote: "真正优秀的软件，应当让离线内容与本地算力契合。当你在静心阅读时，界面应当隐退。",
  },
  {
    id: "h1-future",
    level: 1,
    title: "未来演化 · 人与工具的新距离",
    dek: "最好的阅读器不是展示更多，而是让内容重新成为唯一的中心。",
    paragraphs: [
      {
        id: "p5",
        translation: "When tools become quiet enough, readers can return to the pace of their own thinking. The interface does not disappear; it simply knows when to step back.",
        text: "当工具足够安静，读者才能重新回到自己的思考节奏。界面并不会消失，它只是知道什么时候应该退后一步。",
      },
    ],
  },
]

const DEMO_DOCUMENT: ArticleDocument = {
  title: ARTICLE_SECTIONS[0].title,
  sections: ARTICLE_SECTIONS,
  characterCount: ARTICLE_SECTIONS.reduce(
    (total, section) => total + section.paragraphs.reduce((sum, paragraph) => sum + paragraph.text.length, 0),
    0,
  ),
  format: "text",
}
const DEMO_OUTLINE = articleOutline(DEMO_DOCUMENT)
const DEFAULT_NOTES: ArticleNote[] = [
  {
    id: "default-note",
    blockId: "p3",
    text: "真正的工具应当让人更加接近自己的核心问题，而不是让人在繁杂的操作中离答案越来越远。",
    note: "工具应该降低认知负担。",
    time: "2026-08-13 21:30",
  },
]

const THEME_OPTIONS: Array<{ id: ReaderTheme; label: string; color: string }> = [
  { id: "paper", label: "纸白", color: "#fbfbfb" },
  { id: "warm", label: "暖纸", color: "#f4eee1" },
  { id: "slate", label: "石板", color: "#25262a" },
  { id: "dark", label: "墨黑", color: "#0e0f12" },
  { id: "sepia", label: "复古", color: "#f4ecd8" },
  { id: "eyeCare", label: "护眼", color: "#cce8cc" },
  { id: "custom", label: "自定义", color: "#e8e0d0" },
]

type ActiveArticleReaderMode = Exclude<HavenClientMode, "unavailable">

export function ArticleReaderPage() {
  const clientMode = getHavenClientMode()

  if (clientMode === "unavailable") return <ArticleReaderUnavailable />

  return <ArticleReaderExperience clientMode={clientMode} />
}

function ArticleReaderUnavailable() {
  const navigate = useNavigate()

  return (
    <div className="flex min-h-[100dvh] flex-col bg-[#f4eee1] text-[#3b3226]">
      <header className="flex h-[68px] items-center gap-3 border-b border-black/[0.07] px-5 sm:px-8">
        <button type="button" onClick={() => navigate(-1)} aria-label="返回" className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full transition-colors hover:bg-black/[0.06]">
          <ArrowLeft className="h-4 w-4" />
        </button>
        <p className="text-sm font-semibold">文章阅读</p>
      </header>
      <main className="flex flex-1 items-center justify-center px-6 text-center">
        <div className="max-w-sm space-y-2">
          <h1 className="text-lg font-semibold">当前无法打开文章</h1>
          <p className="text-sm opacity-60">当前环境未连接栖阅本地阅读服务，请在桌面应用中重新打开此内容。</p>
        </div>
      </main>
    </div>
  )
}

function ArticleReaderExperience({ clientMode }: { clientMode: ActiveArticleReaderMode }) {
  const navigate = useNavigate()
  const { mediaItemId } = useParams<{ mediaItemId?: string }>()
  const runtimeState = resolveArticleReaderRuntimeState(clientMode)
  const tauriRuntime = runtimeState === "production"
  const demoRuntime = canUseDemoArticleReaderTools(clientMode)
  const storageId = mediaItemId || "article-agentic-ai"
  const articleMarkerKey = `haven:marker:article:${storageId}`
  const storageScope = `${clientMode}:${storageId}`

  // Session + 进度：Tauri 环境接真实 useMediaSession（engine=article）；
  // 浏览器演示环境保持既有静态演示内容。
  const { state, retry, registerReleaseBarrier } = useMediaSession(mediaItemId, "article")
  const sessionView = selectReaderSessionView(state, mediaItemId)
  const sessionContentUri = sessionView.contentUri
  const { settings: readingSettings, status: readingSettingsStatus } = useReadingSettings()

  const [theme, setTheme] = useState<ReaderTheme>("warm")
  const [font, setFont] = useState<ReaderFont>("serif")
  const [fontSize, setFontSize] = useState(18)
  const [lineHeight, setLineHeight] = useState<LineHeight>("comfortable")
  const [showTranslations, setShowTranslations] = useState(false)
  const [isBookmarked, setIsBookmarked] = useState(() => loadDemoArticleReaderValue(
    clientMode,
    () => (
      localStorage.getItem(`haven:article-bookmark:${storageId}`) === "1"
      || localStorage.getItem(articleMarkerKey) === "1"
    ),
    false,
  ))
  const [highlights, setHighlights] = useState<ArticleHighlight[]>(() => (
    loadDemoArticleReaderValue(clientMode, () => readHighlights(storageId), [])
  ))
  /** Tauri 环境创建成功后的后端标记 ID（供取消书签时软删除）。 */
  const [articleMarkerId, setArticleMarkerId] = useState<string | null>(null)
  const [isBookmarkPending, setIsBookmarkPending] = useState(false)
  const [markersLoaded, setMarkersLoaded] = useState(false)
  const [sessionMarkers, setSessionMarkers] = useState<MarkerDto[]>([])
  const [notes, setNotes] = useState<ArticleNote[]>(() => (
    loadDemoArticleReaderValue(clientMode, () => readNotes(storageId), [])
  ))
  const effectiveHighlights = useMemo<ArticleHighlight[]>(() => {
    if (demoRuntime) return highlights
    return sessionMarkers
      .filter((marker) => marker.markerType === "highlight")
      .map((marker) => {
        const blockId = marker.locator.kind === "article" ? marker.locator.data.blockId : null
        const text = marker.excerpt ?? (marker.locator.kind === "article" ? marker.locator.data.textAnchor?.exact : null) ?? ""
        return blockId && text ? { blockId, text } : null
      })
      .filter((value): value is ArticleHighlight => value !== null)
  }, [demoRuntime, highlights, sessionMarkers])
  const [contentState, setContentState] = useState<ArticleContentState>({ status: "idle" })
  const [contentRetryNonce, setContentRetryNonce] = useState(0)
  const [outlineOpen, setOutlineOpen] = useState(false)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [aiOpen, setAiOpen] = useState(false)
  const [activeHeading, setActiveHeading] = useState(() => demoRuntime ? DEMO_OUTLINE[0].id : "")
  const [progress, setProgress] = useState(0)
  const [chromeDimmed, setChromeDimmed] = useState(false)
  const [selectedText, setSelectedText] = useState("")
  const [selectedBlockId, setSelectedBlockId] = useState<string | null>(null)
  const [selectionPosition, setSelectionPosition] = useState<{ x: number; y: number } | null>(null)
  const [aiInput, setAiInput] = useState("")
  const [aiTyping, setAiTyping] = useState(false)
  const [aiMessages, setAiMessages] = useState<Array<{ role: "user" | "assistant"; text: string }>>(() => (
    demoRuntime
      ? [{ role: "assistant", text: "我是栖阅 AI 阅读助手。可以为你总结文章、解释段落，或围绕选中的文字继续提问。" }]
      : []
  ))
  const articleRef = useRef<HTMLElement>(null)
  const aiScrollRef = useRef<HTMLDivElement>(null)
  const aiTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const progressControllerRef = useRef<ArticleProgressController | null>(null)
  const pdfProgressControllerRef = useRef<PdfProgressController | null>(null)
  const restoredProgressRef = useRef<string | null>(null)
  const restoredPdfProgressRef = useRef<string | null>(null)
  const resourceRequestRef = useRef(0)
  const markerListRequestRef = useRef(0)
  const bookmarkRequestRef = useRef(0)
  const storageScopeRef = useRef(storageScope)
  const persistedStorageScopeRef = useRef(storageScope)
  const readingSettingsAppliedRef = useRef(false)
  const readingSettingsTouchedRef = useRef(false)
  const readingLayout = resolveReadingPresentation(readingSettings, false)
  // scroll 监听是空依赖 effect；用 ref 读取最新 activeHeading 作为 blockId。
  const activeHeadingRef = useRef(demoRuntime ? DEMO_OUTLINE[0].id : "")

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
    setFont(presentation.fontFamily)
    setFontSize(presentation.fontSizePx)
    setLineHeight(readingSettings.lineHeight)
  }, [readingSettings, readingSettingsStatus])

  const contentMatchesSession = contentState.status !== "idle"
    && contentState.storageId === storageId
    && contentState.contentUri === sessionContentUri
  const articleDocument = demoRuntime
    ? DEMO_DOCUMENT
    : (contentState.status === "ready" && contentMatchesSession ? contentState.document : null)
  const articleSections = articleDocument?.sections ?? []
  const outline = useMemo(
    () => articleDocument ? articleOutline(articleDocument) : [],
    [articleDocument],
  )
  const contentReady = demoRuntime
    || (sessionView.status === "ready" && (contentState.status === "ready" || contentState.status === "pdf_ready") && contentMatchesSession)
  const sessionBookmark = tauriRuntime && mediaItemId
    ? findArticleBookmark(
      sessionMarkers,
      mediaItemId,
      activeHeading || null,
      Math.max(0, Math.min(1, progress / 100)),
    )
    : null
  const pdfSource = contentState.status === "pdf_ready" && contentMatchesSession ? contentState.source : null

  const isDark = theme === "slate" || theme === "dark" || theme === "custom"
  const themeClass = {
    paper: "bg-[#fbfbfb] text-[#242426]",
    warm: "bg-[#f4eee1] text-[#3b3226]",
    slate: "bg-[#25262a] text-[#e1e2e6]",
    dark: "bg-[#0e0f12] text-[#d4d5d9]",
    sepia: "bg-[#f4ecd8] text-[#5b4636]",
    eyeCare: "bg-[#cce8cc] text-[#2e4a2e]",
    custom: "bg-[#f4eee1] text-[#3b3226]",
  }[theme]
  const bodyFont = font === "serif"
    ? "font-serif"
    : font === "kai"
      ? "font-serif italic"
      : "font-sans"
  // 会话内排版工具调整优先于全局 Reading 初始值。
  const bodyLineHeight = lineHeight === "compact" ? 1.65 : lineHeight === "airy" ? 2.05 : 1.85
  const contentWidth = `${readingLayout.contentWidthPx}px`

  useEffect(() => {
    recordDemoArticleReaderHistory(clientMode, storageId, recordHistory)
  }, [clientMode, storageId])

  // 路由复用时按媒体重新装载业务状态，避免标注或 marker ID 泄漏到另一篇文章。
  useLayoutEffect(() => {
    storageScopeRef.current = storageScope
    markerListRequestRef.current += 1
    bookmarkRequestRef.current += 1
    setIsBookmarked(loadDemoArticleReaderValue(
      clientMode,
      () => (
        localStorage.getItem(`haven:article-bookmark:${storageId}`) === "1"
        || localStorage.getItem(articleMarkerKey) === "1"
      ),
      false,
    ))
    setArticleMarkerId(null)
    setIsBookmarkPending(false)
    setSessionMarkers([])
    setMarkersLoaded(false)
    setHighlights(loadDemoArticleReaderValue(clientMode, () => readHighlights(storageId), []))
    setNotes(loadDemoArticleReaderValue(clientMode, () => readNotes(storageId), []))
    setSelectedText("")
    setSelectedBlockId(null)
    setSelectionPosition(null)
    setShowTranslations(false)
    const initialHeading = demoRuntime ? DEMO_OUTLINE[0].id : ""
    setActiveHeading(initialHeading)
    activeHeadingRef.current = initialHeading
    setProgress(0)
    restoredProgressRef.current = null
    setContentState({ status: "idle" })
  }, [articleMarkerKey, clientMode, demoRuntime, storageId, storageScope])

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

  useEffect(() => {
    if (!tauriRuntime || !mediaItemId || !markersLoaded) return
    if (sessionMarkers.length > 0) return
    const legacyHighlights = readHighlights(storageId)
    const legacyNotes = readNotes(storageId).filter((note) => note.id !== "default-note")
    if (legacyHighlights.length === 0 && legacyNotes.length === 0) return
    const sections = articleDocument?.sections ?? []
    for (const highlight of legacyHighlights) {
      const paragraphText = findParagraphText(sections, highlight.blockId) ?? highlight.text
      const textAnchor = buildArticleTextAnchor(paragraphText, highlight.text)
      const locator = articleMarkerLocator(highlight.blockId, 0, textAnchor)
      void createMarker({
        mediaItemId,
        locator,
        markerType: "highlight",
        title: null,
        excerpt: highlight.text,
        note: null,
      })
        .then((marker) => setSessionMarkers((current) => [...current, marker]))
        .catch(() => undefined)
    }
    for (const note of legacyNotes) {
      const paragraphText = findParagraphText(sections, note.blockId) ?? note.text
      const textAnchor = buildArticleTextAnchor(paragraphText, note.text)
      const locator = articleMarkerLocator(note.blockId, 0, textAnchor)
      void createMarker({
        mediaItemId,
        locator,
        markerType: "note",
        title: null,
        excerpt: note.text,
        note: note.note ?? null,
      })
        .then((marker) => setSessionMarkers((current) => [...current, marker]))
        .catch(() => undefined)
    }
    try {
      localStorage.removeItem(`haven:article-highlights:${storageId}`)
      localStorage.removeItem(`haven:article-notes:${storageId}`)
      localStorage.removeItem(`haven:article-bookmark:${storageId}`)
      localStorage.removeItem(articleMarkerKey)
    } catch {
      // ignore
    }
  }, [articleDocument, articleMarkerKey, mediaItemId, markersLoaded, sessionMarkers.length, storageId, tauriRuntime])

  useEffect(() => {
    if (!tauriRuntime || sessionView.status !== "ready" || isBookmarkPending || !markersLoaded) return
    setIsBookmarked(sessionBookmark !== null)
    setArticleMarkerId(sessionBookmark?.markerId ?? null)
  }, [isBookmarkPending, markersLoaded, sessionBookmark, sessionView.status, tauriRuntime])

  // Tauri 只读取 Session 签发的受控 URI；Abort 和请求序号共同阻止旧会话覆盖新内容。
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
    setContentState({ status: "loading", storageId, contentUri: sessionContentUri })
    const loadContent = async () => {
      // Probe a bounded prefix first. PDFs use the returned total length to construct
      // a range transport; article HTML is then fetched as one bounded,
      // sanitised snapshot. A PDF source that does not honour Range fails
      // closed instead of silently downloading the whole document.
      const probe = await fetchSessionResource(sessionContentUri, {
        range: `bytes=0-${PDF_RANGE_CHUNK_SIZE - 1}`,
        signal: abortController.signal,
      })
      if (isPdfMimeType(probe.contentType)) {
        if (!probe.partial || probe.totalBytes === null) {
          throw new HavenError({
            code: "SOURCE_RANGE_UNSUPPORTED",
            userMessage: "该 PDF 来源不支持分段读取，请先下载到本地",
            retryable: false,
          })
        }
        return {
          kind: "pdf" as const,
          source: {
            contentUri: sessionContentUri,
            totalBytes: probe.totalBytes,
            initialData: probe.bytes,
          },
        }
      }
      const resource = probe.partial
        ? await fetchSessionResource(sessionContentUri, { signal: abortController.signal })
        : probe
      return { kind: "article" as const, resource }
    }
    void loadContent()
      .then((loaded) => {
        if (resourceRequestRef.current !== requestId || abortController.signal.aborted) return
        if (loaded.kind === "pdf") {
          setContentState({ status: "pdf_ready", storageId, contentUri: sessionContentUri, source: loaded.source })
          return
        }
        if (loaded.resource.kind === "empty") {
          setContentState({ status: "empty", storageId, contentUri: sessionContentUri })
          return
        }
        const text = decodeArticleText(loaded.resource.bytes, loaded.resource.contentType)
        const document = parseArticleContent(text, loaded.resource.contentType)
        setContentState(document
          ? { status: "ready", storageId, contentUri: sessionContentUri, document }
          : { status: "empty", storageId, contentUri: sessionContentUri })
      })
      .catch((error: unknown) => {
        if (resourceRequestRef.current !== requestId || abortController.signal.aborted) return
        const havenError = toHavenError(error)
        setContentState(havenError.retryable
          ? { status: "retryable_error", storageId, contentUri: sessionContentUri, error: havenError }
          : { status: "terminal_error", storageId, contentUri: sessionContentUri, error: havenError })
      })

    return () => abortController.abort()
  }, [contentRetryNonce, sessionContentUri, sessionView.status, storageId, tauriRuntime])

  useEffect(() => {
    if (!contentReady || outline.length === 0) return
    const firstBlockId = outline[0].id
    setActiveHeading((current) => outline.some((item) => item.id === current) ? current : firstBlockId)
    if (!outline.some((item) => item.id === activeHeadingRef.current)) activeHeadingRef.current = firstBlockId
  }, [contentReady, outline])

  useEffect(() => {
    activeHeadingRef.current = activeHeading
  }, [activeHeading])

  // 文章正文与 PDF 使用互斥控制器，避免窗口滚动产生错误的 PDF 进度。
  useEffect(() => {
    progressControllerRef.current = null
    pdfProgressControllerRef.current = null
    if (!tauriRuntime || sessionView.status !== "ready" || state.status !== "ready" || !contentMatchesSession) return
    const controller = contentState.status === "pdf_ready"
      ? createPdfProgressController({ session: state.session, retry })
      : contentState.status === "ready"
        ? createArticleProgressController({ session: state.session, retry })
        : null
    if (!controller) return
    if (contentState.status === "pdf_ready") pdfProgressControllerRef.current = controller as PdfProgressController
    else progressControllerRef.current = controller as ArticleProgressController
    registerReleaseBarrier(() => controller.cleanup())
    return () => {
      progressControllerRef.current = null
      pdfProgressControllerRef.current = null
      registerReleaseBarrier(null)
      void controller.cleanup()
    }
  }, [contentMatchesSession, contentState.status, sessionContentUri, sessionView.status, state, retry, registerReleaseBarrier, tauriRuntime])

  // 恢复进度：优先使用与真实大纲同源的 blockId，结构已变时才回退 progression。
  useEffect(() => {
    if (!contentReady || !articleDocument || sessionView.status !== "ready" || state.status !== "ready") return
    const restored = restoreArticleProgress(state.session, restoredProgressRef)
    if (!restored) return
    const maxScroll = document.documentElement.scrollHeight - window.innerHeight
    if (maxScroll <= 0) return
    // `progression` is the whole-document ratio. A block id is only a coarse
    // activity label, not a second coordinate; applying both double-counts
    // the remaining distance (p * (2 - p)).
    window.scrollTo({ top: Math.min(maxScroll, Math.max(0, restored.progression * maxScroll)) })
  }, [articleDocument, contentReady, outline, sessionView.status, state, sessionContentUri])

  useEffect(() => {
    if (!demoRuntime) {
      persistedStorageScopeRef.current = storageScope
      return
    }
    // storageId 改变后的首轮 effect 仍持有旧 state；跳过该轮，防止覆盖新媒体的本地数据。
    if (persistedStorageScopeRef.current !== storageScope) {
      persistedStorageScopeRef.current = storageScope
      return
    }
    localStorage.setItem(`haven:article-bookmark:${storageId}`, isBookmarked ? "1" : "0")
    localStorage.setItem(articleMarkerKey, isBookmarked ? "1" : "0")
    localStorage.setItem(`haven:article-highlights:${storageId}`, JSON.stringify(highlights))
    localStorage.setItem(`haven:article-notes:${storageId}`, JSON.stringify(notes))
  }, [articleMarkerKey, demoRuntime, highlights, isBookmarked, notes, storageId, storageScope])

  useEffect(() => {
    const onScroll = () => {
      const maxScroll = document.documentElement.scrollHeight - window.innerHeight
      const ratio = maxScroll <= 0 ? 0 : Math.min(1, Math.max(0, window.scrollY / maxScroll))
      setChromeDimmed(window.scrollY > 48)
      if (pdfSource) return
      setProgress(ratio * 100)
      // Tauri 环境向进度控制器报告 progression（0..1）+ 当前小节 blockId。
      if (tauriRuntime && articleDocument && maxScroll > 0) {
        progressControllerRef.current?.scroll(ratio, activeHeadingRef.current)
      }
    }
    window.addEventListener("scroll", onScroll, { passive: true })
    onScroll()
    return () => window.removeEventListener("scroll", onScroll)
  }, [articleDocument, pdfSource, tauriRuntime])

  useEffect(() => {
    const observer = new IntersectionObserver(
      (entries) => {
        const visible = entries.filter((entry) => entry.isIntersecting)
        if (visible[0]?.target.id) setActiveHeading(visible[0].target.id)
      },
      { rootMargin: "-96px 0px -70% 0px" },
    )
    if (contentReady) articleRef.current?.querySelectorAll("h1, h2").forEach((heading) => observer.observe(heading))
    return () => observer.disconnect()
  }, [contentReady, articleDocument])

  /** 文章书签：Tauri 环境创建/软删除真实 marker（article Locator）；浏览器演示环境只切本地状态。 */
  const toggleArticleBookmark = () => {
    if (isBookmarkPending || (tauriRuntime && !markersLoaded)) return
    if (tauriRuntime && isBookmarked && !sessionBookmark && articleMarkerId === null) return
    const next = !isBookmarked
    setIsBookmarked(next)
    if (!tauriRuntime || !mediaItemId) return
    markerListRequestRef.current += 1
    const requestId = ++bookmarkRequestRef.current
    const requestStorageScope = storageScope
    const requestIsCurrent = () => (
      bookmarkRequestRef.current === requestId
      && storageScopeRef.current === requestStorageScope
    )
    setIsBookmarkPending(true)
    if (next) {
      void createMarker({
        mediaItemId,
        locator: articleMarkerLocator(activeHeadingRef.current, Math.max(0, Math.min(1, progress / 100))),
        markerType: "bookmark",
        title: null,
        excerpt: null,
        note: null,
      })
        .then((marker) => {
          if (!requestIsCurrent()) return
          setSessionMarkers((current) => [...current.filter((item) => item.markerId !== marker.markerId), marker])
          setArticleMarkerId(marker.markerId)
        })
        .catch(() => {
          if (!requestIsCurrent()) return
          // 创建失败：回滚书签状态，避免显示后端不存在的标记。
          setIsBookmarked(false)
          setArticleMarkerId(null)
        })
        .finally(() => {
          if (requestIsCurrent()) setIsBookmarkPending(false)
        })
      return
    }
    const removingMarker = sessionBookmark
      ?? sessionMarkers.find((marker) => marker.markerId === articleMarkerId)
      ?? null
    if (removingMarker) {
      const removing = removingMarker.markerId
      setArticleMarkerId(null)
      setSessionMarkers((current) => current.filter((marker) => marker.markerId !== removing))
      void deleteMarker(removing).catch(() => {
        if (!requestIsCurrent()) return
        // 删除失败：恢复书签状态与 markerId。
        setSessionMarkers((current) => [...current, removingMarker])
        setIsBookmarked(true)
        setArticleMarkerId(removing)
      }).finally(() => {
        if (requestIsCurrent()) setIsBookmarkPending(false)
      })
      return
    }
    setIsBookmarked(true)
    setIsBookmarkPending(false)
  }

  useEffect(() => {
    if (aiScrollRef.current) aiScrollRef.current.scrollTop = aiScrollRef.current.scrollHeight  }, [aiMessages, aiTyping])

  useEffect(() => () => {
    if (aiTimerRef.current) clearTimeout(aiTimerRef.current)
  }, [])

  const scrollToHeading = (id: string) => {
    document.getElementById(id)?.scrollIntoView({ behavior: "smooth", block: "start" })
    setActiveHeading(id)
    setOutlineOpen(false)
  }

  const handleSelection = () => {
    const selection = window.getSelection()
    const text = selection?.toString().trim() || ""
    if (!text || !selection?.rangeCount) {
      setSelectedText("")
      setSelectedBlockId(null)
      setSelectionPosition(null)
      return
    }
    const range = selection.getRangeAt(0)
    const startBlock = closestArticleBlock(range.startContainer)
    const endBlock = closestArticleBlock(range.endContainer)
    const blockId = startBlock?.dataset.articleBlockId
    if (!startBlock || startBlock !== endBlock || !blockId) {
      setSelectedText("")
      setSelectedBlockId(null)
      setSelectionPosition(null)
      return
    }
    const rect = range.getBoundingClientRect()
    setSelectedText(text)
    setSelectedBlockId(blockId)
    setSelectionPosition({
      x: Math.min(window.innerWidth - 120, Math.max(120, rect.left + rect.width / 2)),
      y: Math.max(84, rect.top - 12),
    })
  }

  const addHighlight = () => {
    if (!selectedText || !selectedBlockId) return
    const blockId = selectedBlockId
    const text = selectedText
    if (tauriRuntime && mediaItemId) {
      const sections = articleDocument?.sections ?? []
      const paragraphText = findParagraphText(sections, blockId) ?? text
      const textAnchor = buildArticleTextAnchor(paragraphText, text)
      const locator = articleMarkerLocator(blockId, Math.max(0, Math.min(1, progress / 100)), textAnchor)
      const already = sessionMarkers.some(
        (marker) =>
          marker.markerType === "highlight" &&
          marker.locator.kind === "article" &&
          marker.locator.data.blockId === blockId &&
          (marker.excerpt === text || marker.locator.data.textAnchor?.exact === text),
      )
      if (already) {
        dismissSelection()
        return
      }
      void createMarker({
        mediaItemId,
        locator,
        markerType: "highlight",
        title: null,
        excerpt: text,
        note: null,
      })
        .then((marker) => setSessionMarkers((current) => [...current, marker]))
        .catch(() => undefined)
      dismissSelection()
      return
    }
    setHighlights((current) => current.some((highlight) => (
      highlight.blockId === blockId && highlight.text === text
    )) ? current : [...current, { blockId, text }])
    setNotes((current) => [
      { id: Date.now().toString(), blockId, text, time: "刚刚" },
      ...current,
    ])
    dismissSelection()
  }

  const removeHighlight = (highlight: ArticleHighlight) => {
    if (tauriRuntime && mediaItemId) {
      const target = sessionMarkers.find(
        (marker) =>
          marker.markerType === "highlight" &&
          marker.locator.kind === "article" &&
          marker.locator.data.blockId === highlight.blockId &&
          (marker.excerpt === highlight.text || marker.locator.data.textAnchor?.exact === highlight.text),
      )
      if (!target) return
      setSessionMarkers((current) => current.filter((marker) => marker.markerId !== target.markerId))
      void deleteMarker(target.markerId).catch(() => {
        setSessionMarkers((current) => [...current, target])
      })
      return
    }
    setHighlights((current) => current.filter((item) => !(
      item.blockId === highlight.blockId && item.text === highlight.text
    )))
    setNotes((current) => current.filter((note) => !(
      note.blockId === highlight.blockId && note.text === highlight.text
    )))
  }

  const askAboutSelection = () => {
    if (!selectedText) return
    setAiOpen(true)
    setAiMessages((current) => [...current, { role: "user", text: `请解释这段内容：“${selectedText}”` }])
    dismissSelection()
    simulateAiResponse("这段内容强调了本地优先软件的核心价值：减少用户与内容之间的操作距离，让工具把复杂性留在系统内部。")
  }

  const dismissSelection = () => {
    setSelectedText("")
    setSelectedBlockId(null)
    setSelectionPosition(null)
    window.getSelection()?.removeAllRanges()
  }

  const simulateAiResponse = (text: string) => {
    setAiTyping(true)
    if (aiTimerRef.current) clearTimeout(aiTimerRef.current)
    aiTimerRef.current = setTimeout(() => {
      setAiTyping(false)
      setAiMessages((current) => [...current, { role: "assistant", text }])
    }, 900)
  }

  const submitAiMessage = () => {
    const text = aiInput.trim()
    if (!text || aiTyping) return
    setAiInput("")
    setAiMessages((current) => [...current, { role: "user", text }])
    simulateAiResponse(`围绕“${text}”，本文的关键方向是让本地内容、阅读上下文与用户意图保持连续。`)
  }

  const handleAiKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault()
      submitAiMessage()
    }
  }

  const renderHighlightedText = (text: string, blockId: string): ReactNode[] => {
    const marked = effectiveHighlights
      .filter((highlight) => (
        highlight.text.length > 0 && highlight.blockId === blockId
      ))
      .sort((a, b) => b.text.length - a.text.length)
    if (marked.length === 0) return [text]
    let parts: ReactNode[] = [text]
    marked.forEach((highlight) => {
      parts = parts.flatMap((part, partIndex) => {
        if (typeof part !== "string") return part
        const chunks = part.split(highlight.text)
        return chunks.flatMap((chunk, index) => index === chunks.length - 1
          ? [chunk]
          : [chunk, <mark key={`${blockId}-${highlight.text}-${partIndex}-${index}`} onClick={() => removeHighlight(highlight)} className="cursor-pointer rounded-sm bg-amber-300/30 px-0.5 text-inherit underline decoration-amber-700/45 decoration-dotted underline-offset-4 transition-colors hover:bg-amber-300/50" title="点击取消划线">{highlight.text}</mark>])
      })
    })
    return parts
  }

  const renderHighlightedChildren = (children: ReactNode, blockId: string): ReactNode[] =>
    Children.toArray(children).flatMap((child) => (
      typeof child === "string" ? renderHighlightedText(child, blockId) : [child]
    ))

  const renderSanitizedHtml = (html: string, blockId: string): ReactNode => {
    const root = new DOMParser().parseFromString(html, "text/html").body
    const allowed = new Set(["a", "article", "blockquote", "br", "code", "dd", "div", "dl", "dt", "em", "h1", "h2", "h3", "h4", "h5", "h6", "header", "hr", "li", "main", "ol", "p", "pre", "section", "strong", "table", "tbody", "td", "tfoot", "th", "thead", "tr", "ul"])
    const renderNode = (node: Node, key: string, inheritedBlockId: string): ReactNode => {
      if (node.nodeType === Node.TEXT_NODE) return renderHighlightedText(node.textContent ?? "", inheritedBlockId)
      if (node.nodeType !== Node.ELEMENT_NODE) return null
      const element = node as HTMLElement
      const tag = element.tagName.toLowerCase()
      if (!allowed.has(tag)) return Array.from(element.childNodes).map((child, index) => renderNode(child, `${key}-${index}`, inheritedBlockId))
      const currentBlockId = element.dataset.articleBlockId ?? inheritedBlockId
      const props: Record<string, string> = { key }
      for (const attribute of Array.from(element.attributes)) {
        if (["class", "id", "title", "data-article-block-id"].includes(attribute.name)) {
          props[attribute.name === "class" ? "className" : attribute.name] = attribute.value
        }
      }
      return createElement(tag, props, Array.from(element.childNodes).map((child, index) => renderNode(child, `${key}-${index}`, currentBlockId)))
    }
    return Array.from(root.childNodes).map((node, index) => renderNode(node, `html-${index}`, blockId))
  }

  return (
    <div className={cn("min-h-[100dvh] overflow-x-hidden transition-colors duration-500", themeClass)}>
      <header className={cn(
        "fixed inset-x-0 top-0 z-50 flex h-[68px] items-center justify-between border-b px-5 backdrop-blur-2xl transition-opacity duration-500 sm:px-[32px]",
        isDark ? "border-white/10 bg-[#0e0f12]/80" : "border-black/[0.07] bg-white/75",
        chromeDimmed ? "opacity-55 hover:opacity-100" : "opacity-100",
      )}>
        <div className="flex min-w-0 items-center gap-3">
          <button type="button" onClick={() => navigate(-1)} aria-label="返回" className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full transition-colors hover:bg-black/[0.06] dark:hover:bg-white/[0.08]">
            <ArrowLeft className="h-[16px] w-[16px]" />
          </button>
          <div className="min-w-0">
            <p className="truncate text-sm font-semibold">{articleDocument?.title ?? "本地文章"}</p>
            <p className="truncate text-[11px] opacity-55">
              {tauriRuntime ? "本地受控资源" : "栖阅深度研究 · 12 min read"}
            </p>
          </div>
        </div>
        <div className="flex items-center gap-1">
          <button type="button" onClick={() => setOutlineOpen((open) => !open)} disabled={!contentReady || Boolean(pdfSource)} aria-label="打开文章目录" className={cn("flex h-9 w-9 items-center justify-center rounded-full transition-colors hover:bg-black/[0.06] disabled:cursor-not-allowed disabled:opacity-35 dark:hover:bg-white/[0.08]", outlineOpen && "bg-primary/10 text-primary")}>
            <ListTree className="h-[17px] w-[17px]" />
          </button>
          <button type="button" onClick={() => setSettingsOpen((open) => !open)} aria-label="打开阅读排版设置" className={cn("flex h-9 w-9 items-center justify-center rounded-full text-sm font-bold transition-colors hover:bg-black/[0.06] dark:hover:bg-white/[0.08]", settingsOpen && "bg-primary/10 text-primary")}>
            aA
          </button>
          {demoRuntime && (
            <button type="button" onClick={() => setAiOpen((open) => !open)} aria-label="打开 AI 助读" className={cn("flex h-9 w-9 items-center justify-center rounded-full transition-colors hover:bg-black/[0.06] dark:hover:bg-white/[0.08]", aiOpen && "bg-primary/10 text-primary")}>
              <Sparkles className="h-[17px] w-[17px]" />
            </button>
          )}
        </div>
      </header>

      <main className="mx-auto w-full max-w-[820px] px-6 pb-[128px] pt-28 sm:px-10">
        {tauriRuntime && (sessionView.status === "opening" || sessionView.status === "idle") && (
          <p className="py-[64px] text-center text-sm opacity-55">正在准备阅读…</p>
        )}
        {tauriRuntime && (sessionView.status === "retryable_error" || sessionView.status === "terminal_error") && (
          <div className="space-y-3 py-[64px] text-center">
            <p className="text-base font-semibold">{sessionView.message}</p>
            {sessionView.retryable && (
              <button
                type="button"
                onClick={retry}
                className="rounded-full border border-current/20 px-4 py-2 text-sm font-semibold transition-colors hover:bg-current/5"
              >
                重试
              </button>
            )}
          </div>
        )}
        {tauriRuntime && sessionView.status === "ready" && (!contentMatchesSession || contentState.status === "loading") && (
            <p className="py-[64px] text-center text-sm opacity-55">正在读取内容…</p>
        )}
        {tauriRuntime && sessionView.status === "ready" && contentMatchesSession && contentState.status === "empty" && (
          <div className="space-y-2 py-[64px] text-center">
            <p className="text-base font-semibold">内容为空</p>
            <p className="text-sm opacity-55">这个资源没有可显示的内容。</p>
          </div>
        )}
        {tauriRuntime && sessionView.status === "ready" && contentMatchesSession && (contentState.status === "retryable_error" || contentState.status === "terminal_error") && (
          <div className="space-y-3 py-[64px] text-center">
            <p className="text-base font-semibold">{contentState.error.message}</p>
            {contentState.status === "retryable_error" && (
              <button
                type="button"
                onClick={() => setContentRetryNonce((value) => value + 1)}
                className="rounded-full border border-current/20 px-4 py-2 text-sm font-semibold transition-colors hover:bg-current/5"
              >
                重试
              </button>
            )}
          </div>
        )}
        {contentReady && pdfSource && state.status === "ready" && <PdfReader key={state.session.sessionId} source={pdfSource} restoreLocator={(pageCount) => (
          restorePdfProgress(pageCount, state.session, restoredPdfProgressRef)
        )} onLocatorChange={(locator) => {
          const progression = locator.pageCount <= 1 ? 0 : locator.pageIndex / (locator.pageCount - 1)
          setProgress(progression * 100)
          pdfProgressControllerRef.current?.locatorChange(locator)
        }} />}
        {contentReady && articleDocument && (
          <article ref={articleRef} onMouseUp={handleSelection} onTouchEnd={handleSelection} className={cn("mx-auto w-full select-text", bodyFont)} style={{ maxWidth: contentWidth, fontSize: `${fontSize}px`, lineHeight: bodyLineHeight }}>
            <header className="border-b border-current/15 pb-[48px]">
              <p className="text-xs font-semibold uppercase tracking-[0.22em] text-primary">
                {tauriRuntime ? "LOCAL ARTICLE" : "《Torto 架构专栏》 · 2026.08"}
              </p>
              <h1 id={articleDocument.format === "html" ? undefined : articleSections[0]?.id} className="mt-7 scroll-mt-28 text-4xl font-semibold leading-[1.12] tracking-[-0.055em] sm:text-6xl">{articleDocument.title}</h1>
              {articleSections[0]?.dek && <p className="mt-6 max-w-[590px] text-[0.95em] leading-[1.7] opacity-60">{articleSections[0].dek}</p>}
              <div className="mt-7 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] font-medium opacity-45">
                <span>{tauriRuntime ? "纯文本文章" : "深度文章"}</span><span>·</span><span>约 {articleDocument.characterCount.toLocaleString()} 字</span><span>·</span><span>本地快照</span>
              </div>
            </header>

            <div className="mt-14 space-y-[64px]">
              {articleDocument.format === "html" && articleDocument.sanitizedHtml ? (
                <div
                  className="article-html space-y-7 leading-[inherit] [&_a]:underline [&_blockquote]:my-8 [&_blockquote]:border-l-2 [&_blockquote]:border-primary/65 [&_blockquote]:py-2 [&_blockquote]:pl-6 [&_code]:rounded [&_code]:bg-black/[0.06] [&_code]:px-1 [&_pre]:overflow-x-auto [&_pre]:rounded-xl [&_pre]:bg-black/[0.06] [&_pre]:p-4 [&_ul]:list-disc [&_ul]:pl-6 [&_ol]:list-decimal [&_ol]:pl-6"
                >{renderSanitizedHtml(articleDocument.sanitizedHtml, "article-body")}</div>
              ) : articleSections.map((section, sectionIndex) => (
                <section key={section.id} className="scroll-mt-28">
                  {sectionIndex > 0 && (
                    <div className="mb-[48px] h-px w-[64px] bg-primary/50" aria-hidden="true" />
                  )}
                  {sectionIndex > 0 && (
                    <div className="mb-7">
                      <p className="mb-3 text-[10px] font-semibold uppercase tracking-[0.22em] text-primary">Section {String(sectionIndex + 1).padStart(2, "0")}</p>
                      <h2 id={section.id} className="scroll-mt-28 text-3xl font-semibold leading-tight tracking-[-0.04em] sm:text-4xl">{section.title}</h2>
                      {section.dek && <p className="mt-[16px] text-[0.9em] leading-relaxed opacity-55">{section.dek}</p>}
                    </div>
                  )}
                  {section.paragraphs.map((paragraph) => (
                    <div key={paragraph.id} data-article-block-id={paragraph.id} className="mb-7">
                      {articleDocument.format === "markdown" ? (
                        <ReactMarkdown
                          components={{
                            p: ({ children }) => <p>{renderHighlightedChildren(children, paragraph.id)}</p>,
                            strong: ({ children }) => <strong>{renderHighlightedChildren(children, paragraph.id)}</strong>,
                            em: ({ children }) => <em>{renderHighlightedChildren(children, paragraph.id)}</em>,
                            a: ({ children }) => <span className="underline decoration-current/40 underline-offset-4">{renderHighlightedChildren(children, paragraph.id)}</span>,
                            img: () => <span className="opacity-55">[图片已隐藏]</span>,
                          }}
                        >
                          {paragraph.text}
                        </ReactMarkdown>
                      ) : (
                        <p className="leading-[inherit]">{renderHighlightedText(paragraph.text, paragraph.id)}</p>
                      )}
                      {demoRuntime && showTranslations && paragraph.translation && (
                        <p className="mt-[16px] border-l-2 border-primary/35 pl-5 text-[0.82em] leading-[1.8] opacity-55">{paragraph.translation}</p>
                      )}
                    </div>
                  ))}
                  {section.quote && (
                    <blockquote className="my-[48px] border-l-2 border-primary/65 py-[8px] pl-6 text-[1.1em] leading-[1.75] opacity-80">“{section.quote}”</blockquote>
                  )}
                </section>
              ))}
            </div>
          </article>
        )}
      </main>

      {outlineOpen && contentReady && (
        <div className="fixed inset-0 z-[60]" onClick={() => setOutlineOpen(false)}>
          <aside onClick={(event) => event.stopPropagation()} className={cn("absolute left-[16px] top-[80px] w-[min(340px,calc(100vw-2rem))] rounded-2xl border p-5 shadow-2xl backdrop-blur-2xl", isDark ? "border-white/10 bg-[#17181b]/95" : "border-black/[0.08] bg-white/95")}>
            <div className="flex items-start justify-between gap-[16px] border-b border-current/10 pb-[16px]">
              <div><p className="text-[10px] font-semibold uppercase tracking-[0.2em] text-primary">Article Outline</p><h2 className="mt-1 text-base font-semibold">文章目录</h2></div>
              <button type="button" onClick={() => setOutlineOpen(false)} aria-label="关闭目录" className="rounded-full p-1.5 opacity-55 hover:bg-black/[0.06] hover:opacity-100 dark:hover:bg-white/[0.08]"><X className="h-[16px] w-[16px]" /></button>
            </div>
            <nav className="mt-[16px] space-y-[8px]">
              {outline.map((item) => (
                <button key={item.id} type="button" onClick={() => scrollToHeading(item.id)} className={cn("flex min-h-[44px] w-full items-center justify-between rounded-xl px-[16px] py-[10px] text-left text-[13px] leading-5 transition-colors hover:bg-black/[0.05] dark:hover:bg-white/[0.08]", item.level === 2 && "pl-[32px] text-[0.9em] opacity-65", activeHeading === item.id && "bg-primary/10 text-primary font-semibold")}>
                  <span className="truncate">{item.title}</span><ChevronRight className="h-[16px] w-[16px] shrink-0 opacity-45" />
                </button>
              ))}
            </nav>
            <button type="button" onClick={toggleArticleBookmark} disabled={!contentReady || Boolean(pdfSource) || isBookmarkPending || (tauriRuntime && !markersLoaded)} className={cn("mt-[16px] flex min-h-[46px] w-full items-center justify-center gap-[10px] rounded-xl border px-[16px] py-[12px] text-sm leading-5 font-semibold transition-colors disabled:cursor-not-allowed disabled:opacity-35", isBookmarked ? "border-primary/30 bg-primary/10 text-primary" : "border-current/10 hover:bg-black/[0.05] dark:hover:bg-white/[0.08]")}>
              <Bookmark className={cn("h-[16px] w-[16px]", isBookmarked && "fill-current")} />
              {isBookmarked ? "已存文章书签" : "保存文章书签"}
            </button>
          </aside>
        </div>
      )}

      {settingsOpen && (
        <div className="fixed right-[16px] top-[80px] z-[65] w-[min(330px,calc(100vw-2rem))] rounded-2xl border border-black/[0.08] bg-white/95 p-5 text-[#1d1d1f] shadow-2xl backdrop-blur-2xl dark:border-white/10 dark:bg-[#17181b]/95 dark:text-white">
          <div className="flex items-center justify-between border-b border-current/10 pb-[16px]"><div><p className="text-[10px] font-semibold uppercase tracking-[0.2em] text-primary">Reading Settings</p><h2 className="mt-1 text-base font-semibold">阅读排版</h2></div><button type="button" onClick={() => setSettingsOpen(false)} aria-label="关闭排版设置" className="rounded-full p-1.5 opacity-55 hover:bg-black/[0.06] hover:opacity-100 dark:hover:bg-white/[0.08]"><X className="h-[16px] w-[16px]" /></button></div>
          <div className="mt-5 space-y-5 text-sm">
            <div><p className="mb-[8px] text-xs font-semibold opacity-55">字体</p><div className="grid grid-cols-3 gap-[8px]"><button type="button" onClick={() => { markReadingSettingsTouched(); setFont("serif") }} className={cn("rounded-xl border px-3 py-[10px] text-left font-serif", font === "serif" ? "border-primary bg-primary/10 text-primary" : "border-current/10")}>Serif <span className="block text-[10px] opacity-55">精致衬线</span></button><button type="button" onClick={() => { markReadingSettingsTouched(); setFont("sans") }} className={cn("rounded-xl border px-3 py-[10px] text-left font-sans", font === "sans" ? "border-primary bg-primary/10 text-primary" : "border-current/10")}>Sans <span className="block text-[10px] opacity-55">清晰无衬线</span></button><button type="button" onClick={() => { markReadingSettingsTouched(); setFont("kai") }} className={cn("rounded-xl border px-3 py-[10px] text-left font-serif italic", font === "kai" ? "border-primary bg-primary/10 text-primary" : "border-current/10")}>楷体 <span className="block text-[10px] opacity-55">中文阅读</span></button></div></div>
             <div><div className="mb-[8px] flex items-center justify-between text-xs font-semibold opacity-55"><span>字号</span><span>{fontSize}px</span></div><div className="flex items-center gap-3"><button type="button" onClick={() => { markReadingSettingsTouched(); setFontSize((size) => Math.max(15, size - 1)) }} aria-label="减小字号" className="flex h-[32px] w-[32px] items-center justify-center rounded-full bg-black/[0.05] hover:bg-black/[0.1] dark:bg-white/[0.08]"><Minus className="h-[16px] w-[16px]" /></button><div className="h-1 flex-1 rounded-full bg-current/10"><div className="h-full rounded-full bg-primary" style={{ width: `${((fontSize - 15) / 9) * 100}%` }} /></div><button type="button" onClick={() => { markReadingSettingsTouched(); setFontSize((size) => Math.min(24, size + 1)) }} aria-label="增大字号" className="flex h-[32px] w-[32px] items-center justify-center rounded-full bg-black/[0.05] hover:bg-black/[0.1] dark:bg-white/[0.08]"><Plus className="h-[16px] w-[16px]" /></button></div></div>
            <div><p className="mb-[8px] text-xs font-semibold opacity-55">行距</p><div className="flex gap-[8px]"><button type="button" onClick={() => { markReadingSettingsTouched(); setLineHeight("compact") }} className={cn("rounded-full px-3 py-1.5 text-xs font-semibold", lineHeight === "compact" ? "bg-foreground text-background" : "bg-black/[0.05] dark:bg-white/[0.08]")}>紧凑</button><button type="button" onClick={() => { markReadingSettingsTouched(); setLineHeight("comfortable") }} className={cn("rounded-full px-3 py-1.5 text-xs font-semibold", lineHeight === "comfortable" ? "bg-foreground text-background" : "bg-black/[0.05] dark:bg-white/[0.08]")}>标准</button><button type="button" onClick={() => { markReadingSettingsTouched(); setLineHeight("airy") }} className={cn("rounded-full px-3 py-1.5 text-xs font-semibold", lineHeight === "airy" ? "bg-foreground text-background" : "bg-black/[0.05] dark:bg-white/[0.08]")}>舒展</button></div></div>
            <div><p className="mb-[8px] text-xs font-semibold opacity-55">纸张</p><div className="flex gap-3">{THEME_OPTIONS.map((option) => <button key={option.id} type="button" onClick={() => { markReadingSettingsTouched(); setTheme(option.id) }} aria-label={option.label} className={cn("h-[20px] w-[20px] rounded-full border-2 transition-transform", theme === option.id ? "scale-110 ring-2 ring-primary ring-offset-1" : "border-black/15 dark:border-white/20")} style={{ backgroundColor: option.color }} />)}</div></div>
            {demoRuntime && (
              <button type="button" onClick={() => setShowTranslations((show) => !show)} className="flex w-full items-center justify-between border-t border-current/10 pt-[16px] text-left text-xs font-semibold"><span className="flex items-center gap-[8px]"><Languages className="h-[16px] w-[16px] text-primary" />显示英文对照</span><span className={cn("h-5 w-9 rounded-full p-0.5 transition-colors", showTranslations ? "bg-primary" : "bg-black/15 dark:bg-white/15")}><span className={cn("block h-[16px] w-[16px] rounded-full bg-white shadow-sm transition-transform", showTranslations && "translate-x-[16px]")} /></span></button>
            )}
          </div>
        </div>
      )}

      {demoRuntime && aiOpen && (
        <div className="fixed inset-0 z-[55]" onClick={() => setAiOpen(false)}>
          <aside onClick={(event) => event.stopPropagation()} className={cn("absolute right-0 top-0 flex h-full w-[min(390px,100vw)] flex-col border-l pt-[68px] shadow-2xl backdrop-blur-2xl", isDark ? "border-white/10 bg-[#17181b]/95" : "border-black/[0.08] bg-white/95")}>
             <div className="flex items-center justify-between border-b border-current/10 px-5 py-[16px]"><div><p className="text-[10px] font-semibold uppercase tracking-[0.2em] text-primary">Torto Assistant</p><h2 className="mt-1 text-base font-semibold">AI 助读</h2></div><button type="button" onClick={() => setAiOpen(false)} aria-label="关闭 AI 助读" className="rounded-full p-[8px] opacity-55 hover:bg-black/[0.06] hover:opacity-100 dark:hover:bg-white/[0.08]"><X className="h-[16px] w-[16px]" /></button></div>
            <div ref={aiScrollRef} className="flex-1 space-y-[16px] overflow-y-auto px-5 py-5 text-sm">
              {aiMessages.map((message, index) => <div key={`${message.role}-${index}`} className={cn("rounded-2xl px-[16px] py-3 leading-relaxed", message.role === "user" ? "ml-[32px] bg-primary text-white" : "mr-[16px] bg-black/[0.05] dark:bg-white/[0.07]")}>{message.text}</div>)}
              {aiTyping && <div className="mr-[16px] flex gap-1 rounded-2xl bg-black/[0.05] px-[16px] py-[16px] dark:bg-white/[0.07]"><span className="h-1.5 w-1.5 animate-bounce rounded-full bg-primary" /><span className="h-1.5 w-1.5 animate-bounce rounded-full bg-primary [animation-delay:150ms]" /><span className="h-1.5 w-1.5 animate-bounce rounded-full bg-primary [animation-delay:300ms]" /></div>}
            </div>
             <div className="border-t border-current/10 p-[16px]"><textarea value={aiInput} onChange={(event) => setAiInput(event.target.value)} onKeyDown={handleAiKeyDown} rows={2} placeholder="围绕本文提问，Enter 发送" className="w-full resize-none rounded-xl border border-current/10 bg-black/[0.04] px-3 py-[10px] text-sm outline-none placeholder:opacity-45 focus:border-primary dark:bg-white/[0.06]" /><button type="button" onClick={submitAiMessage} disabled={!aiInput.trim() || aiTyping} className="mt-[8px] flex w-full items-center justify-center gap-[8px] rounded-xl bg-primary px-[16px] py-[10px] text-sm font-semibold text-white transition-opacity disabled:opacity-40"><Sparkles className="h-[16px] w-[16px]" />发送给助读</button></div>
          </aside>
        </div>
      )}

      {selectionPosition && (
        <div style={{ left: selectionPosition.x, top: selectionPosition.y }} className="fixed z-[70] flex -translate-x-1/2 -translate-y-full items-center gap-1 rounded-full border border-white/15 bg-[#1c1c1e]/95 px-[8px] py-1.5 text-xs font-semibold text-white shadow-2xl backdrop-blur-xl">
          <button type="button" onClick={addHighlight} disabled={articleDocument?.format !== "text"} title={articleDocument?.format === "text" ? undefined : "当前格式暂不支持正文划线"} className="flex items-center gap-1 rounded-full px-[10px] py-1.5 hover:bg-white/15 disabled:cursor-not-allowed disabled:opacity-40"><Highlighter className="h-3.5 w-3.5 text-amber-300" />划线</button>
          {demoRuntime && <button type="button" onClick={askAboutSelection} className="flex items-center gap-1 rounded-full px-[10px] py-1.5 text-amber-200 hover:bg-white/15"><Sparkles className="h-3.5 w-3.5" />问 AI</button>}
        </div>
      )}

      <div className="fixed inset-x-0 bottom-0 z-50 h-[2px] bg-current/10" aria-label={`阅读进度 ${Math.round(progress)}%`}><div className="h-full bg-primary transition-[width] duration-200" style={{ width: `${progress}%` }} /></div>
    </div>
  )
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function closestArticleBlock(node: Node): HTMLElement | null {
  const element = node instanceof Element ? node : node.parentElement
  return element?.closest<HTMLElement>("[data-article-block-id]") ?? null
}

function legacyDemoBlockId(text: string): string | null {
  if (!text) return null
  const matches = ARTICLE_SECTIONS.flatMap((section) => section.paragraphs)
    .filter((paragraph) => paragraph.text.includes(text))
  return matches.length === 1 ? matches[0].id : null
}

function readHighlights(storageId: string): ArticleHighlight[] {
  try {
    const saved = localStorage.getItem(`haven:article-highlights:${storageId}`)
    if (!saved) return []
    const parsed: unknown = JSON.parse(saved)
    if (!Array.isArray(parsed)) return []
    return parsed.flatMap((value): ArticleHighlight[] => {
      if (typeof value === "string" && value.length > 0) {
        const blockId = legacyDemoBlockId(value)
        return blockId ? [{ blockId, text: value }] : []
      }
      if (!isRecord(value) || typeof value.blockId !== "string" || typeof value.text !== "string") {
        return []
      }
      if (!value.text) return []
      const blockId = value.blockId === "legacy" ? legacyDemoBlockId(value.text) : value.blockId
      return blockId ? [{ blockId, text: value.text }] : []
    })
  } catch {
    return []
  }
}

function readNotes(storageId: string): ArticleNote[] {
  try {
    const saved = localStorage.getItem(`haven:article-notes:${storageId}`)
    if (!saved) return DEFAULT_NOTES
    const parsed: unknown = JSON.parse(saved)
    if (!Array.isArray(parsed)) return DEFAULT_NOTES
    return parsed.flatMap((value): ArticleNote[] => {
      if (!isRecord(value)
        || typeof value.id !== "string"
        || typeof value.text !== "string"
        || typeof value.time !== "string"
        || !value.text) {
        return []
      }
      const blockId = typeof value.blockId === "string" && value.blockId !== "legacy"
        ? value.blockId
        : legacyDemoBlockId(value.text)
      if (!blockId) return []
      return [{
        id: value.id,
        blockId,
        text: value.text,
        note: typeof value.note === "string" ? value.note : undefined,
        time: value.time,
      }]
    })
  } catch {
    return DEFAULT_NOTES
  }
}
