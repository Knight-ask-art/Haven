// Work 级漫画目录的纯展示投影。
//
// 这里只做“后端事实 → 文案/筛选/导航目标”的映射：
// - 章节顺序完全沿用后端返回的数组顺序，绝不按 chapterNumber 重新排序；
// - 上一章/下一章只使用后端给出的 previousMediaItemId / nextMediaItemId；
// - 不构造 Provider URL、远端章节 ID 或页面授权。

import type {
  ComicChapterAggregateStatusDto,
  ComicChapterSourceStatusDto,
  ComicWorkCatalogStatusDto,
  ComicWorkChapterCatalogDto,
  ComicWorkChapterDto,
} from "@/lib/ipc/generated/wire"

/** 章节聚合状态的展示文案（对应 ComicChapterAggregateStatusDto 的五个取值）。 */
const CHAPTER_STATUS_LABELS: Record<ComicChapterAggregateStatusDto, string> = {
  available: "可阅读",
  temporarily_unavailable: "暂不可用",
  external_only: "仅外部来源",
  unknown: "状态未知",
  missing: "最近刷新未出现",
}

const CATALOG_STATUS_LABELS: Record<ComicWorkCatalogStatusDto, string> = {
  never_synced: "尚未同步",
  synced: "已同步",
  refresh_failed: "刷新失败",
  truncated: "目录已截断",
}

export function comicChapterStatusLabel(status: ComicChapterAggregateStatusDto): string {
  return CHAPTER_STATUS_LABELS[status]
}

/** 单个来源的可用性文案；取值集合与章节聚合状态一致，共用同一份文案。 */
export function comicChapterSourceStatusLabel(status: ComicChapterSourceStatusDto): string {
  return CHAPTER_STATUS_LABELS[status]
}

export function comicWorkCatalogStatusLabel(status: ComicWorkCatalogStatusDto): string {
  return CATALOG_STATUS_LABELS[status]
}

/** 章节号是可选事实；缺失时退回到“第 N 章”的位置描述，而不是猜测章节号。 */
export function comicChapterNumberLabel(chapter: ComicWorkChapterDto): string | null {
  if (chapter.chapterNumber === null) return null
  const number = String(chapter.chapterNumber)
  return chapter.volumeNumber === null ? `第 ${number} 话` : `第 ${chapter.volumeNumber} 卷 · 第 ${number} 话`
}

/** 章节标题：后端标题优先，其次章节号，最后保持为空由调用方决定占位。 */
export function comicChapterDisplayTitle(chapter: ComicWorkChapterDto, fallback: string): string {
  if (chapter.title !== null && chapter.title.length > 0) return chapter.title
  return comicChapterNumberLabel(chapter) ?? fallback
}

export function comicChapterPageCountLabel(chapter: ComicWorkChapterDto): string {
  return chapter.pageCount === null ? "页数未知" : `${chapter.pageCount} 页`
}

/** 进度比例（0..1）；后端未给出进度时返回 null，由 UI 决定是否展示。 */
export function comicChapterProgressRatio(chapter: ComicWorkChapterDto): number | null {
  const ratio = chapter.progress?.progressRatio
  return ratio === null || ratio === undefined ? null : ratio
}

/**
 * 后端给出的跨来源匹配需要用户确认时的提示。
 *
 * 低置信度或 `progressMigration === "suggested"` 都表示后端没有自动应用迁移；
 * 前端只提示，不代替后端写入进度。
 */
export function hasSuggestedProgressMigration(chapter: ComicWorkChapterDto): boolean {
  const match = chapter.matchResult
  if (match === null) return false
  return match.progressMigration === "suggested" || match.confidence === "low"
}

/** 按 Edition 过滤章节；`editionId === null` 表示“全部”，顺序保持后端返回顺序。 */
export function selectComicWorkChapters(
  catalog: ComicWorkChapterCatalogDto,
  editionId: string | null,
): ComicWorkChapterDto[] {
  if (editionId === null) return [...catalog.chapters]
  return catalog.chapters.filter((chapter) => chapter.editionId === editionId)
}

/** 当前章节由后端 `currentMediaItemId` 决定，缺失时退回当前 Session 的 mediaItemId。 */
export function currentComicWorkChapter(
  catalog: ComicWorkChapterCatalogDto,
  mediaItemId: string | undefined,
): ComicWorkChapterDto | null {
  const target = catalog.currentMediaItemId ?? mediaItemId ?? null
  if (target === null) return null
  return catalog.chapters.find((chapter) => chapter.mediaItemId === target) ?? null
}

/**
 * 解析后端给出的上一章/下一章。
 *
 * 只在后端目录里能找到该章节时才允许导航；找不到就不把未知 mediaItemId
 * 当成可打开目标。
 */
export function resolveComicChapterNeighbour(
  catalog: ComicWorkChapterCatalogDto,
  mediaItemId: string | null,
): ComicWorkChapterDto | null {
  if (mediaItemId === null) return null
  return catalog.chapters.find((chapter) => chapter.mediaItemId === mediaItemId) ?? null
}
