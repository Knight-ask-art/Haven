import type { LibraryMediaItemData } from "../components/MediaItem"

export function matchesLibraryCategory(item: LibraryMediaItemData, category: string): boolean {
  if (category === "all") return true
  if (category === "video") return item.type === "movie" || item.type === "tv"
  if (category === "periodical") return item.type === "periodical" || item.type === "article"
  return item.type === category
}

export function filterLibraryItemsByCategory(
  items: LibraryMediaItemData[],
  category: string,
): LibraryMediaItemData[] {
  return items.filter((item) => matchesLibraryCategory(item, category))
}
