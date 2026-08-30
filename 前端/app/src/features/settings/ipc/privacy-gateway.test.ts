import { describe, expect, it } from "vitest"
import type { HavenClient } from "@/lib/ipc/client"
import { clearArtworkCache, clearSearchHistory } from "./privacy-gateway"

describe("privacy gateway", () => {
  it("clears only the artwork cache scope", async () => {
    let receivedScope: string | null = null
    const client = {
      cacheClear: async (scope: "artwork") => {
        receivedScope = scope
        return { scope, removedEntries: 2n }
      },
    } as unknown as HavenClient

    await expect(clearArtworkCache(client)).resolves.toEqual({ scope: "artwork", removedEntries: 2n })
    expect(receivedScope).toBe("artwork")
  })

  it("uses the search-history command and does not call playback history clear", async () => {
    let searchCleared = false
    const client = {
      searchHistoryClear: async () => { searchCleared = true },
    } as unknown as HavenClient

    await clearSearchHistory(client)
    expect(searchCleared).toBe(true)
  })
})
