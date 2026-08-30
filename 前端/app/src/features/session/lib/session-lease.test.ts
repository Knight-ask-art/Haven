import { describe, expect, it, vi } from "vitest"
import type { HavenClient } from "@/lib/ipc/client"
import type { SessionOpenResultDto } from "@/lib/ipc/generated/wire"
import { acquireSessionLease } from "./session-lease"

const result = (mediaItemId: string, sessionId: string): SessionOpenResultDto => ({
  schemaVersion: 1,
  sessionId,
  contentUri: "https://example.test/content.mp4",
  workId: "work-1",
  editionId: "edition-1",
  mediaItemId,
  engine: "playback",
  progress: null,
})
const flush = async (): Promise<void> => {
  await Promise.resolve()
  await Promise.resolve()
}

describe("session lease", () => {
  it("shares one open, and closes once after the last consumer releases", async () => {
    const sessionOpen = vi.fn().mockResolvedValue(result("media-1", "0196f0d2-0000-7000-8000-000000000001"))
    const sessionClose = vi.fn().mockResolvedValue({ schemaVersion: 1, closed: true })
    const client = { sessionOpen, sessionClose } as unknown as HavenClient
    const first = acquireSessionLease(client, "media-1:playback:0", { mediaItemId: "media-1", engine: "playback" })
    const second = acquireSessionLease(client, "media-1:playback:0", { mediaItemId: "media-1", engine: "playback" })

    expect(sessionOpen).toHaveBeenCalledOnce()
    await expect(Promise.all([first.promise, second.promise])).resolves.toHaveLength(2)
    first.release()
    await flush()
    expect(sessionClose).not.toHaveBeenCalled()
    second.release()
    await flush()
    expect(sessionClose).toHaveBeenCalledOnce()
    second.release()
  })

  it("closes after release before open resolves", async () => {
    let resolveOpen!: (value: SessionOpenResultDto) => void
    const sessionOpen = vi.fn().mockImplementation(() => new Promise<SessionOpenResultDto>((resolve) => { resolveOpen = resolve }))
    const sessionClose = vi.fn().mockResolvedValue({ schemaVersion: 1, closed: true })
    const client = { sessionOpen, sessionClose } as unknown as HavenClient
    const lease = acquireSessionLease(client, "media-2:playback:retry-1", { mediaItemId: "media-2", engine: "playback" })
    lease.release()
    resolveOpen(result("media-2", "0196f0d2-0000-7000-8000-000000000002"))
    await lease.promise
    await flush()
    expect(sessionClose).toHaveBeenCalledOnce()
  })

  it("isolates request and retry keys", async () => {
    const sessionOpen = vi.fn()
      .mockResolvedValueOnce(result("media-3", "0196f0d2-0000-7000-8000-000000000003"))
      .mockResolvedValueOnce(result("media-3", "0196f0d2-0000-7000-8000-000000000004"))
    const sessionClose = vi.fn().mockResolvedValue({ schemaVersion: 1, closed: true })
    const client = { sessionOpen, sessionClose } as unknown as HavenClient
    const first = acquireSessionLease(client, "media-3:playback:0", { mediaItemId: "media-3", engine: "playback" })
    const retry = acquireSessionLease(client, "media-3:playback:1", { mediaItemId: "media-3", engine: "playback" })
    await Promise.all([first.promise, retry.promise])
    expect(sessionOpen).toHaveBeenCalledTimes(2)
    first.release()
    retry.release()
    await flush()
    expect(sessionClose).toHaveBeenCalledTimes(2)
  })

  it("waits for a release barrier before closing", async () => {
    const sessionOpen = vi.fn().mockResolvedValue(result("media-4", "0196f0d2-0000-7000-8000-000000000004"))
    const sessionClose = vi.fn().mockResolvedValue({ schemaVersion: 1, closed: true })
    const lease = acquireSessionLease({ sessionOpen, sessionClose } as unknown as HavenClient, "media-4:playback:0", { mediaItemId: "media-4", engine: "playback" })
    await lease.promise
    let resolve!: () => void
    const barrier = new Promise<void>((r) => { resolve = r })
    lease.releaseAfter(barrier); lease.release()
    await flush(); expect(sessionClose).not.toHaveBeenCalled()
    resolve(); await flush(); expect(sessionClose).toHaveBeenCalledOnce()
  })

  it("closes after a rejected barrier and claims release only once", async () => {
    const sessionOpen = vi.fn().mockResolvedValue(result("media-5", "0196f0d2-0000-0000-0000-000000000005"))
    const sessionClose = vi.fn().mockResolvedValue({ schemaVersion: 1, closed: true })
    const lease = acquireSessionLease({ sessionOpen, sessionClose } as unknown as HavenClient, "media-5:playback:0", { mediaItemId: "media-5", engine: "playback" })
    await lease.promise
    lease.releaseAfter(Promise.reject(new Error("flush failed"))); lease.release(); lease.releaseAfter(Promise.resolve())
    await flush(); expect(sessionClose).toHaveBeenCalledOnce()
  })
})
