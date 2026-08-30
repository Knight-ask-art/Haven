import { describe, expect, it, vi } from "vitest"
import type { ProgressSaveRequest, ProgressSaveResult, SessionOpenResultDto } from "@/lib/ipc/generated/wire"
import { createComicProgressController, restoreComicProgress } from "./comic-progress-controller"

function fakeSession(mediaItemId = "0196f0d2-0000-7000-8000-000000000001"): SessionOpenResultDto {
  return {
    schemaVersion: 1,
    sessionId: "0196f0d2-0000-7000-8000-000000000002",
    contentUri: "haven-resource://session/0196f0d2-0000-7000-8000-000000000002",
    workId: "work-1",
    editionId: "edition-1",
    mediaItemId,
    engine: "comic",
    progress: null,
  }
}

describe("comic-progress-controller", () => {
  it("throttles page changes and flushes on cleanup", async () => {
    const saved: ProgressSaveRequest[] = []
    const save = vi.fn(async (req: ProgressSaveRequest): Promise<ProgressSaveResult> => {
      saved.push(req)
      return { revision: "rev-1" }
    })
    const controller = createComicProgressController({ session: fakeSession(), totalPages: 45, save, throttleMs: 100 })
    controller.pageChange(5)
    controller.pageChange(10)
    await controller.cleanup()
    expect(save).toHaveBeenCalledTimes(1)
    expect(saved[0].locator.kind).toBe("comic")
    const data = saved[0].locator.data as { chapterItemId: string; pageIndex: number; pageProgression: number | null }
    expect(data.pageIndex).toBe(9) // 0-based
    expect(data.chapterItemId).toBe("0196f0d2-0000-7000-8000-000000000001")
    expect(saved[0].completion).toBe("in_progress")
  })

  it("latches completed on the final page and inherits completed sessions", async () => {
    const saved: ProgressSaveRequest[] = []
    const save = vi.fn(async (request: ProgressSaveRequest): Promise<ProgressSaveResult> => {
      saved.push(request)
      return { revision: `rev-${saved.length}` }
    })
    const controller = createComicProgressController({ session: fakeSession(), totalPages: 45, save })

    controller.pageChange(45)
    await controller.flush()
    controller.pageChange(10)
    await controller.flush()

    const completedSession = fakeSession()
    completedSession.progress = {
      mediaItemId: completedSession.mediaItemId,
      completion: "completed",
      progressRatio: 1,
      revision: "progress-rev-1",
      updatedAt: "2026-08-18T00:00:00.000Z",
      locator: { version: 1, kind: "comic", data: { chapterItemId: completedSession.mediaItemId, pageIndex: 44, pageProgression: null } },
    }
    const inherited = createComicProgressController({ session: completedSession, totalPages: 45, save })
    inherited.pageChange(2)
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
    const controller = createComicProgressController({ session: fakeSession(), totalPages: 45, save, onRevisionConflict: onConflict, retry })
    controller.pageChange(3)
    await controller.flush()
    expect(onConflict).toHaveBeenCalledTimes(1)
    expect(retry).toHaveBeenCalledTimes(1)
    controller.pageChange(4)
    await controller.flush()
    expect(save).toHaveBeenCalledTimes(1)
  })

  it("restoreComicProgress rejects non-comic locators", () => {
    const session = fakeSession()
    session.progress = {
      mediaItemId: session.mediaItemId,
      completion: "in_progress",
      progressRatio: null,
      revision: "progress-rev-2",
      updatedAt: "2026-08-18T00:00:00.000Z",
      locator: { version: 1, kind: "video", data: { positionMs: 1000 } } as never,
    }
    expect(restoreComicProgress(45, session)).toBeNull()
  })

  it("restoreComicProgress returns 1-based pageIndex", () => {
    const session = fakeSession()
    session.progress = {
      mediaItemId: session.mediaItemId,
      completion: "in_progress",
      progressRatio: 0.2,
      revision: "progress-rev-3",
      updatedAt: "2026-08-18T00:00:00.000Z",
      locator: { version: 1, kind: "comic", data: { chapterItemId: session.mediaItemId, pageIndex: 8, pageProgression: null } } as never,
    }
    const restored = { current: null as string | null }
    const result = restoreComicProgress(45, session, restored)
    expect(result).not.toBeNull()
    expect(result!.pageIndex).toBe(9) // 0-based 8 → 1-based 9
    expect(restored.current).toBe(`${session.sessionId}:${session.mediaItemId}:${session.contentUri}`)
  })

  it("restoreComicProgress rejects out-of-range pageIndex", () => {
    const session = fakeSession()
    session.progress = {
      mediaItemId: session.mediaItemId,
      completion: "in_progress",
      progressRatio: null,
      revision: "progress-rev-4",
      updatedAt: "2026-08-18T00:00:00.000Z",
      locator: { version: 1, kind: "comic", data: { chapterItemId: session.mediaItemId, pageIndex: 100, pageProgression: null } } as never,
    }
    expect(restoreComicProgress(45, session)).toBeNull()
  })
})
