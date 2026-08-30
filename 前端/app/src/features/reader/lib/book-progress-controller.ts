import type { ProgressSaveRequest, ProgressSaveResult, SessionOpenResultDto } from "@/lib/ipc/generated/wire"
import { HavenError } from "@/lib/ipc/errors"
import { saveProgress } from "@/features/progress/ipc/progress-gateway"
import { bookOffsetForProgression, setBookPaginationOffsetInstant, type BookPaginationMode, type BookPaginationViewport } from "./book-pagination"

export interface BookProgressControllerOptions {
  session: SessionOpenResultDto
  save?: (request: ProgressSaveRequest) => Promise<ProgressSaveResult>
  onRevisionConflict?: () => void
  retry?: () => void
  throttleMs?: number
}

export interface BookProgressController {
  readonly identity: string
  /** 滚动进度变化时调用（progression 0..1）。 */
  scroll(progression: number): void
  /** 暂停/隐藏时立即刷盘。 */
  flush(): Promise<void>
  cleanup(): Promise<void>
}

/** 0.1 compatibility key: stable across short-lived Session URLs until a safe resource key is exposed. */
function publicationResourceOf(mediaItemId: string): string {
  return `haven-resource://text/${mediaItemId}`
}

export function createBookProgressController(options: BookProgressControllerOptions): BookProgressController {
  const { session } = options
  const save = options.save ?? ((request) => saveProgress(request))
  const throttleMs = options.throttleMs ?? 5000
  const identity = `${session.sessionId}:${session.mediaItemId}:${session.contentUri}`
  let revision = session.progress?.revision ?? null
  let completed = session.progress?.completion === "completed"
  let latest: number | undefined
  let timer: ReturnType<typeof setTimeout> | undefined
  let inFlight: Promise<void> | undefined
  let queued = false
  let stopped = false
  let conflictNotified = false

  const clear = () => { if (timer) clearTimeout(timer); timer = undefined }
  const conflict = (error: unknown) => {
    if (error instanceof HavenError && error.code === "REVISION_CONFLICT") {
      stopped = true; clear(); queued = false
      if (!conflictNotified) { conflictNotified = true; options.onRevisionConflict?.(); options.retry?.() }
    }
  }
  const run = async (): Promise<void> => {
    if (stopped || latest === undefined || inFlight) return
    const progression = latest
    latest = undefined
    const request: ProgressSaveRequest = {
      mediaItemId: session.mediaItemId,
      locator: { version: 1, kind: "book", data: { publicationResource: publicationResourceOf(session.mediaItemId), progression, textAnchor: null, formatLocator: null } },
      completion: completed ? "completed" : "in_progress",
      expectedRevision: revision,
    }
    inFlight = save(request).then((result) => { revision = result.revision }).catch((error) => {
      conflict(error)
      if (!(error instanceof HavenError && error.code === "REVISION_CONFLICT") && !stopped) {
        latest = latest ?? progression
      }
    }).finally(() => { inFlight = undefined })
    await inFlight
    if (!stopped && queued) { queued = false; await run() }
  }
  const flush = async () => { clear(); if (latest !== undefined) { if (inFlight) queued = true; else await run() } if (inFlight) await inFlight; if (queued && !stopped) { queued = false; await run() } }
  const update = (progression: number) => {
    if (stopped || !Number.isFinite(progression) || progression < 0 || progression > 1) return
    if (progression >= 0.99) completed = true
    latest = progression
    if (inFlight) queued = true
    else if (!timer) timer = setTimeout(() => { timer = undefined; void run() }, throttleMs)
  }
  return {
    identity,
    scroll: update,
    flush,
    cleanup: async () => { await flush() },
  }
}

/** 由 SessionOpenResultDto.progress 恢复阅读滚动位置（progression 0..1）。 */
export function restoreBookProgress(
  scrollContainer: {
    scrollTop: number
    scrollHeight: number
    clientHeight: number
    scrollLeft?: number
    scrollWidth?: number
    clientWidth?: number
  },
  session: SessionOpenResultDto,
  restored?: { current: string | null },
  mode: BookPaginationMode = "scroll",
): boolean {
  if (!session.progress) return false
  if (session.progress.locator.kind !== "book") return false
  const identity = `${session.sessionId}:${session.mediaItemId}:${session.contentUri}`
  if (restored?.current === identity) return false
  const progression = session.progress.locator.data.progression
  if (progression == null || !Number.isFinite(progression) || progression < 0) return false
  const viewport: BookPaginationViewport = {
    scrollLeft: scrollContainer.scrollLeft ?? 0,
    scrollTop: scrollContainer.scrollTop,
    scrollWidth: scrollContainer.scrollWidth ?? scrollContainer.clientWidth ?? 0,
    scrollHeight: scrollContainer.scrollHeight,
    clientWidth: scrollContainer.clientWidth ?? 0,
    clientHeight: scrollContainer.clientHeight,
  }
  const target = bookOffsetForProgression(viewport, progression, mode)
  const current = mode === "scroll" ? scrollContainer.scrollTop : (scrollContainer.scrollLeft ?? 0)
  if (Math.abs(current - target) < 2) return false
  if (mode !== "scroll" && scrollContainer.scrollLeft === undefined) return false
  setBookPaginationOffsetInstant(scrollContainer, target, mode)
  if (restored) restored.current = identity
  return true
}
