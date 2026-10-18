import { describe, expect, it, vi } from "vitest"
import type { ProgressSaveResult, SessionOpenResultDto } from "@/lib/ipc/generated/wire"
import { createBookProgressController } from "@/features/reader/lib/book-progress-controller"
import { createArticleProgressController } from "@/features/reader/lib/article-progress-controller"
import { createComicProgressController } from "@/features/comic/lib/comic-progress-controller"
import { createVideoProgressController } from "@/features/player/lib/video-progress-controller"

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => { resolve = done })
  return { promise, resolve }
}

const session: SessionOpenResultDto = {
  schemaVersion: 1, sessionId: "session", contentUri: "resource", workId: "work",
  editionId: "edition", mediaItemId: "media", engine: "reader", progress: null,
}

describe("progress release barriers", () => {
  it.each(["book", "article", "comic", "video"] as const)("%s waits for the final queued write before releasing", async (kind) => {
    const first = deferred<ProgressSaveResult>()
    const second = deferred<ProgressSaveResult>()
    const save = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise)
    const options = { session, save }
    const controller = kind === "book" ? createBookProgressController(options)
      : kind === "article" ? createArticleProgressController(options)
      : kind === "comic" ? createComicProgressController({ ...options, totalPages: 10 })
      : createVideoProgressController(options)
    const update = (value: number) => {
      if ("scroll" in controller) controller.scroll(value / 10)
      else if ("pageChange" in controller) controller.pageChange(value)
      else controller.timeupdate(value)
    }
    update(1)
    const initialFlush = controller.flush()
    update(2)
    let released = false
    const cleanup = controller.cleanup().then(() => { released = true })
    first.resolve({ revision: "r1" } as ProgressSaveResult)
    await vi.waitFor(() => expect(save).toHaveBeenCalledTimes(2))
    expect(released).toBe(false)
    expect(save.mock.calls[1][0].expectedRevision).toBe("r1")
    second.resolve({ revision: "r2" } as ProgressSaveResult)
    await Promise.all([initialFlush, cleanup])
    expect(released).toBe(true)
    update(3)
    await controller.flush()
    expect(save).toHaveBeenCalledTimes(2)
  })
})
