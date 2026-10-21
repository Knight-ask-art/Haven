/** Upper bound applied to a single page's extracted text so pathological
 *  pages cannot balloon search memory. Matches are only searched within this
 *  prefix; real PDF pages stay far below it. */
export const MAX_PDF_SEARCH_PAGE_CHARS = 2_000_000

export interface PdfSearchHit {
  pageNumber: number
  occurrences: number
}

export type PdfPageTextProvider = (pageNumber: number) => Promise<string>

/** The subset of PDF.js `TextItem` the page-text assembler reads. */
export interface PdfTextFragment {
  str: string
  /** Text matrix `[a, b, c, d, e, f]`; `e`/`f` are the device-space origin. */
  transform: ArrayLike<number>
  /** Device-space advance width of the fragment. */
  width: number
  hasEOL?: boolean
}

/** One code unit of `PdfPageTextIndex.text` and where it came from. */
export interface PdfTextSpan {
  /** Index of the source `TextItem` in the page's text content. */
  itemIndex: number
  /** Offset into that item's original `str`. */
  sourceOffset: number
  /** Assembled code units [start, end) this source unit produced. */
  start: number
  end: number
}

export interface PdfPageTextIndex {
  /** Whitespace-normalized page text; the first `MAX_PDF_SEARCH_PAGE_CHARS`
   *  code units are searchable. */
  text: string
  /** Case-folded `text.slice(0, MAX_PDF_SEARCH_PAGE_CHARS)`, code-unit for
   *  code-unit: that index is where `searchPdfPages` reads the page. Matches
   *  found here therefore share offsets with `spans`. */
  searchText: string
  /** Source units of `text`, in order. */
  spans: PdfTextSpan[]
}

function isWhiteSpace(character: string | undefined): boolean {
  return character !== undefined && /\s/.test(character)
}

/** Whether two adjacent fragments sit on one baseline but are apart
 *  horizontally (a real visual gap: a column break, a tab, a missing space).
 *  The threshold is a fraction of the font size, below half an em, so an
 *  inter-word gap is kept while `"hel"`/`"lo"` split by font runs stays glued. */
function fragmentsAreSpaced(previous: PdfTextFragment, next: PdfTextFragment): boolean {
  const previousY = previous.transform[5]
  const nextY = next.transform[5]
  if (!Number.isFinite(previousY) || !Number.isFinite(nextY)) return false
  const tolerance = Math.max(1, Math.abs(previous.transform[3] ?? 0) * 1.5)
  if (Math.abs(previousY - nextY) > tolerance) return false
  const previousRight = (previous.transform[4] ?? 0) + (previous.width || 0)
  const gap = (next.transform[4] ?? 0) - previousRight
  const fontSize = Math.abs(next.transform[3] ?? 0)
  const threshold = fontSize > 0 ? Math.max(0.5, fontSize * 0.2) : 1
  return gap > threshold
}

/**
 * The one owner of "what text is a PDF page". Search counts, highlighted
 * ranges and the locator anchor all read this, so a query cannot match text
 * that no highlight can reach.
 *
 * Fragments are joined geometrically: a word split by font runs (`"hel"`,
 * `"lo"`) stays glued, a line end or a horizontal gap on the same baseline
 * becomes exactly one space, and whitespace already carried by the page is
 * collapsed (never doubled). Every assembled code unit remembers the item of
 * its source string and its offset inside it; a synthesized space is
 * attributed to the unit that follows it. Assembly stops at
 * `MAX_PDF_SEARCH_PAGE_CHARS`, so a pathological page allocates neither text
 * nor spans past the cap. Index and highlight must pass items through the
 * same `indexablePdfItems` filter.
 */
export function indexPdfPageText(items: ReadonlyArray<PdfTextFragment | { str?: undefined }>): PdfPageTextIndex {
  let text = ""
  const spans: PdfTextSpan[] = []
  let limitReached = false
  const push = (value: string, itemIndex: number, sourceOffset: number) => {
    if (limitReached) return
    const remaining = MAX_PDF_SEARCH_PAGE_CHARS - text.length
    if (remaining <= 0) {
      limitReached = true
      return
    }
    const capped = value.length > remaining ? value.slice(0, remaining) : value
    spans.push({ itemIndex, sourceOffset, start: text.length, end: text.length + capped.length })
    text += capped
    if (capped.length < value.length) limitReached = true
  }
  // A space is pending at the start of a line (`lineStart`) or because the
  // source text carried one since the last emitted unit.
  let lineStart = true
  let pendingSpace = false
  // Fragment the next unit continues from (same baseline, geometry check);
  // `null` after a line end.
  let previous: PdfTextFragment | null = null

  for (let itemIndex = 0; itemIndex < items.length && !limitReached; itemIndex += 1) {
    const item = items[itemIndex]
    if (typeof item.str !== "string") continue
    const fragment = item as PdfTextFragment
    const { str } = fragment
    // Geometry is applied once, before the first unit of this item.
    if (previous !== null && !lineStart && !pendingSpace && !text.endsWith(" ")
      && !isWhiteSpace(str[0]) && fragmentsAreSpaced(previous, fragment)) {
      pendingSpace = true
    }
    let offset = 0
    while (offset < str.length && !limitReached) {
      if (isWhiteSpace(str[offset])) {
        pendingSpace = true
        offset += 1
        continue
      }
      if ((pendingSpace || lineStart) && text.length > 0 && !text.endsWith(" ")) {
        // The space is attributed to the unit that follows the gap so a match
        // starting at the space still anchors to a real text node.
        push(" ", itemIndex, offset)
      }
      pendingSpace = false
      lineStart = false
      let end = offset + 1
      while (end < str.length && !isWhiteSpace(str[end])) end += 1
      push(str.slice(offset, end), itemIndex, offset)
      offset = end
    }
    if (limitReached) break
    // A line end and carried whitespace become one space before the next unit.
    if (fragment.hasEOL) {
      pendingSpace = true
      lineStart = true
      previous = null
    } else {
      previous = fragment
    }
  }

  return {
    text,
    searchText: text.toLowerCase(),
    spans,
  }
}

/** The `TextItem`s the page index reads; highlight uses the same filter so a
 *  span's `itemIndex` is also the index into `textDivs`. */
export function indexablePdfItems<T extends PdfTextFragment | object>(items: readonly T[]): Array<Extract<T, PdfTextFragment>> {
  return items.filter((item): item is Extract<T, PdfTextFragment> => "str" in item && typeof item.str === "string")
}

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
