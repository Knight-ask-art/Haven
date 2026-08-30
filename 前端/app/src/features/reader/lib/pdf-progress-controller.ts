import type { ProgressSaveRequest, ProgressSaveResult, SessionOpenResultDto, TextAnchorDto } from "@/lib/ipc/generated/wire"
import { HavenError } from "@/lib/ipc/errors"
import { saveProgress } from "@/features/progress/ipc/progress-gateway"
import { clampPdfZoom } from "./pdf-reader-state"

export interface PdfProgressLocator {
  /** Zero-based PDF page index. */
  pageIndex: number
  pageCount: number
  zoom: number
  /** Optional content-identity anchor extracted from the rendered text layer. */
  textAnchor?: TextAnchorDto | null
}

const ANCHOR_EXACT_MAX = 240

/** Keep only well-formed bounded strings; malformed anchors degrade to null. */
export function sanitizeTextAnchor(anchor: TextAnchorDto | null | undefined): TextAnchorDto | null {
  if (!anchor || typeof anchor !== "object") return null
  const clean = (value: unknown, max: number): string | null => {
    if (typeof value !== "string") return null
    const trimmed = value.trim()
    return trimmed === "" ? null : trimmed.slice(0, max)
  }
  const exact = clean(anchor.exact, ANCHOR_EXACT_MAX)
  if (!exact) return null
  return {
    exact,
    prefix: clean(anchor.prefix, 120),
    suffix: clean(anchor.suffix, 120),
  }
}

export interface PdfProgressControllerOptions {
  session: SessionOpenResultDto
  save?: (request: ProgressSaveRequest) => Promise<ProgressSaveResult>
  onRevisionConflict?: () => void
  retry?: () => void
  throttleMs?: number
}

export interface PdfProgressController {
  readonly identity: string
  locatorChange(locator: PdfProgressLocator): void
  flush(): Promise<void>
  cleanup(): Promise<void>
}

export function createPdfProgressController(options: PdfProgressControllerOptions): PdfProgressController {
  const { session } = options
  const save = options.save ?? ((request) => saveProgress(request))
  const throttleMs = options.throttleMs ?? 5000
  const identity = `${session.sessionId}:${session.mediaItemId}:${session.contentUri}`
  let revision = session.progress?.revision ?? null
  let completed = session.progress?.completion === "completed"
  let latest: PdfProgressLocator | undefined
  let timer: ReturnType<typeof setTimeout> | undefined
  let drainPromise: Promise<boolean> | undefined
  let stopped = false
  let conflictNotified = false

  const clearTimer = () => {
    if (timer) clearTimeout(timer)
    timer = undefined
  }

  const handleConflict = (error: unknown) => {
    if (!(error instanceof HavenError) || error.code !== "REVISION_CONFLICT") return false
    stopped = true
    latest = undefined
    clearTimer()
    if (!conflictNotified) {
      conflictNotified = true
      options.onRevisionConflict?.()
      options.retry?.()
    }
    return true
  }

  const drain = (): Promise<boolean> => {
    if (drainPromise) return drainPromise
    const work = (async (): Promise<boolean> => {
      while (!stopped && latest !== undefined) {
        const entry = latest
        latest = undefined
        const request: ProgressSaveRequest = {
          mediaItemId: session.mediaItemId,
          locator: {
            version: 1,
            kind: "pdf",
            data: {
              pageIndex: entry.pageIndex,
              x: null,
              y: null,
              zoom: entry.zoom,
              textAnchor: sanitizeTextAnchor(entry.textAnchor),
            },
          },
          completion: completed ? "completed" : "in_progress",
          expectedRevision: revision,
        }
        try {
          const result = await save(request)
          revision = result.revision
        } catch (error: unknown) {
          if (!handleConflict(error) && !stopped) latest = latest ?? entry
          return true
        }
      }
      return false
    })()
    drainPromise = work.finally(() => {
      drainPromise = undefined
    })
    return drainPromise
  }

  const flush = async () => {
    clearTimer()
    let failed = false
    do {
      if (!drainPromise && latest === undefined) return
      failed = await drain()
      clearTimer()
    } while (!stopped && !failed && latest !== undefined)
  }

  const locatorChange = (locator: PdfProgressLocator) => {
    if (stopped
      || !Number.isInteger(locator.pageCount)
      || locator.pageCount <= 0
      || !Number.isInteger(locator.pageIndex)
      || locator.pageIndex < 0
      || locator.pageIndex >= locator.pageCount) {
      return
    }
    if (locator.pageIndex === locator.pageCount - 1) completed = true
    latest = { ...locator, zoom: clampPdfZoom(locator.zoom) }
    if (!drainPromise && !timer) {
      timer = setTimeout(() => {
        timer = undefined
        void drain()
      }, throttleMs)
    }
  }

  return {
    identity,
    locatorChange,
    flush,
    cleanup: async () => { await flush() },
  }
}

/** Restores a zero-based PDF page and zoom once for each Reader Session. */
export function restorePdfProgress(
  pageCount: number,
  session: SessionOpenResultDto,
  restored?: { current: string | null },
): { pageIndex: number; zoom: number } | null {
  if (!Number.isInteger(pageCount) || pageCount <= 0) return null
  if (!session.progress || session.progress.locator.kind !== "pdf") return null
  const identity = `${session.sessionId}:${session.mediaItemId}:${session.contentUri}`
  if (restored?.current === identity) return null
  const data = session.progress.locator.data
  if (!Number.isInteger(data.pageIndex) || data.pageIndex < 0) return null
  const zoom = typeof data.zoom === "number" ? clampPdfZoom(data.zoom) : 1
  const result = {
    pageIndex: Math.min(pageCount - 1, data.pageIndex),
    zoom,
  }
  if (restored) restored.current = identity
  return result
}
