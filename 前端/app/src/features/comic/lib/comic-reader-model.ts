import type { ComicPageManifestDto } from "@/lib/ipc/generated/wire"
import type { ComicSettingsValue } from "@/lib/ipc/settings-wire"

export interface ComicPageModel {
  pageId: string
  pageIndex: number
  pageNumber: number
  availability: "ready" | "unavailable"
  contentUri: string | null
}

export interface ComicPageSequence {
  sessionId: string
  mediaItemId: string
  pageCount: number
  pages: readonly ComicPageModel[]
}

export interface ComicReaderDefaults {
  viewMode: "single" | "double" | "strip"
  direction: "rtl" | "ltr"
  pageGapPx: 0 | 12 | 24
  preloadRadius: number
}

/**
 * 将持久化的 Comic Settings 映射为阅读器运行态。
 * `unlimited` 只代表用户不希望人为缩小窗口；读取侧仍固定为安全半径，
 * 防止损坏设置或超大漫画一次性占满资源许可池。
 */
export function resolveComicReaderDefaults(settings: ComicSettingsValue): ComicReaderDefaults {
  const pageGapPx = settings.pageGap === "zero" ? 0 : settings.pageGap === "twenty_four" ? 24 : 12
  const preloadRadius = settings.preloadPages === "one"
    ? 1
    : settings.preloadPages === "five"
      ? 5
      : settings.preloadPages === "unlimited"
        ? 12
        : 3
  return {
    viewMode: settings.viewMode,
    direction: settings.direction,
    pageGapPx,
    preloadRadius,
  }
}

/** Maps the wire's stable 0-based order to the reader's 1-based UI number. */
export function mapComicPageManifest(manifest: ComicPageManifestDto): ComicPageSequence {
  return {
    sessionId: manifest.sessionId,
    mediaItemId: manifest.mediaItemId,
    pageCount: manifest.pageCount,
    pages: manifest.pages.map((page) => ({
      pageId: page.pageId,
      pageIndex: page.pageIndex,
      pageNumber: page.pageIndex + 1,
      availability: page.availability,
      contentUri: page.contentUri,
    })),
  }
}

/** Browser-only demo data. It is never used by the Tauri production branch. */
export function createDemoComicPageSequence(urls: readonly string[], pageCount = 45): ComicPageSequence {
  const pages = Array.from({ length: Math.max(0, pageCount) }, (_, index) => ({
    pageId: `demo-page-${index}`,
    pageIndex: index,
    pageNumber: index + 1,
    availability: "ready" as const,
    contentUri: urls[index % urls.length] ?? null,
  }))
  return {
    sessionId: "demo-session",
    mediaItemId: "demo-comic",
    pageCount: pages.length,
    pages,
  }
}

export function pageAt(sequence: ComicPageSequence | null, pageNumber: number): ComicPageModel | null {
  if (!sequence || !Number.isInteger(pageNumber) || pageNumber < 1 || pageNumber > sequence.pageCount) return null
  return sequence.pages[pageNumber - 1] ?? null
}

export function pageNumbersAround(
  pageNumber: number,
  pageCount: number,
  radius: number,
): number[] {
  if (pageCount <= 0) return []
  const start = Math.max(1, Math.min(pageCount, pageNumber) - Math.max(0, radius))
  const end = Math.min(pageCount, start + Math.max(0, radius * 2))
  return Array.from({ length: end - start + 1 }, (_, index) => start + index)
}
