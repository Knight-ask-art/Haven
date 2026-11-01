import { resolveRestorePosition, type InitialPosition } from "@/features/progress/lib/restore-position"
import type { ProgressSaveRequest, ProgressSaveResult, SessionOpenResultDto } from "@/lib/ipc/generated/wire"
import { HavenError } from "@/lib/ipc/errors"
import { saveProgress } from "@/features/progress/ipc/progress-gateway"

export interface VideoProgressControllerOptions {
  session: SessionOpenResultDto
  save?: (request: ProgressSaveRequest) => Promise<ProgressSaveResult>
  onRevisionConflict?: () => void
  retry?: () => void
  throttleMs?: number
  /** 截取当前帧，成功返回 data:image/jpeg;base64 否则 null */
  captureKeyframe?: () => Promise<string | null>
}

export interface VideoProgressController {
  readonly identity: string
  timeupdate(positionSeconds: number): void
  pause(positionSeconds?: number): Promise<void>
  seeked(positionSeconds: number): Promise<void>
  ended(positionSeconds: number): Promise<void>
  flush(): Promise<void>
  cleanup(): Promise<void>
}

/** Keep user/API seek requests inside the media element's finite timeline. */
export function clampVideoSeek(positionSeconds: number, durationSeconds: number): number {
  if (!Number.isFinite(positionSeconds)) return 0
  const position = Math.max(0, positionSeconds)
  if (!Number.isFinite(durationSeconds) || durationSeconds < 0) return position
  return Math.min(position, durationSeconds)
}

export function createVideoProgressController(options: VideoProgressControllerOptions): VideoProgressController {
  const { session } = options
  const save = options.save ?? ((request) => saveProgress(request))
  const throttleMs = options.throttleMs ?? 5000
  const identity = `${session.sessionId}:${session.mediaItemId}:${session.contentUri}`
  let revision = session.progress?.revision ?? null
  let latest: number | undefined
  // 与 book/pdf/comic 控制器一致：继承 Session 的已完成状态。否则重新打开一部
  // 已看完的作品后，一次普通的暂停/拖动就会把进度从 completed 改回 in_progress。
  let completed = session.progress?.completion === "completed"
  let latestCompletion: ProgressSaveRequest["completion"] = completed ? "completed" : "in_progress"
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
        const position = latest
        const completion = latestCompletion
        latest = undefined
        let keyframe: string | null = null
        if (options.captureKeyframe) {
          try {
            keyframe = await options.captureKeyframe()
          } catch {
            keyframe = null
          }
        }
        const request: ProgressSaveRequest = {
          mediaItemId: session.mediaItemId,
          locator: { version: 1, kind: "video", data: { positionMs: Math.max(0, Math.round(position * 1000)) } },
          completion,
          expectedRevision: revision,
          ...(keyframe ? { keyframe } : {}),
        } as ProgressSaveRequest
        try {
          const result = await save(request)
          revision = result.revision
        } catch (error: unknown) {
          // Ordinary failures retain the latest position for a later flush but
          // must not spin a tight retry loop; conflicts stop saving entirely.
          if (!conflict(error) && !stopped && !closed) latest = latest ?? position
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
  const update = (positionSeconds: number, completion: ProgressSaveRequest["completion"] = "in_progress") => {
    if (stopped || closed || !Number.isFinite(positionSeconds) || positionSeconds < 0) return
    latest = positionSeconds; latestCompletion = completed ? "completed" : completion
    if (!drainPromise && !timer) timer = setTimeout(() => { timer = undefined; void drain() }, throttleMs)
  }
  return {
    identity,
    timeupdate: (position) => update(position),
    pause: (position) => { if (position !== undefined) update(position); return flush() },
    seeked: (position) => { update(position); return flush() },
    ended: (position) => { completed = true; update(position, "completed"); latestCompletion = "completed"; return flush() },
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

export function restoreVideoProgress(video: Pick<HTMLVideoElement, "currentTime" | "duration">, session: SessionOpenResultDto, restored?: { current: string | null }, initial?: InitialPosition): boolean {
  const { locator, reset, identity, markRestored } = resolveRestorePosition(session, initial)
  if (locator?.kind !== "video") return false
  if (restored?.current === identity) return false
  if (reset) {
    // Progress reset intentionally preserves the last locator for audit and
    // conflict semantics, but consumers must resume from the beginning.
    video.currentTime = 0
    if (restored) restored.current = identity
    markRestored()
    return true
  }
  const positionMs = locator.data.positionMs
  if (!Number.isFinite(positionMs) || positionMs < 0) return false
  const duration = video.duration
  video.currentTime = Number.isFinite(duration) && duration >= 0 ? Math.min(positionMs / 1000, duration) : positionMs / 1000
  if (restored) restored.current = identity
  markRestored()
  return true
}
