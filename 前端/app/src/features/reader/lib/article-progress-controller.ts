import { resolveRestorePosition, type InitialPosition } from "@/features/progress/lib/restore-position"
import type { ProgressSaveRequest, ProgressSaveResult, SessionOpenResultDto } from "@/lib/ipc/generated/wire"
import { HavenError } from "@/lib/ipc/errors"
import { saveProgress } from "@/features/progress/ipc/progress-gateway"

export interface ArticleProgressControllerOptions {
  session: SessionOpenResultDto
  save?: (request: ProgressSaveRequest) => Promise<ProgressSaveResult>
  onRevisionConflict?: () => void
  retry?: () => void
  throttleMs?: number
}

export interface ArticleProgressController {
  readonly identity: string
  /** 滚动进度变化时调用（progression 0..1）。 */
  scroll(progression: number, blockId?: string | null): void
  flush(): Promise<void>
  cleanup(): Promise<void>
}

export function createArticleProgressController(options: ArticleProgressControllerOptions): ArticleProgressController {
  const { session } = options
  const save = options.save ?? ((request) => saveProgress(request))
  const throttleMs = options.throttleMs ?? 5000
  const identity = `${session.sessionId}:${session.mediaItemId}:${session.contentUri}`
  let revision = session.progress?.revision ?? null
  let completed = session.progress?.completion === "completed"
  let latest: { progression: number; blockId: string | null } | undefined
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
          locator: { version: 1, kind: "article", data: { blockId: entry.blockId, progression: entry.progression, textAnchor: null } },
          completion: completed ? "completed" : "in_progress",
          expectedRevision: revision,
        }
        try {
          const result = await save(request)
          revision = result.revision
        } catch (error: unknown) {
          // Ordinary failures retain the latest position for a later flush but
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
  const update = (progression: number, blockId?: string | null) => {
    if (stopped || closed || !Number.isFinite(progression) || progression < 0 || progression > 1) return
    if (progression >= 0.99) completed = true
    latest = { progression, blockId: blockId ?? null }
    if (!drainPromise && !timer) timer = setTimeout(() => { timer = undefined; void drain() }, throttleMs)
  }
  return {
    identity,
    scroll: update,
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

/** 由 SessionOpenResultDto.progress 恢复文章滚动位置。 */
export function restoreArticleProgress(
  session: SessionOpenResultDto,
  restored?: { current: string | null },
  initial?: InitialPosition,
): { progression: number; blockId: string | null } | null {
  const { locator, reset, identity, markRestored } = resolveRestorePosition(session, initial)
  if (locator?.kind !== "article") return null
  if (restored?.current === identity) return null
  if (reset) {
    if (restored) restored.current = identity
    markRestored()
    return { progression: 0, blockId: null }
  }
  const data = locator.data
  if (data.progression == null || !Number.isFinite(data.progression) || data.progression < 0) return null
  if (restored) restored.current = identity
  markRestored()
  return { progression: data.progression, blockId: data.blockId }
}
