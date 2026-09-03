import { beforeEach, describe, expect, it, vi } from "vitest"
import type {
  ComicChapterCatalogGetRequest,
  ComicRegisteredChapterCatalogDto,
  ComicChapterSourceCandidatesGetRequestDto,
  ComicProgressMigrationRequestDto,
  ComicPageProgressRemapRequestDto,
  ComicProgressMigrationRevertRequestDto,
  ComicPageManifestGetRequest,
  ProgressSaveRequest,
  ReaderTocGetRequest,
} from "./generated/wire"
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

describe("TauriHavenClient comic_chapter_catalog_get", () => {
  it("maps the source/work request under the command request argument", async () => {
    const request: ComicChapterCatalogGetRequest = {
      sourceId: "mangadex",
      remoteWorkId: "aaaaaaaa-aaaa-4aaa-8000-aaaaaaaaaaaa",
    }
    const catalog = {
      schemaVersion: 1,
      sourceId: request.sourceId,
      remoteWorkId: request.remoteWorkId,
      fetchedAt: "2026-09-04T00:00:00Z",
      total: 0,
      truncated: false,
      chapters: [],
    } as const
    invoke.mockResolvedValueOnce(catalog)

    await expect(new TauriHavenClient().comicChapterCatalogGet(request)).resolves.toBe(catalog)
    expect(invoke).toHaveBeenCalledWith("comic_chapter_catalog_get", { request })
  })
})

describe("TauriHavenClient comic_chapter_catalog_registered_get", () => {
  it("maps the persisted catalog request under the command request argument", async () => {
    const request: ComicChapterCatalogGetRequest = {
      sourceId: "mangadex",
      remoteWorkId: "aaaaaaaa-aaaa-4aaa-8000-aaaaaaaaaaaa",
    }
    const catalog: ComicRegisteredChapterCatalogDto = {
      schemaVersion: 1,
      sourceId: request.sourceId,
      remoteWorkId: request.remoteWorkId,
      refreshState: null,
      chapters: [],
    }
    invoke.mockResolvedValueOnce(catalog)

    await expect(new TauriHavenClient().comicChapterCatalogRegisteredGet(request)).resolves.toBe(catalog)
    expect(invoke).toHaveBeenCalledWith("comic_chapter_catalog_registered_get", { request })
  })
})

describe("TauriHavenClient comic_chapter_catalog_refresh", () => {
  it("maps the explicit refresh request under the command request argument", async () => {
    const request: ComicChapterCatalogGetRequest = {
      sourceId: "mangadex",
      remoteWorkId: "aaaaaaaa-aaaa-4aaa-8000-aaaaaaaaaaaa",
    }
    const catalog = {
      schemaVersion: 1,
      sourceId: request.sourceId,
      remoteWorkId: request.remoteWorkId,
      fetchedAt: "2026-09-04T00:00:00Z",
      total: 0,
      truncated: false,
      chapters: [],
    } as const
    invoke.mockResolvedValueOnce(catalog)

    await expect(new TauriHavenClient().comicChapterCatalogRefresh(request)).resolves.toBe(catalog)
    expect(invoke).toHaveBeenCalledWith("comic_chapter_catalog_refresh", { request })
  })
})

describe("TauriHavenClient comic_chapter_source_candidates_get", () => {
  it("passes the opaque source identity under the command request argument", async () => {
    const request: ComicChapterSourceCandidatesGetRequestDto = {
      source: {
        sourceId: "mangadex",
        remoteWorkId: "aaaaaaaa-aaaa-4aaa-8000-aaaaaaaaaaaa",
        remoteChapterId: "bbbbbbbb-bbbb-4bbb-8000-bbbbbbbbbbbb",
      },
    }
    const result = {
      schemaVersion: 1,
      source: request.source,
      currentMediaItemId: "cccccccc-cccc-4ccc-8000-cccccccccccc",
      candidates: [],
      truncated: false,
    } as const
    invoke.mockResolvedValueOnce(result)

    await expect(new TauriHavenClient().comicChapterSourceCandidatesGet(request)).resolves.toBe(result)
    expect(invoke).toHaveBeenCalledWith("comic_chapter_source_candidates_get", { request })
  })
})

describe("TauriHavenClient comic progress migration", () => {
  const source = {
    sourceId: "mangadex",
    remoteWorkId: "aaaaaaaa-aaaa-4aaa-8000-aaaaaaaaaaaa",
    remoteChapterId: "bbbbbbbb-bbbb-4bbb-8000-bbbbbbbbbbbb",
  }
  const target = {
    sourceId: "reader-ws",
    remoteWorkId: "reader-work-12",
    remoteChapterId: "reader-chapter-12",
  }

  it("passes the migration request under the command request argument", async () => {
    const request: ComicProgressMigrationRequestDto = {
      source,
      target,
      allowTargetOverwrite: false,
    }
    const result = {
      status: "applied",
      matchResult: null,
      pageMigration: {
        targetPageIndex: 1,
        confidence: "high",
        strategy: "stable_key",
        reversible: true,
      },
      snapshotId: "dddddddd-dddd-4ddd-8000-dddddddddddd",
      appliedRevision: "456",
    } as const
    invoke.mockResolvedValueOnce(result)

    await expect(new TauriHavenClient().comicProgressMigrate(request)).resolves.toBe(result)
    expect(invoke).toHaveBeenCalledWith("comic_progress_migrate", { request })
  })

  it("passes page remap and CAS-protected revert requests without reshaping them", async () => {
    const remap: ComicPageProgressRemapRequestDto = {
      sessionId: "cccccccc-cccc-4ccc-8000-cccccccccccc",
      expectedRevision: "123",
    }
    const migrationResult = {
      status: "no_source_progress",
      matchResult: null,
      pageMigration: {
        targetPageIndex: null,
        confidence: "low",
        strategy: "no_target",
        reversible: true,
      },
      snapshotId: null,
      appliedRevision: null,
    } as const
    invoke.mockResolvedValueOnce(migrationResult)
    await expect(new TauriHavenClient().comicProgressRemap(remap)).resolves.toBe(migrationResult)
    expect(invoke).toHaveBeenLastCalledWith("comic_progress_remap", { request: remap })

    const revert: ComicProgressMigrationRevertRequestDto = {
      migrationId: "dddddddd-dddd-4ddd-8000-dddddddddddd",
      expectedAppliedRevision: "456",
    }
    const revertResult = { reverted: true } as const
    invoke.mockResolvedValueOnce(revertResult)
    await expect(new TauriHavenClient().comicProgressRevert(revert)).resolves.toBe(revertResult)
    expect(invoke).toHaveBeenLastCalledWith("comic_progress_revert", { request: revert })
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
