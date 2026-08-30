import { useCallback, useEffect, useMemo, useRef, useReducer, useState } from "react"
import type { ReactNode } from "react"
import { createPortal } from "react-dom"
import { Bell, CheckCircle2, CircleHelp, Info, Loader2, TriangleAlert, X } from "lucide-react"
import { NoticeContext, useNotice } from "./notice-context"
import type { Notice, NoticeCenterApi, NoticeInput, NoticeKind } from "./notice-context"

interface NoticeState {
  notices: Notice[]
}

type NoticeActionMessage =
  | { type: "push"; notice: Notice }
  | { type: "dismiss"; id: string }
  | { type: "clear" }

const MAX_NOTICES = 6
const DEFAULT_DURATION: Record<NoticeKind, number | null> = {
  info: 4200,
  success: 3200,
  warning: 8000,
  error: null,
  announcement: 10000,
  progress: null,
  confirm: null,
}

const DEFAULT_TITLE: Record<NoticeKind, string> = {
  info: "提示",
  success: "已完成",
  warning: "需要注意",
  error: "操作失败",
  announcement: "公告",
  progress: "处理中",
  confirm: "请确认",
}

function reducer(state: NoticeState, action: NoticeActionMessage): NoticeState {
  switch (action.type) {
    case "push": {
      const deduped = action.notice.dedupeKey
        ? state.notices.filter((notice) => notice.dedupeKey !== action.notice.dedupeKey)
        : state.notices
      const next = [...deduped, action.notice]
      if (next.length <= MAX_NOTICES) return { notices: next }

      // 确认通知必须保持可见，否则其 Promise 会永远悬挂。优先淘汰最早的
      // 非确认通知；当队列全是确认通知时暂时允许超过软上限，等待用户处理。
      const firstEvictable = next.findIndex((notice) => notice.kind !== "confirm")
      if (firstEvictable >= 0) next.splice(firstEvictable, 1)
      return { notices: next }
    }
    case "dismiss":
      return { notices: state.notices.filter((notice) => notice.id !== action.id) }
    case "clear":
      return { notices: [] }
    default:
      return state
  }
}

function nextNoticeId(counter: { current: number }): string {
  counter.current += 1
  return `notice-${Date.now().toString(36)}-${counter.current.toString(36)}`
}

function createNotice(input: NoticeInput, id: string): Notice {
  const kind = input.kind ?? "info"
  return {
    ...input,
    id,
    kind,
    title: input.title ?? DEFAULT_TITLE[kind],
    durationMs: input.durationMs === undefined ? DEFAULT_DURATION[kind] : input.durationMs,
    progress: input.progress === null || input.progress === undefined
      ? input.progress
      : Math.max(0, Math.min(1, input.progress)),
    createdAt: Date.now(),
  }
}

export function NoticeProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(reducer, { notices: [] })
  const counterRef = useRef(0)
  const timersRef = useRef(new Map<string, ReturnType<typeof setTimeout>>())
  const confirmResolversRef = useRef(new Map<string, (confirmed: boolean) => void>())
  const dedupeIdsRef = useRef(new Map<string, string>())

  const clearTimer = useCallback((id: string) => {
    const timer = timersRef.current.get(id)
    if (timer) clearTimeout(timer)
    timersRef.current.delete(id)
  }, [])

  const dismiss = useCallback((id: string) => {
    clearTimer(id)
    for (const [key, noticeId] of dedupeIdsRef.current) {
      if (noticeId === id) dedupeIdsRef.current.delete(key)
    }
    const resolver = confirmResolversRef.current.get(id)
    if (resolver) {
      confirmResolversRef.current.delete(id)
      resolver(false)
    }
    dispatch({ type: "dismiss", id })
  }, [clearTimer])

  const resolveConfirm = useCallback((id: string, confirmed: boolean) => {
    clearTimer(id)
    for (const [key, noticeId] of dedupeIdsRef.current) {
      if (noticeId === id) dedupeIdsRef.current.delete(key)
    }
    const resolver = confirmResolversRef.current.get(id)
    if (resolver) {
      confirmResolversRef.current.delete(id)
      resolver(confirmed)
    }
    dispatch({ type: "dismiss", id })
  }, [clearTimer])

  const enqueue = useCallback((input: NoticeInput, forcedId?: string): string => {
    const id = forcedId ?? nextNoticeId(counterRef)
    if (input.dedupeKey) {
      const previousId = dedupeIdsRef.current.get(input.dedupeKey)
      if (previousId && previousId !== id) dismiss(previousId)
      dedupeIdsRef.current.set(input.dedupeKey, id)
    }
    const notice = createNotice(input, id)
    dispatch({ type: "push", notice })
    if (notice.durationMs !== null) {
      const timer = setTimeout(() => dismiss(id), notice.durationMs)
      timersRef.current.set(id, timer)
    }
    return id
  }, [dismiss])

  const push = useCallback((input: NoticeInput) => enqueue(input), [enqueue])

  const clear = useCallback(() => {
    for (const id of timersRef.current.keys()) clearTimer(id)
    for (const resolver of confirmResolversRef.current.values()) resolver(false)
    confirmResolversRef.current.clear()
    dedupeIdsRef.current.clear()
    dispatch({ type: "clear" })
  }, [clearTimer])

  // reducer 为保持视图有界可能淘汰普通通知；同步清理其 timer 和去重索引，
  // 避免通知已经不可见后仍被运行时句柄引用。
  useEffect(() => {
    const visibleIds = new Set(state.notices.map((notice) => notice.id))
    for (const id of timersRef.current.keys()) {
      if (!visibleIds.has(id)) clearTimer(id)
    }
    for (const [key, id] of dedupeIdsRef.current) {
      if (!visibleIds.has(id)) dedupeIdsRef.current.delete(key)
    }
  }, [clearTimer, state.notices])

  const confirm = useCallback((input: Omit<NoticeInput, "kind" | "action"> & {
    confirmLabel?: string
    cancelLabel?: string
  }) => {
    const id = nextNoticeId(counterRef)
    return new Promise<boolean>((resolve) => {
      confirmResolversRef.current.set(id, resolve)
      enqueue({ ...input, kind: "confirm", durationMs: null }, id)
    })
  }, [enqueue])

  useEffect(() => () => {
    for (const timer of timersRef.current.values()) clearTimeout(timer)
    for (const resolver of confirmResolversRef.current.values()) resolver(false)
    timersRef.current.clear()
    confirmResolversRef.current.clear()
  }, [])

  const api = useMemo<NoticeCenterApi>(() => ({
    notices: state.notices,
    push,
    dismiss,
    clear,
    resolveConfirm,
    confirm,
  }), [clear, confirm, dismiss, push, resolveConfirm, state.notices])

  return (
    <NoticeContext.Provider value={api}>
      {children}
      <NoticeViewport />
    </NoticeContext.Provider>
  )
}

function NoticeViewport() {
  const { notices, dismiss, resolveConfirm } = useNotice()

  if (typeof document === "undefined" || notices.length === 0) return null

  return createPortal(
    <div className="pointer-events-none fixed inset-x-0 top-0 z-[120] flex justify-end p-4 sm:p-5" aria-live="polite" aria-relevant="additions text">
      <div className="flex w-full max-w-[420px] flex-col gap-3">
        {notices.map((notice) => (
          <NoticeCard
            key={notice.id}
            notice={notice}
            dismiss={dismiss}
            onConfirm={notice.kind === "confirm" ? (confirmed) => resolveConfirm(notice.id, confirmed) : undefined}
          />
        ))}
      </div>
    </div>,
    document.body,
  )
}

function NoticeCard({
  notice,
  dismiss,
  onConfirm,
}: {
  notice: Notice
  dismiss: (id: string) => void
  onConfirm?: (confirmed: boolean) => void
}) {
  const [busy, setBusy] = useState(false)
  const Icon = notice.kind === "success"
    ? CheckCircle2
    : notice.kind === "warning" || notice.kind === "error"
      ? TriangleAlert
      : notice.kind === "announcement"
        ? Bell
        : notice.kind === "confirm"
          ? CircleHelp
          : notice.kind === "progress"
            ? Loader2
            : Info
  const tone = notice.kind === "success"
    ? "border-emerald-500/25 bg-emerald-50/95 text-emerald-950 dark:bg-emerald-950/90 dark:text-emerald-50"
    : notice.kind === "warning"
      ? "border-amber-500/30 bg-amber-50/95 text-amber-950 dark:bg-amber-950/90 dark:text-amber-50"
      : notice.kind === "error"
        ? "border-red-500/30 bg-red-50/95 text-red-950 dark:bg-red-950/90 dark:text-red-50"
        : "border-border/70 bg-background/95 text-foreground"
  const iconTone = notice.kind === "success"
    ? "text-emerald-600"
    : notice.kind === "warning"
      ? "text-amber-600"
      : notice.kind === "error"
        ? "text-red-600"
        : "text-primary"

  const runAction = async () => {
    if (!notice.action || busy) return
    setBusy(true)
    try {
      await notice.action.onClick()
      if (notice.action.dismiss !== false) dismiss(notice.id)
    } catch {
      // 动作通常已经由调用方发布了结构化错误通知。这里吞掉动作异常，
      // 防止用户点击“重试”后产生未处理 Promise rejection，并保留原通知供再次操作。
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className={`pointer-events-auto rounded-2xl border px-4 py-3 shadow-[0_16px_45px_rgba(0,0,0,0.16)] backdrop-blur-xl ${tone}`} role={notice.kind === "error" ? "alert" : "status"}>
      <div className="flex items-start gap-3">
        <Icon className={`mt-0.5 h-5 w-5 shrink-0 ${iconTone} ${notice.kind === "progress" ? "animate-spin" : ""}`} strokeWidth={2} />
        <div className="min-w-0 flex-1">
          <p className="text-sm font-semibold">{notice.title}</p>
          <p className="mt-1 whitespace-pre-wrap text-[13px] leading-5 opacity-85">{notice.message}</p>
          {notice.code && <p className="mt-1 font-mono text-[10px] opacity-60">{notice.code}</p>}
          {notice.kind === "progress" && notice.progress !== null && notice.progress !== undefined && (
            <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-black/10 dark:bg-white/10" aria-label={`${Math.round(notice.progress * 100)}%`}>
              <div className="h-full rounded-full bg-primary transition-[width]" style={{ width: `${Math.round(notice.progress * 100)}%` }} />
            </div>
          )}
          {notice.kind === "confirm" ? (
            <div className="mt-3 flex justify-end gap-2">
              <button type="button" className="rounded-full px-3 py-1.5 text-xs font-semibold opacity-75 hover:bg-black/5 dark:hover:bg-white/10" onClick={() => onConfirm?.(false)}> {notice.cancelLabel ?? "取消"} </button>
              <button type="button" className="rounded-full bg-primary px-3 py-1.5 text-xs font-semibold text-primary-foreground hover:opacity-90" onClick={() => onConfirm?.(true)}> {notice.confirmLabel ?? "确认"} </button>
            </div>
          ) : notice.action ? (
            <button type="button" disabled={busy} className="mt-2 rounded-full bg-primary px-3 py-1.5 text-xs font-semibold text-primary-foreground hover:opacity-90 disabled:opacity-50" onClick={() => void runAction()}>
              {busy ? "处理中…" : notice.action.label}
            </button>
          ) : null}
        </div>
        {notice.kind !== "confirm" && <button type="button" className="shrink-0 rounded-full p-1 opacity-60 hover:bg-black/5 hover:opacity-100 dark:hover:bg-white/10" aria-label="关闭通知" onClick={() => dismiss(notice.id)}><X className="h-4 w-4" /></button>}
      </div>
    </section>
  )
}
