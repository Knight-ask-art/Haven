import type { MediaCardProps } from "@/components/ui/haven/MediaCard"

export interface LatestContinueSplit {
  hero: MediaCardProps | null
  rest: MediaCardProps[]
}

/**
 * 严格按 progress 0 < x < 100 筛选“最新未看完”的首项作为 Hero。
 * items 已为 progressRecent 原序（updatedAt 倒序），首个匹配即最新。
 */
export function selectLatestUnfinished(items: MediaCardProps[]): LatestContinueSplit {
  const heroIndex = items.findIndex(
    (item) => item.progress !== undefined && item.progress > 0 && item.progress < 100,
  )
  if (heroIndex === -1) {
    return { hero: null, rest: [...items] }
  }
  const hero = items[heroIndex]
  const rest = items.filter((_, index) => index !== heroIndex)
  return { hero, rest }
}
