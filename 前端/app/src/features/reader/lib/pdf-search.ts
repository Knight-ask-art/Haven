/** Upper bound applied to a single page's extracted text so pathological
 *  pages cannot balloon search memory. Matches are only searched within this
 *  prefix; real PDF pages stay far below it. */
export const MAX_PDF_SEARCH_PAGE_CHARS = 2_000_000

export interface PdfSearchHit {
  pageNumber: number
  occurrences: number
}

export type PdfPageTextProvider = (pageNumber: number) => Promise<string>

/** Normalize a user query: collapse whitespace runs, trim, case-fold. */
export function normalizePdfQuery(query: string): string {
  return query.replace(/\s+/g, " ").trim().toLowerCase()
}

/** Bound one page's extracted text to the configured maximum. Both the search
 *  loop and any caller-side cache must use this so per-document text memory
 *  stays formally capped even for pathological pages. */
export function boundPdfPageText(text: string): string {
  return text.length > MAX_PDF_SEARCH_PAGE_CHARS
    ? text.slice(0, MAX_PDF_SEARCH_PAGE_CHARS)
    : text
}

function countOccurrences(haystack: string, needle: string): number {
  if (!needle) return 0
  let count = 0
  let index = haystack.indexOf(needle)
  while (index !== -1) {
    count += 1
    index = haystack.indexOf(needle, index + needle.length)
  }
  return count
}

/**
 * Search every page's extracted text for the normalized query. Pages are read
 * lazily through `getPageText` so callers can cache or cancel; the loop checks
 * `signal` between pages and throws a DOMException "AbortError" when aborted.
 */
export async function searchPdfPages(options: {
  pageCount: number
  query: string
  getPageText: PdfPageTextProvider
  signal?: AbortSignal
  onProgress?: (scannedPages: number) => void
}): Promise<PdfSearchHit[]> {
  const query = normalizePdfQuery(options.query)
  if (!query) return []

  const hits: PdfSearchHit[] = []
  for (let pageNumber = 1; pageNumber <= options.pageCount; pageNumber += 1) {
    if (options.signal?.aborted) {
      throw new DOMException("PDF search was cancelled", "AbortError")
    }
    const raw = await options.getPageText(pageNumber)
    const bounded = boundPdfPageText(raw)
    const occurrences = countOccurrences(bounded.toLowerCase(), query)
    if (occurrences > 0) hits.push({ pageNumber, occurrences })
    options.onProgress?.(pageNumber)
  }
  return hits
}

/** Flatten hit rows into a cycling order for prev/next navigation. */
export function flattenPdfSearchHits(hits: PdfSearchHit[]): Array<{ pageNumber: number; occurrence: number }> {
  const flat: Array<{ pageNumber: number; occurrence: number }> = []
  for (const hit of hits) {
    for (let occurrence = 1; occurrence <= hit.occurrences; occurrence += 1) {
      flat.push({ pageNumber: hit.pageNumber, occurrence })
    }
  }
  return flat
}
