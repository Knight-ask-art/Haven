import { describe, expect, it } from "vitest"

import type { HavenClient } from "@/lib/ipc/client"
import type { DownloadTaskDto, ResourceListDto, ResourceSummaryDto, StorageLocationDto } from "@/lib/ipc/generated/wire"
import { createDownloadForMediaItem, getMediaItemDownloadInfo, getWorkDownloadState } from "./download-gateway"

const BASE_TASK: DownloadTaskDto = {
  schemaVersion: 1,
  taskId: "task-1",
  workId: "work-1",
  editionId: "edition-1",
  mediaItemId: "media-1",
  sourceResourceId: "source-1",
  targetStorageId: "storage-1",
  offlineResourceId: null,
  title: "测试下载",
  mediaType: "book",
  category: "book",
  posterUri: null,
  state: "completed",
  bytesTotal: 100,
  bytesDownloaded: 100,
  speedBps: null,
  etaSeconds: null,
  progressRatio: 1,
  createdAt: "2026-08-20T00:00:00.000Z",
  updatedAt: "2026-08-20T00:00:01.000Z",
}

const EMPTY_RESOURCES: ResourceListDto = { schemaVersion: 1, items: [] }

function clientWith(
  tasks: DownloadTaskDto[],
  resources: ResourceListDto = EMPTY_RESOURCES,
  storageLocations: StorageLocationDto[] = [],
): HavenClient {
  const client = {
    downloadList: async () => tasks,
    resourceListByMediaItem: async () => resources,
    storageLocationList: async () => storageLocations,
  } satisfies Pick<HavenClient, "downloadList" | "resourceListByMediaItem" | "storageLocationList">
  return client as unknown as HavenClient
}

describe("getWorkDownloadState", () => {
  it("uses an active task as the authoritative queued state", async () => {
    const active = { ...BASE_TASK, state: "downloading" as const, offlineResourceId: null }
    await expect(getWorkDownloadState("work-1", "media-1", clientWith([active])))
      .resolves.toBe("queued")
  })

  it("requires a completed task to retain its offline resource", async () => {
    await expect(getWorkDownloadState("work-1", "media-1", clientWith([BASE_TASK])))
      .resolves.toBe("idle")
    const completed = { ...BASE_TASK, offlineResourceId: "offline-1" }
    await expect(getWorkDownloadState("work-1", "media-1", clientWith([completed])))
      .resolves.toBe("downloaded")
  })

  it("still detects Offline Resource after the task record is removed", async () => {
    const resources: ResourceListDto = {
      schemaVersion: 1,
      items: [{
        resourceId: "offline-1",
        resourceType: "local_file",
        availability: "offline_available",
        mimeType: null,
        size: 100,
        storageDisplayName: "离线库",
        sourceDisplayName: null,
        isOffline: true,
        isLocal: true,
        requiresReauthorization: false,
        canDownload: false,
        canOnlineRead: true,
        streamKind: null,
      }],
    }

    await expect(getWorkDownloadState("work-1", "media-1", clientWith([], resources)))
      .resolves.toBe("downloaded")
  })

  it("prefers an existing Offline Resource over a stale queued task", async () => {
    const queued = { ...BASE_TASK, state: "downloading" as const, offlineResourceId: null }
    const resources: ResourceListDto = {
      schemaVersion: 1,
      items: [{
        resourceId: "offline-1",
        resourceType: "local_file",
        availability: "offline_available",
        mimeType: null,
        size: 100,
        storageDisplayName: "离线库",
        sourceDisplayName: null,
        isOffline: true,
        isLocal: true,
        requiresReauthorization: false,
        canDownload: false,
        canOnlineRead: true,
        streamKind: null,
      }],
    }
    await expect(getMediaItemDownloadInfo("media-1", clientWith([queued], resources)))
      .resolves.toMatchObject({ status: "downloaded", hasOfflineResource: true })
  })

  it("does not report a completed task without an offline resource as downloaded", async () => {
    const task = { ...BASE_TASK, state: "completed" as const, offlineResourceId: null }
    await expect(getMediaItemDownloadInfo("media-1", clientWith([task])))
      .resolves.toMatchObject({
        status: "idle",
        canDownload: false,
        hasOfflineResource: false,
        sourceResourceId: null,
      })
  })

  it("accepts an explicit server download capability for a remote resource", async () => {
    const remote = {
      resourceId: "remote-1",
      resourceType: "remote_page_set",
      availability: "available",
      mimeType: "application/zip",
      size: null,
      storageDisplayName: null,
      sourceDisplayName: "MangaDex",
      isOffline: false,
      isLocal: false,
      requiresReauthorization: false,
      streamKind: null,
      canDownload: true,
      canOnlineRead: false,
    } as ResourceSummaryDto & { canDownload: boolean }
    const task = { ...BASE_TASK, state: "queued" as const, sourceResourceId: "remote-1" }
    await expect(getMediaItemDownloadInfo("media-1", clientWith([task], { schemaVersion: 1, items: [remote] })))
      .resolves.toMatchObject({
        status: "queued",
        canDownload: true,
        canOnlineRead: false,
        sourceResourceId: "remote-1",
      })
  })

  it("creates a download from a remote resource without receiving a URL", async () => {
    const remote = {
      resourceId: "remote-1",
      resourceType: "remote_chapter",
      availability: "available",
      mimeType: "application/zip",
      size: null,
      storageDisplayName: null,
      sourceDisplayName: "MangaDex",
      isOffline: false,
      isLocal: false,
      requiresReauthorization: false,
      streamKind: null,
      canDownload: true,
      canOnlineRead: true,
    } as ResourceSummaryDto & { canDownload: boolean }
    const target: StorageLocationDto = {
      locationId: "storage-1",
      displayName: "离线库",
      providerType: "local",
      status: "connected",
    }
    const created = { ...BASE_TASK, state: "queued" as const, sourceResourceId: "remote-1" }
    const client = {
      ...clientWith([], { schemaVersion: 1, items: [remote] }, [target]),
      downloadCreate: async (request: { sourceResourceId: string; targetStorageId: string }) => {
        expect(request).toEqual({ sourceResourceId: "remote-1", targetStorageId: "storage-1" })
        return created
      },
    } satisfies Pick<HavenClient, "resourceListByMediaItem" | "storageLocationList" | "downloadCreate">

    await expect(createDownloadForMediaItem("media-1", client as HavenClient)).resolves.toBe(created)
  })

  it("keeps legacy local resources downloadable but rejects playback streams", async () => {
    const local = {
      resourceId: "local-1",
      resourceType: "local_file",
      availability: "available",
      mimeType: "application/epub+zip",
      size: 100,
      storageDisplayName: "本地库",
      sourceDisplayName: null,
        isOffline: false,
        isLocal: true,
        requiresReauthorization: false,
        canDownload: true,
        canOnlineRead: true,
        streamKind: null,
    } satisfies ResourceSummaryDto
    const stream: ResourceSummaryDto = { ...local, resourceId: "stream-1", resourceType: "remote_stream", isLocal: false }
    const target: StorageLocationDto = {
      locationId: "storage-1",
      displayName: "离线库",
      providerType: "local",
      status: "connected",
    }
    const created = { ...BASE_TASK, state: "queued" as const, sourceResourceId: "local-1" }
    const client = {
      ...clientWith([], { schemaVersion: 1, items: [stream, local] }, [target]),
      downloadCreate: async () => created,
    } satisfies Pick<HavenClient, "resourceListByMediaItem" | "storageLocationList" | "downloadCreate">

    await expect(createDownloadForMediaItem("media-1", client as HavenClient)).resolves.toBe(created)
  })

  it("keeps an explicitly online-readable stream playable without making it downloadable", async () => {
    const stream: ResourceSummaryDto = {
      resourceId: "stream-1",
      resourceType: "hls_stream",
      availability: "available",
      mimeType: "application/vnd.apple.mpegurl",
      size: null,
      storageDisplayName: null,
      sourceDisplayName: null,
      isOffline: false,
      isLocal: false,
      requiresReauthorization: false,
      canDownload: false,
      canOnlineRead: true,
      streamKind: "hls",
    }

    await expect(getMediaItemDownloadInfo(
      "media-1",
      clientWith([], { schemaVersion: 1, items: [stream] }),
    )).resolves.toMatchObject({
      status: "idle",
      canDownload: false,
      canOnlineRead: true,
      hasOfflineResource: false,
      sourceResourceId: null,
    })
  })
})
