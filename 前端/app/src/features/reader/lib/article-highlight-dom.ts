export interface ArticleDomRange {
  start: number
  end: number
}

/** Create a range over the concatenated text content of a rendered block. */
export function createArticleTextRange(
  root: HTMLElement,
  start: number,
  end: number,
): Range | null {
  if (start < 0 || end <= start) return null
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
  let offset = 0
  let startNode: Text | null = null
  let endNode: Text | null = null
  let startOffset = 0
  let endOffset = 0

  while (walker.nextNode()) {
    const node = walker.currentNode as Text
    const next = offset + node.data.length
    if (!startNode && start >= offset && start <= next) {
      startNode = node
      startOffset = start - offset
    }
    if (end > offset && end <= next) {
      endNode = node
      endOffset = end - offset
      break
    }
    offset = next
  }

  if (!startNode || !endNode) return null
  const range = document.createRange()
  range.setStart(startNode, startOffset)
  range.setEnd(endNode, endOffset)
  return range
}

/** Remove marks created by the fallback renderer while preserving their text. */
export function clearArticleFallbackHighlights(root: HTMLElement): void {
  root.querySelectorAll<HTMLElement>("mark[data-haven-marker-id]").forEach((mark) => {
    mark.replaceWith(...Array.from(mark.childNodes))
  })
}

/**
 * Wrap every selected text fragment separately. A Range spanning inline
 * elements cannot be surroundContents()'d safely; splitting text nodes keeps
 * the original Markdown/HTML element tree intact.
 */
export function applyArticleFallbackHighlight(
  range: Range,
  markerId: string,
): HTMLElement[] {
  const textNodes: Text[] = []
  const root = range.commonAncestorContainer.nodeType === Node.TEXT_NODE
    ? range.commonAncestorContainer.parentElement
    : range.commonAncestorContainer as HTMLElement
  if (!root) return []

  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
  while (walker.nextNode()) {
    const node = walker.currentNode as Text
    if (range.intersectsNode(node) && node.data.length > 0) textNodes.push(node)
  }

  const marks: HTMLElement[] = []
  for (const node of textNodes) {
    let start = 0
    let end = node.data.length
    if (node === range.startContainer) start = range.startOffset
    if (node === range.endContainer) end = range.endOffset
    if (end <= start) continue
    const selected = node.splitText(start)
    const remainder = selected.splitText(end - start)
    void remainder
    const parent = selected.parentNode
    const mark = document.createElement("mark")
    mark.dataset.havenMarkerId = markerId
    mark.className = "cursor-pointer rounded-sm bg-amber-300/30 px-0.5 text-inherit underline decoration-amber-700/45 decoration-dotted underline-offset-4 transition-colors hover:bg-amber-300/50"
    mark.title = "点击取消划线"
    parent?.insertBefore(mark, selected)
    mark.appendChild(selected)
    marks.push(mark)
  }
  return marks
}
