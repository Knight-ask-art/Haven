import { describe, expect, it, vi } from "vitest"

import type { HavenClient } from "@/lib/ipc/client"
import type { DownloadTaskDto, ResourceListDto, ResourceSummaryDto, StorageLocationDto } from "@/lib/ipc/generated/wire"
import {
  createDownloadForMediaItem,
  getMediaItemDownloadInfo,
  getMediaItemsDownloadInfo,
  getWorkDownloadState,
} from "./download-gateway"

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

/** 一条真实的远端可下载资源投影；批量隔离用例只需要「有真实能力」这一个事实。 */
function downloadableResources(resourceId: string): ResourceListDto {
  return {
    schemaVersion: 1,
    items: [{
      resourceId,
      resourceType: "publication_file",
      availability: "available",
      mimeType: "application/pdf",
      size: null,
      storageDisplayName: null,
      sourceDisplayName: "期刊来源",
      isOffline: false,
      isLocal: false,
      requiresReauthorization: false,
      canDownload: true,
      canOnlineRead: false,
      streamKind: null,
    }],
  }
}

function clientWith(
  tasks: DownloadTaskDto[],
  resources: ResourceListDto = EMPTY_RESOURCES,
  storageLocations: StorageLocationDto[] = [],
  downloadList: (request: { limit: number; offset: number }) => Promise<DownloadTaskDto[]> = async () => tasks,
): HavenClient {
  const client = {
    downloadList,
    resourceListByMediaItem: async () => resources,
    storageLocationList: async () => storageLocations,
  } satisfies Pick<HavenClient, "downloadList" | "resourceListByMediaItem" | "storageLocationList">
  return client as unknown as HavenClient
}

describe("getWorkDownloadState", () => {
  it("pages through the complete task history instead of truncating at 200 rows", async () => {
    const firstPage = Array.from({ length: 200 }, (_, index) => ({
      ...BASE_TASK,
      taskId: `task-${index}`,
    }))
    const oldTask = { ...BASE_TASK, taskId: "old-task", state: "failed" as const }
    const downloadList = vi.fn(async ({ offset }: { limit: number; offset: number }) => (
      offset === 0 ? firstPage : [oldTask]
    ))

    await expect(getWorkDownloadState("work-1", "media-1", clientWith([], EMPTY_RESOURCES, [], downloadList)))
      .resolves.toBe("idle")
    expect(downloadList).toHaveBeenNthCalledWith(1, { limit: 200, offset: 0 })
    expect(downloadList).toHaveBeenNthCalledWith(2, { limit: 200, offset: 200 })
  })

  it("uses an active task as the authoritative queued state", async () => {
    const active = { ...BASE_TASK, state: "downloading" as const, offlineResourceId: null }
    await expect(getWorkDownloadState("work-1", "media-1", clientWith([active])))
      .resolves.toBe("queued")
  })

  it("does not trust a completed task unless its offline resource is present", async () => {
    await expect(getWorkDownloadState("work-1", "media-1", clientWith([BASE_TASK])))
      .resolves.toBe("idle")
    const completed = { ...BASE_TASK, offlineResourceId: "offline-1" }
    await expect(getWorkDownloadState("work-1", "media-1", clientWith([completed])))
      .resolves.toBe("idle")
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
      .resolves.toMatchObject({ status: "downloaded", hasOfflineResource: true, taskId: null })

    const completed = { ...BASE_TASK, taskId: "completed-task", offlineResourceId: "offline-1" }
    await expect(getMediaItemDownloadInfo("media-1", clientWith([queued, completed], resources)))
      .resolves.toMatchObject({ status: "downloaded", taskId: "completed-task" })
  })

  it("pairs an available offline resource with its exact completed task", async () => {
    const resources: ResourceListDto = {
      schemaVersion: 1,
      items: [{
        resourceId: "offline-available",
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
    const missing = { ...BASE_TASK, taskId: "missing-task", offlineResourceId: "offline-missing" }
    const available = { ...BASE_TASK, taskId: "available-task", offlineResourceId: "offline-available" }

    await expect(getMediaItemDownloadInfo("media-1", clientWith([missing, available], resources)))
      .resolves.toMatchObject({
        status: "downloaded",
        hasOfflineResource: true,
        taskId: "available-task",
      })
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

  it("does not report a completed task as downloaded when its offline resource is absent", async () => {
    const staleCompleted = {
      ...BASE_TASK,
      taskId: "stale-completed-task",
      offlineResourceId: "removed-offline-resource",
    }

    await expect(getMediaItemDownloadInfo("media-1", clientWith([staleCompleted])))
      .resolves.toMatchObject({
        status: "idle",
        hasOfflineResource: false,
        taskId: null,
      })
  })

  it("uses an active task when an earlier completed task points to an absent resource", async () => {
    const staleCompleted = {
      ...BASE_TASK,
      taskId: "stale-completed-task",
      offlineResourceId: "removed-offline-resource",
    }
    const active = {
      ...BASE_TASK,
      taskId: "active-task",
      state: "downloading" as const,
      offlineResourceId: null,
    }

    await expect(getMediaItemDownloadInfo("media-1", clientWith([staleCompleted, active])))
      .resolves.toMatchObject({
        status: "queued",
        hasOfflineResource: false,
        taskId: "active-task",
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
  it("shares the complete download list when projecting several media items", async () => {
    const resourcesByMediaItem: Record<string, ResourceListDto> = {
      "media-1": EMPTY_RESOURCES,
      "media-2": {
        schemaVersion: 1,
        items: [{
          resourceId: "remote-2",
          resourceType: "publication_file",
          availability: "available",
          mimeType: "application/pdf",
          size: null,
          storageDisplayName: null,
          sourceDisplayName: "期刊来源",
          isOffline: false,
          isLocal: false,
          requiresReauthorization: false,
          canDownload: true,
          canOnlineRead: false,
          streamKind: null,
        }],
      },
    }
    const downloadList = vi.fn(async () => [{
      ...BASE_TASK,
      mediaItemId: "media-2",
      state: "downloading" as const,
      offlineResourceId: null,
    }])
    const resourceListByMediaItem = vi.fn(async ({ mediaItemId }: { mediaItemId: string }) => (
      resourcesByMediaItem[mediaItemId] ?? EMPTY_RESOURCES
    ))
    const client = clientWith([], EMPTY_RESOURCES, [], downloadList)
    client.resourceListByMediaItem = resourceListByMediaItem

    const projected = await getMediaItemsDownloadInfo(["media-1", "media-2"], client)

    expect(downloadList).toHaveBeenCalledOnce()
    expect(resourceListByMediaItem).toHaveBeenCalledTimes(2)
    expect(projected.get("media-1")).toMatchObject({ status: "idle", canDownload: false })
    expect(projected.get("media-2")).toMatchObject({ status: "queued", canDownload: true })
  })

  it("keeps the real projection of every item whose resource query succeeded", async () => {
    // 批量里的一篇资源查询失败：它只影响自己，其余各篇仍返回真实投影。
    const downloadList = vi.fn(async () => [{
      ...BASE_TASK,
      mediaItemId: "media-2",
      state: "downloading" as const,
      offlineResourceId: null,
    }])
    const resourceListByMediaItem = vi.fn(async ({ mediaItemId }: { mediaItemId: string }) => {
      if (mediaItemId === "media-broken") throw new Error("resource query failed")
      return mediaItemId === "media-2" ? downloadableResources("remote-2") : EMPTY_RESOURCES
    })
    const client = clientWith([], EMPTY_RESOURCES, [], downloadList)
    client.resourceListByMediaItem = resourceListByMediaItem

    const projected = await getMediaItemsDownloadInfo(
      ["media-1", "media-broken", "media-2"],
      client,
    )

    expect(resourceListByMediaItem).toHaveBeenCalledTimes(3)
    expect(projected.get("media-2")).toMatchObject({
      status: "queued",
      canDownload: true,
      canOnlineRead: false,
      sourceResourceId: "remote-2",
      taskId: "task-1",
    })
    expect(projected.get("media-1")).toMatchObject({ status: "idle", canDownload: false })
  })

  it("leaves a failed item out of the batch instead of inventing an unavailable one", async () => {
    // 缺失 = 「这次没拿到这一篇的事实」。把它写成零能力的不可用投影，就是拿缺失当结论：
    // 调用者既看不出该重试，也读不到「这一篇没有资源」以外的任何事实。
    const client = clientWith([])
    client.resourceListByMediaItem = vi.fn(async ({ mediaItemId }: { mediaItemId: string }) => {
      if (mediaItemId === "media-broken") throw new Error("resource query failed")
      return EMPTY_RESOURCES
    })

    const projected = await getMediaItemsDownloadInfo(["media-1", "media-broken"], client)

    expect(projected.has("media-broken")).toBe(false)
    expect(projected.get("media-broken")).toBeUndefined()
    expect([...projected.keys()]).toEqual(["media-1"])
    // 空投影只能来自真实条目（这里 media-1 就是一条），不能用来补缺失的那一篇。
    expect(projected.get("media-1")).toMatchObject({
      status: "idle",
      canDownload: false,
      hasOfflineResource: false,
      canOnlineRead: false,
      sourceResourceId: null,
    })
  })

  it("still rejects the single-item helper when its resource query fails", async () => {
    // 单篇调用必须保留 reject 语义：调用者要能识别失败并重试，而不是收到一份
    // 「这一篇什么都没有」的明确结论。
    const client = clientWith([])
    client.resourceListByMediaItem = vi.fn(async () => {
      throw new Error("resource query failed")
    })

    await expect(getMediaItemDownloadInfo("media-1", client)).rejects.toThrow("resource query failed")
  })

  it("still rejects the whole batch when the shared download list fails", async () => {
    // 共享的任务扫描不是「某一篇的失败」：没有它就没有任何一篇的投影依据。
    const downloadList = vi.fn(async () => {
      throw new Error("download list failed")
    })
    const client = clientWith([], EMPTY_RESOURCES, [], downloadList)

    await expect(getMediaItemsDownloadInfo(["media-1", "media-2"], client))
      .rejects.toThrow("download list failed")
  })
})
