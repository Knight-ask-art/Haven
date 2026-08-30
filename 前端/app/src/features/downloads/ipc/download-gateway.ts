import { HavenError } from "@/lib/ipc/errors"
import { getHavenClient } from "@/lib/ipc/runtime"
import type { DownloadEvent, DownloadTaskDto } from "@/lib/ipc/generated/wire"

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

export async function createDownloadForMediaItem(mediaItemId: string): Promise<DownloadTaskDto> {
  const client = getHavenClient()
  const [resources, storageLocations] = await Promise.all([
    client.resourceListByMediaItem({ mediaItemId }),
    client.storageLocationList(),
  ])
  const source = resources.items.find((item) => (
    !item.isOffline
    && item.isLocal
    && (item.availability === "available" || item.availability === "offline_available")
  ))
  if (!source) {
    throw new HavenError({
      code: "DOWNLOAD_SOURCE_UNAVAILABLE",
      userMessage: "当前内容没有可保存到离线位置的本地资源",
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

export async function getWorkDownloadState(
  workId: string,
  mediaItemId?: string,
  client = getHavenClient(),
): Promise<"idle" | "queued" | "downloaded"> {
  const tasks = await client.downloadList({ limit: 200 })
  const task = tasks.find((item) => (
    item.workId === workId
    && item.state !== "cancelled"
    && (item.state !== "completed" || item.offlineResourceId !== null)
  ))
  if (task) return task.state === "completed" ? "downloaded" : "queued"
  if (!mediaItemId) return "idle"
  const resources = await client.resourceListByMediaItem({ mediaItemId })
  return resources.items.some((item) => (
    item.isOffline && item.availability === "offline_available"
  )) ? "downloaded" : "idle"
}
