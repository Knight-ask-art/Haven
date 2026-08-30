import { describe, expect, it } from "vitest"
import type { HavenClient } from "./client"
import { MockHavenClient } from "./mock-client"

describe("Settings HavenClient contract", () => {
  it("reads defaults and persists an update through the mock implementation", async () => {
    const client: HavenClient = new MockHavenClient(false, { seedSettings: false })

    const initial = await client.settingsGet("general")
    expect(initial.revision).toBeNull()
    expect(initial.value).toMatchObject({ section: "general", launchPage: "home" })

    const updated = await client.settingsUpdate({
      section: "general",
      expectedRevision: null,
      patch: { section: "general", launchPage: "library" },
    })
    expect(updated.changed).toBe(true)
    expect(updated.revision).toEqual(expect.any(String))
    expect(updated.value).toMatchObject({ section: "general", launchPage: "library" })

    await expect(client.settingsGet("general")).resolves.toEqual({
      value: updated.value,
      revision: updated.revision,
    })
  })

  it("round-trips the downloads policy through the same CAS client", async () => {
    const client: HavenClient = new MockHavenClient(false, { seedSettings: false })

    const initial = await client.settingsGet("downloads")
    expect(initial).toEqual({
      value: { section: "downloads", concurrentTasks: "three", speedLimit: "unlimited", autoContinue: true },
      revision: null,
    })

    const updated = await client.settingsUpdate({
      section: "downloads",
      expectedRevision: initial.revision,
      patch: { section: "downloads", concurrentTasks: "five", speedLimit: "mbps2", autoContinue: false },
    })
    expect(updated.changed).toBe(true)
    expect(updated.value).toEqual({ section: "downloads", concurrentTasks: "five", speedLimit: "mbps2", autoContinue: false })

    await expect(client.settingsGet("downloads")).resolves.toEqual({
      value: updated.value,
      revision: updated.revision,
    })
  })

  it("round-trips playback and reading history privacy through the same CAS client", async () => {
    const client: HavenClient = new MockHavenClient(false, { seedSettings: false })
    const initial = await client.settingsGet("privacy")
    expect(initial).toEqual({
      value: { section: "privacy", searchHistory: true, playbackHistory: true },
      revision: null,
    })

    const updated = await client.settingsUpdate({
      section: "privacy",
      expectedRevision: initial.revision,
      patch: { section: "privacy", playbackHistory: false },
    })
    expect(updated.changed).toBe(true)
    expect(updated.value).toEqual({ section: "privacy", searchHistory: true, playbackHistory: false })
  })

  it("round-trips automatic next playback policy through the same CAS client", async () => {
    const client: HavenClient = new MockHavenClient(false, { seedSettings: false })
    const initial = await client.settingsGet("playback")
    expect(initial).toEqual({
      value: { section: "playback", defaultPlaybackRate: "one", autoResume: true, autoNext: true },
      revision: null,
    })

    const updated = await client.settingsUpdate({
      section: "playback",
      expectedRevision: initial.revision,
      patch: { section: "playback", autoNext: false },
    })
    expect(updated.changed).toBe(true)
    expect(updated.value).toEqual({ section: "playback", defaultPlaybackRate: "one", autoResume: true, autoNext: false })
  })
})
