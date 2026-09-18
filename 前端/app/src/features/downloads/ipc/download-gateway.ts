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

const DOWNLOAD_PAGE_SIZE = 200

export async function listDownloads(client = getHavenClient()): Promise<DownloadTaskDto[]> {
  const tasks: DownloadTaskDto[] = []
  for (let offset = 0; ; offset += DOWNLOAD_PAGE_SIZE) {
    const page = await client.downloadList({ limit: DOWNLOAD_PAGE_SIZE, offset })
    tasks.push(...page)
    if (page.length < DOWNLOAD_PAGE_SIZE) return tasks
  }
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
 *
 * A failed resource query stays a failure and rejects the call. Answering with
 * an all-false projection would turn "we did not get an answer" into the
 * verdict "this MediaItem has no resource", which the caller cannot retry out
 * of. Batch isolation (below) deliberately does not soften this path.
 */
export async function getMediaItemDownloadInfo(
  mediaItemId: string,
  client = getHavenClient(),
): Promise<MediaItemDownloadInfo> {
  const [tasks, resources] = await Promise.all([
    listDownloads(client),
    client.resourceListByMediaItem({ mediaItemId }),
  ])
  return projectMediaItemDownloadInfo(mediaItemId, tasks, resources)
}

/**
 * Read capability projections for several MediaItems while sharing the
 * complete download-task scan. Edition pages often contain many rows and
 * `listDownloads` is paginated, so doing that scan once is materially cheaper
 * than calling the single-item helper in a loop.
 *
 * Each MediaItem is isolated: one row whose resource query fails must not turn
 * the whole page into a failed batch. Such an item is left **out** of the
 * returned map, so the caller reads a missing key as "no fact for this item"
 * and can offer a retry. It is never filled in with an all-false projection,
 * which would state "this item has no resource" as a fact — and never with a
 * fabricated `unavailable`. The shared task scan is *not* isolated: without
 * `downloadList` there is no projection basis for anyone, so the whole call
 * rejects.
 */
export async function getMediaItemsDownloadInfo(
  mediaItemIds: readonly string[],
  client = getHavenClient(),
): Promise<Map<string, MediaItemDownloadInfo>> {
  const uniqueMediaItemIds = [...new Set(mediaItemIds)]
  if (uniqueMediaItemIds.length === 0) return new Map()

  const [tasks, resources] = await Promise.all([
    listDownloads(client),
    Promise.all(uniqueMediaItemIds.map(async (mediaItemId) => {
      try {
        return await client.resourceListByMediaItem({ mediaItemId })
      } catch {
        return null
      }
    })),
  ])

  const projected = new Map<string, MediaItemDownloadInfo>()
  uniqueMediaItemIds.forEach((mediaItemId, index) => {
    const resourceList = resources[index]
    if (resourceList) projected.set(mediaItemId, projectMediaItemDownloadInfo(mediaItemId, tasks, resourceList))
  })
  return projected
}

function projectMediaItemDownloadInfo(
  mediaItemId: string,
  tasks: DownloadTaskDto[],
  resources: ResourceListDto,
): MediaItemDownloadInfo {
  const offlineResourceIds = new Set(resources.items
    .filter((item) => item.isOffline && item.availability === "offline_available")
    .map((item) => item.resourceId))
  const completedOfflineTask = tasks.find((task) => (
    task.mediaItemId === mediaItemId
    && task.state === "completed"
    && task.offlineResourceId !== null
    && offlineResourceIds.has(task.offlineResourceId)
  ))
  const activeTask = tasks.find((task) => (
    task.mediaItemId === mediaItemId
    && task.state !== "completed"
    && task.state !== "cancelled"
    && task.state !== "failed"
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
        ? "queued"
        : "idle",
    canDownload: source !== null,
    hasOfflineResource: offlineResource,
    canOnlineRead: onlineResource,
    sourceResourceId: source?.resourceId ?? null,
    taskId: offlineResource
      ? completedOfflineTask?.taskId ?? null
      : activeTask?.taskId ?? null,
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
  const tasks = await listDownloads(client)
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
