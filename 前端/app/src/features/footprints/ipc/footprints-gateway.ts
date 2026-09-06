// Footprints Gateway（足迹页唯一数据通道，禁止散落 invoke）。
// 环境分流（对齐 library gateway 裁决）：
// - 浏览器 dev = 演示环境 → 既有 localStorage / mock 演示投影。
// - Tauri WebView = 生产环境 → 真实 progress_recent / history_list / marker_list / favorite。
//
// 收藏：library_list 过滤 WorkCardDto.favorite（收藏本体在 SQLite）。
// 继续/最近：progress_recent（首页 Continue 数据源）+ history_list（最近活动）。
//   这俩 DTO 只含 mediaItemId/workId/editionId，不含标题/封面；展示需联查 WorkCard 投影。
//   为避免 N+1 且零新 IPC，gateway 内部一次性拉取完整 library_list（loadAllLibraryPages 复用）
//   构建 mediaItemId → WorkCard 索引，继续/最近活动据此补全展示信息。
// 书签：marker_list（按 MediaItem 列出未软删除标记）。

import { getHavenClient, getHavenClientMode } from "@/lib/ipc/runtime"
import { artworkRequestUri } from "@/lib/artwork-url"
import { pickCardImage } from "@/lib/artwork-policy"
import { defaultCoverCategoryForMediaType } from "@/lib/default-cover"

import type {

  HistoryEntryDto,
  MarkerDto,
  PageDto,
  ProgressSummaryDto,
  WorkCardDto,
} from "@/lib/ipc/generated/wire"
import type { MediaCardProps } from "@/components/ui/haven/MediaCard"
import type { PrimaryActionDto } from "@/lib/ipc/generated/wire"

import { getCatalogItems, getStoredFavoriteIds } from "@/lib/havenState"


/** 首屏拉取上限：与 library gateway 一致，分页/游标随 IPC-FE-002 落地。 */
const LIST_LIMIT = 200

/** 拉取完整 WorkCard 投影 cursor 链（复用 library gateway 的 listRequest 形状）。
 *  返回原始 WorkCardDto（含 progress.mediaItemId，供继续/最近活动联查）。 */
async function loadAllWorkCards(): Promise<WorkCardDto[]> {
  const client = getHavenClient()
  const items: WorkCardDto[] = []
  const seen = new Set<string>()
  let cursor: string | null = null
  for (;;) {
    const page: PageDto<WorkCardDto> = await client.libraryList({
      category: "all",
      mediaTypes: null,
      query: null,
      sort: "recently_added",
      cursor,
      limit: LIST_LIMIT,
    })
    items.push(...page.items)
    if (page.nextCursor === null) return items
    if (seen.has(page.nextCursor)) return items // 循环 cursor 协议保护
    seen.add(page.nextCursor)
    cursor = page.nextCursor
  }
}

const CATEGORY_LABELS: Record<string, string> = {
  video: "影视",
  book: "图书",
  comic: "漫画",
  article: "文章",
}

function categoryLabel(card: WorkCardDto): string {
  return CATEGORY_LABELS[card.categories[0]] ?? card.categories[0] ?? "媒体"
}

export type FootprintActionCard = MediaCardProps & {
  workId: string
  mediaItemId: string | null
  primaryAction: PrimaryActionDto | null
  favorite: boolean
}

function toMediaCard(card: WorkCardDto): FootprintActionCard {
  return {
    id: card.workId,
    workId: card.workId,
    mediaItemId: card.primaryAction?.mediaItemId ?? null,
    primaryAction: card.primaryAction,
    favorite: card.favorite,
    title: card.title,
    subtitle: `已收藏 · ${categoryLabel(card)}`,
    typeBadge: categoryLabel(card),
    imageUrl: artworkRequestUri(card.posterUri),
    artworkCategory: defaultCoverCategoryForMediaType(card.categories[0]),
  }
}

/** 拉取「我的喜爱 · 收藏」投影：真实收藏（浏览器为 localStorage 演示收藏）。 */
export async function getFavoriteFootprintItems(): Promise<FootprintActionCard[]> {
  const mode = getHavenClientMode()
  if (mode === "mock") {
    return getCatalogItems(getStoredFavoriteIds()).map((item) => ({
      id: item.id,
      workId: item.id,
      mediaItemId: null,
      primaryAction: null,
      favorite: true,
      title: item.title,
      subtitle: `已收藏 · ${item.badge || "已收藏"}`,
      typeBadge: item.badge,
      imageUrl: item.imageUrl,
      artworkCategory: defaultCoverCategoryForMediaType(item.type),
    }))
  }
  if (mode !== "tauri") return []
  const page: PageDto<WorkCardDto> = await getHavenClient().libraryList({
    category: "all",
    mediaTypes: null,
    query: null,
    sort: "recently_added",
    cursor: null,
    limit: LIST_LIMIT,
  })
  return page.items.filter((card) => card.favorite).map(toMediaCard)
}

const PROGRESS_LABEL: Record<string, string> = {
  video: "继续观看",
  book: "继续阅读",
  comic: "继续阅读",
  article: "继续阅读",
}

/** mediaItemId / workId → WorkCard 索引：progress.mediaItemId 与 workId 双键，便于继续/历史联查（零新 IPC）。 */
function buildMediaItemIndex(cards: WorkCardDto[]): Map<string, WorkCardDto> {
  const index = new Map<string, WorkCardDto>()
  for (const card of cards) {
    if (card.progress) index.set(card.progress.mediaItemId, card)
    index.set(card.workId, card)
  }
  return index
}

function deriveMediaType(card: WorkCardDto): string {
  const media = card.availableMediaTypes
  if (media.includes("movie") || media.includes("series") || media.includes("episode")) return "video"
  if (media.includes("book") || media.includes("document")) return "book"
  if (media.includes("comic")) return "comic"
  if (media.includes("article")) return "article"
  return card.categories[0] ?? "video"
}

function progressToCard(p: ProgressSummaryDto, card: WorkCardDto | undefined): FootprintActionCard | null {
  if (!card) return null
  const mediaType = deriveMediaType(card)
  return {
    id: card.workId,
    workId: card.workId,
    mediaItemId: p.mediaItemId,
    primaryAction: card.primaryAction,
    favorite: card.favorite,
    title: card.title,
    subtitle: PROGRESS_LABEL[mediaType] ?? "继续",
    typeBadge: categoryLabel(card),
    layout: "landscape",
    progress:
      p.progressRatio != null
        ? Math.round(p.progressRatio * 100)
        : p.completion === "in_progress"
          ? 1
          : undefined,
    imageUrl: p.keyframeUri ?? pickCardImage(card),
    artworkCategory: defaultCoverCategoryForMediaType(mediaType),
    description: card.description ?? "",
    releaseYear: card.releaseYear ?? undefined,
  }
}

export type HistoryCardProps = MediaCardProps & {
  lastActiveAt: string
  workId: string
  mediaItemId: string | null
  primaryAction: PrimaryActionDto | null
}

function historyToCard(
  entry: HistoryEntryDto,
  card: WorkCardDto | undefined,
  _progress?: ProgressSummaryDto | null,
): HistoryCardProps | null {
  if (!card) return null
  // 浏览记录一律海报，不取关键帧（产品要求）
  const p = card.progress
  return {
    id: card.workId,
    workId: card.workId,
    mediaItemId: entry.mediaItemId,
    primaryAction: card.primaryAction,
    title: card.title,
    subtitle: `最近活动 · ${categoryLabel(card)}`,
    typeBadge: categoryLabel(card),
    layout: "landscape",
    progress:
      p?.progressRatio != null
        ? Math.round(p.progressRatio * 100)
        : p?.completion === "in_progress"
          ? 1
          : undefined,
    imageUrl: pickCardImage(card),
    artworkCategory: defaultCoverCategoryForMediaType(card.categories[0]),
    lastActiveAt: entry.lastActiveAt,
  }
}

/** 拉取「继续」分组：progress_recent 联查 WorkCard 投影（浏览器演示环境返回空，由页面兜底 mock）。
 *  去重：同一 work 仅保留最新一条（progress_recent 已按 updatedAt 倒序），避免多集同一作品刷屏。 */
export async function getContinueFootprintItems(): Promise<FootprintActionCard[]> {
  if (getHavenClientMode() !== "tauri") return []
  const [progressItems, cards] = await Promise.all([
    getHavenClient().progressRecent({ limit: LIST_LIMIT }),
    loadAllWorkCards(),
  ])
  const index = buildMediaItemIndex(cards)
  const seenWork = new Set<string>()
  const result: FootprintActionCard[] = []
  for (const p of progressItems) {
    const card = index.get(p.mediaItemId)
    if (!card) continue
    if (seenWork.has(card.workId)) continue
    seenWork.add(card.workId)
    const mapped = progressToCard(p, card)
    if (mapped) result.push(mapped)
  }
  return result
}

/** 拉取「最近活动」分组：history_list 联查 WorkCard 投影（浏览器演示环境返回空，由页面兜底 mock）。
 *  浏览记录一律海报，不取关键帧。 */
export async function getRecentActivityFootprintItems(): Promise<HistoryCardProps[]> {
  if (getHavenClientMode() !== "tauri") return []
  const [historyItems, cards] = await Promise.all([
    getHavenClient().historyList({ limit: LIST_LIMIT }),
    loadAllWorkCards(),
  ])
  const index = buildMediaItemIndex(cards)
  return historyItems
    .map((entry) => historyToCard(entry, index.get(entry.mediaItemId) ?? index.get(entry.workId)))
    .filter((item): item is HistoryCardProps => item !== null)
}

/** 清空历史（契约 §23.2：只清历史，不动 Progress/Favorite/Marker）。 */
export async function clearFootprintHistory(): Promise<void> {
  if (getHavenClientMode() !== "tauri") return
  await getHavenClient().historyClear()
}

/** 重置某 MediaItem 的进度（契约 §23.2：业务操作，不删实体）。 */
export async function resetFootprintProgress(mediaItemId: string): Promise<void> {
  if (getHavenClientMode() !== "tauri") return
  await getHavenClient().progressReset({ mediaItemId })
}

/** 拉取「书签」分组：marker_list_all（浏览器演示环境返回空，由页面兜底 mock）。
 *  返回原始 MarkerDto，页面自行映射为 MarkerItem（mode 由 locator kind 派生）。 */
export async function getMarkerFootprintItems(): Promise<MarkerDto[]> {
  if (getHavenClientMode() !== "tauri") return []
  return getHavenClient().markerListAll({ limit: LIST_LIMIT })
}

/** 删除标记（契约 §23.2：软删除墓碑语义）。 */
export async function deleteFootprintMarker(markerId: string): Promise<boolean> {
  if (getHavenClientMode() !== "tauri") return false
  return getHavenClient().markerDelete({ markerId })
}

/** 由 MarkerDto.locator.kind 派生足迹页书签 mode（阅读/播放/漫画/文章）。 */
export function markerModeFromLocator(marker: MarkerDto): "book" | "comic" | "article" | "video" {
  // locator 是联合类型；kind 字段在 wire 中是字面量。运行时取 (marker.locator as {kind:string}).kind。
  const kind = (marker.locator as { kind?: string }).kind
  switch (kind) {
    case "video":
      return "video"
    case "comic":
      return "comic"
    case "article":
      return "article"
    default:
      return "book"
  }
}

/** 足迹页书签卡片形状（与页面 MarkerItem 对齐，避免在页面里拼 IPC 参数）。 */
export interface FootprintMarkerCard {
  id: string
  mode: string
  mediaItemId: string
  title: string
  subtitle: string
  type: string
  imageUrl: string
  artworkCategory: "video" | "book" | "comic" | "article"
}

const MARKER_MODE_LABEL: Record<string, string> = {
  video: "播放",
  book: "阅读",
  comic: "漫画",
  article: "文章",
}

/** 拉取「书签」分组并联查 WorkCard 投影补全标题/封面（浏览器演示环境返回空，由页面兜底 mock）。 */
export async function getMarkerFootprintCards(): Promise<FootprintMarkerCard[]> {
  if (getHavenClientMode() !== "tauri") return []
  const [markers, cards] = await Promise.all([
    getHavenClient().markerListAll({ limit: LIST_LIMIT }),
    loadAllWorkCards(),
  ])
  // marker.workId 直接匹配 WorkCard.workId（无需经 progress.mediaItemId 反查）。
  const workById = new Map<string, WorkCardDto>()
  for (const card of cards) workById.set(card.workId, card)
  return markers.map((marker) => {
    const mode = markerModeFromLocator(marker)
    const card = workById.get(marker.workId)
    const imageUrl = card ? pickCardImage(card) : ""
    return {
      id: marker.markerId,
      mode,
      mediaItemId: marker.mediaItemId,
      title: card?.title ?? "媒体条目",
      subtitle: marker.title ?? "在阅读 / 播放时标记的位置",
      type: MARKER_MODE_LABEL[mode] ?? "阅读",
      imageUrl,
      artworkCategory: defaultCoverCategoryForMediaType(mode),
    }
  })
}
