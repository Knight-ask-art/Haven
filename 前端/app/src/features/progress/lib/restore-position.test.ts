import { describe, expect, it } from "vitest"
import type { LocatorDto, SessionOpenResultDto } from "@/lib/ipc/generated/wire"
import { restoreBookProgress } from "@/features/reader/lib/book-progress-controller"
import { restoreArticleProgress } from "@/features/reader/lib/article-progress-controller"
import { restorePdfProgress } from "@/features/reader/lib/pdf-progress-controller"
import { restoreComicProgress } from "@/features/comic/lib/comic-progress-controller"
import { restoreVideoProgress } from "@/features/player/lib/video-progress-controller"
import { resolveRestorePosition, type InitialPosition } from "./restore-position"

const mediaItemId = "media-1"
function session(id: string, locator: LocatorDto): SessionOpenResultDto {
  return {
    schemaVersion: 1, sessionId: id, contentUri: `content:${id}`,
    workId: "work-1", editionId: "edition-1", mediaItemId, engine: "reader",
    progress: { mediaItemId, locator, completion: "in_progress", progressRatio: null,
      revision: "r1", updatedAt: "2026-11-01T00:00:00Z" },
  }
}

const cases: { name: string; locator: LocatorDto; restore: (s: SessionOpenResultDto, initial: InitialPosition) => unknown }[] = [
  { name: "book", locator: { version: 1, kind: "book", data: { publicationResource: "resource", progression: 0.5, textAnchor: null, formatLocator: null } },
    restore: (s, initial) => restoreBookProgress({ scrollTop: 0, scrollHeight: 1000, clientHeight: 200 }, s, undefined, "scroll", initial) },
  { name: "article", locator: { version: 1, kind: "article", data: { blockId: "block", progression: 0.5, textAnchor: null } },
    restore: (s, initial) => restoreArticleProgress(s, undefined, initial) },
  { name: "pdf", locator: { version: 1, kind: "pdf", data: { pageIndex: 2, x: null, y: null, zoom: 1, textAnchor: null } },
    restore: (s, initial) => restorePdfProgress(10, s, undefined, initial) },
  { name: "comic", locator: { version: 1, kind: "comic", data: { chapterItemId: mediaItemId, pageIndex: 2, pageProgression: null } },
    restore: (s, initial) => restoreComicProgress(10, s, undefined, initial) },
  { name: "video", locator: { version: 1, kind: "video", data: { positionMs: 5000 } },
    restore: (s, initial) => restoreVideoProgress({ currentTime: 0, duration: 100 }, s, undefined, initial) },
]

describe("marker restoration ownership", () => {
  it.each(cases)("$name consumes a restored marker across all later session retries", ({ locator, restore }) => {
    const initial: InitialPosition = { key: "navigation-1", mediaItemId, locator }
    const first = session("first", locator)
    expect(restore(first, initial)).toBeTruthy()
    expect(initial.restoredSessionId).toBe("first")
    for (const id of ["retry-1", "retry-2"]) {
      const retry = session(id, { version: 1, kind: "video", data: { positionMs: 9000 } })
      expect(resolveRestorePosition(retry, initial).locator).toBe(retry.progress!.locator)
      resolveRestorePosition(retry, initial).markRestored()
    }
    const fresh: InitialPosition = { key: "navigation-2", mediaItemId, locator }
    expect(resolveRestorePosition(session("retry-2", locator), fresh).identity).toContain("navigation-2")
  })

  it("does not consume PDF navigation before a usable document is ready", () => {
    const locator = cases.find(({ name }) => name === "pdf")!.locator
    const initial: InitialPosition = { key: "navigation", mediaItemId, locator }
    expect(restorePdfProgress(0, session("first", locator), undefined, initial)).toBeNull()
    expect(initial.restoredSessionId).toBeUndefined()
    expect(restorePdfProgress(10, session("retry", locator), undefined, initial)).toEqual({ pageIndex: 2, zoom: 1 })
    expect(initial.restoredSessionId).toBe("retry")
  })

  it("consumes a book marker already at the target without moving the viewport", () => {
    const locator = cases[0].locator
    const initial: InitialPosition = { key: "navigation", mediaItemId, locator }
    expect(restoreBookProgress({ scrollTop: 400, scrollHeight: 1000, clientHeight: 200 }, session("first", locator), undefined, "scroll", initial)).toBe(false)
    expect(initial.restoredSessionId).toBe("first")
  })
})
