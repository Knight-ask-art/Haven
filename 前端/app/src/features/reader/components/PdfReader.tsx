import { useEffect, useRef, useState } from "react"
import { ChevronLeft, ChevronRight, Minus, Plus, Search, X } from "lucide-react"
import { cn } from "@/lib/utils"
import type { TextAnchorDto } from "@/lib/ipc/generated/wire"
import { HavenError, toHavenError, type HavenError as HavenErrorType } from "@/lib/ipc/errors"
import { TextLayer } from "pdfjs-dist/legacy/build/pdf.mjs"
import type { PDFDocumentProxy, PDFPageProxy, RenderTask, TextLayer as PdfTextLayer } from "pdfjs-dist/legacy/build/pdf.mjs"
import { destroyPdfDocument, loadPdfDocument, PdfReaderError, type PdfDocumentSource } from "../lib/pdf-document"
import {
  clampPdfPage,
  clampPdfZoom,
  MAX_PDF_ZOOM,
  MIN_PDF_ZOOM,
  PDF_ZOOM_STEP,
  resolvePdfInitialView,
  type PdfInitialLocator,
} from "../lib/pdf-reader-state"
import {
  boundPdfPageText,
  flattenPdfSearchHits,
  searchPdfPages,
  type PdfSearchHit,
} from "../lib/pdf-search"

export interface PdfReaderLocator {
  pageIndex: number
  pageCount: number
  zoom: number
  /** Content-identity anchor derived from the rendered text layer. */
  textAnchor?: TextAnchorDto | null
}

interface PdfReaderProps {
  /** A bounded in-memory payload for legacy/local callers. */
  bytes?: ArrayBuffer
  /** Range-backed session source for remote or large local PDFs. */
  source?: PdfDocumentSource
  restoreLocator?: (pageCount: number) => PdfInitialLocator | null
  onLocatorChange?: (locator: PdfReaderLocator) => void
  className?: string
}

type PdfState =
  | { status: "loading" }
  | { status: "ready"; document: PDFDocumentProxy; page: number; numPages: number }
  | { status: "error"; error: HavenErrorType }

type PdfSearchUiState =
  | { status: "idle" }
  | { status: "scanning"; scanned: number }
  | { status: "done"; hits: PdfSearchHit[]; flat: Array<{ pageNumber: number; occurrence: number }>; totalMatches: number; activeIndex: number }

const MAX_CANVAS_PIXELS = 16_000_000
const MAX_CANVAS_SIDE = 8_192

function readablePdfError(error: unknown, fallback: string): HavenErrorType {
  if (!(error instanceof PdfReaderError)) return toHavenError(error)
  const userMessage = error.code === "PDF_TOO_LARGE"
    ? "PDF 文件超过当前版本的大小限制"
    : error.code === "PDF_CANCELLED"
      ? "PDF 读取已取消"
      : error.code === "PDF_RANGE_UNSUPPORTED"
        ? "该 PDF 来源不支持分段读取，请先下载到本地"
        : error.code === "PDF_RANGE_FAILED"
          ? "PDF 分段读取失败，请重试或下载到本地"
      : fallback
  return new HavenError({
    code: "FORMAT_UNSUPPORTED",
    userMessage,
    retryable: false,
  })
}

/**
 * PDF rendering is intentionally page-at-a-time. A single canvas and a
 * destroyed loading task keep large documents from accumulating WebView DOM
 * or raster memory. The text layer and per-page text cache are rebuilt per
 * document and bounded the same way.
 */
export function PdfReader({ bytes, source, restoreLocator, onLocatorChange, className }: PdfReaderProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const stageRef = useRef<HTMLDivElement>(null)
  const textLayerRef = useRef<HTMLDivElement>(null)
  const documentRef = useRef<PDFDocumentProxy | null>(null)
  const renderTaskRef = useRef<RenderTask | null>(null)
  const textLayerTaskRef = useRef<PdfTextLayer | null>(null)
  const requestRef = useRef(0)
  const restoreLocatorRef = useRef(restoreLocator)
  const locatorChangeRef = useRef(onLocatorChange)
  const pageTextCacheRef = useRef(new Map<number, string>())
  const searchAbortRef = useRef<AbortController | null>(null)
  const [zoom, setZoom] = useState(1)
  const [state, setState] = useState<PdfState>({ status: "loading" })
  const [pageDraft, setPageDraft] = useState("1")
  const [searchOpen, setSearchOpen] = useState(false)
  const [searchQuery, setSearchQuery] = useState("")
  const [searchState, setSearchState] = useState<PdfSearchUiState>({ status: "idle" })

  useEffect(() => {
    restoreLocatorRef.current = restoreLocator
  }, [restoreLocator])

  useEffect(() => {
    locatorChangeRef.current = onLocatorChange
  }, [onLocatorChange])

  useEffect(() => {
    if (state.status !== "ready") return
    setPageDraft(String(state.page))
  }, [state])

  useEffect(() => {
    const documentSource = source ?? bytes
    if (!documentSource) {
      setState({
        status: "error",
        error: new HavenError({
          code: "INVALID_ARGUMENT",
          userMessage: "PDF 资源不可用",
          retryable: false,
        }),
      })
      return
    }
    const requestId = ++requestRef.current
    setState({ status: "loading" })
    setZoom(1)
    setSearchOpen(false)
    setSearchQuery("")
    setSearchState({ status: "idle" })
    setPageDraft("1")
    pageTextCacheRef.current.clear()
    searchAbortRef.current?.abort()
    searchAbortRef.current = null

    const abortController = new AbortController()
    let active = true
    let loadedDocument: PDFDocumentProxy | null = null
    void loadPdfDocument(documentSource, { signal: abortController.signal })
      .then((document) => {
        if (!active || requestRef.current !== requestId) {
          void document.destroy()
          return
        }
        loadedDocument = document
        documentRef.current = document
        const initialView = resolvePdfInitialView(
          document.numPages,
          restoreLocatorRef.current?.(document.numPages),
        )
        setZoom(initialView?.zoom ?? 1)
        setState({ status: "ready", document, page: initialView?.page ?? 1, numPages: document.numPages })
      })
      .catch((error: unknown) => {
        if (!active || requestRef.current !== requestId) return
        setState({ status: "error", error: readablePdfError(error, "PDF 已损坏或格式不受支持") })
      })

    return () => {
      active = false
      renderTaskRef.current?.cancel()
      renderTaskRef.current = null
      textLayerTaskRef.current?.cancel()
      textLayerTaskRef.current = null
      searchAbortRef.current?.abort()
      searchAbortRef.current = null
      const document = loadedDocument
      loadedDocument = null
      documentRef.current = null
      abortController.abort()
      void destroyPdfDocument(document)
    }
  }, [bytes, source])

  useEffect(() => {
    if (state.status !== "ready") return
    const canvas = canvasRef.current
    const stage = stageRef.current
    if (!canvas || !stage) return
    const requestId = requestRef.current
    let cancelled = false
    let page: PDFPageProxy | null = null

    renderTaskRef.current?.cancel()
    renderTaskRef.current = null
    textLayerTaskRef.current?.cancel()
    textLayerTaskRef.current = null
    textLayerRef.current?.replaceChildren()

    const buildTextLayer = async (source: NonNullable<typeof page>, viewport: import("pdfjs-dist/legacy/build/pdf.mjs").PageViewport): Promise<string | null> => {
      const layerElement = textLayerRef.current
      if (!layerElement || cancelled || requestRef.current !== requestId) return null
      const textLayer = new TextLayer({
        textContentSource: source.streamTextContent({ includeMarkedContent: false }),
        container: layerElement,
        viewport,
      })
      textLayerTaskRef.current = textLayer
      try {
        await textLayer.render()
        const normalized = (layerElement.textContent ?? "").replace(/\s+/g, " ").trim()
        return normalized
      } catch {
        // Selection overlay is best-effort; canvas rendering remains authoritative.
        return null
      }
    }

    const render = async () => {
      try {
        page = await state.document.getPage(state.page)
        if (cancelled || requestRef.current !== requestId) return
        const baseViewport = page.getViewport({ scale: 1 })
        const boundedZoom = Math.min(
          zoom,
          MAX_CANVAS_SIDE / Math.max(1, baseViewport.width),
          MAX_CANVAS_SIDE / Math.max(1, baseViewport.height),
        )
        const viewport = page.getViewport({ scale: boundedZoom })
        const deviceScale = window.devicePixelRatio || 1
        const outputScale = Math.min(
          deviceScale,
          Math.sqrt(MAX_CANVAS_PIXELS / Math.max(1, viewport.width * viewport.height)),
        )
        stage.style.width = `${Math.ceil(viewport.width)}px`
        stage.style.height = `${Math.ceil(viewport.height)}px`
        canvas.width = Math.ceil(viewport.width * outputScale)
        canvas.height = Math.ceil(viewport.height * outputScale)
        const context = canvas.getContext("2d")
        if (!context) throw new Error("PDF canvas unavailable")
        context.clearRect(0, 0, canvas.width, canvas.height)
        const renderTask = page.render({
          canvasContext: context,
          viewport,
          transform: outputScale === 1 ? undefined : [outputScale, 0, 0, outputScale, 0, 0],
        })
        renderTaskRef.current = renderTask
        await renderTask.promise
        const pageText = await buildTextLayer(page, viewport)
        let textAnchor: TextAnchorDto | null = null
        if (pageText && pageText.length >= 12) {
          textAnchor = { exact: pageText.slice(0, 200), prefix: null, suffix: null }
        }
        if (!cancelled && requestRef.current === requestId) {
          locatorChangeRef.current?.({
            pageIndex: state.page - 1,
            pageCount: state.numPages,
            zoom,
            textAnchor,
          })
        }
      } catch (error: unknown) {
        if (cancelled || requestRef.current !== requestId) return
        if (error instanceof Error && error.name === "RenderingCancelledException") return
        setState({ status: "error", error: readablePdfError(error, "PDF 页面渲染失败") })
      } finally {
        page?.cleanup()
        page = null
        renderTaskRef.current = null
      }
    }
    void render()
    return () => {
      cancelled = true
      renderTaskRef.current?.cancel()
      renderTaskRef.current = null
      textLayerTaskRef.current?.cancel()
      textLayerTaskRef.current = null
      page = null
    }
  }, [state, zoom])

  if (state.status === "loading") {
    return <div className={cn("flex min-h-[240px] items-center justify-center text-sm opacity-60", className)}>正在解析 PDF…</div>
  }
  if (state.status === "error") {
    return <div className={cn("flex min-h-[240px] items-center justify-center px-6 text-center text-sm", className)}>{state.error.message}</div>
  }

  const setPage = (page: number) => {
    setState((current) => current.status === "ready"
      ? { ...current, page: clampPdfPage(page, current.numPages) }
      : current)
  }

  const commitPageDraft = () => {
    const parsed = Number.parseInt(pageDraft, 10)
    if (Number.isFinite(parsed) && parsed >= 1) {
      setPage(parsed)
    } else {
      setPageDraft(String(state.page))
    }
  }

  const getTextForPage = async (pageNumber: number): Promise<string> => {
    const cached = pageTextCacheRef.current.get(pageNumber)
    if (cached !== undefined) return cached
    const page = await state.document.getPage(pageNumber)
    const content = await page.getTextContent()
    const text = boundPdfPageText(content.items.map((item) => ("str" in item ? item.str : "")).join(" "))
    pageTextCacheRef.current.set(pageNumber, text)
    return text
  }

  const runSearch = async () => {
    const query = searchQuery.trim()
    if (!query) return
    searchAbortRef.current?.abort()
    const controller = new AbortController()
    searchAbortRef.current = controller
    setSearchState({ status: "scanning", scanned: 0 })
    try {
      const hits = await searchPdfPages({
        pageCount: state.numPages,
        query,
        getPageText: getTextForPage,
        signal: controller.signal,
        onProgress: (scanned) => {
          setSearchState((current) => current.status === "scanning" ? { ...current, scanned } : current)
        },
      })
      if (controller.signal.aborted) return
      const flat = flattenPdfSearchHits(hits)
      setSearchState({ status: "done", hits, flat, totalMatches: flat.length, activeIndex: 0 })
      if (flat[0]) setPage(flat[0].pageNumber)
    } catch {
      // Aborted by a newer search or unmount; the newer run owns the UI.
    }
  }

  const jumpSearchHit = (delta: number) => {
    if (searchState.status !== "done" || searchState.flat.length === 0) return
    const nextIndex = (searchState.activeIndex + delta + searchState.flat.length) % searchState.flat.length
    setSearchState({ ...searchState, activeIndex: nextIndex })
    setPage(searchState.flat[nextIndex].pageNumber)
  }

  const clearSearch = () => {
    searchAbortRef.current?.abort()
    searchAbortRef.current = null
    setSearchQuery("")
    setSearchState({ status: "idle" })
  }

  const searchSummary = (() => {
    if (searchState.status === "scanning") {
      return <span className="opacity-60">扫描中 {searchState.scanned}/{state.numPages}</span>
    }
    if (searchState.status === "done") {
      return searchState.totalMatches > 0
        ? <span className="tabular-nums">共 {searchState.totalMatches} 处 · 第 {searchState.flat[searchState.activeIndex].pageNumber} 页</span>
        : <span className="opacity-60">无匹配</span>
    }
    return null
  })()

  return (
    <section className={cn("flex min-h-full flex-col items-center gap-4 px-4 py-8", className)} aria-label="PDF 阅读器">
      <div className="sticky top-3 z-10 flex flex-wrap items-center justify-center gap-2 rounded-full border border-current/10 bg-background/90 px-3 py-2 text-xs shadow-lg backdrop-blur-xl">
        <button type="button" onClick={() => setPage(Math.max(1, state.page - 1))} disabled={state.page <= 1} aria-label="上一页" className="rounded-full p-1.5 transition-colors hover:bg-current/10 disabled:opacity-30"><ChevronLeft className="h-4 w-4" /></button>
        <input
          type="number"
          min={1}
          max={state.numPages}
          value={pageDraft}
          onChange={(event) => setPageDraft(event.target.value)}
          onBlur={commitPageDraft}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.currentTarget.blur()
              commitPageDraft()
            }
          }}
          aria-label="跳转到指定页"
          className="w-[54px] rounded-md border border-current/15 bg-transparent px-1 py-0.5 text-center tabular-nums outline-none focus:border-primary/50"
        />
        <span className="opacity-60">/ {state.numPages} 页</span>
        <button type="button" onClick={() => setPage(Math.min(state.numPages, state.page + 1))} disabled={state.page >= state.numPages} aria-label="下一页" className="rounded-full p-1.5 transition-colors hover:bg-current/10 disabled:opacity-30"><ChevronRight className="h-4 w-4" /></button>
        <span className="mx-1 h-4 w-px bg-current/15" aria-hidden="true" />
        <button type="button" onClick={() => setZoom((value) => clampPdfZoom(Number((value - PDF_ZOOM_STEP).toFixed(2))))} disabled={zoom <= MIN_PDF_ZOOM} aria-label="缩小 PDF" className="rounded-full p-1.5 transition-colors hover:bg-current/10 disabled:opacity-30"><Minus className="h-4 w-4" /></button>
        <span className="min-w-[42px] text-center tabular-nums">{Math.round(zoom * 100)}%</span>
        <button type="button" onClick={() => setZoom((value) => clampPdfZoom(Number((value + PDF_ZOOM_STEP).toFixed(2))))} disabled={zoom >= MAX_PDF_ZOOM} aria-label="放大 PDF" className="rounded-full p-1.5 transition-colors hover:bg-current/10 disabled:opacity-30"><Plus className="h-4 w-4" /></button>
        <span className="mx-1 h-4 w-px bg-current/15" aria-hidden="true" />
        <button
          type="button"
          onClick={() => setSearchOpen((open) => !open)}
          aria-label={searchOpen ? "关闭全文搜索" : "打开全文搜索"}
          aria-expanded={searchOpen}
          className={cn(
            "rounded-full p-1.5 transition-colors hover:bg-current/10",
            searchOpen && "bg-primary/10 text-primary"
          )}
        >
          <Search className="h-4 w-4" />
        </button>
      </div>

      {searchOpen && (
        <div className="sticky top-16 z-10 flex max-w-full flex-wrap items-center gap-2 rounded-full border border-current/10 bg-background/90 px-3 py-2 text-xs shadow-lg backdrop-blur-xl">
          <Search className="h-4 w-4 shrink-0 opacity-50" aria-hidden="true" />
          <input
            value={searchQuery}
            onChange={(event) => setSearchQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") void runSearch()
              if (event.key === "Escape") clearSearch()
            }}
            placeholder="搜索全文后回车"
            aria-label="PDF 全文搜索"
            className="w-44 bg-transparent outline-none placeholder:opacity-45"
          />
          {searchSummary}
          {searchState.status === "done" && searchState.totalMatches > 0 && (
            <>
              <button type="button" onClick={() => jumpSearchHit(-1)} aria-label="上一处匹配" className="rounded-full px-2 py-1 transition-colors hover:bg-current/10">上一处</button>
              <button type="button" onClick={() => jumpSearchHit(1)} aria-label="下一处匹配" className="rounded-full px-2 py-1 transition-colors hover:bg-current/10">下一处</button>
            </>
          )}
          <button type="button" onClick={clearSearch} aria-label="清除搜索" className="rounded-full p-1 transition-colors hover:bg-current/10"><X className="h-3.5 w-3.5" /></button>
        </div>
      )}

      <div ref={stageRef} className="relative max-w-full overflow-auto rounded-sm bg-white shadow-xl">
        <canvas ref={canvasRef} aria-label={`PDF 第 ${state.page} 页`} className="block h-full w-full" />
        <div ref={textLayerRef} className="pdf-text-layer" aria-hidden="true" />
      </div>
    </section>
  )
}
