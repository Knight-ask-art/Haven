import { getHavenClient, isTauriRuntime } from "@/lib/ipc/runtime"
import { toHavenError } from "@/lib/ipc/errors"
import type { TrendingBoardsDto, TrendingItemDto } from "@/lib/ipc/generated/wire"

const BROWSER_MOCK_TRENDING: TrendingBoardsDto = {
  schemaVersion: 1,
  boards: [
    {
      boardId: "anime",
      title: "动漫热门",
      subtitle: "TOP 10",
      items: [
        {
          title: "怪奇物语：1985故事集 第一季",
          subtitle: "2026 / 美国",
          description: "欢迎回到1985年冬天的霍金斯…",
          posterUri: null,
          statusBadge: null,
        },
      ],
    },
    {
      boardId: "cn_drama",
      title: "国产剧热门",
      subtitle: "TOP 10",
      items: [
        {
          title: "黑夜告白",
          subtitle: "2026 / 中国",
          description: "1997年神秘父女消失…",
          posterUri: null,
          statusBadge: "更新至12集",
        },
      ],
    },
    {
      boardId: "variety",
      title: "综艺热门",
      subtitle: "TOP 10",
      items: [
        {
          title: "乘风2026",
          subtitle: "2026 / 中国",
          description: "国际女性文化交流与音乐竞演…",
          posterUri: null,
          statusBadge: null,
        },
      ],
    },
    {
      boardId: "us_drama",
      title: "英美剧热门",
      subtitle: "TOP 10",
      items: [
        {
          title: "黑袍纠察队 第五季",
          subtitle: "2026 / 美国",
          description: "祖国人的世界…",
          posterUri: null,
          statusBadge: null,
        },
      ],
    },
  ],
}
export async function getTrendingBoards(): Promise<TrendingBoardsDto> {
  if (!isTauriRuntime()) return cloneMockTrending()
  try {
    const result = await getHavenClient().trendingBoardsGet()
    return normalizeTrendingBoards(result)
  } catch (error) {
    throw toHavenError(error)
  }
}

export async function refreshTrendingBoards(): Promise<TrendingBoardsDto> {
  if (!isTauriRuntime()) return cloneMockTrending()
  try {
    const result = await getHavenClient().trendingBoardsRefresh()
    return normalizeTrendingBoards(result)
  } catch (error) {
    throw toHavenError(error)
  }
}

export function normalizeTrendingError(error: unknown) {
  return toHavenError(error)
}

function cloneMockTrending(): TrendingBoardsDto {
  return structuredClone(BROWSER_MOCK_TRENDING)
}

export function normalizeTrendingBoards(value: unknown): TrendingBoardsDto {
  if (!isTrendingBoards(value)) {
    throw toHavenError({ code: "INVALID_RESPONSE", userMessage: "热榜格式无效", retryable: false })
  }
  return {
    schemaVersion: 1,
    boards: value.boards.map((board) => ({
      ...board,
      items: board.items.map((item) => ({
        ...item,
        // Tauri 生产路径 fail closed：契约漂移为 https 时直接变成空海报。
        posterUri: normalizePosterUri(item.posterUri),
      })),
    })),
  }
}

function normalizePosterUri(value: string | null): string | null {
  return value && /^haven:\/\/artwork\/[A-Za-z0-9_-]+$/.test(value) ? value : null
}

function isTrendingBoards(value: unknown): value is TrendingBoardsDto {
  if (typeof value !== "object" || value === null) return false
  const root = value as Record<string, unknown>
  if (root.schemaVersion !== 1 || !Array.isArray(root.boards)) return false
  return root.boards.every((board) => {
    if (typeof board !== "object" || board === null) return false
    const record = board as Record<string, unknown>
    return typeof record.boardId === "string" && typeof record.title === "string"
      && typeof record.subtitle === "string" && Array.isArray(record.items)
      && record.items.every(isTrendingItem)
  })
}

function isTrendingItem(value: unknown): value is TrendingItemDto {
  if (typeof value !== "object" || value === null) return false
  const record = value as Record<string, unknown>
  return typeof record.title === "string"
    && typeof record.subtitle === "string"
    && typeof record.description === "string"
    && (record.posterUri === null || typeof record.posterUri === "string")
    && (record.statusBadge === null || typeof record.statusBadge === "string")
}
