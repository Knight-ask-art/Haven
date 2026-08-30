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
  let latestCompletion: ProgressSaveRequest["completion"] = "in_progress"
  let timer: ReturnType<typeof setTimeout> | undefined
  let inFlight: Promise<void> | undefined
  let queued = false
  let stopped = false
  let completed = false
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
    inFlight = save(request).then((result) => { revision = result.revision }).catch((error) => {
      conflict(error)
      if (!(error instanceof HavenError && error.code === "REVISION_CONFLICT") && !stopped) {
        latest = latest ?? position
      }
    }).finally(() => { inFlight = undefined })
    await inFlight
    if (!stopped && queued) { queued = false; await run() }
  }
  const flush = async () => { clear(); if (latest !== undefined) { if (inFlight) queued = true; else await run() } if (inFlight) await inFlight; if (queued && !stopped) { queued = false; await run() } }
  const update = (positionSeconds: number, completion: ProgressSaveRequest["completion"] = "in_progress") => {
    if (stopped || !Number.isFinite(positionSeconds) || positionSeconds < 0) return
    latest = positionSeconds; latestCompletion = completed ? "completed" : completion
    if (inFlight) queued = true
    else if (!timer) timer = setTimeout(() => { timer = undefined; void run() }, throttleMs)
  }
  return {
    identity,
    timeupdate: (position) => update(position),
    pause: (position) => { if (position !== undefined) update(position); return flush() },
    seeked: (position) => { update(position); return flush() },
    ended: (position) => { completed = true; update(position, "completed"); latestCompletion = "completed"; return flush() },
    flush,
    cleanup: async () => { await flush() },
  }
}

export function restoreVideoProgress(video: Pick<HTMLVideoElement, "currentTime" | "duration">, session: SessionOpenResultDto, restored?: { current: string | null }): boolean {
  if (session.progress?.locator.kind !== "video") return false
  const identity = `${session.sessionId}:${session.mediaItemId}:${session.contentUri}`
  if (restored?.current === identity) return false
  const positionMs = session.progress.locator.data.positionMs
  if (!Number.isFinite(positionMs) || positionMs < 0) return false
  const duration = video.duration
  video.currentTime = Number.isFinite(duration) && duration >= 0 ? Math.min(positionMs / 1000, duration) : positionMs / 1000
  if (restored) restored.current = identity
  return true
}
