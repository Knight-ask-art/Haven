export const MAX_RENDERED_PDF_PAGES = 1
export const MIN_PDF_ZOOM = 0.5
export const MAX_PDF_ZOOM = 3
export const PDF_ZOOM_STEP = 0.1

/** Keep the PDF MIME check strict so arbitrary bytes are never sent to PDF.js by mistake. */
export function normalizePdfMimeType(mimeType: string | null | undefined): string | null {
  if (!mimeType) return null
  const normalized = mimeType.split(";", 1)[0]?.trim().toLowerCase()
  return normalized || null
}

export function isPdfMimeType(mimeType: string | null | undefined): boolean {
  return normalizePdfMimeType(mimeType) === "application/pdf"
}

export function clampPdfPage(pageNumber: number, pageCount: number): number {
  if (pageCount <= 0) return 0
  const page = Number.isFinite(pageNumber) ? Math.trunc(pageNumber) : 1
  return Math.min(pageCount, Math.max(1, page))
}

export function clampPdfZoom(zoom: number): number {
  if (!Number.isFinite(zoom)) return 1
  return Math.min(MAX_PDF_ZOOM, Math.max(MIN_PDF_ZOOM, zoom))
}

export interface PdfInitialLocator {
  /** Zero-based PDF page index. */
  pageIndex: number
  zoom: number | null
}

/** Resolve a persisted locator only after PDF.js has provided the real page count. */
export function resolvePdfInitialView(
  pageCount: number,
  locator?: PdfInitialLocator | null,
): { page: number; zoom: number } | null {
  if (!Number.isInteger(pageCount) || pageCount <= 0) return null
  const pageIndex = locator?.pageIndex ?? 0
  const page = clampPdfPage(pageIndex + 1, pageCount)
  const zoom = clampPdfZoom(locator?.zoom ?? 1)
  return { page, zoom }
}
