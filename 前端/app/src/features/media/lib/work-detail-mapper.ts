import { artworkRequestUri } from "@/lib/artwork-url"
import type { WorkDetailHeaderDto } from "@/lib/ipc/generated/wire"
import type { MediaDetailData } from "../pages/MediaDetailPage"

function deriveType(mediaTypes: WorkDetailHeaderDto["availableMediaTypes"], categories: WorkDetailHeaderDto["categories"]): MediaDetailData["type"] {
  if (mediaTypes.includes("series") || mediaTypes.includes("episode")) return "tv"
  if (mediaTypes.includes("movie")) return "movie"
  if (mediaTypes.includes("document")) return "document"
  if (mediaTypes.includes("article")) return "article"
  if (mediaTypes.includes("comic")) return "comic"
  if (mediaTypes.includes("book")) return "book"
  const category = categories[0]
  return category === "periodical" ? "periodical" : "document"
}

/** Maps only WorkDetailHeaderDto facts; editions are loaded by edition_list_by_work. */
export function mapWorkDetailHeaderToMediaDetail(dto: WorkDetailHeaderDto): MediaDetailData {
  const progress = dto.progress?.progressRatio
  const backdropUrl = artworkRequestUri(dto.backdropUri) || artworkRequestUri(dto.posterUri) || dto.backdropUri || ""
  return {
    id: dto.workId,
    title: dto.title,
    originalTitle: dto.originalTitle ?? undefined,
    type: deriveType(dto.availableMediaTypes, dto.categories),
    year: dto.releaseYear ?? 0,
    backdropUrl,
    posterUrl: artworkRequestUri(dto.posterUri),
    description: dto.description ?? "",
    authorOrDirector: (dto as unknown as { director?: string | null }).director ?? dto.originalTitle ?? undefined,
    publisherOrStudio: (dto as unknown as { actor?: string | null }).actor ?? undefined,
    favorite: dto.favorite,
    progress: progress == null ? undefined : progress * 100,
    episodesOrChapters: [],
    ...(dto.primaryAction ? { primaryAction: dto.primaryAction } : {}),
  }
}
