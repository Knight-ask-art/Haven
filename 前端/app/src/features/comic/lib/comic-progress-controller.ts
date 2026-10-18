import type { ProgressSaveRequest, ProgressSaveResult, SessionOpenResultDto } from "@/lib/ipc/generated/wire"
import { HavenError } from "@/lib/ipc/errors"
import { saveProgress } from "@/features/progress/ipc/progress-gateway"

export interface ComicProgressControllerOptions {
  session: SessionOpenResultDto
  totalPages: number
  save?: (request: ProgressSaveRequest) => Promise<ProgressSaveResult>
  onRevisionConflict?: () => void
  retry?: () => void
  throttleMs?: number
}

export interface ComicProgressController {
  readonly identity: string
  /** 翻页时调用（pageIndex 从 1 开始；pageProgression 0..1 可选）。 */
  pageChange(pageIndex: number, pageProgression?: number): void
  flush(): Promise<void>
  cleanup(): Promise<void>
}

export function createComicProgressController(options: ComicProgressControllerOptions): ComicProgressController {
  const { session, totalPages } = options
  const save = options.save ?? ((request) => saveProgress(request))
  const throttleMs = options.throttleMs ?? 5000
  const identity = `${session.sessionId}:${session.mediaItemId}:${session.contentUri}`
  let revision = session.progress?.revision ?? null
  let completed = session.progress?.completion === "completed"
  let latest: { pageIndex: number; pageProgression: number | null } | undefined
  let timer: ReturnType<typeof setTimeout> | undefined
  let drainPromise: Promise<boolean> | undefined
  let stopped = false
  let closed = false
  let conflictNotified = false

  const clear = () => { if (timer) clearTimeout(timer); timer = undefined }
  const conflict = (error: unknown): boolean => {
    if (!(error instanceof HavenError) || error.code !== "REVISION_CONFLICT") return false
    stopped = true; clear(); latest = undefined
    if (!conflictNotified) { conflictNotified = true; options.onRevisionConflict?.(); options.retry?.() }
    return true
  }
  /** Single drain chain: every queued update is saved before this resolves. */
  const drain = (): Promise<boolean> => {
    if (drainPromise) return drainPromise
    const work = (async (): Promise<boolean> => {
      while (!stopped && !closed && latest !== undefined) {
        const entry = latest
        latest = undefined
        const request: ProgressSaveRequest = {
          mediaItemId: session.mediaItemId,
          locator: { version: 1, kind: "comic", data: { chapterItemId: session.mediaItemId, pageIndex: Math.max(0, entry.pageIndex - 1), pageProgression: entry.pageProgression } },
          completion: completed ? "completed" : "in_progress",
          expectedRevision: revision,
        }
        try {
          const result = await save(request)
          revision = result.revision
        } catch (error: unknown) {
          // Ordinary failures retain the latest page for a later flush but
          // must not spin a tight retry loop; conflicts stop saving entirely.
          if (!conflict(error) && !stopped && !closed) latest = latest ?? entry
          return true
        }
      }
      return false
    })()
    drainPromise = work.finally(() => { drainPromise = undefined })
    return drainPromise
  }
  const flush = async () => {
    let failed = false
    do {
      clear()
      if (!drainPromise && latest === undefined) return
      failed = await drain()
    } while (!stopped && !closed && !failed && latest !== undefined)
  }
  const update = (pageIndex: number, pageProgression?: number) => {
    if (stopped || closed || !Number.isInteger(pageIndex) || pageIndex < 1 || pageIndex > totalPages) return
    const pp = pageProgression != null && Number.isFinite(pageProgression) && pageProgression >= 0 && pageProgression <= 1 ? pageProgression : null
    if (pageIndex === totalPages) completed = true
    latest = { pageIndex, pageProgression: pp }
    if (!drainPromise && !timer) timer = setTimeout(() => { timer = undefined; void drain() }, throttleMs)
  }
  return {
    identity,
    pageChange: update,
    flush,
    cleanup: async () => {
      try {
        await flush()
      } finally {
        closed = true
        clear()
      }
    },
  }
}

/** 由 SessionOpenResultDto.progress 恢复漫画阅读位置。 */
export function restoreComicProgress(
  totalPages: number,
  session: SessionOpenResultDto,
  restored?: { current: string | null },
): { pageIndex: number; pageProgression: number | null } | null {
  if (!session.progress) return null
  if (session.progress.locator.kind !== "comic") return null
  if (!Number.isInteger(totalPages) || totalPages <= 0) return null
  const identity = `${session.sessionId}:${session.mediaItemId}:${session.contentUri}`
  if (restored?.current === identity) return null
  if (session.progress.completion === "not_started") {
    if (restored) restored.current = identity
    return { pageIndex: 1, pageProgression: null }
  }
  const data = session.progress.locator.data
  if (!Number.isInteger(data.pageIndex) || data.pageIndex < 0 || data.pageIndex >= totalPages) return null
  if (restored) restored.current = identity
  return { pageIndex: data.pageIndex + 1, pageProgression: data.pageProgression ?? null }
}
