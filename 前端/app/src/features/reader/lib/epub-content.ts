import { HavenError } from "@/lib/ipc/errors"
import type { BookChapter } from "./book-content"

/** The resource protocol currently caps one response at 32 MiB. */
export const MAX_EPUB_BYTES = 32 * 1024 * 1024
export const MAX_EPUB_ENTRIES = 10_000
export const MAX_EPUB_CHAPTER_BYTES = 4 * 1024 * 1024
export const MAX_EPUB_TEXT_BYTES = 24 * 1024 * 1024

const ZIP_END_OF_CENTRAL_DIRECTORY = 0x06054b50
const ZIP_CENTRAL_DIRECTORY = 0x02014b50
const ZIP_LOCAL_FILE = 0x04034b50
const EPUB_MIMETYPE = "application/epub+zip"

interface ZipEntry {
  name: string
  directory: boolean
  flags: number
  method: number
  compressedSize: number
  uncompressedSize: number
  crc32: number
  localHeaderOffset: number
}

interface PublicationManifestItem {
  id: string
  href: string
  mediaType: string
}

export interface EpubPublication {
  title: string | null
  chapters: BookChapter[]
}

function epubError(code: string, userMessage: string, retryable = false): HavenError {
  return new HavenError({ code, userMessage, retryable })
}

function unsupported(message: string): HavenError {
  return epubError("FORMAT_UNSUPPORTED", message)
}

function denied(message: string): HavenError {
  return epubError("SECURITY_POLICY_DENIED", message)
}

function cancelled(): HavenError {
  return epubError("OPERATION_CANCELLED", "EPUB 读取已取消")
}

function checkCancelled(signal?: AbortSignal): void {
  if (signal?.aborted) throw cancelled()
}

function readU16(view: DataView, offset: number): number {
  return view.getUint16(offset, true)
}

function readU32(view: DataView, offset: number): number {
  return view.getUint32(offset, true)
}

function decodeUtf8(bytes: Uint8Array): string {
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes)
  } catch {
    throw unsupported("EPUB 包含无法读取的文件名或正文编码")
  }
}

function safeArchiveName(name: string): string {
  if (
    !name
    || name.startsWith("/")
    || name.includes("\\")
    || name.includes("\0")
    || name.includes(":")
  ) {
    throw denied("EPUB 归档条目路径不安全")
  }
  const parts = (name.endsWith("/") ? name.slice(0, -1) : name).split("/")
  if (parts.some((part) => part.length === 0 || part === "." || part === "..")) {
    throw denied("EPUB 归档条目路径不安全")
  }
  return parts.join("/")
}

function findEndOfCentralDirectory(view: DataView): number {
  if (view.byteLength < 22) throw unsupported("EPUB 归档目录损坏")
  const first = Math.max(0, view.byteLength - 65_557)
  for (let offset = view.byteLength - 22; offset >= first; offset -= 1) {
    if (readU32(view, offset) !== ZIP_END_OF_CENTRAL_DIRECTORY) continue
    const commentLength = readU16(view, offset + 20)
    if (offset + 22 + commentLength === view.byteLength) return offset
  }
  throw unsupported("EPUB 归档目录损坏")
}

function readZipEntries(bytes: ArrayBuffer): Map<string, ZipEntry> {
  if (bytes.byteLength === 0 || bytes.byteLength > MAX_EPUB_BYTES) {
    throw unsupported("EPUB 文件超过当前版本的 32 MiB 大小限制")
  }
  const view = new DataView(bytes)
  const endOffset = findEndOfCentralDirectory(view)
  const diskNumber = readU16(view, endOffset + 4)
  const centralDisk = readU16(view, endOffset + 6)
  const entriesOnDisk = readU16(view, endOffset + 8)
  const entries = readU16(view, endOffset + 10)
  const centralSize = readU32(view, endOffset + 12)
  const centralOffset = readU32(view, endOffset + 16)
  if (
    diskNumber !== 0
    || centralDisk !== 0
    || entriesOnDisk !== entries
    || entries === 0
    || entries > MAX_EPUB_ENTRIES
    || centralOffset > view.byteLength
    || centralSize > view.byteLength - centralOffset
    || entries === 0xffff
    || centralSize === 0xffffffff
    || centralOffset === 0xffffffff
  ) {
    throw unsupported("EPUB 归档使用了当前版本不支持的目录格式")
  }

  const result = new Map<string, ZipEntry>()
  let offset = centralOffset
  for (let index = 0; index < entries; index += 1) {
    if (offset > centralOffset + centralSize - 46 || readU32(view, offset) !== ZIP_CENTRAL_DIRECTORY) {
      throw unsupported("EPUB 归档目录条目损坏")
    }
    const flags = readU16(view, offset + 8)
    const method = readU16(view, offset + 10)
    const crc32 = readU32(view, offset + 16)
    const compressedSize = readU32(view, offset + 20)
    const uncompressedSize = readU32(view, offset + 24)
    const nameLength = readU16(view, offset + 28)
    const extraLength = readU16(view, offset + 30)
    const commentLength = readU16(view, offset + 32)
    const localHeaderOffset = readU32(view, offset + 42)
    const recordLength = 46 + nameLength + extraLength + commentLength
    if (recordLength > centralOffset + centralSize - offset) throw unsupported("EPUB 归档目录条目越界")
    const nameBytes = new Uint8Array(bytes, offset + 46, nameLength)
    const rawName = decodeUtf8(nameBytes)
    const name = safeArchiveName(rawName)
    if ((flags & 0x1) !== 0 || method !== 0 && method !== 8) {
      throw denied("EPUB 归档包含加密或不受支持的压缩条目")
    }
    // Do not apply the chapter limit to every archive member here. EPUBs may
    // carry images or fonts that this text-only slice never reads. The limit
    // is enforced before/after decoding the spine XHTML in readEntry instead.
    if (compressedSize > MAX_EPUB_BYTES) throw denied("EPUB 归档条目超过安全大小限制")
    if (result.has(name)) throw denied("EPUB 归档包含重复条目名称")
    result.set(name, {
      name,
      directory: rawName.endsWith("/"),
      flags,
      method,
      compressedSize,
      uncompressedSize,
      crc32,
      localHeaderOffset,
    })
    offset += recordLength
  }
  if (offset !== centralOffset + centralSize) throw unsupported("EPUB 归档目录长度不一致")
  return result
}

function crc32(bytes: Uint8Array): number {
  let value = 0xffffffff
  for (const byte of bytes) {
    value ^= byte
    for (let bit = 0; bit < 8; bit += 1) {
      value = (value >>> 1) ^ (value & 1 ? 0xedb88320 : 0)
    }
  }
  return (value ^ 0xffffffff) >>> 0
}

async function inflateRaw(bytes: Uint8Array, maxOutputBytes: number, signal?: AbortSignal): Promise<Uint8Array> {
  checkCancelled(signal)
  const DecompressionStreamCtor = globalThis.DecompressionStream
  if (typeof DecompressionStreamCtor !== "function") {
    throw unsupported("当前 WebView 不支持 EPUB 压缩章节")
  }
  let reader: ReadableStreamDefaultReader<Uint8Array> | null = null
  const cancelReader = async () => {
    if (!reader) return
    try {
      await reader.cancel()
    } catch {
      // The stream may already be closed; the original stable error wins.
    }
  }
  try {
    const ownedBuffer = new ArrayBuffer(bytes.byteLength)
    new Uint8Array(ownedBuffer).set(bytes)
    const stream = new Blob([ownedBuffer]).stream().pipeThrough(new DecompressionStreamCtor("deflate-raw"))
    reader = stream.getReader()
    const chunks: Uint8Array[] = []
    let total = 0
    const onAbort = () => {
      void cancelReader()
    }
    signal?.addEventListener("abort", onAbort, { once: true })
    try {
      while (true) {
        checkCancelled(signal)
        const { done, value } = await reader.read()
        if (done) break
        if (!value || value.byteLength === 0) continue
        if (value.byteLength > maxOutputBytes - total) {
          await cancelReader()
          throw denied("EPUB 条目超过安全大小限制")
        }
        chunks.push(value)
        total += value.byteLength
      }
      checkCancelled(signal)
    } finally {
      signal?.removeEventListener("abort", onAbort)
    }
    const inflated = new Uint8Array(total)
    let offset = 0
    for (const chunk of chunks) {
      inflated.set(chunk, offset)
      offset += chunk.byteLength
    }
    return inflated
  } catch (error) {
    if (signal?.aborted) {
      await cancelReader()
      throw cancelled()
    }
    if (error instanceof HavenError) throw error
    throw unsupported("EPUB 章节压缩数据损坏")
  } finally {
    reader?.releaseLock()
  }
}

async function readEntry(
  bytes: ArrayBuffer,
  entries: Map<string, ZipEntry>,
  name: string,
  signal?: AbortSignal,
  maxOutputBytes = MAX_EPUB_CHAPTER_BYTES,
): Promise<Uint8Array> {
  checkCancelled(signal)
  const entry = entries.get(name)
  if (!entry) throw unsupported("EPUB 引用的章节不存在")
  const view = new DataView(bytes)
  const header = entry.localHeaderOffset
  if (
    header > view.byteLength - 30
    || readU32(view, header) !== ZIP_LOCAL_FILE
  ) {
    throw unsupported("EPUB 本地条目头损坏")
  }
  const nameLength = readU16(view, header + 26)
  const extraLength = readU16(view, header + 28)
  const localFlags = readU16(view, header + 6)
  const localMethod = readU16(view, header + 8)
  const dataOffset = header + 30 + nameLength + extraLength
  if (
    dataOffset > view.byteLength
    || header + 30 + nameLength > view.byteLength
    || entry.compressedSize > view.byteLength - dataOffset
  ) {
    throw unsupported("EPUB 章节数据越界")
  }
  const localName = safeArchiveName(decodeUtf8(new Uint8Array(bytes, header + 30, nameLength)))
  if (localName !== entry.name) throw unsupported("EPUB 本地条目名称不一致")
  if (entry.directory) throw unsupported("EPUB 引用了目录条目")
  if (localFlags !== entry.flags || localMethod !== entry.method) throw unsupported("EPUB 本地条目属性不一致")
  if (entry.uncompressedSize > maxOutputBytes) throw denied("EPUB 条目超过安全大小限制")
  if (entry.method === 0 && entry.compressedSize > maxOutputBytes) throw denied("EPUB 条目超过安全大小限制")
  const compressed = new Uint8Array(bytes, dataOffset, entry.compressedSize)
  const output = entry.method === 0
    ? new Uint8Array(compressed)
    : await inflateRaw(compressed, maxOutputBytes, signal)
  if (output.byteLength !== entry.uncompressedSize || crc32(output) !== entry.crc32) {
    throw unsupported("EPUB 章节校验失败")
  }
  if (output.byteLength > maxOutputBytes) throw denied("EPUB 条目超过安全大小限制")
  return output
}

function stripXmlUnsafeConstructs(xml: string): string {
  if (/<!(?:DOCTYPE|ENTITY)\b/i.test(xml)) throw denied("EPUB XML 包含不允许的实体声明")
  return xml.replace(/<!--(?:.|[\r\n])*?-->/g, "")
}

/**
 * One quote-aware tag scan shared by attribute parsing, title extraction,
 * anchor annotation and plain-text conversion, mirroring the backend
 * `reader_search.rs` rules: a tag ends at the first `>` outside `"`/`'`
 * quotes, comments run to `-->` (or swallow the rest of the document), and a
 * `<` that never closes is literal text. Attribute tokens are split on
 * whitespace so `data-id="x"` is never read as `id` and text inside one
 * quoted value is never read as another attribute.
 */
type MarkupToken =
  | { kind: "text"; start: number; end: number }
  | { kind: "comment"; start: number; end: number }
  | { kind: "tag"; start: number; end: number; name: string; closing: boolean; attributes: Map<string, string> }

const ATTRIBUTE_NAME = /[A-Za-z_:][\w:.-]*/y

function attributeNameAt(tag: string, offset: number): string | null {
  ATTRIBUTE_NAME.lastIndex = offset
  const match = ATTRIBUTE_NAME.exec(tag)
  return match ? match[0].toLowerCase() : null
}

/** Parse attributes from the end of the tag name; quoted `>` never terminates. */
function parseTagAttributes(tag: string, cursor: number): Map<string, string> {
  const attributes = new Map<string, string>()
  const length = tag.length
  while (cursor < length) {
    const ch = tag[cursor]
    if (/\s/.test(ch) || ch === "/") {
      cursor += 1
      continue
    }
    const name = attributeNameAt(tag, cursor)
    if (!name) {
      cursor += 1
      continue
    }
    cursor += name.length
    while (cursor < length && /\s/.test(tag[cursor])) cursor += 1
    if (tag[cursor] !== "=") continue
    cursor += 1
    while (cursor < length && /\s/.test(tag[cursor])) cursor += 1
    const quote = tag[cursor]
    if (quote !== '"' && quote !== "'") {
      let valueEnd = cursor
      while (valueEnd < length && !/[\s>]/.test(tag[valueEnd])) valueEnd += 1
      if (!attributes.has(name)) attributes.set(name, tag.slice(cursor, valueEnd))
      cursor = valueEnd
      continue
    }
    const valueEnd = tag.indexOf(quote, cursor + 1)
    if (valueEnd < 0) break
    if (!attributes.has(name)) attributes.set(name, tag.slice(cursor + 1, valueEnd))
    cursor = valueEnd + 1
  }
  return attributes
}

function findTagEnd(html: string, cursor: number): number {
  let quote = ""
  while (cursor < html.length) {
    const ch = html[cursor]
    if (quote) {
      if (ch === quote) quote = ""
    } else if (ch === '"' || ch === "'") {
      quote = ch
    } else if (ch === ">") {
      return cursor
    }
    cursor += 1
  }
  return -1
}

function* lexMarkup(html: string): Generator<MarkupToken> {
  let cursor = 0
  while (cursor < html.length) {
    const tagStart = html.indexOf("<", cursor)
    if (tagStart < 0) {
      yield { kind: "text", start: cursor, end: html.length }
      return
    }
    if (tagStart > cursor) yield { kind: "text", start: cursor, end: tagStart }
    if (html.startsWith("<!--", tagStart)) {
      const commentEnd = html.indexOf("-->", tagStart + 4)
      if (commentEnd < 0) {
        // A malformed comment has no end: discard the remainder rather than
        // exposing active markup.
        yield { kind: "comment", start: tagStart, end: html.length }
        return
      }
      yield { kind: "comment", start: tagStart, end: commentEnd + 3 }
      cursor = commentEnd + 3
      continue
    }
    const tagEnd = findTagEnd(html, tagStart + 1)
    if (tagEnd < 0) {
      yield { kind: "text", start: tagStart, end: html.length }
      return
    }
    const body = html.slice(tagStart + 1, tagEnd)
    // A closing tag's leading `/` and any spaces before it belong to the
    // marker, not the name; `< /p>` would otherwise yield an empty name and
    // never close the skipped or title block.
    let nameStart = 0
    while (nameStart < body.length && /\s/.test(body[nameStart])) nameStart += 1
    const closing = body[nameStart] === "/"
    if (closing) nameStart += 1
    let nameEnd = nameStart
    while (nameEnd < body.length && !/[\s/]/.test(body[nameEnd])) nameEnd += 1
    yield {
      kind: "tag",
      start: tagStart,
      end: tagEnd + 1,
      name: body.slice(nameStart, nameEnd).toLowerCase(),
      closing,
      attributes: parseTagAttributes(body, nameEnd),
    }
    cursor = tagEnd + 1
  }
}

function findTags(xml: string, tagName: string): Map<string, string>[] {
  const tags: Map<string, string>[] = []
  for (const token of lexMarkup(xml)) {
    if (token.kind !== "tag" || token.closing) continue
    if (token.name === tagName || token.name.endsWith(`:${tagName}`)) tags.push(token.attributes)
  }
  return tags
}

function xmlEntityText(value: string): string {
  return value
    .replace(/&(#x[0-9a-f]+|#\d+|amp|lt|gt|quot|apos);/gi, (_whole, entity: string) => {
      if (entity.toLowerCase() === "amp") return "&"
      if (entity.toLowerCase() === "lt") return "<"
      if (entity.toLowerCase() === "gt") return ">"
      if (entity.toLowerCase() === "quot") return '"'
      if (entity.toLowerCase() === "apos") return "'"
      return decodeNumericEntity(entity)
    })
}

function decodeNumericEntity(entity: string): string {
  const code = entity.toLowerCase().startsWith("#x")
    ? Number.parseInt(entity.slice(2), 16)
    : Number.parseInt(entity.slice(1), 10)
  if (
    !Number.isInteger(code)
    || code < 0
    || code > 0x10ffff
    || (code >= 0xd800 && code <= 0xdfff)
  ) {
    throw unsupported("EPUB 文本包含无效数字实体")
  }
  return String.fromCodePoint(code)
}

function resolveHref(directory: string, rawHref: string): string {
  const path = xmlEntityText(rawHref).split("#", 1)[0]
  if (!path) throw denied("EPUB 文档引用路径不安全")
  let decoded: string
  try {
    decoded = decodeURIComponent(path)
  } catch {
    throw unsupported("EPUB 引用路径编码无效")
  }
  // OPC hrefs are percent-encoded (EPUB 3). Validation must run on the decoded
  // form so encoded traversal (%2e%2e) and separators (%2f %5c %00 %3A) are
  // caught by the same deny rules as their literal counterparts.
  if (
    decoded.startsWith("/")
    || decoded.includes("\\")
    || decoded.includes("\0")
    || decoded.includes("://")
    || decoded.includes("?")
  ) {
    throw denied("EPUB 文档引用路径不安全")
  }
  return safeArchiveName(directory ? `${directory}/${decoded}` : decoded)
}

/** The first boundary-valid opening tag decides that tag name (backend parity). */
function extractTagText(html: string, names: readonly string[]): string | null {
  const tags = [...lexMarkup(html)].filter((token) => token.kind === "tag") as Extract<MarkupToken, { kind: "tag" }>[]
  for (const name of names) {
    const tagsByName = tags.filter((token) => token.name === name)
    const openingIndex = tagsByName.findIndex((token) => !token.closing)
    if (openingIndex < 0) continue
    const opening = tagsByName[openingIndex]
    const closing = tagsByName.slice(openingIndex + 1).find((token) => token.closing)
    // An unterminated candidate leaves the scan looking for a later pair.
    if (!closing) continue
    const value = htmlToPlainText(html.slice(opening.end, closing.start))
    if (value) return value.split("\n", 1)[0].trim().slice(0, 160)
  }
  return null
}

function htmlEntityText(value: string): string {
  return value
    .replace(/&(#x[0-9a-f]+|#\d+|nbsp|amp|lt|gt|quot|apos);/gi, (_whole, entity: string) => {
      const normalized = entity.toLowerCase()
      if (normalized === "nbsp") return " "
      if (normalized === "amp") return "&"
      if (normalized === "lt") return "<"
      if (normalized === "gt") return ">"
      if (normalized === "quot") return '"'
      if (normalized === "apos") return "'"
      return decodeNumericEntity(normalized)
    })
}

const ACTIVE_TAGS = new Set(["head", "script", "style", "iframe", "object", "embed", "svg", "math", "form", "textarea"])
const BOUNDARY_TAGS = new Set(["p", "div", "section", "article", "header", "footer", "h1", "h2", "h3", "h4", "h5", "h6", "li", "blockquote", "pre", "tr", "td", "th"])

/** Convert untrusted XHTML to plain text; scripts and active embeds never reach React. */
export function htmlToPlainText(html: string): string {
  return convertMarkupToPlainText(html)
}

function convertMarkupToPlainText(html: string): string {
  const chunks: string[] = []
  let skippedTag = ""
  for (const token of lexMarkup(html)) {
    if (token.kind === "comment") continue
    if (token.kind === "text") {
      if (!skippedTag) chunks.push(html.slice(token.start, token.end))
      continue
    }
    if (skippedTag) {
      if (token.closing && token.name === skippedTag) skippedTag = ""
      continue
    }
    const selfClosing = /\/\s*>$/.test(html.slice(token.start, token.end))
    if (ACTIVE_TAGS.has(token.name)) {
      if (!token.closing && !selfClosing) skippedTag = token.name
      continue
    }
    if (token.name === "br") {
      chunks.push("\n")
      continue
    }
    if (BOUNDARY_TAGS.has(token.name)) chunks.push("\n")
  }
  return htmlEntityText(chunks.join(""))
    .replace(/\u00a0/g, " ")
    .replace(/[ \t]+/g, " ")
    .replace(/\n[ \t]+/g, "\n")
    .replace(/[ \t]+\n/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim()
}

/**
 * Gutenberg 导出常把「START OF THE PROJECT GUTENBERG EBOOK」头页整节、
 * 以及「*** END OF … ***」之后的许可与捐赠样板拼进正文文档。
 * 阅读流按标记裁剪；磁盘上的 EPUB 文件保持原样，不修改用户数据。
 *
 * 只有真实样板形态（`*** … ***`）的 START/END 标记才裁剪：正文里偶然提到的
 * 书名或句子不会被当成标记。标记与正文同处一个文档时裁掉标记之前的头页并
 * 保留其后的正文；裁剪后只有真正没有内容的文档才跳过，其余内容（包括无法
 * 判定的许可文案）一律保留，不做关键词猜测。
 *
 * 后端 `reader_search.rs` 必须保持同一套标记与裁剪语义，否则同一章的段落
 * 下标会与阅读器 DOM 漂移。
 */
const PG_START_MARKER = /\*{3}\s*START OF (?:THE|THIS) PROJECT GUTENBERG EBOOK\b[^\r\n]{0,200}?\*{3}/i
const PG_END_MARKER = /\*{3}\s*END OF THE PROJECT GUTENBERG EBOOK/i

/**
 * Markers are often wrapped in an HTML comment. Cropping through the marker
 * alone would leave an unterminated comment (or a stray `-->`) in the body, so
 * resolve the enclosing comment bounds before slicing.
 */
function enclosingCommentStart(html: string, index: number): number {
  const commentStart = html.lastIndexOf("<!--", index)
  if (commentStart < 0) return -1
  if (html.lastIndexOf("-->", index) > commentStart) return -1
  return commentStart
}

function enclosingCommentEnd(html: string, index: number): number {
  if (enclosingCommentStart(html, index) < 0) return -1
  const commentEnd = html.indexOf("-->", index)
  return commentEnd < 0 ? -1 : commentEnd + 3
}

function trimGutenbergBoilerplate(html: string): string | null {
  let body = html
  const startMatch = PG_START_MARKER.exec(body)
  if (startMatch) {
    const markerEnd = startMatch.index + startMatch[0].length
    const commentEnd = enclosingCommentEnd(body, startMatch.index)
    body = body.slice(commentEnd > markerEnd ? commentEnd : markerEnd)
  }
  const endMatch = PG_END_MARKER.exec(body)
  if (endMatch) {
    const commentStart = enclosingCommentStart(body, endMatch.index)
    body = body.slice(0, commentStart >= 0 ? commentStart : endMatch.index)
  }
  // Cropping is the only reason to drop a document: what survives is kept
  // verbatim, including licence prose this parser cannot classify.
  return htmlToPlainText(body) ? body : null
}

/** Project inert anchor markers through the same whole-document text conversion. */
function buildAnchorMap(html: string, paragraphs: readonly string[], titleRemoved: boolean): Readonly<Record<string, number>> {
  const result: Record<string, number> = Object.create(null) as Record<string, number>
  if (paragraphs.length === 0) return result
  let markerPrefix = "\uE000HAVEN"
  while (html.includes(markerPrefix)) markerPrefix += "\uE000"
  const anchors: string[] = []
  const annotated: string[] = []
  let cursor = 0
  for (const token of lexMarkup(html)) {
    if (token.kind === "text" || token.kind === "comment") continue
    annotated.push(html.slice(cursor, token.end))
    cursor = token.end
    // Attribute tokens come from the shared lexer, so `data-id` is not `id`
    // and text inside an unrelated quoted value is never an anchor.
    if (token.closing || ACTIVE_TAGS.has(token.name)) continue
    for (const name of ["id", "name"]) {
      const raw = token.attributes.get(name)
      if (raw === undefined) continue
      const anchor = htmlEntityText(raw).trim()
      if (!anchor || anchor.length > 512
        || [...anchor].some((ch) => ch.codePointAt(0)! < 0x20 || /[\\/?#<>"']/.test(ch))) continue
      annotated.push(`${markerPrefix}${anchors.length}\uE001`)
      anchors.push(anchor)
    }
  }
  annotated.push(html.slice(cursor))
  const markerPattern = new RegExp(`${markerPrefix}(\\d+)\uE001`, "g")
  let paragraphIndex = 0
  for (const chunk of htmlToPlainText(annotated.join("")).split(/\n{2,}/)) {
    const indexes = [...chunk.matchAll(markerPattern)].map((match) => Number(match[1]))
    const content = chunk.replace(markerPattern, "").replace(/\s*\n\s*/g, " ").trim()
    const target = Math.max(0, Math.min(paragraphs.length - 1, paragraphIndex - Number(titleRemoved)))
    for (const index of indexes) {
      const anchor = anchors[index]
      if (!Object.prototype.hasOwnProperty.call(result, anchor)) result[anchor] = target
    }
    // A container's marker can occupy an empty chunk before its first child.
    // It points forward but must never count as a rendered paragraph itself.
    if (content) paragraphIndex += 1
  }
  return result
}

function chapterFromXhtml(index: number, spineCount: number, item: PublicationManifestItem, html: string, sourceHref: string): BookChapter | null {
  const trimmedHtml = trimGutenbergBoilerplate(html)
  if (!trimmedHtml) return null
  const text = htmlToPlainText(trimmedHtml)
  if (!text) return null
  // The title must come from the surviving content, otherwise a cropped header
  // page would name a chapter whose text now starts at the real story.
  const title = extractTagText(trimmedHtml, ["h1", "h2", "h3", "title"])
    ?? `第 ${index + 1} 章`
  const paragraphs = text
    .split(/\n{2,}/)
    .map((paragraph) => paragraph.replace(/\s*\n\s*/g, " ").trim())
    .filter(Boolean)
  const titleRemoved = paragraphs[0] === title
  if (titleRemoved) paragraphs.shift()
  if (paragraphs.length === 0) return null
  return {
    id: `epub-chapter-${index + 1}`,
    kicker: `EPUB · ${index + 1}`,
    title: title || item.id,
    paragraphs,
    sourceHref,
    spineIndex: index,
    spineCount,
    anchorMap: buildAnchorMap(trimmedHtml, paragraphs, titleRemoved),
  }
}

/**
 * Parse a scanner-approved EPUB after it has crossed the owner-bound Session
 * resource protocol. The parser repeats package/spine checks because the
 * resource may have changed since scan; no path or archive entry leaves this
 * module, and chapter XHTML is reduced to inert text before React renders it.
 */
export async function parseEpubBook(bytes: ArrayBuffer, signal?: AbortSignal): Promise<EpubPublication> {
  checkCancelled(signal)
  const entries = readZipEntries(bytes)
  const firstEntry = entries.keys().next().value as string | undefined
  if (firstEntry !== "mimetype") throw unsupported("EPUB mimetype 条目必须是归档目录第一项")
  const mimetype = entries.get("mimetype")
  if (!mimetype || mimetype.method !== 0 || mimetype.uncompressedSize !== EPUB_MIMETYPE.length) {
    throw unsupported("EPUB mimetype 条目无效")
  }
  const mimetypeBytes = await readEntry(bytes, entries, "mimetype", signal)
  if (decodeUtf8(mimetypeBytes) !== EPUB_MIMETYPE) throw unsupported("EPUB mimetype 内容无效")

  const containerBytes = await readEntry(bytes, entries, "META-INF/container.xml", signal)
  const containerXml = stripXmlUnsafeConstructs(decodeUtf8(containerBytes))
  const rootfile = findTags(containerXml, "rootfile").find((attributes) => (
    attributes.get("media-type")?.toLowerCase() === "application/oebps-package+xml"
    || attributes.has("full-path")
  ))
  const opfPath = rootfile?.get("full-path")
  if (!opfPath) throw unsupported("EPUB 缺少 package 文档")
  const safeOpfPath = safeArchiveName(xmlEntityText(opfPath))
  const opfBytes = await readEntry(bytes, entries, safeOpfPath, signal)
  const opfXml = stripXmlUnsafeConstructs(decodeUtf8(opfBytes))
  if (findTags(opfXml, "package").length === 0 || findTags(opfXml, "manifest").length === 0 || findTags(opfXml, "spine").length === 0) {
    throw unsupported("EPUB package 文档结构无效")
  }

  const opfDirectory = safeOpfPath.includes("/") ? safeOpfPath.slice(0, safeOpfPath.lastIndexOf("/")) : ""
  const manifest = new Map<string, PublicationManifestItem>()
  for (const attributes of findTags(opfXml, "item")) {
    const id = attributes.get("id")
    const href = attributes.get("href")
    const mediaType = attributes.get("media-type")?.split(";", 1)[0].trim().toLowerCase()
    if (!id || !href || !mediaType) continue
    if (manifest.has(id)) throw denied("EPUB manifest 包含重复条目")
    manifest.set(id, { id, href, mediaType })
  }
  if (manifest.size === 0) throw unsupported("EPUB package 缺少 manifest")

  const spineAttributes = findTags(opfXml, "itemref")
  if (spineAttributes.length === 0) throw unsupported("EPUB package 缺少 spine")
  const chapters: BookChapter[] = []
  let totalTextBytes = 0
  for (const [index, attributes] of spineAttributes.entries()) {
    checkCancelled(signal)
    const idref = attributes.get("idref")
    if (!idref) throw unsupported("EPUB spine 引用无效")
    const item = manifest.get(idref)
    if (!item || (item.mediaType !== "application/xhtml+xml" && item.mediaType !== "text/html")) {
      throw unsupported("EPUB spine 包含不支持的文档")
    }
    const path = resolveHref(opfDirectory, item.href)
    // A spine document that cannot be read fails the whole book: silently
    // dropping it would hide content and shift what the reader shows.
    const chapterBytes = await readEntry(bytes, entries, path, signal)
    totalTextBytes += chapterBytes.byteLength
    if (totalTextBytes > MAX_EPUB_TEXT_BYTES) throw denied("EPUB 正文总大小超过安全限制")
    const chapterHtml = decodeUtf8(chapterBytes)
    const chapter = chapterFromXhtml(index, spineAttributes.length, item, chapterHtml, path)
    if (chapter) chapters.push(chapter)
  }
  const title = extractTagText(opfXml, ["dc:title", "title"])
  return { title, chapters }
}
