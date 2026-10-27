import { HavenError } from "@/lib/ipc/errors"
import { getHavenClient } from "@/lib/ipc/runtime"
import type {
  DownloadEvent,
  DownloadTaskDto,
  ResourceListDto,
  ResourceSummaryDto,
} from "@/lib/ipc/generated/wire"

/**
 * 作品级下载投影。
 *
 * - `queued`：任务仍在活动（queued / resolving / downloading / verifying），
 *   或刚由本页发起、尚未读到权威任务；按钮保持禁用。
 * - `paused` / `interrupted`：后端保留了同一任务（`.part` 可续传），但不会
 *   自动运行（Interrupted 受 `downloads.autoContinue` 控制）。这是可恢复的
 *   真实状态，不能折叠成 `queued`；对用户的动作是「继续下载」/「重试下载」。
 * - `downloaded`：离线资源已落地。
 */
export type DownloadStatus = "idle" | "queued" | "paused" | "interrupted" | "downloaded"

/**
 * 作品详情页需要的最小下载投影。
 *
 * `sourceResourceId` 只留在 Feature 内供 `download_create` 使用，不进入
 * React 页面或通用 Wire。远端 URL、路径和 locator 永远不会跨过 Client。
 */
export interface MediaItemDownloadInfo {
  /** `paused` / `interrupted` / `queued` already encode the active task state. */
  status: DownloadStatus
  /**
   * Optional raw backend task state behind `status` (`null` for idle / just-
   * requested queued before the authoritative projection is visible).
   */
  taskState?: DownloadTaskDto["state"] | null
  canDownload: boolean
  hasOfflineResource: boolean
  canOnlineRead: boolean
  sourceResourceId: string | null
  taskId: string | null
  /**
   * The Offline Resource backing `hasOfflineResource`.
   *
   * This survives removal of the completed DownloadTask record, so resource
   * reveal/delete do not depend on a task row that the user may have removed
   * from the downloads list.
   */
  offlineResourceId: string | null
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

/**
 * Resource-level offline management.
 *
 * A completed task record can be removed while its Offline Resource remains;
 * these actions use the Resource identity only.
 */
export async function deleteOfflineResource(resourceId: string) {
  return getHavenClient().downloadDeleteOfflineResource({ resourceId })
}

export async function revealOfflineResource(resourceId: string) {
  return getHavenClient().downloadRevealOfflineResource({ resourceId })
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

/**
 * 每项内容当前可执行的下载动作。
 *
 * - `download`：没有任务，或没有活动任务但可重新开始（后端 create 会新建任务）。
 * - `resume`：后端持有同一 Paused / Interrupted 任务，只能经 `download_resume`
 *   继续（`create` 会因 find_active 返回旧任务，并不会启动它）。
 * - `retry`：后端持有同一 Interrupted 任务，`download_retry` 重新排队。
 * - `none`：已下载 / 正在排队下载，没有可发起的动作。
 */
export type MediaItemDownloadAction = "download" | "resume" | "retry" | "none"

/** ACTIVE / RESUME / RETRY / NONE presentation of an authoritative projection. */
export interface MediaItemDownloadActionPresentation {
  kind: MediaItemDownloadAction
  /** Actionable label shown on the primary download control. */
  label: string
  /** Explanation shown next to the control; `null` for plain download / none. */
  hint: string | null
  /** `queued` / `downloading` style states must stay disabled. */
  disabled: boolean
}

/** Pure, exhaustive mapping from projection status to the user-facing action. */
export function presentMediaItemDownloadAction(status: DownloadStatus): MediaItemDownloadActionPresentation {
  switch (status) {
    case "idle":
      return { kind: "download", label: "下载至本地", hint: null, disabled: false }
    case "queued":
      return { kind: "none", label: "下载中", hint: "已加入下载列表", disabled: true }
    case "paused":
      return { kind: "resume", label: "继续下载", hint: "下载已暂停，可继续", disabled: false }
    case "interrupted":
      return { kind: "retry", label: "重试下载", hint: "下载已中断，可继续或重试", disabled: false }
    case "downloaded":
      return { kind: "none", label: "已下载", hint: null, disabled: true }
  }
}

/**
 * Explicit user download intent (button click).
 *
 * `mediaItemId` is the caller's actual MediaItem identity (pages / hooks know
 * it); the projection deliberately does not carry a redundant copy.
 *
 * A recoverable task (Paused / Interrupted) is resumed / retried **by its own
 * task id**: `download_create` would only hand back the existing active task
 * without starting it. A failed resume / retry propagates as-is — it never
 * implicitly creates a different task. Only an idle projection creates a new
 * task. Reading a page never calls this.
 */
export async function startDownloadFromProjection(
  info: Pick<MediaItemDownloadInfo, "status" | "taskId">,
  mediaItemId: string,
  client = getHavenClient(),
): Promise<DownloadTaskDto> {
  if (info.status === "queued" || info.status === "downloaded") {
    throw new HavenError({
      code: "DOWNLOAD_ALREADY_ACTIVE",
      userMessage: info.status === "downloaded" ? "该内容已有离线内容" : "该内容正在下载",
      retryable: false,
    })
  }
  if (info.status === "paused" || info.status === "interrupted") {
    if (!info.taskId) {
      throw new HavenError({ code: "DOWNLOAD_TASK_UNAVAILABLE", userMessage: "下载任务身份暂不可用，请刷新后重试", retryable: true })
    }
    return info.status === "paused"
      ? client.downloadResume({ taskId: info.taskId })
      : client.downloadRetry({ taskId: info.taskId })
  }
  return createDownloadForMediaItem(mediaItemId, client)
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
  const task = await client.downloadCreate({
    sourceResourceId: source.resourceId,
    targetStorageId: target.locationId,
  })
  // 创建接口可能幂等地返回刚被暂停/中断的旧任务；用户的本次下载意图仍须真实启动它。
  if (task.state === "paused") return client.downloadResume({ taskId: task.taskId })
  if (task.state === "interrupted") return client.downloadRetry({ taskId: task.taskId })
  return task
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
  const offlineItems = resources.items.filter((item) => (
    item.isOffline && item.availability === "offline_available"
  ))
  const offlineResourceIds = new Set(offlineItems.map((item) => item.resourceId))
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
  // Prefer the resource named by the completed task; otherwise the first
  // explicit Offline Resource is still a valid resource-level handle.
  const offlineResource = offlineItems.find((item) => (
    item.resourceId === completedOfflineTask?.offlineResourceId
  )) ?? offlineItems[0] ?? null
  const offlineResourcePresent = offlineResource !== null
  const onlineResource = resources.items.some((item) => (
    item.canOnlineRead
    && (item.availability === "available" || item.availability === "offline_available")
  ))
  const source = selectDownloadSource(resources)
  const activeState: DownloadStatus = activeTask === undefined
    ? "idle"
    : activeTask.state === "paused"
      ? "paused"
      : activeTask.state === "interrupted"
        ? "interrupted"
        : "queued"
  return {
    // An existing Offline Resource is always preferred over a stale queued
    // task. This mirrors the resolver's offline-first rule and prevents a
    // duplicate-download affordance after a task has already completed.
    status: offlineResourcePresent ? "downloaded" : activeState,
    // Paused / Interrupted tasks are not running; the page must not present
    // them as downloading and must offer resume / retry on the same task id.
    taskState: offlineResourcePresent
      ? completedOfflineTask?.state ?? null
      : activeTask?.state ?? null,
    canDownload: source !== null,
    hasOfflineResource: offlineResourcePresent,
    canOnlineRead: onlineResource,
    sourceResourceId: source?.resourceId ?? null,
    taskId: offlineResourcePresent
      ? completedOfflineTask?.taskId ?? null
      : activeTask?.taskId ?? null,
    offlineResourceId: offlineResource?.resourceId ?? null,
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
  if (task) {
    if (task.state === "completed") return "downloaded"
    if (task.state === "paused") return "paused"
    if (task.state === "interrupted") return "interrupted"
    return "queued"
  }
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
