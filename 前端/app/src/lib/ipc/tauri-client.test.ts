import { beforeEach, describe, expect, it, vi } from "vitest"
import type { ComicPageManifestGetRequest, ProgressSaveRequest, ReaderTocGetRequest } from "./generated/wire"
import type { SettingsUpdateRequest } from "./settings-wire"

const { invoke, check } = vi.hoisted(() => ({ invoke: vi.fn(), check: vi.fn() }))
vi.mock("@tauri-apps/api/core", () => ({
  Channel: class Channel<T> { onmessage?: (event: T) => void },
  invoke,
}))
vi.mock("@tauri-apps/plugin-updater", () => ({ check }))

import { TauriHavenClient } from "./tauri-client"

beforeEach(() => {
  invoke.mockReset()
})

describe("TauriHavenClient progress_save", () => {
  it("maps the request under the command's request argument", async () => {
    invoke.mockResolvedValueOnce({ revision: "revision-1" })
    const request: ProgressSaveRequest = {
      mediaItemId: "0196f0d2-0000-7000-8000-000000000000",
      locator: { version: 1, kind: "video", data: { positionMs: 1234 } },
      completion: "in_progress",
      expectedRevision: null,
    }
    await expect(new TauriHavenClient().progressSave(request)).resolves.toEqual({ revision: "revision-1" })
    expect(invoke).toHaveBeenCalledWith("progress_save", { request })
  })
})

describe("TauriHavenClient comic_page_manifest_get", () => {
  it("maps only the typed session request under the command request argument", async () => {
    const request: ComicPageManifestGetRequest = {
      sessionId: "0196f0d2-0000-7000-8000-000000000001",
    }
    const manifest = {
      schemaVersion: 1,
      sessionId: request.sessionId,
      mediaItemId: "0196f0d2-0000-7000-8000-000000000000",
      pageCount: 0,
      pages: [],
    } as const
    invoke.mockResolvedValueOnce(manifest)

    await expect(new TauriHavenClient().comicPageManifestGet(request)).resolves.toBe(manifest)
    expect(invoke).toHaveBeenCalledWith("comic_page_manifest_get", { request })
  })
})

describe("TauriHavenClient reader_toc_get", () => {
  it("maps only the typed session request under the command request argument", async () => {
    const request: ReaderTocGetRequest = {
      sessionId: "0196f0d2-0000-7000-8000-000000000001",
    }
    const result = {
      schemaVersion: 1,
      sessionId: request.sessionId,
      items: [
        { id: "a1b2c3d4e5f60718", title: "序言", depth: 0, progression: 0 },
      ],
    } as const
    invoke.mockResolvedValueOnce(result)

    await expect(new TauriHavenClient().readerTocGet(request)).resolves.toBe(result)
    expect(invoke).toHaveBeenCalledWith("reader_toc_get", { request })
  })
})

describe("TauriHavenClient settings", () => {
  it("maps settings_get and settings_update to their command argument shapes", async () => {
    const snapshot = {
      value: {
        section: "general",
        launchPage: "home",
        restoreSession: false,
        language: "zh_cn",
        notifications: true,
      },
      revision: null,
    }
    invoke.mockResolvedValueOnce(snapshot)

    const client = new TauriHavenClient()
    await expect(client.settingsGet("general")).resolves.toEqual(snapshot)
    expect(invoke).toHaveBeenLastCalledWith("settings_get", { section: "general" })

    const request: SettingsUpdateRequest = {
      section: "general",
      expectedRevision: null,
      patch: { section: "general", launchPage: "library" },
    }
    const result = {
      value: { ...snapshot.value, launchPage: "library" },
      revision: "settings-revision-1",
      changed: true,
    }
    invoke.mockResolvedValueOnce(result)

    await expect(client.settingsUpdate(request)).resolves.toEqual(result)
    expect(invoke).toHaveBeenLastCalledWith("settings_update", {
      section: "general",
      expectedRevision: null,
      patch: request.patch,
    })
  })

  it("rejects an invalid Settings response at the typed client boundary", async () => {
    invoke.mockResolvedValueOnce({ value: { section: "general" }, revision: null })

    await expect(new TauriHavenClient().settingsGet("general")).rejects.toMatchObject({
      code: "INTERNAL_ERROR",
      retryable: false,
    })
  })
})

describe("TauriHavenClient updater", () => {
  beforeEach(() => {
    check.mockReset()
  })

  it("returns a redacted up_to_date result when no signed update is available", async () => {
    check.mockResolvedValueOnce(null)

    await expect(new TauriHavenClient().updateCheck()).resolves.toEqual({
      status: "up_to_date",
      currentVersion: null,
      availableVersion: null,
      releaseNotes: null,
      publishedAt: null,
    })
    expect(check).toHaveBeenCalledWith({ timeout: 10_000 })
  })

  it("keeps only bounded update metadata and installs the pending update", async () => {
    const close = vi.fn().mockResolvedValue(undefined)
    const downloadAndInstall = vi.fn().mockResolvedValue(undefined)
    check.mockResolvedValueOnce({
      currentVersion: "0.1.0-beta.1",
      version: "0.1.1",
      body: "修复\n\u0000泄漏",
      date: "2026-08-30T00:00:00Z",
      rawJson: { signature: "must-not-cross-boundary", body: "raw" },
      close,
      downloadAndInstall,
    })

    const client = new TauriHavenClient()
    await expect(client.updateCheck()).resolves.toEqual({
      status: "available",
      currentVersion: "0.1.0-beta.1",
      availableVersion: "0.1.1",
      releaseNotes: "修复  泄漏",
      publishedAt: "2026-08-30T00:00:00Z",
    })
    await expect(client.updateInstall()).resolves.toEqual({ status: "installed" })
    expect(downloadAndInstall).toHaveBeenCalledOnce()
    expect(close).not.toHaveBeenCalled()
  })

  it("rejects install before a check without exposing updater internals", async () => {
    await expect(new TauriHavenClient().updateInstall()).rejects.toMatchObject({
      code: "UPDATER_NO_UPDATE",
      retryable: true,
    })
  })
})
