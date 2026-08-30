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
    const entry = latest
    latest = undefined
    const request: ProgressSaveRequest = {
      mediaItemId: session.mediaItemId,
      locator: { version: 1, kind: "article", data: { blockId: entry.blockId, progression: entry.progression, textAnchor: null } },
      completion: completed ? "completed" : "in_progress",
      expectedRevision: revision,
    }
    inFlight = save(request).then((result) => { revision = result.revision }).catch((error) => {
      conflict(error)
      if (!(error instanceof HavenError && error.code === "REVISION_CONFLICT") && !stopped) {
        latest = latest ?? entry
      }
    }).finally(() => { inFlight = undefined })
    await inFlight
    if (!stopped && queued) { queued = false; await run() }
  }
  const flush = async () => { clear(); if (latest !== undefined) { if (inFlight) queued = true; else await run() } if (inFlight) await inFlight; if (queued && !stopped) { queued = false; await run() } }
  const update = (progression: number, blockId?: string | null) => {
    if (stopped || !Number.isFinite(progression) || progression < 0 || progression > 1) return
    if (progression >= 0.99) completed = true
    latest = { progression, blockId: blockId ?? null }
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

/** 由 SessionOpenResultDto.progress 恢复文章滚动位置。 */
export function restoreArticleProgress(
  session: SessionOpenResultDto,
  restored?: { current: string | null },
): { progression: number; blockId: string | null } | null {
  if (!session.progress) return null
  if (session.progress.locator.kind !== "article") return null
  const identity = `${session.sessionId}:${session.mediaItemId}:${session.contentUri}`
  if (restored?.current === identity) return null
  const data = session.progress.locator.data
  if (data.progression == null || !Number.isFinite(data.progression) || data.progression < 0) return null
  if (restored) restored.current = identity
  return { progression: data.progression, blockId: data.blockId }
}
