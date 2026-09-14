import type { MediaCardProps } from "@/components/ui/haven/MediaCard"
import type { CompletionWire } from "@/lib/ipc/generated/wire"

type CompletionAwareCard = MediaCardProps & {
  completion?: CompletionWire
}

export interface LatestContinueSplit<T extends MediaCardProps = MediaCardProps> {
  hero: T | null
  rest: T[]
}

/**
 * 严格按 progress 0 < x < 100 筛选“最新未看完”的首项作为 Hero。
 * items 已为 progressRecent 原序（updatedAt 倒序），首个匹配即最新。
 */
export function selectLatestUnfinished<T extends CompletionAwareCard>(items: T[]): LatestContinueSplit<T> {
  const heroIndex = items.findIndex(
    (item) => (item.completion === undefined || item.completion === "in_progress")
      && item.progress !== undefined
      && item.progress > 0
      && item.progress < 100,
  )
  if (heroIndex === -1) {
    return { hero: null, rest: [...items] }
  }
  const hero = items[heroIndex]
  const rest = items.filter((_, index) => index !== heroIndex)
  return { hero, rest }
}
