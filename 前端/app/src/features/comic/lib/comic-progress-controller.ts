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
      locator: { version: 1, kind: "comic", data: { chapterItemId: session.mediaItemId, pageIndex: Math.max(0, entry.pageIndex - 1), pageProgression: entry.pageProgression } },
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
  const update = (pageIndex: number, pageProgression?: number) => {
    if (stopped || !Number.isInteger(pageIndex) || pageIndex < 1 || pageIndex > totalPages) return
    const pp = pageProgression != null && Number.isFinite(pageProgression) && pageProgression >= 0 && pageProgression <= 1 ? pageProgression : null
    if (pageIndex === totalPages) completed = true
    latest = { pageIndex, pageProgression: pp }
    if (inFlight) queued = true
    else if (!timer) timer = setTimeout(() => { timer = undefined; void run() }, throttleMs)
  }
  return {
    identity,
    pageChange: update,
    flush,
    cleanup: async () => { await flush() },
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
  const identity = `${session.sessionId}:${session.mediaItemId}:${session.contentUri}`
  if (restored?.current === identity) return null
  const data = session.progress.locator.data
  if (!Number.isInteger(data.pageIndex) || data.pageIndex < 0 || data.pageIndex >= totalPages) return null
  if (restored) restored.current = identity
  return { pageIndex: data.pageIndex + 1, pageProgression: data.pageProgression ?? null }
}
