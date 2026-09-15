import { describe, expect, it, vi } from "vitest"
import type { HavenClient } from "@/lib/ipc/client"
import type { LocatorDto, ProgressMarkCompletedRequest, ProgressSaveRequest } from "@/lib/ipc/generated/wire"
import { MockHavenClient } from "@/lib/ipc/mock-client"
import { markCompletedProgress, saveProgress } from "./progress-gateway"

const request: ProgressSaveRequest = {
  mediaItemId: "0196f0d2-0000-7000-8000-000000000000",
  locator: { version: 1, kind: "video", data: { positionMs: 1200 } },
  completion: "in_progress",
  expectedRevision: null,
}

const completedRequest: ProgressMarkCompletedRequest = {
  mediaItemId: request.mediaItemId,
  initialLocator: { version: 1, kind: "video", data: { positionMs: 0 } },
}

/** 构造带有额外顶层键/缺失顶层键的 Locator；测试只关心外层守卫。 */
function locatorWithEnvelope(envelope: Record<string, unknown>): LocatorDto {
  return envelope as unknown as LocatorDto
}

describe("progress gateway", () => {
  it("passes the typed request and rejects malformed responses", async () => {
    const progressSave = vi.fn<HavenClient["progressSave"]>().mockResolvedValue({ revision: "r1" })
    const client = { progressSave } as unknown as HavenClient
    await expect(saveProgress(request, client)).resolves.toEqual({ revision: "r1" })
    expect(progressSave).toHaveBeenCalledWith(request)

    progressSave.mockResolvedValue({ revision: "" })
    await expect(saveProgress(request, client)).rejects.toHaveProperty("code", "PROGRESS_INVALID_RESPONSE")
  })

  it("enforces Mock CAS on first write, update, and stale conflict", async () => {
    const client = new MockHavenClient()
    const first = await client.progressSave(request)
    const second = await client.progressSave({
      ...request,
      expectedRevision: first.revision,
      locator: { version: 1, kind: "video", data: { positionMs: 2400 } },
    })
    expect(second.revision).not.toBe(first.revision)
    await expect(client.progressSave({ ...request, expectedRevision: first.revision }))
      .rejects.toHaveProperty("code", "REVISION_CONFLICT")
    await expect(client.progressSave(request)).rejects.toHaveProperty("code", "REVISION_CONFLICT")
  })

  it("fails closed before calling the client for non-canonical IDs", async () => {
    const progressSave = vi.fn<HavenClient["progressSave"]>()
    await expect(saveProgress({ ...request, mediaItemId: request.mediaItemId.toUpperCase() }, { progressSave } as unknown as HavenClient))
      .rejects.toHaveProperty("code", "INVALID_ARGUMENT")
    expect(progressSave).not.toHaveBeenCalled()
  })

  it("accepts nullable text anchors across locator variants", async () => {
    const progressSave = vi.fn<HavenClient["progressSave"]>().mockResolvedValue({ revision: "r1" })
    const client = { progressSave } as unknown as HavenClient
    const locators: ProgressSaveRequest["locator"][] = [
      { version: 1, kind: "book", data: { publicationResource: "chapter.xhtml", progression: null, textAnchor: null, formatLocator: null } },
      { version: 1, kind: "pdf", data: { pageIndex: 0, x: null, y: null, zoom: null, textAnchor: null } },
      { version: 1, kind: "article", data: { blockId: null, progression: null, textAnchor: null } },
    ]
    for (const locator of locators) {
      await expect(saveProgress({ ...request, locator }, client)).resolves.toEqual({ revision: "r1" })
    }
  })

  it("rejects locators with unknown or missing top-level envelope keys", async () => {
    const progressSave = vi.fn<HavenClient["progressSave"]>().mockResolvedValue({ revision: "r1" })
    const client = { progressSave } as unknown as HavenClient
    const invalidLocators = [
      locatorWithEnvelope({ version: 1, kind: "video", data: { positionMs: 0 }, extra: null }),
      locatorWithEnvelope({ version: 1, kind: "video", data: { positionMs: 0 }, positionMs: 0 }),
      locatorWithEnvelope({ version: 1, data: { positionMs: 0 } }),
      locatorWithEnvelope({ kind: "video", data: { positionMs: 0 } }),
      locatorWithEnvelope({ version: 1, kind: "video" }),
      locatorWithEnvelope({ version: 1, kind: "video", data: { positionMs: 0 }, chapterItemId: null }),
    ]
    for (const locator of invalidLocators) {
      await expect(saveProgress({ ...request, locator }, client))
        .rejects.toHaveProperty("code", "INVALID_ARGUMENT")
    }
    expect(progressSave).not.toHaveBeenCalled()
  })

  it("rejects mark-completed requests whose initial locator envelope is not exact", async () => {
    const progressMarkCompleted = vi.fn<HavenClient["progressMarkCompleted"]>()
      .mockResolvedValue({ revision: "r1" })
    const client = { progressMarkCompleted } as unknown as HavenClient

    await expect(markCompletedProgress(completedRequest, client)).resolves.toEqual({ revision: "r1" })

    for (const initialLocator of [
      locatorWithEnvelope({ version: 1, kind: "video", data: { positionMs: 0 }, extra: 1 }),
      locatorWithEnvelope({ version: 1, kind: "video" }),
      locatorWithEnvelope({ kind: "video", data: { positionMs: 0 } }),
    ]) {
      await expect(markCompletedProgress({ ...completedRequest, initialLocator }, client))
        .rejects.toHaveProperty("code", "INVALID_ARGUMENT")
    }
    expect(progressMarkCompleted).toHaveBeenCalledTimes(1)
    expect(progressMarkCompleted).toHaveBeenCalledWith(completedRequest)
  })

  it("rejects non-enumerable and symbol keys on either locator envelope", async () => {
    const progressSave = vi.fn<HavenClient["progressSave"]>().mockResolvedValue({ revision: "r1" })
    const progressMarkCompleted = vi.fn<HavenClient["progressMarkCompleted"]>()
      .mockResolvedValue({ revision: "r1" })
    const client = { progressSave, progressMarkCompleted } as unknown as HavenClient

    const nonEnumerableExtra = { version: 1, kind: "video", data: { positionMs: 0 } }
    Object.defineProperty(nonEnumerableExtra, "extra", { value: true, enumerable: false })
    const symbolExtra = {
      version: 1,
      kind: "video",
      data: { positionMs: 0 },
      [Symbol("extra")]: true,
    }

    for (const locator of [nonEnumerableExtra, symbolExtra]) {
      await expect(saveProgress({ ...request, locator: locator as LocatorDto }, client))
        .rejects.toHaveProperty("code", "INVALID_ARGUMENT")
      await expect(markCompletedProgress({
        ...completedRequest,
        initialLocator: locator as LocatorDto,
      }, client)).rejects.toHaveProperty("code", "INVALID_ARGUMENT")
    }

    expect(progressSave).not.toHaveBeenCalled()
    expect(progressMarkCompleted).not.toHaveBeenCalled()
  })

  it("keeps the comic locator self-reference check and rejects cross-item locators", async () => {
    const progressSave = vi.fn<HavenClient["progressSave"]>().mockResolvedValue({ revision: "r1" })
    const client = { progressSave } as unknown as HavenClient
    const chapterItemId = "0196f0d2-0000-7000-8000-0000000000ff"

    await expect(saveProgress({
      ...request,
      locator: { version: 1, kind: "comic", data: { chapterItemId, pageIndex: 0, pageProgression: null } },
      mediaItemId: chapterItemId,
    }, client)).resolves.toEqual({ revision: "r1" })

    await expect(saveProgress({
      ...request,
      locator: { version: 1, kind: "comic", data: { chapterItemId, pageIndex: 0, pageProgression: null } },
    }, client)).rejects.toHaveProperty("code", "INVALID_ARGUMENT")
  })
})
