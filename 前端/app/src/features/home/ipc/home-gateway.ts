// Home Gateway（FE-HOME-001：首页唯一数据通道，禁止散落 invoke）。
// 环境分流（对齐 library gateway 裁决）：
// - 浏览器 dev = 演示环境 → 既有静态首页（HavenStage 无 Continue/Recently 真实投影）。
// - Tauri WebView = 生产环境 → 真实 home_get（Continue + RecentlyAdded + Shelves）。
//
// `home_get` 是聚合 Query：Continue 联查 progress_recent + WorkCard；
// RecentlyAdded 复用 library_list（sort=recently_added）。首页 0.1 不等远程 Source。

import type { ContinueItemDto, HomeDto, WorkCardDto } from "@/lib/ipc/generated/wire"
import { artworkRequestUri } from "@/lib/artwork-url"
import { pickCardImage } from "@/lib/artwork-policy"
import { defaultCoverCategoryForMediaType } from "@/lib/default-cover"

import type { MediaCardProps } from "@/components/ui/haven/MediaCard"

import { getHavenClient, isTauriRuntime } from "@/lib/ipc/runtime"

import { HavenError, toHavenError } from "@/lib/ipc/errors"


const PROGRESS_LABEL: Record<string, string> = {
  video: "继续观看",
  book: "继续阅读",
  comic: "继续阅读",
  article: "继续阅读",
  document: "继续查阅",
}

function deriveMediaType(card: WorkCardDto): string {
  const media = card.availableMediaTypes
  if (media.includes("movie") || media.includes("series") || media.includes("episode")) return "video"
  if (media.includes("book") || media.includes("document")) return "book"
  if (media.includes("comic")) return "comic"
  if (media.includes("article")) return "article"
  return card.categories[0] ?? "video"
}

/** ContinueItem → MediaCard（首页 Continue 分组横向卡片）。 */
export function continueItemToCard(item: ContinueItemDto, cards: WorkCardDto[]): MediaCardProps | null {
  const card = cards.find((c) => c.workId === item.workId)
  if (!card) return null
  const mediaType = deriveMediaType(card)
  return {
    id: item.mediaItemId,
    title: card.title,
    subtitle: PROGRESS_LABEL[mediaType] ?? "继续",
    typeBadge: card.categories[0] ?? "媒体",
    layout: "landscape",
    progress: item.progress.progressRatio != null ? Math.round(item.progress.progressRatio * 100) : undefined,
    imageUrl: pickCardImage(card),
    artworkCategory: defaultCoverCategoryForMediaType(card.categories[0] ?? mediaType),
  }
}

/** WorkCard → MediaCard（首页 RecentlyAdded 分组）。 */
export function workCardToMediaCard(card: WorkCardDto): MediaCardProps {
  return {
    id: card.workId,
    title: card.title,
    subtitle: card.categories[0] ?? "媒体",
    typeBadge: card.categories[0] ?? "媒体",
    imageUrl: artworkRequestUri(card.posterUri),
    artworkCategory: defaultCoverCategoryForMediaType(card.categories[0]),
  }
}

function isHomeDto(value: unknown): value is HomeDto {
  if (typeof value !== "object" || value === null) return false
  const dto = value as Record<string, unknown>
  return dto.schemaVersion === 1 && Array.isArray(dto.continueItems) && Array.isArray(dto.recentlyAdded) && Array.isArray(dto.shelves)
}

/** 拉取首页投影：Tauri 环境真实 home_get；浏览器演示环境返回空（由页面兜底静态展示）。 */
export async function getHomeProjection(): Promise<HomeDto> {
  if (!isTauriRuntime()) {
    return { schemaVersion: 1, continueItems: [], recentlyAdded: [], shelves: [] }
  }
  try {
    const result = await getHavenClient().homeGet()
    if (!isHomeDto(result)) {
      throw new HavenError({ code: "HOME_INVALID_RESPONSE", userMessage: "首页数据不可用，请稍后重试", retryable: false })
    }
    return result
  } catch (error) {
    throw toHavenError(error)
  }
}

export { isTauriRuntime }
