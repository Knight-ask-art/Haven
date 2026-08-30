/**
 * Text-reader pagination geometry.
 *
 * The reader persists only a format-independent progression in the Progress
 * contract.  Page index is derived from the current viewport and is never
 * persisted, so changing the window size or font does not change the stored
 * locator shape.
 *
 * Pitfall fix: double's maxOffset is not necessarily a multiple of pageStride;
 * the last page must snap to maxOffset exactly, otherwise the tail jitters.
 */

export type BookPaginationMode = "scroll" | "paginated" | "double"

export interface BookPaginationViewport {
  scrollLeft: number
  scrollTop: number
  scrollWidth: number
  scrollHeight: number
  clientWidth: number
  clientHeight: number
}

export interface BookPaginationScrollContainer {
  scrollTop: number
  scrollLeft?: number
  /** Optional DOM style surface; omitted by the pure unit-test doubles. */
  style?: { scrollBehavior?: string }
}

export interface BookPaginationMetrics {
  mode: BookPaginationMode
  pageCount: number
  pageIndex: number
  progression: number
  offset: number
  maxOffset: number
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value))
}

function finite01(value: number): number {
  const number = Number.isFinite(value) ? (value as number) : 0
  return clamp(number, 0, 1)
}

// ---- 1. Pagination geometry (gap-aware) ----

export function paginationGeometry(
  mode: "paginated" | "double",
  contentWidth: number,
): {
  gap: number
  columnWidth: number
  columnsPerPage: number
  columnStride: number
  pageStride: number
} {
  const gap = mode === "double" ? 48 : 28
  const columnWidth =
    mode === "double" ? Math.max(1, (contentWidth - gap) / 2) : Math.max(1, contentWidth)
  const columnsPerPage = mode === "double" ? 2 : 1
  const columnStride = columnWidth + gap
  const pageStride = columnStride * columnsPerPage
  return { gap, columnWidth, columnsPerPage, columnStride, pageStride }
}

function offsetForPage(
  index: number,
  pageCount: number,
  pageStride: number,
  maxOffset: number,
): number {
  if (pageCount <= 1) return 0
  const clamped = clamp(Math.round(index), 0, pageCount - 1)
  if (clamped === pageCount - 1) return maxOffset
  return clamp(clamped * pageStride, 0, maxOffset)
}

function pageIndexForOffset(
  offset: number,
  pageCount: number,
  pageStride: number,
  maxOffset: number,
): number {
  if (pageCount <= 1) return 0
  // Distance to last snap may be < pageStride/2; prefer last if closer
  const normalIndex = clamp(Math.round(offset / pageStride), 0, pageCount - 1)
  const normalOffset = offsetForPage(normalIndex, pageCount, pageStride, maxOffset)
  const distanceNormal = Math.abs(offset - normalOffset)
  const distanceLast = Math.abs(offset - maxOffset)
  if (distanceLast < distanceNormal) return pageCount - 1
  return normalIndex
}

/**
 * Derive the current reader position from a DOM-like viewport.  Paginated
 * modes treat one viewport width as one page/spread; scroll mode keeps the
 * existing continuous vertical progression semantics.
 */
export function getBookPaginationMetrics(
  viewport: BookPaginationViewport,
  mode: BookPaginationMode,
): BookPaginationMetrics {
  const horizontal = mode !== "scroll"
  const pageStride = Math.max(1, horizontal ? viewport.clientWidth : viewport.clientHeight)
  const pageExtent = pageStride
  const contentExtent = Math.max(pageExtent, horizontal ? viewport.scrollWidth : viewport.scrollHeight)
  const rawOffset = horizontal ? viewport.scrollLeft : viewport.scrollTop
  const maxOffset = Math.max(0, contentExtent - pageExtent)
  const offset = clamp(Number.isFinite(rawOffset) ? rawOffset : 0, 0, maxOffset)

  if (!horizontal) {
    return {
      mode,
      pageCount: 1,
      pageIndex: 0,
      progression: maxOffset <= 0 ? 0 : offset / maxOffset,
      offset,
      maxOffset,
    }
  }

  if (maxOffset <= 2 || pageStride <= 0) {
    return {
      mode,
      pageCount: 1,
      pageIndex: 0,
      progression: 0,
      offset: 0,
      maxOffset,
    }
  }

  const pageCount = Math.max(1, Math.ceil(maxOffset / pageStride) + 1)
  const pageIndex = pageIndexForOffset(offset, pageCount, pageStride, maxOffset)
  return {
    mode,
    pageCount,
    pageIndex,
    progression: pageCount <= 1 ? 0 : pageIndex / (pageCount - 1),
    offset,
    maxOffset,
  }
}

/**
 * Convert a persisted progression to the current viewport's scroll offset.
 * In paginated modes the nearest page is selected, while scroll mode maps the
 * ratio continuously across the vertical range.
 */
export function bookOffsetForProgression(
  viewport: BookPaginationViewport,
  progression: number,
  mode: BookPaginationMode,
): number {
  const safeProgression = finite01(progression)
  const metrics = getBookPaginationMetrics(viewport, mode)
  if (mode === "scroll" || metrics.pageCount <= 1) return safeProgression * metrics.maxOffset
  const pageStride = Math.max(1, viewport.clientWidth)
  const pageIndex = Math.round(safeProgression * (metrics.pageCount - 1))
  return offsetForPage(pageIndex, metrics.pageCount, pageStride, metrics.maxOffset)
}

/**
 * Apply a restored/remapped offset without inheriting the reader's CSS
 * `scroll-behavior: smooth`.  Restoration is a state operation: animating it
 * can expose intermediate positions to the progress controller and persist a
 * value different from the server locator.
 */
export function setBookPaginationOffsetInstant(
  container: BookPaginationScrollContainer,
  target: number,
  mode: BookPaginationMode,
): void {
  const style = container.style
  const previous = style?.scrollBehavior
  if (style) style.scrollBehavior = "auto"
  if (mode === "scroll") {
    container.scrollTop = target
    if (container.scrollLeft !== undefined) container.scrollLeft = 0
  } else {
    if (container.scrollLeft !== undefined) {
      if (Math.abs(container.scrollLeft - target) > 2) container.scrollLeft = target
    }
    container.scrollTop = 0
  }
  if (style) style.scrollBehavior = previous ?? ""
}

/** Align an element's horizontal offset to the beginning of its page/spread. */
export function alignBookOffsetToPage(
  viewport: BookPaginationViewport,
  offset: number,
  mode: BookPaginationMode,
): number {
  if (mode === "scroll") {
    const metrics = getBookPaginationMetrics(viewport, mode)
    return clamp(offset, 0, metrics.maxOffset)
  }
  const metrics = getBookPaginationMetrics(viewport, mode)
  if (metrics.pageCount <= 1) return 0
  const pageStride = Math.max(1, viewport.clientWidth)
  const normalIndex = clamp(Math.round(offset / pageStride), 0, metrics.pageCount - 1)
  const normal = offsetForPage(normalIndex, metrics.pageCount, pageStride, metrics.maxOffset)
  const distanceNormal = Math.abs(offset - normal)
  const distanceLast = Math.abs(offset - metrics.maxOffset)
  return distanceLast < distanceNormal ? metrics.maxOffset : normal
}

/** Return the offset for a relative page/spread move. */
export function bookOffsetForPageDelta(
  viewport: BookPaginationViewport,
  delta: -1 | 1,
  mode: BookPaginationMode,
): number {
  const metrics = getBookPaginationMetrics(viewport, mode)
  if (mode === "scroll") {
    return clamp(viewport.scrollTop + delta * Math.max(1, viewport.clientHeight), 0, metrics.maxOffset)
  }
  const pageStride = Math.max(1, viewport.clientWidth)
  const currentPage = pageIndexForOffset(viewport.scrollLeft, metrics.pageCount, pageStride, metrics.maxOffset)
  const nextPage = clamp(currentPage + delta, 0, metrics.pageCount - 1)
  return offsetForPage(nextPage, metrics.pageCount, pageStride, metrics.maxOffset)
}
