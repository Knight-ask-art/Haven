import type { HavenClient } from "@/lib/ipc/client"
import { artworkRequestUri } from "@/lib/artwork-url"

import type {

  LibraryListRequest,
  PageDto,
  QueryCategory,
  WorkCardDto,
} from "@/lib/ipc/generated/wire"
import { getHavenClient } from "@/lib/ipc/runtime"


export type SearchCategory = QueryCategory

export interface LocalSearchResult {
  id: string
  title: string
  originalTitle?: string
  description?: string
  category: Exclude<QueryCategory, "all">
  year?: number
  imageUrl?: string
  rating?: number
}

const SEARCH_PAGE_LIMIT = 100

function searchRequest(
  query: string,
  category: SearchCategory,
  cursor: string | null,
): LibraryListRequest {
  return {
    category,
    mediaTypes: null,
    query,
    sort: "recently_added",
    cursor,
    limit: SEARCH_PAGE_LIMIT,
  }
}

function primaryCategory(card: WorkCardDto): LocalSearchResult["category"] {
  return card.categories[0] ?? "periodical"
}

export function toLocalSearchResult(card: WorkCardDto): LocalSearchResult {
  return {
    id: card.workId,
    title: card.title,
    originalTitle: card.originalTitle ?? undefined,
    description: card.description ?? undefined,
    category: primaryCategory(card),
    year: card.releaseYear ?? undefined,
    imageUrl: artworkRequestUri(card.posterUri) || undefined,
    rating: card.ratingValue ?? undefined,
  }
}

function acceptCursor(seen: Set<string>, cursor: string | null): string | null {
  if (cursor === null) return null
  if (seen.has(cursor)) throw new Error("library_list 返回了循环 cursor")
  seen.add(cursor)
  return cursor
}

export async function searchLocalLibrary(
  query: string,
  category: SearchCategory,
  signal: AbortSignal,
  client: HavenClient = getHavenClient(),
): Promise<LocalSearchResult[]> {
  const normalizedQuery = query.trim()
  if (!normalizedQuery || signal.aborted) return []

  const results: LocalSearchResult[] = []
  const seen = new Set<string>()
  let cursor: string | null = null

  for (;;) {
    const page: PageDto<WorkCardDto> = await client.libraryList(
      searchRequest(normalizedQuery, category, cursor),
    )
    if (signal.aborted) return []
    results.push(...page.items.map(toLocalSearchResult))
    cursor = acceptCursor(seen, page.nextCursor)
    if (cursor === null) return results
  }
}