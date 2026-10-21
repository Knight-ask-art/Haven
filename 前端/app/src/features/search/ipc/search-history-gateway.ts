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

function removeLegacySearchHistoryKey(): void {
  if (typeof localStorage === "undefined") return
  localStorage.removeItem(STORAGE_KEYS.searchHistory)
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
    removeLegacySearchHistoryKey()
    return true
  } catch {
    return false
  }
}

/**
 * 遗留迁移与显式清除共用同一个串行队列（模块级 owner）：清除不会插进迁移的
 * "写入中、尚未删 Key" 窗口，迁移也不会在清除之后把旧词写回来。
 * 失败照常抛给调用方，队列本身不吞错。
 */
let historyMutationQueue: Promise<unknown> = Promise.resolve()

function enqueueHistoryMutation<T>(task: () => Promise<T>): Promise<T> {
  const run = historyMutationQueue.then(task, task)
  historyMutationQueue = run.then(() => undefined, () => undefined)
  return run
}

async function listSearchHistoryUnlocked(client: HavenClient): Promise<string[]> {
  const migrated = await migrateLegacySearchHistory(client)
  const entries = await client.searchHistoryList({ limit: SEARCH_HISTORY_LIMIT } satisfies SearchHistoryListRequest)
  // Migration writes may refresh timestamps; re-query is the authoritative result.
  return mapTerms(migrated ? await client.searchHistoryList({ limit: SEARCH_HISTORY_LIMIT }) : entries)
}

function mapTerms(entries: SearchHistoryEntryDto[]): string[] {
  return entries.map((entry) => entry.term).filter((term) => term.trim().length > 0)
}

export async function listSearchHistory(
  client: HavenClient = getHavenClient(),
): Promise<string[]> {
  return enqueueHistoryMutation(() => listSearchHistoryUnlocked(client))
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

/**
 * 显式清除：与遗留迁移同一队列。排队期间不会有迁移在跑；执行前先删掉遗留 Key，
 * 之后任何一次 list 都看不到旧词，也就不会再迁移回来。SQLite 清除失败时照常抛出，
 * 不当作已清除（遗留 Key 已删，不会因此在下次 list 时被重新写回）。
 * 成功返回清除后的权威列表（调用方可直接 setHistory）。
 */
export async function clearSearchHistory(
  client: HavenClient = getHavenClient(),
): Promise<string[]> {
  return enqueueHistoryMutation(async () => {
    removeLegacySearchHistoryKey()
    await client.searchHistoryClear()
    return [] as string[]
  })
}

/** 隐私快照；读取失败或形状不符时抛出，不猜测默认值。 */
export async function getSearchHistorySetting(
  client: HavenClient = getHavenClient(),
): Promise<boolean> {
  const snapshot = await client.settingsGet("privacy")
  const value: { section?: unknown; searchHistory?: unknown } = snapshot.value
  if (value.section === "privacy" && typeof value.searchHistory === "boolean") {
    return value.searchHistory
  }
  // 形状不符不得被当成"开启"：抛出后由调用方按未知隐私处理（禁止记录）。
  throw new Error("privacy settings snapshot is not usable")
}
