// 生产漫画详情页的章节展示投影。
//
// 直接消费 Work 级目录 DTO（catalog.chapters / catalog.editions），不再先把漫画目录压成
// EditionListItem 再还原成剧集行：每一行都按引用保留原始 ComicWorkChapterDto，因此顺序
// （数组顺序 / backendOrder）、status、canOpen、subjectId、sources 与 matchResult 都完整
// 留在投影里，详情页按后端事实渲染与导航。Edition 维度同样直接取 catalog.editions 并保留
// profile，前端不重新推导章节归属。

import type {
  ComicChapterAggregateStatusDto,
  ComicEditionProfileDto,
  ComicWorkChapterCatalogDto,
  ComicWorkChapterDto,
  PrimaryActionDto,
} from "@/lib/ipc/generated/wire"
import {
  comicChapterDisplayTitle,
  comicChapterNumberLabel,
  comicChapterPageCountLabel,
  comicChapterProgressRatio,
  comicChapterSourceStatusLabel,
  comicChapterStatusLabel,
  hasSuggestedProgressMigration,
} from "@/features/comic/lib/comic-work-catalog-view"

/** 详情页的一行章节；展示字段来自 ComicWorkChapterDto，原始 chapter 原样保留，不发明事实。 */
export interface ComicWorkChapterRow {
  /** 本地 MediaItem 身份：React key、导航与进度定位都用它。 */
  id: string
  editionId: string
  /**
   * 后端返回的原始章节 DTO，按引用原样保留（不复制、不改写）。backendOrder、完整的
   * sources、matchResult、previous/nextMediaItemId 等事实都随这一行留在投影里，详情页
   * 需要时直接读取，不必回到 catalog 重新查找。
   */
  chapter: ComicWorkChapterDto
  /** 章节所属主体；保留后端事实，前端不用它推导导航或写入。 */
  subjectId: string | null
  number: string
  title: string
  pageCountLabel: string
  /** 后端进度比例 ×100；后端未给出进度时为 null。 */
  progressPercent: number | null
  status: ComicChapterAggregateStatusDto
  statusLabel: string
  canOpen: boolean
  isCurrent: boolean
  sourceCount: number
  sourceSummaryLabel: string
  /** 后端跨来源匹配需要用户确认（低置信度或 suggested 迁移）；前端只提示，不代替后端写入。 */
  needsMatchReview: boolean
  primaryAction: PrimaryActionDto | null
}

/** Edition 维度直接来自 catalog.editions，保留后端 profile，前端不重新推导。 */
export interface ComicWorkEditionOption {
  editionId: string
  label: string
  chapterCount: number
  /** 后端返回的语言 / 译名线 / 扫图组 / 彩色模式画像，按引用原样保留。 */
  profile: ComicEditionProfileDto
}

function comicSourceSummaryLabel(chapter: ComicWorkChapterDto): string {
  if (chapter.sources.length === 0) return "无来源记录"
  const statuses = Array.from(new Set(chapter.sources.map((source) => comicChapterSourceStatusLabel(source.status))))
  return `${chapter.sources.length} 个来源 · ${statuses.join(" / ")}`
}

/**
 * 章节主操作。
 *
 * `kind: "comic"` 与路由 `/comic/:mediaItemId` 都由既有契约固定；mediaItemId 是后端
 * 返回的本地身份，不使用远端章节 ID、Provider URL 或页面授权。后端判定不可打开的
 * 章节不提供主操作，详情页因此只提示、不导航。
 */
function comicChapterPrimaryAction(
  chapter: ComicWorkChapterDto,
  currentMediaItemId: string | null,
): PrimaryActionDto | null {
  if (!chapter.canOpen) return null
  return {
    kind: "comic",
    labelHint: chapter.mediaItemId === currentMediaItemId ? "continue" : "start",
    editionId: chapter.editionId,
    mediaItemId: chapter.mediaItemId,
    locator: chapter.progress?.locator ?? null,
  }
}

/** 顺序完全沿用后端返回顺序；前端不按章节号排序、补全或改写 backendOrder。 */
export function projectComicWorkChapterRows(catalog: ComicWorkChapterCatalogDto): ComicWorkChapterRow[] {
  return catalog.chapters.map((chapter) => {
    const ratio = comicChapterProgressRatio(chapter)
    return {
      id: chapter.mediaItemId,
      editionId: chapter.editionId,
      chapter,
      subjectId: chapter.subjectId,
      number: comicChapterNumberLabel(chapter) ?? "章节",
      title: comicChapterDisplayTitle(chapter, "未命名章节"),
      pageCountLabel: comicChapterPageCountLabel(chapter),
      progressPercent: ratio === null ? null : ratio * 100,
      status: chapter.status,
      statusLabel: comicChapterStatusLabel(chapter.status),
      canOpen: chapter.canOpen,
      isCurrent: chapter.mediaItemId === catalog.currentMediaItemId,
      sourceCount: chapter.sources.length,
      sourceSummaryLabel: comicSourceSummaryLabel(chapter),
      needsMatchReview: hasSuggestedProgressMigration(chapter),
      primaryAction: comicChapterPrimaryAction(chapter, catalog.currentMediaItemId),
    }
  })
}

export function projectComicWorkEditions(catalog: ComicWorkChapterCatalogDto): ComicWorkEditionOption[] {
  return catalog.editions.map((edition) => ({
    editionId: edition.editionId,
    label: edition.displayLabel,
    chapterCount: edition.chapterCount,
    profile: edition.profile,
  }))
}
