import type { TextAnchorDto } from "@/lib/ipc/generated/wire"

const MAX_EXACT = 240
const MAX_CONTEXT = 120

function clean(value: string | null | undefined, max: number): string | null {
  if (typeof value !== "string") return null
  const trimmed = value.trim()
  return trimmed === "" ? null : trimmed.slice(0, max)
}

/** 复用 pdf-progress-controller 的 sanitize 语义，保持 TextAnchor 上限一致。 */
export function sanitizeArticleTextAnchor(anchor: TextAnchorDto | null | undefined): TextAnchorDto | null {
  if (!anchor || typeof anchor !== "object") return null
  const exact = clean(anchor.exact, MAX_EXACT)
  if (!exact) return null
  return {
    exact,
    prefix: clean(anchor.prefix, MAX_CONTEXT),
    suffix: clean(anchor.suffix, MAX_CONTEXT),
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