import { describe, expect, it, vi } from "vitest"
import type { HavenClient } from "@/lib/ipc/client"
import type { ProgressSaveRequest } from "@/lib/ipc/generated/wire"
import { MockHavenClient } from "@/lib/ipc/mock-client"
import { saveProgress } from "./progress-gateway"

const request: ProgressSaveRequest = {
  mediaItemId: "0196f0d2-0000-7000-8000-000000000000",
  locator: { version: 1, kind: "video", data: { positionMs: 1200 } },
  completion: "in_progress",
  expectedRevision: null,
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
})
