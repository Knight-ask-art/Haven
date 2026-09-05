import { HavenError } from "@/lib/ipc/errors"
import DOMPurify, { type Config } from "dompurify"

export const MAX_ARTICLE_TEXT_BYTES = 8 * 1024 * 1024

export interface ArticleParagraph {
  id: string
  text: string
  translation?: string
}

export interface ArticleSection {
  id: string
  level: 1 | 2
  title: string
  dek?: string
  paragraphs: ArticleParagraph[]
  quote?: string
}

export interface ArticleDocument {
  title: string
  sections: ArticleSection[]
  characterCount: number
  format: "text" | "markdown" | "html"
  sanitizedHtml?: string
}

export interface ArticleOutlineItem {
  id: string
  title: string
  level: 1 | 2
}

function unsupportedText(userMessage: string): HavenError {
  return new HavenError({ code: "FORMAT_UNSUPPORTED", userMessage, retryable: false })
}

export function decodeArticleText(
  bytes: ArrayBuffer,
  contentType: string,
  maxBytes = MAX_ARTICLE_TEXT_BYTES,
): string {
  const normalizedType = contentType.split(";", 1)[0].trim().toLowerCase()
  if (normalizedType !== "text/plain" && normalizedType !== "text/markdown" && normalizedType !== "text/html") {
    throw unsupportedText("当前文章阅读器支持纯文本、Markdown 和 HTML")
  }
  if (bytes.byteLength > maxBytes) {
    throw unsupportedText("文章文件过大，暂时无法打开")
  }

  try {
    const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes)
    if (text.includes("\0")) throw unsupportedText("文章内容格式无效")
    return text
  } catch (error) {
    if (error instanceof HavenError) throw error
    throw unsupportedText("文章不是有效的 UTF-8 编码")
  }
}

const HTML_SANITIZE_CONFIG: Config = {
  ALLOW_DATA_ATTR: false,
  ALLOWED_ATTR: ["class", "id", "title"],
  ALLOWED_TAGS: [
    "a", "article", "blockquote", "br", "code", "dd", "div", "dl", "dt",
    "em", "h1", "h2", "h3", "h4", "h5", "h6", "header", "hr", "li", "main",
    "ol", "p", "pre", "section", "strong", "table", "tbody", "td", "tfoot",
    "th", "thead", "tr", "ul",
  ],
  FORBID_TAGS: ["embed", "form", "iframe", "img", "input", "link", "meta", "object", "script", "style", "svg", "template"],
  KEEP_CONTENT: true,
  RETURN_TRUSTED_TYPE: false,
}

export const ARTICLE_HTML_BLOCK_TAGS = ["h1", "h2", "h3", "h4", "h5", "h6", "p", "blockquote", "li", "td", "th"] as const
const ARTICLE_HTML_PARAGRAPH_TAGS = ARTICLE_HTML_BLOCK_TAGS.slice(6)

/**
 * Sanitize untrusted local HTML before it enters the WebView DOM.  The
 * allowlist intentionally excludes every resource-bearing attribute/tag, so
 * an imported document cannot fetch files, execute script, or replace CSP.
 */
export function sanitizeArticleHtml(html: string): string {
  if (typeof document === "undefined") {
    throw unsupportedText("当前环境无法安全解析 HTML")
  }
  return DOMPurify.sanitize(html, HTML_SANITIZE_CONFIG)
}

function plainTextFromHtml(html: string): string {
  const template = document.createElement("template")
  template.innerHTML = html
  const lines: string[] = []
  for (const element of template.content.querySelectorAll(ARTICLE_HTML_BLOCK_TAGS.join(","))) {
    const text = element.textContent?.replace(/\s+/g, " ").trim()
    if (!text) continue
    if (/^h[1-6]$/i.test(element.tagName)) {
      const prefix = element.tagName.toLowerCase() === "h1" ? "#" : "##"
      lines.push(`${prefix} ${text}`)
    } else {
      lines.push(text)
    }
    lines.push("")
  }
  return lines.join("\n")
}

function addStableHtmlBlockIds(html: string, documentModel: ArticleDocument): string {
  const template = document.createElement("template")
  template.innerHTML = html
  const headings = Array.from(template.content.querySelectorAll("h1,h2,h3,h4,h5,h6"))
  // Keep this order identical to plainTextFromHtml. List items and table cells
  // are paragraphs in the article model too; omitting them shifts every later
  // marker onto the wrong DOM block.
  const blocks = Array.from(template.content.querySelectorAll(ARTICLE_HTML_PARAGRAPH_TAGS.join(",")))
  const sections = documentModel.sections
  headings.forEach((heading, index) => {
    const section = sections[index]
    if (section) heading.id = section.id
  })
  let paragraphIndex = 0
  for (const section of sections) {
    for (const paragraph of section.paragraphs) {
      const block = blocks[paragraphIndex]
      if (block) block.setAttribute("data-article-block-id", paragraph.id)
      paragraphIndex += 1
    }
  }
  return template.innerHTML
}

interface RawArticleSection {
  level: 1 | 2
  title: string
  paragraphs: string[]
}

function heading(line: string): { level: 1 | 2; title: string } | null {
  const markdown = /^(#{1,6})\s+(.+)$/.exec(line)
  if (!markdown) return null
  const title = markdown[2].trim()
  if (!title || title.length > 200) return null
  return { level: markdown[1].length === 1 ? 1 : 2, title }
}

function hashText(value: string): string {
  let hash = 0x811c9dc5
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index)
    hash = Math.imul(hash, 0x01000193)
  }
  return (hash >>> 0).toString(36)
}

function uniqueBlockId(prefix: string, value: string, used: Map<string, number>): string {
  const base = `${prefix}-${hashText(value.trim().replace(/\s+/g, " ").toLowerCase())}`
  const occurrence = (used.get(base) ?? 0) + 1
  used.set(base, occurrence)
  return occurrence === 1 ? base : `${base}-${occurrence}`
}

export function parseArticleText(text: string, format: "text" | "markdown" = "text"): ArticleDocument | null {
  const lines = text.replace(/\r\n?/g, "\n").split("\n")
  const firstContentIndex = lines.findIndex((line) => line.trim().length > 0)
  if (firstContentIndex < 0) return null

  const firstLine = lines[firstContentIndex].trim()
  const firstHeading = heading(firstLine)
  const title = firstHeading?.title ?? firstLine
  const rawSections: RawArticleSection[] = []
  let current: RawArticleSection | null = {
    level: firstHeading?.level ?? 1,
    title,
    paragraphs: [],
  }
  let paragraphLines: string[] = []

  const ensureCurrent = (): RawArticleSection => {
    if (current === null) {
      current = { level: 1, title, paragraphs: [] }
    }
    return current
  }
  const flushParagraph = () => {
    if (paragraphLines.length === 0) return
    ensureCurrent().paragraphs.push(paragraphLines.join(format === "markdown" ? "\n" : " "))
    paragraphLines = []
  }
  const commitSection = () => {
    flushParagraph()
    if (current !== null) rawSections.push(current)
    current = null
  }

  for (const rawLine of lines.slice(firstContentIndex + 1)) {
    const line = rawLine.trim()
    if (!line) {
      flushParagraph()
      continue
    }
    const nextHeading = heading(line)
    if (nextHeading) {
      commitSection()
      current = { ...nextHeading, paragraphs: [] }
      continue
    }
    paragraphLines.push(line)
  }
  commitSection()

  const sectionIds = new Map<string, number>()
  const sections = rawSections.map((section) => {
    const sectionId = uniqueBlockId("article-section", `${section.level}:${section.title}`, sectionIds)
    const paragraphIds = new Map<string, number>()
    return {
      id: sectionId,
      level: section.level,
      title: section.title,
      paragraphs: section.paragraphs.map((paragraph) => ({
        id: uniqueBlockId(`${sectionId}-paragraph`, paragraph, paragraphIds),
        text: paragraph,
      })),
    }
  })

  return {
    title,
    sections,
    characterCount: sections.reduce(
      (total, section) => total + section.title.length + section.paragraphs.reduce((sum, paragraph) => sum + paragraph.text.length, 0),
      0,
    ),
    format,
  }
}

export function parseArticleContent(text: string, contentType: string): ArticleDocument | null {
  const normalizedType = contentType.split(";", 1)[0].trim().toLowerCase()
  if (normalizedType === "text/html") {
    const sanitizedHtml = sanitizeArticleHtml(text)
    const parsed = parseArticleText(plainTextFromHtml(sanitizedHtml), "text")
    if (!parsed) return null
    return {
      ...parsed,
      format: "html",
      sanitizedHtml: addStableHtmlBlockIds(sanitizedHtml, parsed),
    }
  }
  if (normalizedType === "text/markdown") return parseArticleText(text, "markdown")
  return parseArticleText(text, "text")
}

export function articleOutline(document: ArticleDocument): ArticleOutlineItem[] {
  return document.sections.map(({ id, title, level }) => ({ id, title, level }))
}
