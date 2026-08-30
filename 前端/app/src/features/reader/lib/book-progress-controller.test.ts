import { describe, expect, it, vi } from "vitest"
import type { ProgressSaveRequest, ProgressSaveResult, SessionOpenResultDto } from "@/lib/ipc/generated/wire"
import { createBookProgressController, restoreBookProgress } from "./book-progress-controller"

function fakeSession(mediaItemId = "0196f0d2-0000-7000-8000-000000000001"): SessionOpenResultDto {
  return {
    schemaVersion: 1,
    sessionId: "0196f0d2-0000-7000-8000-000000000002",
    contentUri: "haven-resource://session/0196f0d2-0000-7000-8000-000000000002",
    workId: "work-1",
    editionId: "edition-1",
    mediaItemId,
    engine: "reader",
    progress: null,
  }
}

describe("book-progress-controller", () => {
  it("throttles scroll updates and flushes on cleanup", async () => {
    const saved: ProgressSaveRequest[] = []
    const save = vi.fn(async (req: ProgressSaveRequest): Promise<ProgressSaveResult> => {
      saved.push(req)
      return { revision: "rev-1" }
    })
    const controller = createBookProgressController({ session: fakeSession(), save, throttleMs: 100 })
    controller.scroll(0.1)
    controller.scroll(0.2)
    await controller.cleanup()
    expect(save).toHaveBeenCalledTimes(1)
    expect(saved[0].locator.kind).toBe("book")
    expect((saved[0].locator.data as { progression: number }).progression).toBe(0.2)
    expect((saved[0].locator.data as { publicationResource: string }).publicationResource).toContain(fakeSession().mediaItemId)
    expect(saved[0].completion).toBe("in_progress")
  })

  it("latches completed near the end and inherits completed sessions", async () => {
    const saved: ProgressSaveRequest[] = []
    const save = vi.fn(async (request: ProgressSaveRequest): Promise<ProgressSaveResult> => {
      saved.push(request)
      return { revision: `rev-${saved.length}` }
    })
    const controller = createBookProgressController({ session: fakeSession(), save })

    controller.scroll(0.99)
    await controller.flush()
    controller.scroll(0.4)
    await controller.flush()

    const completedSession = fakeSession()
    completedSession.progress = {
      mediaItemId: completedSession.mediaItemId,
      completion: "completed",
      progressRatio: 1,
      revision: "progress-rev-1",
      updatedAt: "2026-08-18T00:00:00.000Z",
      locator: { version: 1, kind: "book", data: { publicationResource: "res", progression: 1, textAnchor: null, formatLocator: null } },
    }
    const inherited = createBookProgressController({ session: completedSession, save })
    inherited.scroll(0.2)
    await inherited.flush()

    expect(saved.map((request) => request.completion)).toEqual(["completed", "completed", "completed"])
  })

  it("stops on REVISION_CONFLICT and notifies retry", async () => {
    const { HavenError } = await import("@/lib/ipc/errors")
    const save = vi.fn(async (): Promise<ProgressSaveResult> => {
      throw new HavenError({ code: "REVISION_CONFLICT", userMessage: "conflict", retryable: false })
    })
    const onConflict = vi.fn()
    const retry = vi.fn()
    const controller = createBookProgressController({ session: fakeSession(), save, onRevisionConflict: onConflict, retry })
    controller.scroll(0.5)
    await controller.flush()
    expect(onConflict).toHaveBeenCalledTimes(1)
    expect(retry).toHaveBeenCalledTimes(1)
    controller.scroll(0.6)
    await controller.flush()
    expect(save).toHaveBeenCalledTimes(1)
  })

  it("uses the opaque progress revision instead of the display timestamp", async () => {
    const session = fakeSession()
    session.progress = {
      mediaItemId: session.mediaItemId,
      completion: "in_progress",
      progressRatio: 0.2,
      revision: "opaque-progress-revision",
      updatedAt: "2026-08-18T00:00:00.000Z",
      locator: { version: 1, kind: "book", data: { publicationResource: "res", progression: 0.2, textAnchor: null, formatLocator: null } },
    }
    const save = vi.fn(async (): Promise<ProgressSaveResult> => ({ revision: "next-revision" }))
    const controller = createBookProgressController({ session, save })
    controller.scroll(0.3)
    await controller.flush()
    expect(save).toHaveBeenCalledWith(expect.objectContaining({ expectedRevision: "opaque-progress-revision" }))
  })

  it("restoreBookProgress rejects non-book locators", () => {
    const session = fakeSession()
    session.progress = {
      mediaItemId: session.mediaItemId,
      completion: "in_progress",
      progressRatio: null,
      revision: "progress-rev-2",
      updatedAt: "2026-08-18T00:00:00.000Z",
      locator: { version: 1, kind: "video", data: { positionMs: 1000 } } as never,
    }
    const container = { scrollTop: 0, scrollHeight: 1000, clientHeight: 500 }
    expect(restoreBookProgress(container, session)).toBe(false)
  })

  it("restoreBookProgress sets scrollTop from progression", () => {
    const session = fakeSession()
    session.progress = {
      mediaItemId: session.mediaItemId,
      completion: "in_progress",
      progressRatio: 0.5,
      revision: "progress-rev-3",
      updatedAt: "2026-08-18T00:00:00.000Z",
      locator: { version: 1, kind: "book", data: { publicationResource: "res", progression: 0.5, textAnchor: null, formatLocator: null } } as never,
    }
    const container = { scrollTop: 0, scrollHeight: 1000, clientHeight: 500 }
    const restored = { current: null as string | null }
    expect(restoreBookProgress(container, session, restored)).toBe(true)
    expect(container.scrollTop).toBe(250)
    expect(restored.current).toBe(`${session.sessionId}:${session.mediaItemId}:${session.contentUri}`)
  })

  it("restores paginated progress on the horizontal page axis", () => {
    const session = fakeSession()
    session.progress = {
      mediaItemId: session.mediaItemId,
      completion: "in_progress",
      progressRatio: 0.5,
      revision: "progress-rev-4",
      updatedAt: "2026-08-18T00:00:00.000Z",
      locator: { version: 1, kind: "book", data: { publicationResource: "res", progression: 0.5, textAnchor: null, formatLocator: null } } as never,
    }
    const container = { scrollTop: 0, scrollLeft: 0, scrollHeight: 600, scrollWidth: 2400, clientHeight: 600, clientWidth: 800 }
    const restored = { current: null as string | null }
    expect(restoreBookProgress(container, session, restored, "paginated")).toBe(true)
    expect(container.scrollTop).toBe(0)
    expect(container.scrollLeft).toBe(800)
    expect(restored.current).toBe(`${session.sessionId}:${session.mediaItemId}:${session.contentUri}`)
  })
})
