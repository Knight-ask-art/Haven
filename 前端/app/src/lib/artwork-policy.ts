import { artworkRequestUri } from "@/lib/artwork-url"
import type { WorkCardDto } from "@/lib/ipc/generated/wire"

export function isBookOrPeriodical(
  card: Pick<WorkCardDto, "categories" | "availableMediaTypes">,
): boolean {
  if (card.categories.includes("book") || card.categories.includes("periodical")) return true
  const hasVisual = card.availableMediaTypes.some((v) =>
    ["movie", "series", "episode", "comic", "article"].includes(v),
  )
  if (card.availableMediaTypes.includes("book") || card.availableMediaTypes.includes("document")) {
    return !hasVisual
  }
  return false
}

export function pickCardImage(card: WorkCardDto): string {
  const poster = artworkRequestUri(card.posterUri)
  const backdrop = artworkRequestUri(card.backdropUri)
  return isBookOrPeriodical(card) ? poster : backdrop || poster
}
