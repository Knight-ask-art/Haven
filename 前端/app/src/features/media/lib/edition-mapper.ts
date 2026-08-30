import type { PrimaryActionDto } from "@/lib/ipc/generated/wire"
import type { EditionListByWorkResultDto, EditionSummaryDto } from "../ipc/edition-wire"
import type { MediaDetailData } from "../pages/MediaDetailPage"

export interface EditionListItem {
  id: string
  number: string
  title: string
  durationOrPages: string
  progress?: number
  primaryAction: PrimaryActionDto | null
  mediaType: string
}

export interface EditionGroup {
  mediaType: string
  label: string
  items: EditionListItem[]
}

const MEDIA_TYPE_LABELS: Record<string, string> = {
  movie: "电影",
  series: "剧集",
  episode: "单集",
  book: "图书",
  document: "资料",
  comic: "漫画",
  article: "文章",
  audio: "音频",
  unknown: "未知",
}

export function mediaTypeLabel(mediaType: string): string {
  return MEDIA_TYPE_LABELS[mediaType] ?? mediaType
}

/** 固定展示顺序的媒体类型分组（“分页后分组”：只分组当前已加载的一页，O(n)）。 */
export function partitionEditionItems(items: readonly EditionListItem[]): EditionGroup[] {
  const order = ["movie", "series", "episode", "book", "document", "comic", "article", "audio", "unknown"]
  const groups = new Map<string, EditionListItem[]>()
  for (const item of items) {
    const list = groups.get(item.mediaType)
    if (list) list.push(item)
    else groups.set(item.mediaType, [item])
  }
  const out: EditionGroup[] = []
  for (const mediaType of order) {
    const groupItems = groups.get(mediaType)
    if (groupItems && groupItems.length > 0) {
      out.push({ mediaType, label: mediaTypeLabel(mediaType), items: groupItems })
    }
  }
  for (const [mediaType, groupItems] of groups) {
    if (!order.includes(mediaType)) {
      out.push({ mediaType, label: mediaTypeLabel(mediaType), items: groupItems })
    }
  }
  return out
}

function formatEditionMeta(edition: EditionSummaryDto): string {
  const facts = [edition.releaseDate, edition.language, edition.region]
    .filter((value): value is string => value !== null && value !== undefined && value !== "")
  return facts.length > 0 ? facts.join(" · ") : `${edition.mediaItemCount} 个可打开项`
}

/** Maps only server facts; no media item or primary action is invented here. */
export function mapEditionSummaryToDetailItem(edition: EditionSummaryDto): EditionListItem {
  const ratio = edition.progress?.progressRatio
  return {
    id: edition.editionId,
    number: edition.mediaType,
    title: edition.subtitle ? `${edition.title} · ${edition.subtitle}` : edition.title,
    durationOrPages: formatEditionMeta(edition),
    progress: ratio == null ? undefined : ratio * 100,
    primaryAction: edition.primaryAction,
    mediaType: edition.mediaType,
  }
}

export function mapEditionListToDetailItems(result: EditionListByWorkResultDto): EditionListItem[] {
  return result.items.map(mapEditionSummaryToDetailItem)
}

export function toMediaDetailEpisodes(items: readonly EditionListItem[]): NonNullable<MediaDetailData["episodesOrChapters"]> {
  return items.map((item) => ({
    id: item.id,
    number: item.number,
    title: item.title,
    durationOrPages: item.durationOrPages,
    progress: item.progress,
    primaryAction: item.primaryAction,
  }))
}

export function isEditionListByWorkResult(value: unknown): value is EditionListByWorkResultDto {
  if (typeof value !== "object" || value === null) return false
  const result = value as Record<string, unknown>
  return (
    result.schemaVersion === 1 &&
    Array.isArray(result.items) &&
    (typeof result.nextCursor === "string" || result.nextCursor === null) &&
    (typeof result.total === "number" || result.total === null) &&
    (typeof result.revision === "string" || result.revision === null)
  )
}
