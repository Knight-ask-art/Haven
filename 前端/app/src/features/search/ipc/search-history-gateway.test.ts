import { afterEach, describe, expect, it } from "vitest"
import type { HavenClient } from "@/lib/ipc/client"
import { MockHavenClient } from "@/lib/ipc/mock-client"
import { STORAGE_KEYS } from "@/lib/havenState"
import {
  clearSearchHistory,
  getSearchHistorySetting,
  listSearchHistory,
  recordSearchHistory,
  removeSearchHistory,
} from "./search-history-gateway"

function installLocalStorage(initial: Record<string, string> = {}) {
  const values = new Map(Object.entries(initial))
  const storage: Storage = {
    get length() { return values.size },
    clear: () => values.clear(),
    getItem: (key) => values.get(key) ?? null,
    key: (index) => [...values.keys()][index] ?? null,
    removeItem: (key) => { values.delete(key) },
    setItem: (key, value) => { values.set(key, value) },
  }
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: storage })
  return storage
}

afterEach(() => {
  installLocalStorage()
})

describe("search history gateway", () => {
  it("uses the explicit SQLite-shaped client commands for list, record, remove and clear", async () => {
    const client = new MockHavenClient(false, { seedSettings: false })

    expect(await listSearchHistory(client)).toEqual([])
    expect(await recordSearchHistory("  Dune  ", client)).toEqual(["Dune"])
    expect(await removeSearchHistory("Dune", client)).toEqual([])
    await recordSearchHistory("Dune", client)
    await clearSearchHistory(client)
    await expect(client.searchHistoryList({ limit: 10 })).resolves.toEqual([])
  })

  it("migrates valid legacy values only after all records are written", async () => {
    const storage = installLocalStorage({
      [STORAGE_KEYS.searchHistory]: JSON.stringify(["Dune", " Dune ", "Foundation"]),
    })
    const client = new MockHavenClient(false, { seedSettings: false })

    const migrated = await listSearchHistory(client)
    expect(migrated).toHaveLength(2)
    expect(migrated).toEqual(expect.arrayContaining(["Dune", "Foundation"]))
    expect(storage.getItem(STORAGE_KEYS.searchHistory)).toBeNull()
  })

  it("keeps the legacy key when migration cannot write every term", async () => {
    const storage = installLocalStorage({
      [STORAGE_KEYS.searchHistory]: JSON.stringify(["Dune"]),
    })
    const failingClient = {
      searchHistoryRecord: async () => { throw new Error("database unavailable") },
      searchHistoryList: async () => [],
    } as unknown as HavenClient

    await expect(listSearchHistory(failingClient)).resolves.toEqual([])
    expect(storage.getItem(STORAGE_KEYS.searchHistory)).toBe(JSON.stringify(["Dune"]))
  })

  it("ignores malformed legacy data and reads the authoritative client list", async () => {
    const storage = installLocalStorage({ [STORAGE_KEYS.searchHistory]: "not-json" })
    const client = new MockHavenClient(false, { seedSettings: false })
    await client.searchHistoryRecord({ term: "From SQLite" })

    await expect(listSearchHistory(client)).resolves.toEqual(["From SQLite"])
    expect(storage.getItem(STORAGE_KEYS.searchHistory)).toBe("not-json")
  })

  it("reads the privacy switch without treating history_clear as its source", async () => {
    const client = new MockHavenClient(false, { seedSettings: false })
    const initial = await client.settingsGet("privacy")
    expect(await getSearchHistorySetting(client)).toBe(true)

    await client.settingsUpdate({
      section: "privacy",
      expectedRevision: initial.revision,
      patch: { section: "privacy", searchHistory: false },
    })
    expect(await getSearchHistorySetting(client)).toBe(false)
  })
})
