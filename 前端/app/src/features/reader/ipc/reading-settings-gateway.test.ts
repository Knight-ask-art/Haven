import { describe, expect, it } from "vitest"
import type { HavenClient } from "@/lib/ipc/client"
import { MockHavenClient } from "@/lib/ipc/mock-client"
import type { PreferenceGetResult } from "@/lib/ipc/settings-wire"
import { DEFAULT_READING_SETTINGS, getReadingSettings, getReadingSettingsForResource } from "./reading-settings-gateway"

describe("reading settings gateway", () => {
  it("reads the persisted Reading section through the typed client", async () => {
    const client = new MockHavenClient(false, { seedSettings: false })
    const initial = await getReadingSettings(client)
    expect(initial).toEqual(DEFAULT_READING_SETTINGS)

    const snapshot = await client.settingsGet("reading")
    await client.settingsUpdate({
      section: "reading",
      expectedRevision: snapshot.revision,
      patch: { section: "reading", fontFamily: "kai", contentWidth: "wide" },
    })
    await expect(getReadingSettings(client)).resolves.toMatchObject({
      section: "reading",
      fontFamily: "kai",
      contentWidth: "wide",
    })
  })

  it("defensively falls back when a client returns a different section", async () => {
    const client = {
      settingsGet: async () => ({
        value: { section: "appearance", theme: "dark", density: "compact", sidebar: "auto", reduceMotion: false },
        revision: "wrong-section",
      }),
    } as unknown as HavenClient

    await expect(getReadingSettings(client)).resolves.toEqual(DEFAULT_READING_SETTINGS)
  })

  it("lets transport errors reach the hook boundary instead of inventing saved values", async () => {
    const client = {
      settingsGet: async () => { throw new Error("database unavailable") },
    } as unknown as HavenClient

    await expect(getReadingSettings(client)).rejects.toThrow("database unavailable")
  })

  it("reads the backend-computed effective Reading value for a resource", async () => {
    const effective: PreferenceGetResult = {
      schemaVersion: 1,
      mediaItemId: "media-1",
      editionId: "edition-1",
      readingPatch: { fontFamily: "kai", fontSize: "large" },
      comicPatch: null,
      editionReadingPatch: { fontFamily: "kai", fontSize: "large" },
      editionComicPatch: null,
      mediaItemReadingPatch: null,
      mediaItemComicPatch: null,
      effectiveReading: { ...DEFAULT_READING_SETTINGS, fontFamily: "kai", fontSize: "large", pagination: "scroll" },
      effectiveComic: { section: "comic", viewMode: "single", direction: "rtl", pageGap: "twelve", preloadPages: "three" },
      mediaItemRevision: "media-rev-1",
      editionRevision: "edition-rev-1",
    }
    const client = {
      preferenceGet: async (request: { mediaItemId: string; editionId: string }) => {
        expect(request).toEqual({ mediaItemId: "media-1", editionId: "edition-1" })
        return effective
      },
    } as Pick<HavenClient, "preferenceGet">

    await expect(getReadingSettingsForResource("media-1", "edition-1", client)).resolves.toMatchObject({
      fontFamily: "kai",
      fontSize: "large",
    })
  })
})
