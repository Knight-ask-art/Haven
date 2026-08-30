import { useCallback, useEffect, useRef, useState } from "react"
import type { ChangeEvent } from "react"
import "./foliate-js/view.js"
import { useMediaSession } from "@/features/session/useMediaSession"
import { fetchSessionResource } from "@/features/session/ipc/resource-fetch"
import { selectReaderSessionView } from "@/features/reader/lib/reader-session-view"
import { isTauriRuntime } from "@/lib/ipc/runtime.js"
import { HavenError } from "@/lib/ipc/errors"

/**
 * SPIKE-FOLIATE-001 渲染验证台。
 *
 * 仅存在于 codex/spike-foliate-js-001 分支；路由经 import.meta.env.DEV 门控，
 * 生产构建整块消除。浏览器层只验证库本身能力，不构成任何桌面验收证据
 * （真实链路验收归 Day 2/3 的 custom-protocol 构建）。
 */

interface FoliateBookMetadata {
  title?: unknown
}

interface FoliateTocItem {
  label?: string
  href?: string
  subitems?: FoliateTocItem[]
}

interface FoliateView extends HTMLElement {
  open(book: Blob | string): Promise<void>
  init(options?: { lastLocation?: string | null; showTextStart?: boolean }): Promise<unknown>
  goTo(target: string | number): Promise<unknown>
  prev(distance?: number): Promise<unknown>
  next(distance?: number): Promise<unknown>
  close(): void
  readonly book?: {
    metadata?: FoliateBookMetadata
    toc?: FoliateTocItem[]
  }
}

interface RelocateDetail {
  cfi?: string
  fraction?: number
  location?: unknown
}

interface TocRow {
  depth: number
  label: string
  href: string
}

type SpikeStatus =
  | { kind: "idle" }
  | { kind: "loading"; name: string }
  | { kind: "ready"; title: string }
  | { kind: "error"; message: string }

const VIEW_TAG = "foliate-view"

function pickTitle(metadata: FoliateBookMetadata | undefined): string {
  const title = metadata?.title
  if (typeof title === "string" && title.length > 0) return title
  if (title && typeof title === "object") {
    const first = Object.values(title as Record<string, unknown>)
      .find(value => typeof value === "string")
    if (typeof first === "string") return first
  }
  return "未命名书籍"
}

function flattenToc(items: FoliateTocItem[], depth = 0, acc: TocRow[] = []): TocRow[] {
  items.forEach(item => {
    if (item.label && item.href) acc.push({ depth, label: item.label, href: item.href })
    if (item.subitems?.length) flattenToc(item.subitems, depth + 1, acc)
  })
  return acc
}

export default function FoliateSpikePage() {
  const hostRef = useRef<HTMLDivElement>(null)
  const viewRef = useRef<FoliateView | null>(null)
  const openSeqRef = useRef(0)
  const [status, setStatus] = useState<SpikeStatus>({ kind: "idle" })
  const [toc, setToc] = useState<TocRow[]>([])
  const [relocate, setRelocate] = useState<RelocateDetail | null>(null)
  const [autoNote, setAutoNote] = useState("")
  const [debug, setDebug] = useState<{ errors: number; pwned: string }>({ errors: 0, pwned: "未检测" })
  const [openMs, setOpenMs] = useState<number | null>(null)

  // Spike 运行参数（一次性解析）：?mediaItemId= 走桌面受控链路；?src= 走浏览器层。
  const urlParamsRef = useRef<URLSearchParams | null>(null)
  if (urlParamsRef.current === null) urlParamsRef.current = new URLSearchParams(window.location.search)
  const urlParams = urlParamsRef.current
  const spikeMediaItemId = urlParams.get("mediaItemId") ?? undefined
  const tauriMode = isTauriRuntime() && spikeMediaItemId !== undefined

  // 受控会话（桌面层）。浏览器层传入 undefined，hook 保持 idle。
  const sessionState = useMediaSession(tauriMode ? spikeMediaItemId : undefined, "reader")
  const sessionView = selectReaderSessionView(sessionState.state, spikeMediaItemId)

  const ready = status.kind === "ready"

  const disposeView = useCallback(() => {
    const view = viewRef.current
    viewRef.current = null
    setToc([])
    setRelocate(null)
    if (!view) return
    try {
      view.close()
    } catch {
      // close() 在 renderer 已销毁时可能抛错；卸载路径允许吞掉。
    }
    view.remove()
  }, [])

  // 卸载时销毁视图，防止 iframe/blob 资源泄漏到下一个挂载。
  useEffect(() => disposeView, [disposeView])

  const handleFile = useCallback(async (file: File) => {
    const startedAt = performance.now()
    const seq = ++openSeqRef.current
    disposeView()
    setStatus({ kind: "loading", name: file.name })
    const host = hostRef.current
    if (!host) return
    const view = document.createElement(VIEW_TAG) as FoliateView
    view.style.display = "block"
    view.style.width = "100%"
    view.style.height = "100%"
    view.addEventListener("relocate", event => {
      if (openSeqRef.current !== seq) return
      setRelocate((event as CustomEvent<RelocateDetail>).detail)
    })
    viewRef.current = view
    host.append(view)
    try {
      await view.open(file)
      if (openSeqRef.current !== seq) {
        view.close()
        view.remove()
        return
      }
      await view.init({ showTextStart: true })
      if (openSeqRef.current !== seq) return
      setOpenMs(Math.round(performance.now() - startedAt))
      setToc(flattenToc(view.book?.toc ?? []))
      setStatus({ kind: "ready", title: pickTitle(view.book?.metadata) })
    } catch (error) {
      if (openSeqRef.current !== seq) return
      try {
        view.close()
      } catch {
        // 同上：终态清理以 remove() 为准。
      }
      view.remove()
      if (viewRef.current === view) viewRef.current = null
      setStatus({
        kind: "error",
        message: error instanceof Error ? error.message : String(error),
      })
    }
  }, [disposeView])

  const onInputChange = useCallback((event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0]
    if (file) void handleFile(file)
    event.target.value = ""
  }, [handleFile])

  const step = useCallback((delta: 1 | -1) => {
    const view = viewRef.current
    if (!view) return
    if (delta > 0) void view.next(1)
    else void view.prev(1)
  }, [])

  const goToHref = useCallback((href: string) => {
    void viewRef.current?.goTo(href)
  }, [])

  // 来源获取（浏览器层）：?src= 从 dev/public 取书后走与手选文件相同的 open 路径。
  const browserSourceStartedRef = useRef(false)
  useEffect(() => {
    if (tauriMode || browserSourceStartedRef.current) return
    const src = urlParams.get("src")
    if (!src) return
    browserSourceStartedRef.current = true
    void (async () => {
      try {
        const response = await fetch(src)
        if (!response.ok) throw new Error(`HTTP ${response.status}`)
        const blob = await response.blob()
        const name = src.split("/").pop() ?? "book.epub"
        await handleFile(new File([blob], name, { type: "application/epub+zip" }))
      } catch (error) {
        setAutoNote(`auto-error: ${error instanceof Error ? error.message : String(error)}`)
      }
    })()
  }, [handleFile, tauriMode, urlParams])

  // 桌面受控链路：session_open(engine=reader) 的 contentUri → 受控字节 → foliate。
  const sessionLoadedRef = useRef<string | null>(null)
  useEffect(() => {
    if (!tauriMode || sessionView.status !== "ready" || sessionView.contentUri === null) return
    if (sessionLoadedRef.current === sessionView.contentUri) return
    sessionLoadedRef.current = sessionView.contentUri
    const contentUri = sessionView.contentUri
    void (async () => {
      try {
        const result = await fetchSessionResource(contentUri)
        if (result.kind === "empty") throw new Error("受控资源为空")
        const file = new File([result.bytes], `${spikeMediaItemId}.epub`, { type: result.contentType })
        await handleFile(file)
      } catch (error) {
        const message = error instanceof HavenError
          ? `${error.dto.code}: ${error.dto.userMessage}`
          : error instanceof Error ? error.message : String(error)
        setStatus({ kind: "error", message })
        setAutoNote(`auto-error: ${message}`)
      }
    })()
  }, [handleFile, sessionView, spikeMediaItemId, tauriMode])

  // 就绪后的自动序列：&goto= 目录跳转；&step=N 翻页 N 次（供无头取证）。
  const seqDoneRef = useRef(false)
  useEffect(() => {
    if (!ready || seqDoneRef.current) return
    const gotoTarget = urlParams.get("goto")
    const stepCount = Number(urlParams.get("step") ?? "0")
    if (gotoTarget === null && stepCount === 0) return
    seqDoneRef.current = true
    let cancelled = false
    void (async () => {
      try {
        await new Promise(resolve => setTimeout(resolve, 900))
        if (cancelled) return
        if (gotoTarget && viewRef.current) await viewRef.current.goTo(gotoTarget)
        for (let i = 0; i < stepCount; i++) {
          viewRef.current?.next(1)
          await new Promise(resolve => setTimeout(resolve, 350))
        }
        if (!cancelled) setAutoNote(note => note.startsWith("auto-error") ? note : "auto-sequence-done")
      } catch (error) {
        if (!cancelled) setAutoNote(`auto-error: ${error instanceof Error ? error.message : String(error)}`)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [ready, urlParams])


  // 调试面板数据源：顶层错误计数 + 恶意脚本标记的跨 frame 轮询（blob 同源可读）。
  useEffect(() => {
    let errors = 0
    const onError = () => {
      errors += 1
      setDebug(prev => ({ ...prev, errors }))
    }
    window.addEventListener("error", onError)
    window.addEventListener("unhandledrejection", onError)
    const timer = window.setInterval(() => {
      let pwned = "未检测"
      document.querySelectorAll("iframe").forEach(el => {
        try {
          const marker = (el.contentWindow as (Window & { __spike_pwned?: number }) | null)
            ?.__spike_pwned
          if (typeof marker === "number") pwned = `执行(${marker})`
        } catch {
          pwned = "读取受阻"
        }
      })
      setDebug(prev => (prev.pwned === pwned && prev.errors === errors ? prev : { errors, pwned }))
    }, 400)
    return () => {
      window.removeEventListener("error", onError)
      window.removeEventListener("unhandledrejection", onError)
      window.clearInterval(timer)
    }
  }, [])


  useEffect(() => {
    if (!ready) return
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowRight") {
        event.preventDefault()
        step(1)
      } else if (event.key === "ArrowLeft") {
        event.preventDefault()
        step(-1)
      }
    }
    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [ready, step])

  return (
    <div className="flex h-[100dvh] flex-col bg-neutral-950 text-neutral-100">
      <header className="flex h-[56px] shrink-0 items-center gap-3 border-b border-white/10 px-4">
        <span className="rounded bg-amber-400/15 px-2 py-0.5 text-[11px] font-semibold text-amber-300">
          SPIKE-FOLIATE-001
        </span>
        <p className="truncate text-sm font-semibold">
          {status.kind === "ready" ? status.title : "foliate-js 渲染验证台"}
        </p>
        <label className="ml-auto cursor-pointer rounded-full border border-white/20 px-3 py-1.5 text-xs font-semibold hover:bg-white/10">
          选择 EPUB 文件
          <input
            className="hidden"
            type="file"
            accept=".epub,application/epub+zip"
            onChange={onInputChange}
          />
        </label>
      </header>

      <div className="flex min-h-0 flex-1">
        <main className="relative min-w-0 flex-1 bg-white">
          <div ref={hostRef} className="absolute inset-0" />
          {status.kind !== "ready" && (
            <div className="absolute inset-0 z-10 flex items-center justify-center bg-neutral-950 px-6 text-center text-sm">
              {status.kind === "idle" && (
                <div className="max-w-md space-y-2 text-white/50">
                  <p>
                    {tauriMode
                      ? `受控会话状态：${sessionView.status}`
                      : "选择一本 EPUB 开始渲染。本页面仅验证 foliate-js 库能力（分页 / 目录 / 图片 / CSS / CFI）。"}
                  </p>
                  {sessionView.message && (
                    <p className="text-xs text-red-400">会话信息：{sessionView.message}</p>
                  )}
                </div>
              )}
              {status.kind === "loading" && (
                <p className="text-white/60">正在打开 {status.name}…</p>
              )}
              {status.kind === "error" && (
                <div className="max-w-md space-y-3">
                  <p className="text-base font-semibold text-red-400">打开失败</p>
                  <p className="break-all text-xs text-white/55">{status.message}</p>
                </div>
              )}
            </div>
          )}
        </main>

        <aside className="flex w-[280px] shrink-0 flex-col border-l border-white/10">
          <section className="border-b border-white/10 p-3 text-xs">
            <p className="mb-1 font-semibold text-white/70">当前进度（relocate）</p>
            <p className="break-all text-white/45">
              {relocate
                ? `fraction: ${typeof relocate.fraction === "number" ? relocate.fraction.toFixed(4) : "-"} · loc: ${String(relocate.location ?? "-")}`
                : "尚未产生 relocate 事件"}
            </p>
            <p className="mt-1 break-all text-white/35">
              {relocate?.cfi ? `CFI: ${relocate.cfi.slice(0, 96)}` : "CFI: -"}
            </p>
          </section>
          <nav className="min-h-0 flex-1 overflow-y-auto p-2">
            <p className="px-2 pb-1 text-[11px] font-semibold uppercase tracking-wider text-white/40">
              目录 ({toc.length})
            </p>
            {toc.map(row => (
              <button
                key={`${row.depth}-${row.href}`}
                type="button"
                onClick={() => goToHref(row.href)}
                className="block w-full truncate rounded px-2 py-1.5 text-left text-xs text-white/75 hover:bg-white/10"
                style={{ paddingLeft: `${8 + row.depth * 14}px` }}
              >
                {row.label}
              </button>
            ))}
            {ready && toc.length === 0 && (
              <p className="px-2 py-3 text-xs text-white/40">本书无目录数据</p>
            )}
          </nav>
          <section className="border-t border-white/10 p-3 font-mono text-[11px] leading-relaxed text-white/50">
            <p>debug.errors: {debug.errors}</p>
            <p>debug.script_pwned: {debug.pwned}</p>
            <p>perf.open_ms: {openMs ?? "-"}</p>
            <p>debug.auto: {autoNote || "—"}</p>
          </section>
          <footer className="flex gap-2 border-t border-white/10 p-3">
            <button
              type="button"
              disabled={!ready}
              onClick={() => step(-1)}
              className="flex-1 rounded border border-white/20 py-1.5 text-xs font-semibold disabled:opacity-30"
            >
              上一页 ←
            </button>
            <button
              type="button"
              disabled={!ready}
              onClick={() => step(1)}
              className="flex-1 rounded border border-white/20 py-1.5 text-xs font-semibold disabled:opacity-30"
            >
              → 下一页
            </button>
          </footer>
        </aside>
      </div>
    </div>
  )
}
