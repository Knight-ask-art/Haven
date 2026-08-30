import type {
  SearchHistoryEntryDto,
  SearchHistoryListRequest,
} from "@/lib/ipc/generated/wire"
import { getHavenClient } from "@/lib/ipc/runtime"
import { STORAGE_KEYS } from "@/lib/havenState"
import type { HavenClient } from "@/lib/ipc/client"

const SEARCH_HISTORY_LIMIT = 10

function readLegacySearchHistory(): { valid: true; terms: string[] } | { valid: false } {
  if (typeof localStorage === "undefined") return { valid: true, terms: [] }
  try {
    const raw = localStorage.getItem(STORAGE_KEYS.searchHistory)
    if (raw === null) return { valid: true, terms: [] }
    const parsed: unknown = JSON.parse(raw)
    if (!Array.isArray(parsed) || parsed.some((value) => typeof value !== "string")) return { valid: false }
    const terms = [...new Set(
      parsed
        .map((value) => value.trim())
        .filter((value) => value.length > 0 && value.length <= 200),
    )].slice(0, SEARCH_HISTORY_LIMIT)
    return { valid: true, terms }
  } catch {
    return { valid: false }
  }
}

/**
 * 一次性迁移旧 localStorage 搜索历史：全部写入成功后才删除旧 Key。
 * 失败时保留旧 Key，下一次 Query 会再次尝试，不把迁移失败变成数据丢失。
 */
async function migrateLegacySearchHistory(client: HavenClient): Promise<boolean> {
  const legacy = readLegacySearchHistory()
  if (!legacy.valid || legacy.terms.length === 0) return false
  try {
    for (const term of legacy.terms) {
      await client.searchHistoryRecord({ term })
    }
    localStorage.removeItem(STORAGE_KEYS.searchHistory)
    return true
  } catch {
    return false
  }
}

function mapTerms(entries: SearchHistoryEntryDto[]): string[] {
  return entries.map((entry) => entry.term).filter((term) => term.trim().length > 0)
}

export async function listSearchHistory(
  client: HavenClient = getHavenClient(),
): Promise<string[]> {
  const migrated = await migrateLegacySearchHistory(client)
  const entries = await client.searchHistoryList({ limit: SEARCH_HISTORY_LIMIT } satisfies SearchHistoryListRequest)
  // Migration writes may refresh timestamps; re-query is the authoritative result.
  return mapTerms(migrated ? await client.searchHistoryList({ limit: SEARCH_HISTORY_LIMIT }) : entries)
}

export async function recordSearchHistory(
  term: string,
  client: HavenClient = getHavenClient(),
): Promise<string[]> {
  await client.searchHistoryRecord({ term })
  return mapTerms(await client.searchHistoryList({ limit: SEARCH_HISTORY_LIMIT }))
}

export async function removeSearchHistory(
  term: string,
  client: HavenClient = getHavenClient(),
): Promise<string[]> {
  await client.searchHistoryRemove({ term })
  return mapTerms(await client.searchHistoryList({ limit: SEARCH_HISTORY_LIMIT }))
}

export async function clearSearchHistory(
  client: HavenClient = getHavenClient(),
): Promise<string[]> {
  await client.searchHistoryClear()
  return []
}

export async function getSearchHistorySetting(
  client: HavenClient = getHavenClient(),
): Promise<boolean> {
  const snapshot = await client.settingsGet("privacy")
  return snapshot.value.section === "privacy" ? snapshot.value.searchHistory : true
}
