import type { ContentCategory, MediaTypeDto, PrimaryActionDto } from "@/lib/ipc/generated/wire"
import { primaryActionRoute } from "./primary-action-route"
import type { MediaItemDownloadInfo } from "@/features/downloads/ipc/download-gateway"

const CONTENT_CATEGORY_LABELS: Record<string, string> = {
  video: "影视",
  book: "图书",
  comic: "漫画",
  periodical: "报刊资料",
  document: "资料",
  article: "文章",
}

const MEDIA_TYPE_LABELS: Record<string, string> = {
  movie: "电影",
  tv: "剧集",
  series: "剧集",
  episode: "单集",
  book: "图书",
  document: "资料",
  comic: "漫画",
  article: "文章",
  audio: "音频",
  unknown: "未知",
}

/** ContentCategory is the product grouping; it must not be inferred from a reader kind. */
export function contentCategoryLabel(category: ContentCategory | string | null | undefined): string {
  if (!category) return "媒体"
  return CONTENT_CATEGORY_LABELS[category] ?? category
}

/** MediaType describes the concrete variant inside a content category. */
export function mediaTypeLabel(mediaType: MediaTypeDto | string): string {
  return MEDIA_TYPE_LABELS[mediaType] ?? mediaType
}

/** Periodical documents/articles keep their concrete media type while gaining a product label. */
export function mediaPresentationLabel(
  mediaType: MediaTypeDto | string,
  category?: ContentCategory | string | null,
): string {
  if (category === "periodical" && mediaType === "document") return "报刊资料"
  if (category === "periodical" && mediaType === "article") return "报刊文章"
  return mediaTypeLabel(mediaType)
}

export interface PrimaryActionPresentation {
  kind: PrimaryActionDto["kind"] | "none"
  label: string
  route: string | null
}

/** Route generation remains centralized in primaryActionRoute; this function adds display semantics only. */
export function primaryActionPresentation(
  action: PrimaryActionDto | null | undefined,
): PrimaryActionPresentation {
  if (!action) return { kind: "none", label: "当前内容暂不可用", route: null }

  const label = action.kind === "playback"
    ? "在线播放"
    : action.kind === "open_edition"
      ? "查看版本"
      : "在线阅读"

  return {
    kind: action.kind,
    label,
    route: primaryActionRoute(action),
  }
}

export type MediaItemCapabilityState =
  | "loading"
  | "error"
  | "offline"
  | "queued"
  | "online"
  | "download_only"
  | "unavailable"

export interface MediaItemCapabilityPresentation {
  state: MediaItemCapabilityState
  label: string
}

/** Convert the backend capability projection to stable user-facing status text. */
export function resolveMediaItemCapability(
  info: Pick<MediaItemDownloadInfo, "status" | "canDownload" | "hasOfflineResource" | "canOnlineRead"> | null | undefined,
  failed = false,
): MediaItemCapabilityPresentation {
  if (failed) return { state: "error", label: "能力读取失败" }
  if (!info) return { state: "loading", label: "正在读取能力" }
  if (info.hasOfflineResource || info.status === "downloaded") return { state: "offline", label: "已下载" }
  if (info.status === "queued") return { state: "queued", label: "下载中" }
  if (info.canOnlineRead) return { state: "online", label: "可在线阅读" }
  if (info.canDownload) return { state: "download_only", label: "需要下载后阅读" }
  return { state: "unavailable", label: "当前内容暂不可用" }
}
