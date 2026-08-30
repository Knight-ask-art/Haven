import { describe, expect, it, vi } from "vitest"
import type { ProgressSaveRequest, ProgressSaveResult, SessionOpenResultDto } from "@/lib/ipc/generated/wire"
import { createArticleProgressController, restoreArticleProgress } from "./article-progress-controller"

function fakeSession(mediaItemId = "0196f0d2-0000-7000-8000-000000000001"): SessionOpenResultDto {
  return {
    schemaVersion: 1,
    sessionId: "0196f0d2-0000-7000-8000-000000000002",
    contentUri: "haven-resource://session/0196f0d2-0000-7000-8000-000000000002",
    workId: "work-1",
    editionId: "edition-1",
    mediaItemId,
    engine: "article",
    progress: null,
  }
}

describe("article-progress-controller", () => {
  it("throttles scroll updates and flushes on cleanup", async () => {
    const saved: ProgressSaveRequest[] = []
    const save = vi.fn(async (req: ProgressSaveRequest): Promise<ProgressSaveResult> => {
      saved.push(req)
      return { revision: "rev-1" }
    })
    const controller = createArticleProgressController({ session: fakeSession(), save, throttleMs: 100 })
    controller.scroll(0.1, "h1-intro")
    controller.scroll(0.3, "h2-agentic")
    await controller.cleanup()
    expect(save).toHaveBeenCalledTimes(1)
    expect(saved[0].locator.kind).toBe("article")
    const data = saved[0].locator.data as { progression: number; blockId: string | null }
    expect(data.progression).toBe(0.3)
    expect(data.blockId).toBe("h2-agentic")
    expect(saved[0].completion).toBe("in_progress")
  })

  it("latches completed near the end and inherits completed sessions", async () => {
    const saved: ProgressSaveRequest[] = []
    const save = vi.fn(async (request: ProgressSaveRequest): Promise<ProgressSaveResult> => {
      saved.push(request)
      return { revision: `rev-${saved.length}` }
    })
    const controller = createArticleProgressController({ session: fakeSession(), save })

    controller.scroll(0.99, "last-block")
    await controller.flush()
    controller.scroll(0.4, "earlier-block")
    await controller.flush()

    const completedSession = fakeSession()
    completedSession.progress = {
      mediaItemId: completedSession.mediaItemId,
      completion: "completed",
      progressRatio: 1,
      revision: "progress-rev-1",
      updatedAt: "2026-08-18T00:00:00.000Z",
      locator: { version: 1, kind: "article", data: { blockId: "last-block", progression: 1, textAnchor: null } },
    }
    const inherited = createArticleProgressController({ session: completedSession, save })
    inherited.scroll(0.2, "intro")
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
    const controller = createArticleProgressController({ session: fakeSession(), save, onRevisionConflict: onConflict, retry })
    controller.scroll(0.5)
    await controller.flush()
    expect(onConflict).toHaveBeenCalledTimes(1)
    expect(retry).toHaveBeenCalledTimes(1)
    controller.scroll(0.6)
    await controller.flush()
    expect(save).toHaveBeenCalledTimes(1)
  })

  it("restoreArticleProgress rejects non-article locators", () => {
    const session = fakeSession()
    session.progress = {
      mediaItemId: session.mediaItemId,
      completion: "in_progress",
      progressRatio: null,
      revision: "progress-rev-2",
      updatedAt: "2026-08-18T00:00:00.000Z",
      locator: { version: 1, kind: "video", data: { positionMs: 1000 } } as never,
    }
    expect(restoreArticleProgress(session)).toBeNull()
  })

  it("restoreArticleProgress returns progression and blockId", () => {
    const session = fakeSession()
    session.progress = {
      mediaItemId: session.mediaItemId,
      completion: "in_progress",
      progressRatio: 0.4,
      revision: "progress-rev-3",
      updatedAt: "2026-08-18T00:00:00.000Z",
      locator: { version: 1, kind: "article", data: { blockId: "h2-agentic", progression: 0.4, textAnchor: null } } as never,
    }
    const restored = { current: null as string | null }
    const result = restoreArticleProgress(session, restored)
    expect(result).not.toBeNull()
    expect(result!.progression).toBe(0.4)
    expect(result!.blockId).toBe("h2-agentic")
    expect(restored.current).toBe(`${session.sessionId}:${session.mediaItemId}:${session.contentUri}`)
  })
})
