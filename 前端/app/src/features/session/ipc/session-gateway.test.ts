import { afterEach, describe, expect, it, vi } from "vitest"
import type { HavenClient } from "@/lib/ipc/client"
import type { SessionCloseResultDto, SessionOpenResultDto } from "@/lib/ipc/generated/wire"
import { MockHavenClient } from "@/lib/ipc/mock-client"
import { closeSession, openSession } from "./session-gateway"

const mediaItemId = "0196f0d2-0000-7000-8000-000000000000"
const request = { mediaItemId, engine: "playback" as const }
const browserResult: SessionOpenResultDto = {
  schemaVersion: 1,
  sessionId: "0196f0d2-0000-7000-8000-000000000001",
  contentUri: "https://example.test/content.mp4",
  workId: "work-1",
  editionId: "edition-1",
  mediaItemId,
  engine: request.engine,
  progress: null,
}
const comicRequest = { mediaItemId, engine: "comic" as const }
const comicResult: SessionOpenResultDto = {
  ...browserResult,
  contentUri: null,
  engine: "comic",
}

const originalWindow = globalThis.window

function clientWithSession(sessionOpen: HavenClient["sessionOpen"]): HavenClient {
  return { sessionOpen } as unknown as HavenClient
}

function useTauriRuntime(): void {
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: { __TAURI_INTERNALS__: {} },
  })
}

function useBrowserRuntime(): void {
  if (originalWindow) {
    Object.defineProperty(globalThis, "window", { configurable: true, value: originalWindow })
  } else {
    Reflect.deleteProperty(globalThis, "window")
  }
}

afterEach(() => {
  vi.restoreAllMocks()
  useBrowserRuntime()
})

describe("session gateway", () => {
  it("passes a canonical close request and accepts the successful result", async () => {
    const sessionClose = vi.fn<HavenClient["sessionClose"]>().mockResolvedValue({ schemaVersion: 1, closed: true })
    const client = { sessionClose } as unknown as HavenClient
    const sessionId = "0196f0d2-0000-7000-8000-000000000001"

    await expect(closeSession(sessionId, client)).resolves.toEqual({ schemaVersion: 1, closed: true })
    expect(sessionClose).toHaveBeenCalledWith({ sessionId })
  })

  it("rejects an invalid session id before calling the client", async () => {
    const sessionClose = vi.fn<HavenClient["sessionClose"]>()
    await expect(closeSession("SESSION-1", { sessionClose } as unknown as HavenClient))
      .rejects.toHaveProperty("code", "INVALID_ARGUMENT")
    expect(sessionClose).not.toHaveBeenCalled()
  })

  it.each([
    { schemaVersion: 2 as 1, closed: true },
    { schemaVersion: 1 as const, closed: false },
  ])("rejects malformed close result %#", async (result: SessionCloseResultDto) => {
    const sessionClose = vi.fn<HavenClient["sessionClose"]>().mockResolvedValue(result)
    await expect(closeSession("0196f0d2-0000-7000-8000-000000000001", { sessionClose } as unknown as HavenClient))
      .rejects.toHaveProperty("code", "SESSION_INVALID_RESPONSE")
  })

  it("passes the typed request through and accepts browser HTTPS content", async () => {
    const sessionOpen = vi.fn<HavenClient["sessionOpen"]>().mockResolvedValue(browserResult)
    const client = clientWithSession(sessionOpen)

    await expect(openSession(request, client)).resolves.toBe(browserResult)
    expect(sessionOpen).toHaveBeenCalledOnce()
    expect(sessionOpen).toHaveBeenCalledWith(request)
  })

  it("accepts comic only with a null root content URI in browser and Tauri runtimes", async () => {
    for (const tauri of [false, true]) {
      if (tauri) useTauriRuntime()
      else useBrowserRuntime()
      const sessionOpen = vi.fn<HavenClient["sessionOpen"]>().mockResolvedValue(comicResult)
      await expect(openSession(comicRequest, clientWithSession(sessionOpen))).resolves.toBe(comicResult)
    }
  })

  it.each([
    "https://example.test/comic.cbz",
    "haven-resource://session/0196f0d2-0000-7000-8000-000000000001",
  ])("rejects a comic root content URI: %s", async (contentUri) => {
    const sessionOpen = vi.fn<HavenClient["sessionOpen"]>()
      .mockResolvedValue({ ...comicResult, contentUri })
    await expect(openSession(comicRequest, clientWithSession(sessionOpen)))
      .rejects.toHaveProperty("code", "SESSION_INVALID_RESPONSE")
  })

  it("rejects null content for every non-comic engine", async () => {
    for (const engine of ["playback", "reader", "article"] as const) {
      const engineRequest = { mediaItemId, engine }
      const sessionOpen = vi.fn<HavenClient["sessionOpen"]>()
        .mockResolvedValue({ ...browserResult, engine, contentUri: null })
      await expect(openSession(engineRequest, clientWithSession(sessionOpen)))
        .rejects.toHaveProperty("code", "SESSION_INVALID_RESPONSE")
    }
  })

  it("rejects invalid schema, media item, and engine responses", async () => {
    const cases: Array<Partial<SessionOpenResultDto>> = [
      { schemaVersion: 2 as 1 },
      { sessionId: "session-1" },
      { mediaItemId: "other-media" },
      { engine: "reader" },
    ]

    for (const change of cases) {
      const sessionOpen = vi.fn<HavenClient["sessionOpen"]>().mockResolvedValue({ ...browserResult, ...change })
      await expect(openSession(request, clientWithSession(sessionOpen)))
        .rejects.toHaveProperty("code", "SESSION_INVALID_RESPONSE")
    }
  })

  it("requires a haven-resource session URI in the real Tauri runtime", async () => {
    useTauriRuntime()
    const invalid = vi.fn<HavenClient["sessionOpen"]>().mockResolvedValue(browserResult)
    await expect(openSession(request, clientWithSession(invalid)))
      .rejects.toHaveProperty("code", "SESSION_INVALID_RESPONSE")

    const valid = { ...browserResult, contentUri: "haven-resource://session/0196f0d2-0000-7000-8000-000000000001" }
    const sessionOpen = vi.fn<HavenClient["sessionOpen"]>().mockResolvedValue(valid)
    await expect(openSession(request, clientWithSession(sessionOpen))).resolves.toBe(valid)
  })

  it("rejects uppercase production URI scheme and authority", async () => {
    useTauriRuntime()
    for (const contentUri of [
      "HAVEN-RESOURCE://session/0196f0d2-0000-7000-8000-000000000001",
      "haven-resource://SESSION/0196f0d2-0000-7000-8000-000000000001",
    ]) {
      const sessionOpen = vi.fn<HavenClient["sessionOpen"]>().mockResolvedValue({ ...browserResult, contentUri })
      await expect(openSession(request, clientWithSession(sessionOpen)))
        .rejects.toHaveProperty("code", "SESSION_INVALID_RESPONSE")
    }
  })

  it.each([
    "haven-resource://session/0196f0d2-0000-7000-8000-000000000001?x=1",
    "haven-resource://session/0196f0d2-0000-7000-8000-000000000001#x",
    "haven-resource://session/0196f0d2-0000-7000-8000-000000000001/extra",
    "haven-resource://session/%2F0196f0d2-0000-7000-8000-000000000001",
    "haven-resource://session\\0196f0d2-0000-7000-8000-000000000001",
  ])("rejects a non-canonical production URI: %s", async (contentUri) => {
    useTauriRuntime()
    const sessionOpen = vi.fn<HavenClient["sessionOpen"]>().mockResolvedValue({ ...browserResult, contentUri })
    await expect(openSession(request, clientWithSession(sessionOpen)))
      .rejects.toHaveProperty("code", "SESSION_INVALID_RESPONSE")
  })

  it("accepts the browser Mock client's HTTPS fixture", async () => {
    const result = await openSession({ mediaItemId: "4", engine: "playback" }, new MockHavenClient())
    expect(result.contentUri).toMatch(/^https:\/\//)
  })

  it("rejects an unknown browser Mock media item without a fallback fixture", async () => {
    await expect(openSession({ mediaItemId: "unknown-media", engine: "playback" }, new MockHavenClient()))
      .rejects.toHaveProperty("code", "MEDIA_ITEM_NOT_FOUND")
  })
})
