// 选集抽屉的动作 API：收藏（Work Header）与每集下载（下载投影）。
//
// 组件只拿这里的结果与回调；IPC、事件订阅与写入顺序都留在本模块。

import { HavenError, toHavenError } from "@/lib/ipc/errors"
import type { DownloadEvent } from "@/lib/ipc/generated/wire"
import { onFavoriteChanged, setFavorite } from "@/features/media/ipc/favorite-gateway"
import { settleFavoriteMutation } from "@/features/media/lib/favorite-mutation"
import { getWorkDetail } from "@/features/media/ipc/work-gateway"
import {
  createDownloadForMediaItem,
  getMediaItemsDownloadInfo,
  subscribeDownloadEvents,
  type MediaItemDownloadInfo,
} from "@/features/downloads/ipc/download-gateway"
import { downloadErrorMessage } from "@/features/downloads/lib/download-error"
import {
  type EpisodeDownloadView,
  canStartEpisodeDownload,
  viewFromDownloadInfo,
} from "./episode-actions-state"

export interface WorkFavoriteTruth {
  workId: string
  favorite: boolean
}

/** 读取权威收藏：Work Detail Header 的 `favorite` 由服务端给出。 */
export async function readWorkFavorite(workId: string): Promise<WorkFavoriteTruth> {
  const header = await getWorkDetail(workId)
  return { workId, favorite: header.favorite }
}

export interface FavoriteWriteResult {
  favorite: boolean
  error: HavenError | null
}

/**
 * 写入收藏。失败时回滚到 `previous` 并把错误交回调用方展示；
 * 成功时以 `favoriteSet` 响应的 `favorite` 为准（与 favorite-changed 同源）。
 */
export async function writeWorkFavorite(
  workId: string,
  favorite: boolean,
  previous: boolean,
): Promise<FavoriteWriteResult> {
  const settled = await settleFavoriteMutation(previous, () => setFavorite({ workId, favorite }))
  return {
    favorite: settled.favorite,
    error: settled.error === null ? null : toHavenError(settled.error),
  }
}

/**
 * 订阅 favorite-changed；订阅失败（例如事件通道不可用）不抛给页面，
 * 收藏仍可经 `writeWorkFavorite` 直接写入。退订由订阅方负责。
 */
export function subscribeFavoriteChanged(
  onChanged: (change: WorkFavoriteTruth) => void,
): Promise<() => void> {
  return onFavoriteChanged((payload) => onChanged({ workId: payload.workId, favorite: payload.favorite }))
    .then((unlisten) => () => unlisten())
    .catch(() => () => undefined)
}

export type EpisodeDownloadViews = Map<string, EpisodeDownloadView>

/** 一次性读取多集的真实下载投影；没有读到投影的集不出现在结果里。 */
export async function readEpisodeDownloadViews(mediaItemIds: readonly string[]): Promise<EpisodeDownloadViews> {
  const projected = await getMediaItemsDownloadInfo(mediaItemIds)
  const views: EpisodeDownloadViews = new Map()
  for (const [mediaItemId, info] of projected) views.set(mediaItemId, viewFromDownloadInfo(info))
  return views
}

/** 下载事件只给出 taskId；没有归属的事件不据此改动任何一集。退订本身是异步 IPC。 */
export function subscribeEpisodeDownloadEvents(
  onTaskEvent: (event: DownloadEvent) => void,
): Promise<() => Promise<void>> {
  return subscribeDownloadEvents(onTaskEvent)
}

export type EpisodeDownloadStartResult =
  | { outcome: "started"; mediaItemId: string; taskId: string }
  | { outcome: "already"; mediaItemId: string; state: "downloaded" | "queued"; taskId: string | null }
  | { outcome: "blocked"; mediaItemId: string; reason: string }
  | { outcome: "failed"; mediaItemId: string; message: string }

/**
 * 为一集创建真实下载任务。
 *
 * 创建前按点击那一刻的后端投影复核：没有可下载来源就不发起；已下载 / 已排队
 * 不重复创建。创建成功只代表任务已排队，返回 `started`，由调用方重新读投影。
 */
export async function startEpisodeDownload(mediaItemId: string): Promise<EpisodeDownloadStartResult> {
  let info: MediaItemDownloadInfo | null = null
  try {
    info = (await getMediaItemsDownloadInfo([mediaItemId])).get(mediaItemId) ?? null
  } catch {
    // 没读到投影不等于没有来源：交给 gateway 按同一规则重新选源并给出明确失败。
    info = null
  }
  if (info !== null) {
    if (info.status === "downloaded") return { outcome: "already", mediaItemId, state: "downloaded", taskId: null }
    if (info.status === "queued") return { outcome: "already", mediaItemId, state: "queued", taskId: info.taskId }
  }
  if (!canStartEpisodeDownload(info)) {
    return { outcome: "blocked", mediaItemId, reason: "当前剧集没有可保存到离线位置的资源" }
  }
  try {
    const task = await createDownloadForMediaItem(mediaItemId)
    return { outcome: "started", mediaItemId, taskId: task.taskId }
  } catch (cause) {
    return { outcome: "failed", mediaItemId, message: downloadStartFailureMessage(cause) }
  }
}

function downloadStartFailureMessage(cause: unknown): string {
  const error = toHavenError(cause)
  if (error.code === "DOWNLOAD_SOURCE_UNAVAILABLE" || error.code === "DOWNLOAD_TARGET_UNAVAILABLE") {
    return error.dto.userMessage
  }
  if (error.code.startsWith("DOWNLOAD_")) return downloadErrorMessage(error.code)
  return error.dto.userMessage
}
