import type { TextAnchorDto } from "@/lib/ipc/generated/wire"

const MAX_EXACT = 240
const MAX_CONTEXT = 120

function clean(value: string | null | undefined, max: number): string | null {
  if (typeof value !== "string") return null
  const trimmed = value.trim()
  return trimmed === "" ? null : trimmed.slice(0, max)
}

function cleanContext(value: string | null | undefined): string | null {
  if (typeof value !== "string" || value.length === 0) return null
  return value.slice(0, MAX_CONTEXT)
}

/** 复用 pdf-progress-controller 的 sanitize 语义，保持 TextAnchor 上限一致。 */
export function sanitizeArticleTextAnchor(anchor: TextAnchorDto | null | undefined): TextAnchorDto | null {
  if (!anchor || typeof anchor !== "object") return null
  const exact = clean(anchor.exact, MAX_EXACT)
  if (!exact) return null
  return {
    exact,
    prefix: cleanContext(anchor.prefix),
    suffix: cleanContext(anchor.suffix),
  }
}

/**
 * 从段落原文与选中文本构建 TextAnchor。
 * exact 取选中文本（≤240），prefix/suffix 各取段落内前后 30 字符（≤120 上限由 sanitize 兜底）。
 * 若选中文本不在段落内，仍返回仅含 exact 的锚点（前端检索的 exact 12..240 语义由调用方保证）。
 */
export function buildArticleTextAnchor(paragraphText: string, selectedText: string): TextAnchorDto | null {
  const exact = selectedText.trim().slice(0, MAX_EXACT)
  if (!exact) return null
  const offset = paragraphText.indexOf(exact)
  if (offset === -1) {
    return sanitizeArticleTextAnchor({ exact, prefix: null, suffix: null })
  }
  const prefix = paragraphText.slice(Math.max(0, offset - 30), offset) || null
  const suffix = paragraphText.slice(offset + exact.length, offset + exact.length + 30) || null
  return sanitizeArticleTextAnchor({ exact, prefix, suffix })
}

/** Build an anchor from the actual rendered selection offset, avoiding first-match ambiguity. */
export function buildArticleTextAnchorAtOffset(
  blockText: string,
  selectedText: string,
  startOffset: number,
): TextAnchorDto | null {
  const exact = selectedText.trim()
  if (!exact || exact.length > MAX_EXACT) return null
  const start = Math.max(0, Math.min(startOffset, blockText.length))
  if (blockText.slice(start, start + exact.length) !== exact) return null
  return sanitizeArticleTextAnchor({
    exact,
    prefix: blockText.slice(Math.max(0, start - 30), start) || null,
    suffix: blockText.slice(start + exact.length, start + exact.length + 30) || null,
  })
}

export function resolveArticleTextAnchor(
  blockText: string,
  anchor: TextAnchorDto | null | undefined,
): { start: number; end: number } | null {
  const cleanAnchor = sanitizeArticleTextAnchor(anchor)
  if (!cleanAnchor || !cleanAnchor.exact) return null
  const exact = cleanAnchor.exact
  const matches: number[] = []
  let offset = blockText.indexOf(exact)
  while (offset !== -1) {
    const prefixMatches = !cleanAnchor.prefix
      || blockText.slice(Math.max(0, offset - cleanAnchor.prefix.length), offset) === cleanAnchor.prefix
    const suffixMatches = !cleanAnchor.suffix
      || blockText.slice(offset + exact.length, offset + exact.length + cleanAnchor.suffix.length) === cleanAnchor.suffix
    if (prefixMatches && suffixMatches) matches.push(offset)
    offset = blockText.indexOf(exact, offset + 1)
  }
  if (matches.length !== 1) return null
  return { start: matches[0], end: matches[0] + exact.length }
}

/** 按 blockId 在文档中查找段落原文；未找到返回 null。 */
export function findParagraphText(
  sections: Array<{ paragraphs: Array<{ id: string; text: string }> }>,
  blockId: string,
): string | null {
  for (const section of sections) {
    for (const paragraph of section.paragraphs) {
      if (paragraph.id === blockId) return paragraph.text
    }
  }
  return null
}
