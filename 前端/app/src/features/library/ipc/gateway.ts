// Library Gateway（SLICE-LIBRARY-001：页面唯一的数据通道，禁止散落 invoke）。
// 环境分流（Human Owner 2026-08-16 裁决）：
// - 显式 Mock = 演示环境 → 呈现完整演示目录（REPRESENTATIVE_ITEMS，恢复既有演示体验）；
//   MockHavenClient 仅消费公开的 IPC 契约样例，不进入生产路径。
// - Tauri WebView = 生产环境 → 真实 library_list 投影。
// - 普通生产浏览器 = unavailable → 由统一 Haven Client 边界 fail closed。

import type { LibraryMediaItemData } from "../components/MediaItem"
import { artworkRequestUri } from "@/lib/artwork-url"

import { REPRESENTATIVE_ITEMS } from "../components/LibraryGrid"

import { getHavenClient, getHavenClientMode } from "@/lib/ipc/runtime"

import type { HavenClient } from "@/lib/ipc/client"

import type { LibraryListRequest, LocatorDto, PageDto, WorkCardDto } from "@/lib/ipc/generated/wire"

import { resolveLibraryRuntimeState } from "../lib/library-runtime-state"
import { saveProgress } from "@/features/progress/ipc/progress-gateway"


/** 每个 IPC 请求的上限；调用方必须继续消费 nextCursor。 */
export const LIST_LIMIT = 200

function deriveCardType(card: WorkCardDto): string {
  const media = card.availableMediaTypes
  if (media.includes("series") || media.includes("episode")) return "tv"
  if (media.includes("movie")) return "movie"
  if (media.includes("audio")) return "movie"
  if (media.includes("document")) return "document"
  if (media.includes("article")) return "article"
  if (media.includes("comic")) return "comic"
  if (media.includes("book")) return "book"
  return card.categories[0] ?? "document"
}

/**
 * A library card can have a consumable media item before it has any saved
 * progress. These are the only locator kinds whose start position is
 * unambiguous from the card contract. Books are intentionally excluded: a
 * BookLocator needs the real publication resource (for example an EPUB
 * spine item), which is not exposed by WorkCardDto.
 */
export function defaultCompletionLocator(card: WorkCardDto): LocatorDto | null {
  const mediaItemId = card.primaryAction?.mediaItemId ?? card.progress?.mediaItemId
  if (!mediaItemId) return null

  const primaryKind = card.primaryAction?.kind
  if (primaryKind === "playback") {
    return { version: 1, kind: "video", data: { positionMs: 0 } }
  }
  if (primaryKind === "comic") {
    return { version: 1, kind: "comic", data: { chapterItemId: mediaItemId, pageIndex: 0, pageProgression: null } }
  }
  if (primaryKind === "article") {
    return { version: 1, kind: "article", data: { blockId: null, progression: 0, textAnchor: null } }
  }
  if (primaryKind === "reader" && card.availableMediaTypes.length === 1 && card.availableMediaTypes[0] === "document") {
    return { version: 1, kind: "pdf", data: { pageIndex: 0, x: null, y: null, zoom: null, textAnchor: null } }
  }

  // Progress can exist without a current primary action. Only infer a kind
  // when the card exposes exactly one compatible media type.
  const mediaTypes = card.availableMediaTypes
  if (mediaTypes.length !== 1) return null
  switch (mediaTypes[0]) {
    case "movie":
    case "series":
    case "episode":
    case "audio":
      return { version: 1, kind: "video", data: { positionMs: 0 } }
    case "document":
      return { version: 1, kind: "pdf", data: { pageIndex: 0, x: null, y: null, zoom: null, textAnchor: null } }
    case "comic":
      return { version: 1, kind: "comic", data: { chapterItemId: mediaItemId, pageIndex: 0, pageProgression: null } }
    case "article":
      return { version: 1, kind: "article", data: { blockId: null, progression: 0, textAnchor: null } }
    default:
      return null
  }
}

function toMediaItemData(card: WorkCardDto): LibraryMediaItemData {
  const progressLocator = card.primaryAction?.locator ?? card.progress?.locator ?? defaultCompletionLocator(card)
  return {
    id: card.workId,
    title: card.title,
    originalTitle: card.originalTitle ?? undefined,
    type: deriveCardType(card),
    year: card.releaseYear ?? 0,
    imageUrl: artworkRequestUri(card.posterUri),
    backdropUrl: card.backdropUri ?? undefined,
    rating: card.ratingValue != null ? String(card.ratingValue) : undefined,
    description: card.description ?? undefined,
    favorite: card.favorite,
    progressMediaItemId: card.primaryAction?.mediaItemId ?? card.progress?.mediaItemId,
    progressLocator,
  }
}

export interface LibraryBrowseQuery {
  category?: LibraryListRequest["category"]
  query?: string
  sort?: LibraryListRequest["sort"]
}

function listRequest(cursor: string | null, options: LibraryBrowseQuery = {}): LibraryListRequest {
  return {
    category: options.category ?? "all",
    mediaTypes: null,
    query: options.query?.trim() || null,
    sort: options.sort ?? "recently_added",
    cursor,
    limit: LIST_LIMIT,
  }
}

export interface LibraryBrowsePage {
  items: LibraryMediaItemData[]
  nextCursor: string | null
  total: number | null
}

/** Accept a cursor exactly once; a repeated cursor is a protocol failure. */
export function acceptLibraryCursor(seen: Set<string>, nextCursor: string | null): string | null {
  if (nextCursor === null) return null
  if (seen.has(nextCursor)) throw new Error("library_list 返回了循环 cursor")
  seen.add(nextCursor)
  return nextCursor
}

/** 拉取一页真实投影；浏览器演示环境只有一页。 */
export async function getLibraryBrowsePage(
  cursor: string | null = null,
  options: LibraryBrowseQuery = {},
): Promise<LibraryBrowsePage> {
  const runtimeState = resolveLibraryRuntimeState(getHavenClientMode())
  if (runtimeState === "demo") {
    if (cursor !== null) return { items: [], nextCursor: null, total: REPRESENTATIVE_ITEMS.length }
    return { items: REPRESENTATIVE_ITEMS, nextCursor: null, total: REPRESENTATIVE_ITEMS.length }
  }
  const page: PageDto<WorkCardDto> = await getHavenClient().libraryList(listRequest(cursor, options))
  return {
    items: page.items.map(toMediaItemData),
    nextCursor: page.nextCursor,
    total: page.total,
  }
}

/** 消费完整 cursor 链；循环 cursor 是协议错误，不能静默重复请求。 */
export async function loadAllLibraryPages(client: HavenClient = getHavenClient()): Promise<LibraryMediaItemData[]> {
  const items: LibraryMediaItemData[] = []
  const seen = new Set<string>()
  let cursor: string | null = null
  for (;;) {
    const page = await client.libraryList(listRequest(cursor))
    items.push(...page.items.map(toMediaItemData))
    cursor = acceptLibraryCursor(seen, page.nextCursor)
    if (cursor === null) return items
  }
}

export async function findLibraryItemById(
  client: HavenClient,
  workId: string,
): Promise<LibraryMediaItemData | undefined> {
  return (await loadAllLibraryPages(client)).find((item) => item.id === workId)
}

/** 拉取媒体库卡片投影：浏览器演示目录 or 真实 library_list（完整 cursor 序列）。 */
export async function getLibraryBrowseItems(): Promise<LibraryMediaItemData[]> {
  const runtimeState = resolveLibraryRuntimeState(getHavenClientMode())
  if (runtimeState === "demo") return REPRESENTATIVE_ITEMS
  return loadAllLibraryPages(getHavenClient())
}

/** 详情页收藏初始化使用同一服务端 WorkCard 投影，避免 localStorage 伪造权威状态。 */
export async function getLibraryBrowseItem(workId: string): Promise<LibraryMediaItemData | undefined> {
  const runtimeState = resolveLibraryRuntimeState(getHavenClientMode())
  if (runtimeState === "demo") return REPRESENTATIVE_ITEMS.find((item) => item.id === workId)
  return findLibraryItemById(getHavenClient(), workId)
}

/** Complete one server-selected media item without inventing a locator. */
export async function markLibraryItemCompleted(item: LibraryMediaItemData): Promise<void> {
  if (!item.progressMediaItemId || !item.progressLocator) {
    throw new Error(`${item.title}没有可用的阅读定位`)
  }
  await saveProgress({
    mediaItemId: item.progressMediaItemId,
    locator: item.progressLocator,
    completion: "completed",
    expectedRevision: null,
  })
}
