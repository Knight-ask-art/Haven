import { HavenError } from "@/lib/ipc/errors"
import { getHavenClient } from "@/lib/ipc/runtime"
import type {
  DownloadEvent,
  DownloadTaskDto,
  ResourceListDto,
  ResourceSummaryDto,
} from "@/lib/ipc/generated/wire"

export type DownloadStatus = "idle" | "queued" | "downloaded"

/**
 * 作品详情页需要的最小下载投影。
 *
 * `sourceResourceId` 只留在 Feature 内供 `download_create` 使用，不进入
 * React 页面或通用 Wire。远端 URL、路径和 locator 永远不会跨过 Client。
 */
export interface MediaItemDownloadInfo {
  status: DownloadStatus
  canDownload: boolean
  hasOfflineResource: boolean
  canOnlineRead: boolean
  sourceResourceId: string | null
  taskId: string | null
}

export async function listDownloads(): Promise<DownloadTaskDto[]> {
  return getHavenClient().downloadList({ limit: 200 })
}

export async function pauseDownload(taskId: string): Promise<DownloadTaskDto> {
  return getHavenClient().downloadPause({ taskId })
}

export async function resumeDownload(taskId: string): Promise<DownloadTaskDto> {
  return getHavenClient().downloadResume({ taskId })
}

export async function cancelDownload(taskId: string): Promise<DownloadTaskDto> {
  return getHavenClient().downloadCancel({ taskId })
}

export async function retryDownload(taskId: string): Promise<DownloadTaskDto> {
  return getHavenClient().downloadRetry({ taskId })
}

export async function removeDownloadRecord(taskId: string) {
  return getHavenClient().downloadRemoveRecord({ taskId })
}

export async function deleteOfflineDownload(taskId: string) {
  return getHavenClient().downloadDeleteOffline({ taskId })
}

export async function revealOfflineDownload(taskId: string) {
  return getHavenClient().downloadRevealOffline({ taskId })
}

let subscriptionSequence = 0

export function subscribeDownloadEvents(
  onEvent: (event: DownloadEvent) => void,
): Promise<() => Promise<void>> {
  subscriptionSequence += 1
  const randomId = globalThis.crypto?.randomUUID?.()
  const subscriptionId = randomId ?? `download-${Date.now()}-${subscriptionSequence}`
  return getHavenClient().downloadSubscribe(subscriptionId, onEvent)
}

export async function createDownloadForMediaItem(
  mediaItemId: string,
  client = getHavenClient(),
): Promise<DownloadTaskDto> {
  const [resources, storageLocations] = await Promise.all([
    client.resourceListByMediaItem({ mediaItemId }),
    client.storageLocationList(),
  ])
  const source = selectDownloadSource(resources)
  if (!source) {
    throw new HavenError({
      code: "DOWNLOAD_SOURCE_UNAVAILABLE",
      userMessage: "当前内容没有可保存到离线位置的资源",
      retryable: false,
    })
  }
  const target = storageLocations.find((item) => (
    item.providerType === "local" && item.status === "connected"
  ))
  if (!target) {
    throw new HavenError({
      code: "DOWNLOAD_TARGET_UNAVAILABLE",
      userMessage: "请先在设置中添加可用的本地存储位置",
      retryable: false,
    })
  }
  return client.downloadCreate({
    sourceResourceId: source.resourceId,
    targetStorageId: target.locationId,
  })
}

/**
 * Read the server's safe resource summary for a MediaItem.
 *
 * This is deliberately separate from `createDownloadForMediaItem`: opening a
 * detail page must not create a task or touch the filesystem. `canDownload`
 * is a backend capability projection; this feature never infers it from a
 * locator, `isLocal`, or a remote identity.
 */
export async function getMediaItemDownloadInfo(
  mediaItemId: string,
  client = getHavenClient(),
): Promise<MediaItemDownloadInfo> {
  const [tasks, resources] = await Promise.all([
    client.downloadList({ limit: 200 }),
    client.resourceListByMediaItem({ mediaItemId }),
  ])
  const activeTask = tasks.find((task) => (
    task.mediaItemId === mediaItemId
    && task.state !== "cancelled"
    && task.state !== "failed"
    && (task.state !== "completed" || task.offlineResourceId !== null)
  ))
  const offlineResource = resources.items.some((item) => (
    item.isOffline && item.availability === "offline_available"
  ))
  const onlineResource = resources.items.some((item) => (
    item.canOnlineRead
    && (item.availability === "available" || item.availability === "offline_available")
  ))
  const source = selectDownloadSource(resources)
  return {
    // An existing Offline Resource is always preferred over a stale queued
    // task. This mirrors the resolver's offline-first rule and prevents a
    // duplicate-download affordance after a task has already completed.
    status: offlineResource
      ? "downloaded"
      : activeTask
        ? activeTask.state === "completed" ? "downloaded" : "queued"
        : "idle",
    canDownload: source !== null,
    hasOfflineResource: offlineResource,
    canOnlineRead: onlineResource,
    sourceResourceId: source?.resourceId ?? null,
    taskId: activeTask?.taskId ?? null,
  }
}

export async function getWorkDownloadState(
  workId: string,
  mediaItemId?: string,
  client = getHavenClient(),
): Promise<DownloadStatus> {
  if (mediaItemId) {
    return (await getMediaItemDownloadInfo(mediaItemId, client)).status
  }
  const tasks = await client.downloadList({ limit: 200 })
  const task = tasks.find((item) => (
    item.workId === workId
    && item.state !== "cancelled"
    && item.state !== "failed"
    && (item.state !== "completed" || item.offlineResourceId !== null)
  ))
  if (task) return task.state === "completed" ? "downloaded" : "queued"
  return "idle"
}

/**
 * Select a safe source resource from the backend summary.
 *
 * Offline resources are intentionally excluded: they already satisfy the
 * user's local-open path. The backend's explicit capability is authoritative;
 * the frontend never inspects a locator or remote identity.
 */
function selectDownloadSource(resources: ResourceListDto): ResourceSummaryDto | null {
  return resources.items.find((item) => (
    !item.isOffline
    && item.availability === "available"
    && !item.requiresReauthorization
    && isDownloadableResource(item)
  )) ?? null
}

const STREAM_RESOURCE_TYPES = new Set<ResourceSummaryDto["resourceType"]>([
  "video_stream",
  "hls_stream",
  "dash_stream",
  "remote_stream",
])

function isDownloadableResource(item: ResourceSummaryDto): boolean {
  return item.canDownload && !STREAM_RESOURCE_TYPES.has(item.resourceType)
}
