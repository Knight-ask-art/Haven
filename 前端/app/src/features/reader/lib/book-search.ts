import type { TextAnchorDto } from "@/lib/ipc/generated/wire"
import type { BookChapter } from "./book-content"

/**
 * 图书全文检索（EPUB/TXT/Markdown；PDF 走 pdf-search.ts）。
 *
 * 正文解析目前位于前端（epub-content.ts/book-content.ts 是迁移债务），
 * 检索在同一数据源上实现，避免后端重复抽取造成双源漂移；后端检索
 * 待正文抽取迁移到 Rust 后跟进（Tech Debt 登记，契约 §19.4 修订记录）。
 *
 * 实现遵循题2方案：
 * - normalizeForMatch 与 tokenizeForRank 分离（匹配用折叠文本，排名用术语）；
 * - 稀疏 checkpoint 映射归一化 offset → 原文 offset（段落级 1:1 映射 + 段间 -1）；
 * - 全局 BinaryHeap 语义：每章 top 20，全局 top 200（按 TF-IDF 分数降序）；
 * - 术语分数 = ln((D+1)/(df+1)) + 1（D=章数，df=含术语章数）；
 * - exact 存原文 12..240 字符，prefix/suffix 各 30 字符；
 * - 每扫描 4KiB 归一化字符检查一次取消。
 */

export const MAX_BOOK_SEARCH_HITS = 200
export const MAX_HITS_PER_CHAPTER = 20
export const MAX_QUERY_CHARS = 128
export const MAX_PREFIX_SUFFIX_CHARS = 30
export const MIN_EXACT_CHARS = 12
export const MAX_EXACT_CHARS = 240
const CANCEL_CHECK_INTERVAL = 4096

export interface BookSearchHit {
  chapterId: string
  chapterTitle: string
  chapterIndex: number
  /** 命中所在段落（chapters[chapterIndex].paragraphs 下标）。 */
  paragraphIndex: number
  /** 命中起点在章 norm 文本中的比例（0..1），用于滚动定位。 */
  progressionInChapter: number
  exact: string
  prefix: string | null
  suffix: string | null
  score: number
}

export interface BookSearchIndexChapter {
  id: string
  title: string
  /** 归一化全文（段落以单个空格连接；段间空格映射为 -1）。 */
  norm: string
  /** norm 字符 i → 章原文（paragraphs.join("\n\n")）字符偏移；段间空格为 -1。 */
  map: number[]
  /** 每段起点在 join 原文中的偏移（段长 + 2 分隔符）。 */
  paragraphStarts: number[]
  originalLength: number
}

export interface BookSearchIndex {
  chapters: BookSearchIndexChapter[]
  /** 有内容的章数（D）。 */
  documents: number
  /** 每个术语的出现章数（df）。 */
  termDocumentFrequencies: ReadonlyMap<string, number>
}

export function isWhitespaceCode(code: number): boolean {
  return code === 0x20
    || code === 0x09
    || code === 0x0a
    || code === 0x0d
    || code === 0x0c
    || code === 0x3000
}

/** 全角 → 半角（U+FF01..FF5E → 0x21..0x7E，U+3000 → 空格）。 */
export function fullWidthToHalfWidth(text: string): string {
  let result = ""
  for (const character of text) {
    const code = character.codePointAt(0)!
    if (code >= 0xff01 && code <= 0xff5e) {
      result += String.fromCodePoint(code - 0xfee0)
    } else if (code === 0x3000) {
      result += " "
    } else {
      result += character
    }
  }
  return result
}

/** 匹配用归一化：全角→半角、空白折叠、小写。纯函数（查询输入用）。 */
export function normalizeForMatch(text: string): string {
  let result = ""
  let pendingSpace = false
  for (const character of fullWidthToHalfWidth(text)) {
    const code = character.codePointAt(0)!
    if (isWhitespaceCode(code)) {
      pendingSpace = true
      continue
    }
    if (pendingSpace && result.length > 0) result += " "
    pendingSpace = false
    result += character.toLowerCase()
  }
  return result
}

/** 归一化 + 稀疏 checkpoint 映射（norm 字符 → 原文字符偏移）。 */
export function normalizeWithMap(text: string): { norm: string; map: number[] } {
  const norm: string[] = []
  const map: number[] = []
  let pendingSpace = false
  let index = 0
  for (const character of fullWidthToHalfWidth(text)) {
    const code = character.codePointAt(0)!
    if (isWhitespaceCode(code)) {
      pendingSpace = true
      index += 1
      continue
    }
    if (pendingSpace && norm.length > 0) {
      norm.push(" ")
      map.push(-1)
    }
    pendingSpace = false
    norm.push(character.toLowerCase())
    map.push(index)
    index += 1
  }
  return { norm: norm.join(""), map }
}

/** 排名用分词：CJK 连续串按 2-gram，ASCII 按单词，其余丢弃。 */
export function tokenizeForRank(text: string): string[] {
  const tokens: string[] = []
  let cjkRun = ""
  let wordRun = ""
  const flushWord = () => {
    if (wordRun) {
      tokens.push(wordRun.toLowerCase())
      wordRun = ""
    }
  }
  const flushCjk = () => {
    if (cjkRun) {
      if (cjkRun.length === 1) {
        tokens.push(cjkRun)
      } else {
        for (let index = 0; index < cjkRun.length - 1; index += 1) {
          tokens.push(cjkRun.slice(index, index + 2))
        }
      }
      cjkRun = ""
    }
  }
  for (const character of fullWidthToHalfWidth(text)) {
    const code = character.codePointAt(0)!
    if (code >= 0x4e00 && code <= 0x9fff) {
      flushWord()
      cjkRun += character
    } else if (code >= 0x3400 && code <= 0x4dbf) {
      flushWord()
      cjkRun += character
    } else if (/[a-z0-9]/i.test(character)) {
      flushCjk()
      wordRun += character
    } else {
      flushCjk()
      flushWord()
    }
  }
  flushCjk()
  flushWord()
  return tokens
}

/** 构建章级索引。段落以 "\n\n" 连接作为章原文（与渲染一致）。 */
export function buildBookSearchIndex(chapters: readonly BookChapter[]): BookSearchIndex {
  const indexed: BookSearchIndexChapter[] = []
  const termDocuments = new Map<string, Set<number>>()
  let documents = 0
  chapters.forEach((chapter, chapterIndex) => {
    const original = chapter.paragraphs.join("\n\n")
    const normParts: string[] = []
    const map: number[] = []
    const paragraphStarts: number[] = []
    let originalOffset = 0
    chapter.paragraphs.forEach((paragraph, paragraphIndex) => {
      paragraphStarts.push(originalOffset)
      const { norm, map: paragraphMap } = normalizeWithMap(paragraph)
      if (paragraphIndex > 0) {
        normParts.push(" ")
        map.push(-1)
        originalOffset += 2
      }
      normParts.push(norm)
      for (const mapped of paragraphMap) {
        map.push(mapped === -1 ? -1 : mapped + originalOffset)
      }
      originalOffset += paragraph.length
    })
    const norm = normParts.join("")
    indexed.push({
      id: chapter.id,
      title: chapter.title,
      norm,
      map,
      paragraphStarts,
      originalLength: original.length,
    })
    if (norm.length > 0) documents += 1
    const terms = new Set(tokenizeForRank(norm))
    for (const term of terms) {
      const documentsWithTerm = termDocuments.get(term) ?? new Set<number>()
      documentsWithTerm.add(chapterIndex)
      termDocuments.set(term, documentsWithTerm)
    }
  })
  return {
    chapters: indexed,
    documents,
    termDocumentFrequencies: new Map(
      [...termDocuments.entries()].map(([term, documentsWithTerm]) => [term, documentsWithTerm.size]),
    ),
  }
}

function termScore(term: string, index: BookSearchIndex): number {
  const df = index.termDocumentFrequencies.get(term) ?? 0
  return Math.log((index.documents + 1) / (df + 1)) + 1
}

/** 查询术语集（精确匹配用完整 query，排名用术语）。 */
export function searchQueryTerms(query: string): string[] {
  const terms = tokenizeForRank(query)
  const unique = new Set<string>()
  for (const term of terms) unique.add(term)
  return [...unique]
}

export interface SearchBookOptions {
  query: string
  signal?: AbortSignal
}

function cancelled(): Error {
  return new DOMException("图书搜索已取消", "AbortError")
}

/**
 * 全文检索。命中为原文片段（exact/prefix/suffix 直接可作 TextAnchor）。
 * 扫描章内非重叠出现；取消信号每 4KiB 归一化字符检查一次。
 */
export function searchBook(
  chapters: readonly BookChapter[],
  index: BookSearchIndex,
  options: SearchBookOptions,
): BookSearchHit[] {
  const queryNorm = normalizeForMatch(options.query)
  if (!queryNorm) return []
  if (queryNorm.length > MAX_QUERY_CHARS) return []

  const terms = searchQueryTerms(queryNorm)
  const scores = new Map(terms.map((term) => [term, termScore(term, index)]))
  const all: BookSearchHit[] = []
  let scanned = 0

  for (let chapterIndex = 0; chapterIndex < index.chapters.length; chapterIndex += 1) {
    const chapter = index.chapters[chapterIndex]
    if (chapter.norm.length === 0) continue
    const original = chapters[chapterIndex].paragraphs.join("\n\n")
    const hits: BookSearchHit[] = []
    let searchFrom = 0
    while (hits.length < MAX_HITS_PER_CHAPTER) {
      scanned += CANCEL_CHECK_INTERVAL
      if (scanned >= CANCEL_CHECK_INTERVAL && options.signal?.aborted) throw cancelled()
      scanned %= CANCEL_CHECK_INTERVAL
      const found = chapter.norm.indexOf(queryNorm, searchFrom)
      if (found === -1) break
      const end = found + queryNorm.length
      let crossesSegment = false
      let originalStart = -1
      for (let offset = found; offset < end; offset += 1) {
        const mapped = chapter.map[offset]
        if (mapped === -1) {
          crossesSegment = true
          break
        }
        if (originalStart === -1) originalStart = mapped
      }
      if (crossesSegment) {
        searchFrom = found + queryNorm.length
        continue
      }
      const originalEnd = chapter.map[end - 1] + 1
      let exactStart = originalStart
      let exactEnd = originalEnd
      while (exactEnd - exactStart < MIN_EXACT_CHARS) {
        if (exactStart > 0) {
          exactStart -= 1
        } else if (exactEnd < original.length) {
          exactEnd += 1
        } else {
          break
        }
      }
      const exact = original.slice(exactStart, exactEnd)
      if (exact.length > MAX_EXACT_CHARS) {
        searchFrom = found + queryNorm.length
        continue
      }
      const prefix = original.slice(
        Math.max(0, exactStart - MAX_PREFIX_SUFFIX_CHARS),
        exactStart,
      )
      const suffix = original.slice(
        exactEnd,
        Math.min(original.length, exactEnd + MAX_PREFIX_SUFFIX_CHARS),
      )
      let score = 0
      for (const term of terms) score += scores.get(term) ?? 0
      let paragraphIndex = 0
      for (let index = 0; index < chapter.paragraphStarts.length; index += 1) {
        if (chapter.paragraphStarts[index] <= originalStart) paragraphIndex = index
      }
      hits.push({
        chapterId: chapter.id,
        chapterTitle: chapter.title,
        chapterIndex,
        paragraphIndex,
        progressionInChapter: chapter.norm.length === 0 ? 0 : found / chapter.norm.length,
        exact,
        prefix: prefix || null,
        suffix: suffix || null,
        score,
      })
      searchFrom = found + queryNorm.length
    }
    all.push(...hits)
  }

  all.sort((left, right) => right.score - left.score || left.chapterIndex - right.chapterIndex)
  return all.slice(0, MAX_BOOK_SEARCH_HITS)
}

/** 命中直接转为 TextAnchor（exact 12..240 原文；prefix/suffix 各 30 字符）。 */
export function textAnchorFromHit(hit: BookSearchHit): TextAnchorDto {
  return {
    exact: hit.exact,
    prefix: hit.prefix,
    suffix: hit.suffix,
  }
}