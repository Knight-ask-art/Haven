import { describe, expect, it, vi } from "vitest"
import type {
  CompletionWire,
  LocatorDto,
  ProgressSaveRequest,
  ProgressSaveResult,
  SessionOpenResultDto,
} from "@/lib/ipc/generated/wire"
import { HavenError } from "@/lib/ipc/errors"
import { MAX_PDF_ZOOM, MIN_PDF_ZOOM } from "./pdf-reader-state"
import { createPdfProgressController, restorePdfProgress } from "./pdf-progress-controller"

const MEDIA_ITEM_ID = "0196f0d2-0000-7000-8000-000000000001"
const SESSION_ID = "0196f0d2-0000-7000-8000-000000000002"

function fakeSession(sessionId = SESSION_ID): SessionOpenResultDto {
  return {
    schemaVersion: 1,
    sessionId,
    contentUri: `haven-resource://session/${sessionId}`,
    workId: "work-1",
    editionId: "edition-1",
    mediaItemId: MEDIA_ITEM_ID,
    engine: "reader",
    progress: null,
  }
}

function withProgress(
  locator: LocatorDto,
  options: { sessionId?: string; completion?: CompletionWire } = {},
): SessionOpenResultDto {
  const session = fakeSession(options.sessionId)
  session.progress = {
    mediaItemId: session.mediaItemId,
    completion: options.completion ?? "in_progress",
    progressRatio: null,
    revision: "progress-rev-1",
    updatedAt: "2026-08-18T00:00:00.000Z",
    locator,
  }
  return session
}

function pdfLocator(pageIndex: number, zoom: number | null): LocatorDto {
  return {
    version: 1,
    kind: "pdf",
    data: { pageIndex, x: null, y: null, zoom, textAnchor: null },
  }
}

describe("pdf-progress-controller", () => {
  it("saves the latest zero-based page and clamped zoom on cleanup", async () => {
    const saved: ProgressSaveRequest[] = []
    const save = vi.fn(async (request: ProgressSaveRequest): Promise<ProgressSaveResult> => {
      saved.push(request)
      return { revision: "rev-1" }
    })
    const controller = createPdfProgressController({ session: fakeSession(), save, throttleMs: 100 })

    controller.locatorChange({ pageIndex: 2, pageCount: 10, zoom: 1.25 })
    controller.locatorChange({ pageIndex: 4, pageCount: 10, zoom: 99 })
    await controller.cleanup()

    expect(save).toHaveBeenCalledTimes(1)
    expect(saved[0]).toEqual({
      mediaItemId: MEDIA_ITEM_ID,
      locator: {
        version: 1,
        kind: "pdf",
        data: { pageIndex: 4, x: null, y: null, zoom: MAX_PDF_ZOOM, textAnchor: null },
      },
      completion: "in_progress",
      expectedRevision: null,
    })
  })

  it("latches completion after reaching the final page", async () => {
    const saved: ProgressSaveRequest[] = []
    const save = vi.fn(async (request: ProgressSaveRequest): Promise<ProgressSaveResult> => {
      saved.push(request)
      return { revision: `rev-${saved.length}` }
    })
    const controller = createPdfProgressController({ session: fakeSession(), save })

    controller.locatorChange({ pageIndex: 4, pageCount: 5, zoom: 1 })
    await controller.flush()
    controller.locatorChange({ pageIndex: 1, pageCount: 5, zoom: 1 })
    await controller.flush()

    expect(saved.map((request) => request.completion)).toEqual(["completed", "completed"])
    expect(saved[1].expectedRevision).toBe("rev-1")
  })

  it("inherits a completed Session when the reader resumes on an earlier page", async () => {
    const saved: ProgressSaveRequest[] = []
    const save = vi.fn(async (request: ProgressSaveRequest): Promise<ProgressSaveResult> => {
      saved.push(request)
      return { revision: "rev-next" }
    })
    const session = withProgress(pdfLocator(9, 1), { completion: "completed" })
    const controller = createPdfProgressController({ session, save })

    controller.locatorChange({ pageIndex: 1, pageCount: 10, zoom: 0.75 })
    await controller.flush()

    expect(saved[0].completion).toBe("completed")
    expect(saved[0].expectedRevision).toBe(session.progress?.revision)
  })

  it("rejects invalid page boundaries and normalizes non-finite zoom", async () => {
    const saved: ProgressSaveRequest[] = []
    const save = vi.fn(async (request: ProgressSaveRequest): Promise<ProgressSaveResult> => {
      saved.push(request)
      return { revision: `rev-${saved.length}` }
    })
    const controller = createPdfProgressController({ session: fakeSession(), save })

    controller.locatorChange({ pageIndex: 0, pageCount: 0, zoom: 1 })
    controller.locatorChange({ pageIndex: 0, pageCount: 1.5, zoom: 1 })
    controller.locatorChange({ pageIndex: -1, pageCount: 10, zoom: 1 })
    controller.locatorChange({ pageIndex: 1.5, pageCount: 10, zoom: 1 })
    controller.locatorChange({ pageIndex: 10, pageCount: 10, zoom: 1 })
    await controller.flush()
    expect(save).not.toHaveBeenCalled()

    controller.locatorChange({ pageIndex: 2, pageCount: 10, zoom: Number.NaN })
    await controller.flush()
    controller.locatorChange({ pageIndex: 3, pageCount: 10, zoom: Number.POSITIVE_INFINITY })
    await controller.flush()

    expect(saved.map((request) => (
      request.locator.kind === "pdf" ? request.locator.data.zoom : null
    ))).toEqual([1, 1])
  })

  it("stops after a revision conflict and invokes retry once", async () => {
    const save = vi.fn(async (): Promise<ProgressSaveResult> => {
      throw new HavenError({ code: "REVISION_CONFLICT", userMessage: "conflict", retryable: false })
    })
    const onRevisionConflict = vi.fn()
    const retry = vi.fn()
    const controller = createPdfProgressController({
      session: fakeSession(),
      save,
      onRevisionConflict,
      retry,
    })

    controller.locatorChange({ pageIndex: 2, pageCount: 10, zoom: 1 })
    await controller.flush()
    controller.locatorChange({ pageIndex: 3, pageCount: 10, zoom: 1 })
    await controller.flush()

    expect(save).toHaveBeenCalledTimes(1)
    expect(onRevisionConflict).toHaveBeenCalledTimes(1)
    expect(retry).toHaveBeenCalledTimes(1)
  })

  it("waits for in-flight work and saves only the latest queued locator", async () => {
    const saved: ProgressSaveRequest[] = []
    const completions: Array<(result: ProgressSaveResult) => void> = []
    const save = vi.fn((request: ProgressSaveRequest) => new Promise<ProgressSaveResult>((resolve) => {
      saved.push(request)
      completions.push(resolve)
    }))
    const controller = createPdfProgressController({ session: fakeSession(), save })

    controller.locatorChange({ pageIndex: 0, pageCount: 10, zoom: 1 })
    const firstFlush = controller.flush()
    await vi.waitFor(() => expect(save).toHaveBeenCalledTimes(1))
    controller.locatorChange({ pageIndex: 1, pageCount: 10, zoom: 1.1 })
    controller.locatorChange({ pageIndex: 2, pageCount: 10, zoom: 1.2 })
    let cleanupDone = false
    const cleanup = controller.cleanup().then(() => { cleanupDone = true })

    completions[0]({ revision: "rev-1" })
    await vi.waitFor(() => expect(save).toHaveBeenCalledTimes(2))
    expect(cleanupDone).toBe(false)
    completions[1]({ revision: "rev-2" })
    await Promise.all([firstFlush, cleanup])

    expect(saved.map((request) => request.locator.kind === "pdf" && request.locator.data.pageIndex)).toEqual([0, 2])
    expect(saved[1].expectedRevision).toBe("rev-1")
  })
})

describe("restorePdfProgress", () => {
  it("rejects absent progress and non-PDF locators", () => {
    expect(restorePdfProgress(10, fakeSession())).toBeNull()
    const video = withProgress({ version: 1, kind: "video", data: { positionMs: 1000 } })
    expect(restorePdfProgress(10, video)).toBeNull()
  })

  it("keeps zero-based pages and clamps overflow to the real document boundary", () => {
    expect(restorePdfProgress(10, withProgress(pdfLocator(4, 1.25)))).toEqual({ pageIndex: 4, zoom: 1.25 })
    expect(restorePdfProgress(10, withProgress(pdfLocator(99, 1.25)))).toEqual({ pageIndex: 9, zoom: 1.25 })
  })

  it("rejects invalid page counts and invalid page indices", () => {
    const valid = withProgress(pdfLocator(1, 1))
    expect(restorePdfProgress(0, valid)).toBeNull()
    expect(restorePdfProgress(1.5, valid)).toBeNull()
    expect(restorePdfProgress(10, withProgress(pdfLocator(-1, 1)))).toBeNull()
    expect(restorePdfProgress(10, withProgress(pdfLocator(1.5, 1)))).toBeNull()
  })

  it("defaults null and non-finite zoom and clamps supported boundaries", () => {
    expect(restorePdfProgress(10, withProgress(pdfLocator(1, null)))?.zoom).toBe(1)
    expect(restorePdfProgress(10, withProgress(pdfLocator(1, Number.NaN)))?.zoom).toBe(1)
    expect(restorePdfProgress(10, withProgress(pdfLocator(1, Number.POSITIVE_INFINITY)))?.zoom).toBe(1)
    expect(restorePdfProgress(10, withProgress(pdfLocator(1, 0)))?.zoom).toBe(MIN_PDF_ZOOM)
    expect(restorePdfProgress(10, withProgress(pdfLocator(1, 99)))?.zoom).toBe(MAX_PDF_ZOOM)
  })

  it("restores once per Session identity and permits a replacement Session", () => {
    const restored = { current: null as string | null }
    const first = withProgress(pdfLocator(2, 1.1))
    expect(restorePdfProgress(10, first, restored)).toEqual({ pageIndex: 2, zoom: 1.1 })
    expect(restorePdfProgress(10, first, restored)).toBeNull()

    const replacement = withProgress(pdfLocator(7, 1.5), {
      sessionId: "0196f0d2-0000-7000-8000-000000000003",
    })
    expect(restorePdfProgress(10, replacement, restored)).toEqual({ pageIndex: 7, zoom: 1.5 })
  })
})


describe("pdf text anchor sanitization", () => {
  it("degrades malformed anchors to null and bounds exact", async () => {
    const { sanitizeTextAnchor } = await import("./pdf-progress-controller")
    expect(sanitizeTextAnchor(null)).toBeNull()
    expect(sanitizeTextAnchor({ exact: "   ", prefix: null, suffix: null })).toBeNull()
    const long = "x".repeat(500)
    const sanitized = sanitizeTextAnchor({ exact: long, prefix: "  p  ", suffix: 42 as unknown as string })
    expect(sanitized?.exact?.length).toBe(240)
    expect(sanitized?.prefix).toBe("p")
    expect(sanitized?.suffix).toBeNull()
  })

  it("persists sanitized anchors through the save path", async () => {
    const saved: ProgressSaveRequest[] = []
    const controller = createPdfProgressController({
      session: fakeSession(),
      save: async (request) => {
        saved.push(request)
        return { revision: "rev-anchor-1" }
      },
      throttleMs: 1,
    })
    const long = "y".repeat(500)
    controller.locatorChange({ pageIndex: 0, pageCount: 3, zoom: 1, textAnchor: { exact: long, prefix: null, suffix: null } })
    await controller.flush()
    const locator = saved[0]?.locator
    if (locator.kind !== "pdf") throw new Error("expected pdf locator")
    expect(locator.data.textAnchor?.exact?.length).toBe(240)
    await controller.cleanup()
  })
})
