import { describe, expect, it, vi } from "vitest"
import type { SessionOpenResultDto } from "@/lib/ipc/generated/wire"
import { HavenError } from "@/lib/ipc/errors"
import { clampVideoSeek, createVideoProgressController, restoreVideoProgress } from "./video-progress-controller"

const session = (progress: SessionOpenResultDto["progress"] = null): SessionOpenResultDto => ({
  schemaVersion: 1, sessionId: "0196f0d2-0000-7000-8000-000000000001", contentUri: "x", workId: "w", editionId: "e", mediaItemId: "0196f0d2-0000-7000-8000-000000000000", engine: "playback", progress,
})

describe("video progress controller", () => {
  it("clamps seek requests at the empty, negative, and finite duration boundaries", () => {
    expect(clampVideoSeek(-5, 100)).toBe(0)
    expect(clampVideoSeek(105, 100)).toBe(100)
    expect(clampVideoSeek(5, 0)).toBe(0)
    expect(clampVideoSeek(Number.NaN, 100)).toBe(0)
    expect(clampVideoSeek(Number.POSITIVE_INFINITY, Number.NaN)).toBe(0)
    expect(clampVideoSeek(5, Number.NaN)).toBe(5)
  })

  it("throttles and saves latest position", async () => {
    vi.useFakeTimers(); const save = vi.fn().mockResolvedValue({ revision: "r1" }); const c = createVideoProgressController({ session: session(), save })
    c.timeupdate(1); c.timeupdate(2); await vi.advanceTimersByTimeAsync(5000); await c.flush(); expect(save).toHaveBeenCalledWith(expect.objectContaining({ locator: expect.objectContaining({ data: { positionMs: 2000 } }) })); vi.useRealTimers()
  })
  it("uses ended position and completed", async () => {
    const save = vi.fn().mockResolvedValue({ revision: "r1" }); const c = createVideoProgressController({ session: session(), save }); await c.ended(7.25); expect(save).toHaveBeenCalledWith(expect.objectContaining({ completion: "completed", locator: expect.objectContaining({ data: { positionMs: 7250 } }) }))
  })
  it("queues only the latest update while a save is in flight", async () => {
    let resolveFirst!: (value: { revision: string }) => void
    const save = vi.fn()
      .mockImplementationOnce(() => new Promise<{ revision: string }>((resolve) => { resolveFirst = resolve }))
      .mockResolvedValueOnce({ revision: "r2" })
    const c = createVideoProgressController({ session: session(), save }); c.timeupdate(1)
    const first = c.flush(); c.timeupdate(2); c.timeupdate(3); resolveFirst({ revision: "r1" }); await first
    expect(save).toHaveBeenCalledTimes(2)
    expect(save.mock.calls[1][0]).toEqual(expect.objectContaining({ expectedRevision: "r1", locator: expect.objectContaining({ data: { positionMs: 3000 } }) }))
  })
  it("absorbs ordinary failures and retries latest on next flush", async () => {
    const save = vi.fn().mockRejectedValueOnce(new Error("offline")).mockResolvedValue({ revision: "r2" }); const c = createVideoProgressController({ session: session(), save }); c.timeupdate(3); await c.flush(); c.timeupdate(4); await c.flush(); expect(save).toHaveBeenCalledTimes(2)
  })
  it("stops on conflict and retries once", async () => {
    const retry = vi.fn(); const save = vi.fn().mockRejectedValue(new HavenError({ code: "REVISION_CONFLICT", userMessage: "conflict", retryable: true })); const c = createVideoProgressController({ session: session(), save, retry }); c.timeupdate(1); await c.flush(); c.timeupdate(2); await c.flush(); expect(retry).toHaveBeenCalledOnce(); expect(save).toHaveBeenCalledOnce()
  })
  it("restores video once, rejects invalid and clamps duration", () => {
    const s = session({ mediaItemId: session().mediaItemId, completion: "in_progress", progressRatio: null, revision: "progress-rev-1", updatedAt: "2026-08-18T00:00:00.000Z", locator: { version: 1, kind: "video", data: { positionMs: 9000 } } }); const video = { currentTime: 0, duration: 5 }; const once = { current: null as string | null }; expect(restoreVideoProgress(video, s, once)).toBe(true); expect(video.currentTime).toBe(5); expect(restoreVideoProgress(video, s, once)).toBe(false); const other = session({ ...s.progress!, locator: { version: 1, kind: "video", data: { positionMs: 1000 } } }); other.sessionId = "0196f0d2-0000-0000-0000-000000000099"; expect(restoreVideoProgress(video, other, once)).toBe(true); expect(restoreVideoProgress(video, session({ ...s.progress!, locator: { version: 1, kind: "book", data: { publicationResource: "x", progression: null, textAnchor: null, formatLocator: null } } }), once)).toBe(false)
  })
})
