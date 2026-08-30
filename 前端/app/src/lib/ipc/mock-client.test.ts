import { describe, expect, it } from "vitest"
import { MockHavenClient } from "./mock-client"

const request = { mediaItemId: "4", engine: "playback" as const }
const uuidPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/

describe("MockHavenClient session lifecycle", () => {
  it("creates unique canonical tokens and closes known, unknown, and repeated tokens idempotently", async () => {
    const client = new MockHavenClient()
    const first = await client.sessionOpen(request)
    const second = await client.sessionOpen(request)

    expect(first.sessionId).toMatch(uuidPattern)
    expect(second.sessionId).toMatch(uuidPattern)
    expect(second.sessionId).not.toBe(first.sessionId)
    await expect(client.sessionClose({ sessionId: first.sessionId })).resolves.toEqual({ schemaVersion: 1, closed: true })
    await expect(client.sessionClose({ sessionId: first.sessionId })).resolves.toEqual({ schemaVersion: 1, closed: true })
    await expect(client.sessionClose({ sessionId: "0196f0d2-0000-7000-8000-ffffffffffff" }))
      .resolves.toEqual({ schemaVersion: 1, closed: true })
  })

  it("merges sparse edition and media-item preference patches field by field", async () => {
    const client = new MockHavenClient(false, { seedSettings: false })
    const identity = { mediaItemId: "media-1", editionId: "edition-1" }

    await expect(client.preferenceGet(identity)).resolves.toMatchObject({
      effectiveReading: { fontSize: "medium", theme: "warm" },
    })

    await client.preferenceUpdate({
      ...identity,
      target: "edition",
      readingPatch: { fontSize: "large" },
      comicPatch: null,
      expectedRevision: null,
    })
    await client.preferenceUpdate({
      ...identity,
      target: "media_item",
      readingPatch: { theme: "dark" },
      comicPatch: null,
      expectedRevision: null,
    })

    const result = await client.preferenceGet(identity)
    expect(result.effectiveReading).toMatchObject({ fontSize: "large", theme: "dark" })
    expect(result.effectiveReading.section).toBe("reading")
    expect(result.effectiveComic.section).toBe("comic")
  })

  it("keeps comic manifests stable while active and rotates every runtime identity after close", async () => {
    const client = new MockHavenClient()
    const firstSession = await client.sessionOpen({ mediaItemId: "4", engine: "comic" })
    expect(firstSession.contentUri).toBeNull()

    const first = await client.comicPageManifestGet({ sessionId: firstSession.sessionId })
    const repeated = await client.comicPageManifestGet({ sessionId: firstSession.sessionId })
    expect(repeated).toEqual(first)
    expect(first.pageCount).toBe(first.pages.length)
    expect(new Set(first.pages.map((page) => page.pageId)).size).toBe(first.pages.length)
    const grants = first.pages.flatMap((page) => page.contentUri ? [page.contentUri] : [])
    expect(new Set(grants).size).toBe(grants.length)

    await client.sessionClose({ sessionId: firstSession.sessionId })
    await expect(client.comicPageManifestGet({ sessionId: firstSession.sessionId }))
      .rejects.toHaveProperty("code", "RESOURCE_NOT_FOUND")

    const reopened = await client.sessionOpen({ mediaItemId: "4", engine: "comic" })
    const second = await client.comicPageManifestGet({ sessionId: reopened.sessionId })
    expect(reopened.sessionId).not.toBe(firstSession.sessionId)
    expect(second.pages.map((page) => page.pageId)).not.toEqual(first.pages.map((page) => page.pageId))
    expect(second.pages.map((page) => page.contentUri)).not.toEqual(first.pages.map((page) => page.contentUri))
  })

  it("rejects manifest access for a non-comic session", async () => {
    const client = new MockHavenClient()
    const session = await client.sessionOpen(request)
    await expect(client.comicPageManifestGet({ sessionId: session.sessionId }))
      .rejects.toHaveProperty("code", "FORMAT_UNSUPPORTED")
  })

  it("returns deterministic reader TOC for reader sessions and rejects unknown/foreign sessions", async () => {
    const client = new MockHavenClient()
    const readerSession = await client.sessionOpen({ mediaItemId: "4", engine: "reader" })
    const toc = await client.readerTocGet({ sessionId: readerSession.sessionId })
    expect(toc.schemaVersion).toBe(1)
    expect(toc.sessionId).toBe(readerSession.sessionId)
    expect(toc.items.length).toBeGreaterThan(0)
    expect(toc.items.every((item) => /^[0-9a-f]{16}$/.test(item.id))).toBe(true)
    expect(toc.items.every((item) => item.progression >= 0 && item.progression <= 1)).toBe(true)
    const repeated = await client.readerTocGet({ sessionId: readerSession.sessionId })
    expect(repeated).toEqual(toc)

    await expect(client.readerTocGet({ sessionId: "0196f0d2-0000-7000-8000-ffffffffffff" }))
      .rejects.toHaveProperty("code", "RESOURCE_NOT_FOUND")

    const playbackSession = await client.sessionOpen(request)
    await expect(client.readerTocGet({ sessionId: playbackSession.sessionId }))
      .rejects.toHaveProperty("code", "FORMAT_UNSUPPORTED")
  })

  it("returns explicit Mock About facts and rejects directory access", async () => {
    const client = new MockHavenClient()
    const info = await client.appInfoGet()
    expect(info.appVersion).toBe("Mock")
    expect(info.directories.every((directory) => directory.canOpen === false)).toBe(true)
    await expect(client.openDataDirectory()).rejects.toHaveProperty("code", "APP_DIRECTORY_UNAVAILABLE")
  })

  it("keeps updater unavailable in Browser Mock", async () => {
    const client = new MockHavenClient()
    await expect(client.updateCheck()).rejects.toMatchObject({
      code: "UPDATER_UNAVAILABLE",
      retryable: false,
    })
    await expect(client.updateInstall()).rejects.toMatchObject({
      code: "UPDATER_UNAVAILABLE",
      retryable: false,
    })
  })

  it("projects custom sources into the registry and keeps their enabled state scoped", async () => {
    const client = new MockHavenClient()
    const added = await client.sourceAdd({
      displayName: "我的 OPDS 书库",
      endpoint: "https://example.invalid/opds",
    })
    let registry = await client.sourceRegistryList()
    let custom = registry.sources.find((source) => source.sourceId === added.sourceId)
    expect(custom).toMatchObject({
      displayName: "我的 OPDS 书库",
      categories: ["book"],
      mode: "single",
      endpointConfigured: true,
      enabled: false,
    })

    await client.sourceRegistrySet({ sourceId: added.sourceId, enabled: true })
    registry = await client.sourceRegistryList()
    custom = registry.sources.find((source) => source.sourceId === added.sourceId)
    expect(custom?.enabled).toBe(true)

    await client.sourceRemove({ sourceId: added.sourceId })
    registry = await client.sourceRegistryList()
    expect(registry.sources.some((source) => source.sourceId === added.sourceId)).toBe(false)
  })
})
