import { describe, expect, it } from "vitest"

import type { HavenClient } from "@/lib/ipc/client"
import type { DownloadTaskDto, ResourceListDto } from "@/lib/ipc/generated/wire"
import { getWorkDownloadState } from "./download-gateway"

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
): HavenClient {
  const client = {
    downloadList: async () => tasks,
    resourceListByMediaItem: async () => resources,
  } satisfies Pick<HavenClient, "downloadList" | "resourceListByMediaItem">
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
        streamKind: null,
      }],
    }

    await expect(getWorkDownloadState("work-1", "media-1", clientWith([], resources)))
      .resolves.toBe("downloaded")
  })
})
