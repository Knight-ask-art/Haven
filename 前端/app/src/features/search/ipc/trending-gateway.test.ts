import { describe, expect, it } from "vitest"

import { normalizeTrendingBoards } from "./trending-gateway"

describe("trending gateway", () => {
  it("fails closed for external poster URLs", () => {
    const result = normalizeTrendingBoards({
      schemaVersion: 1,
      boards: [
        {
          boardId: "anime",
          title: "动漫热门",
          subtitle: "TOP 10",
          items: [
            {
              title: "作品",
              subtitle: "2026",
              description: "简介",
              posterUri: "https://img.example.invalid/poster.jpg",
              statusBadge: null,
            },
          ],
        },
      ],
    })
    expect(result.boards[0]?.items[0]?.posterUri).toBeNull()
  })

  it("keeps controlled artwork identities and partial boards", () => {
    const result = normalizeTrendingBoards({
      schemaVersion: 1,
      boards: [
        {
          boardId: "cn_drama",
          title: "国产剧热门",
          subtitle: "TOP 10",
          items: [
            {
              title: "作品",
              subtitle: "2026",
              description: "简介",
              posterUri: "haven://artwork/poster-1",
              statusBadge: null,
            },
          ],
        },
      ],
    })
    expect(result.boards).toHaveLength(1)
    expect(result.boards[0]?.items[0]?.posterUri).toBe("haven://artwork/poster-1")
  })
})
