import { HavenError } from "@/lib/ipc/errors"

export const MAX_BOOK_TEXT_BYTES = 8 * 1024 * 1024

export interface BookChapter {
  id: string
  kicker: string
  title: string
  paragraphs: string[]
  quote?: string
  /** EPUB-only source identity retained for internal TOC/anchor resolution. */
  sourceHref?: string
  /** EPUB fragment/name anchors mapped to the rendered paragraph index. */
  anchorMap?: Readonly<Record<string, number>>
}

export type BookContentFormat = "text" | "markdown" | "epub"

function unsupportedText(userMessage: string): HavenError {
  return new HavenError({ code: "FORMAT_UNSUPPORTED", userMessage, retryable: false })
}

export function decodeBookText(
  bytes: ArrayBuffer,
  contentType: string,
  maxBytes = MAX_BOOK_TEXT_BYTES,
): string {
  const normalizedType = contentType.split(";", 1)[0].trim().toLowerCase()
  if (normalizedType !== "text/plain" && normalizedType !== "text/markdown") {
    throw unsupportedText("当前图书阅读器支持 TXT 和 Markdown")
  }
  if (bytes.byteLength > maxBytes) {
    throw unsupportedText("文本文件过大，暂时无法打开")
  }

  let text: string
  try {
    text = new TextDecoder("utf-8", { fatal: true }).decode(bytes)
  } catch {
    try {
      text = new TextDecoder("gb18030", { fatal: true }).decode(bytes)
    } catch {
      throw unsupportedText("文本不是支持的 UTF-8 或 GB18030 编码")
    }
  }
  if (text.includes("\0")) throw unsupportedText("文本内容格式无效")
  return text
}

function chapterTitle(line: string): string | null {
  if (line.length > 160) return null
  const markdown = /^#{1,6}\s+(.+)$/.exec(line)
  if (markdown) return markdown[1].trim()
  if (/^第\s*[0-9一二三四五六七八九十百千万零〇两]+\s*[章节卷篇部](?:[\s.:：·-].*)?$/.test(line)) return line
  if (/^(?:序章|楔子|尾声|后记|前言|引言)(?:[\s.:：·-].*)?$/.test(line)) return line
  if (/^(?:chapter|part|book)\s+[0-9ivxlcdm]+(?:[\s.:：·-].*)?$/i.test(line)) return line
  return null
}

export function parseBookText(text: string, format: BookContentFormat = "text"): BookChapter[] {
  const chapters: Array<{ title: string; paragraphs: string[] }> = []
  let title = "全文"
  let paragraphs: string[] = []
  let paragraphLines: string[] = []

  const flushParagraph = () => {
    if (paragraphLines.length === 0) return
    paragraphs.push(paragraphLines.join(format === "markdown" ? "\n" : " "))
    paragraphLines = []
  }

  const commit = () => {
    if (paragraphs.length === 0 && title === "全文") return
    chapters.push({ title, paragraphs })
  }

  for (const rawLine of text.replace(/\r\n?/g, "\n").split("\n")) {
    const line = rawLine.trim()
    if (!line) {
      flushParagraph()
      continue
    }
    const heading = chapterTitle(line)
    if (heading) {
      flushParagraph()
      commit()
      title = heading
      paragraphs = []
      continue
    }
    paragraphLines.push(line)
  }
  flushParagraph()
  commit()

  return chapters.map((chapter, index) => ({
    id: `chapter-${index + 1}`,
    kicker: `Chapter ${index + 1}`,
    title: chapter.title,
    paragraphs: chapter.paragraphs,
  }))
}
