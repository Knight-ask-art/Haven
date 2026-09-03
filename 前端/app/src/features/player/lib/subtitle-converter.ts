/**
 * Bounded subtitle normalisation for the player.
 *
 * Subtitle bytes are untrusted input. This module deliberately accepts only
 * formats with a small, deterministic parser in the first slice. It produces
 * WebVTT for the browser and never hands raw subtitle markup to the DOM.
 */

export type SubtitleFormat =
  | "srt"
  | "vtt"
  | "sbv"
  | "ass"
  | "ssa"
  | "ttml"
  | "dfxp"
  | "sub"
  | "lrc"
  | "unknown"

export type SubtitleDiagnosticCode =
  | "empty_input"
  | "invalid_encoding"
  | "unsupported_format"
  | "invalid_header"
  | "malformed_cue"
  | "invalid_timestamp"
  | "reversed_timestamp"
  | "empty_cue"
  | "unsafe_markup"
  | "overlapping_cues"
  | "file_too_large"
  | "cue_limit_exceeded"
  | "cue_text_too_large"
  | "total_text_too_large"
  | "no_valid_cues"
  | "resource_unavailable"
  | "invalid_resource"

export interface SubtitleDiagnostic {
  code: SubtitleDiagnosticCode
  message: string
  cueIndex?: number
  line?: number
}

export interface SubtitleNormalizationOptions {
  /** Maximum encoded input size. */
  maxBytes?: number
  /** Maximum number of cues emitted into the WebVTT result. */
  maxCues?: number
  /** Maximum UTF-16 code units in one cue after markup sanitisation. */
  maxCueTextLength?: number
  /** Maximum UTF-16 code units across all emitted cues. */
  maxTotalTextLength?: number
}

export type SubtitleInput = string | ArrayBuffer | Uint8Array

export type SubtitleNormalizationResult =
  | {
      status: "ok" | "partial"
      format: SubtitleFormat
      webVtt: string
      cueCount: number
      warnings: SubtitleDiagnostic[]
    }
  | {
      status: "rejected"
      format: SubtitleFormat
      warnings: SubtitleDiagnostic[]
    }

const DEFAULT_OPTIONS: Required<SubtitleNormalizationOptions> = {
  maxBytes: 8 * 1024 * 1024,
  maxCues: 5_000,
  maxCueTextLength: 4_096,
  maxTotalTextLength: 2 * 1024 * 1024,
}

const SUPPORTED_FORMATS = new Set<SubtitleFormat>(["srt", "vtt", "sbv"])
const PARTIAL_WARNING_CODES = new Set<SubtitleDiagnosticCode>([
  "malformed_cue",
  "invalid_timestamp",
  "reversed_timestamp",
  "empty_cue",
  "overlapping_cues",
  "cue_limit_exceeded",
  "cue_text_too_large",
  "total_text_too_large",
])

interface ParsedCue {
  startMs: number
  endMs: number
  text: string
  settings?: string
  sourceOrder: number
  line: number
}

interface TimingLine {
  start: string
  end: string
  settings?: string
}

function diagnostic(
  code: SubtitleDiagnosticCode,
  message: string,
  cueIndex?: number,
  line?: number,
): SubtitleDiagnostic {
  return {
    code,
    message,
    ...(cueIndex === undefined ? {} : { cueIndex }),
    ...(line === undefined ? {} : { line }),
  }
}

function inputBytes(input: SubtitleInput): Uint8Array {
  if (typeof input === "string") return new TextEncoder().encode(input)
  if (input instanceof Uint8Array) return input
  return new Uint8Array(input)
}

function decodeInput(bytes: Uint8Array): { text: string } | { warning: SubtitleDiagnostic } {
  let encoding = "utf-8"
  let offset = 0
  if (bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) {
    offset = 3
  } else if (bytes[0] === 0xff && bytes[1] === 0xfe) {
    encoding = "utf-16le"
    offset = 2
  } else if (bytes[0] === 0xfe && bytes[1] === 0xff) {
    encoding = "utf-16be"
    offset = 2
  }

  try {
    return { text: new TextDecoder(encoding, { fatal: true }).decode(bytes.subarray(offset)) }
  } catch {
    return {
      warning: diagnostic("invalid_encoding", "字幕编码无法安全解码，已拒绝加载。"),
    }
  }
}

function splitLines(text: string): string[] {
  return text.replace(/\r\n?/g, "\n").split("\n")
}

function splitBlocks(lines: string[]): Array<{ lines: string[]; startLine: number }> {
  const blocks: Array<{ lines: string[]; startLine: number }> = []
  let current: string[] = []
  let startLine = 1
  const flush = () => {
    if (current.length > 0) blocks.push({ lines: current, startLine })
    current = []
  }

  lines.forEach((line, index) => {
    if (line.trim() === "") {
      flush()
      startLine = index + 2
      return
    }
    if (current.length === 0) startLine = index + 1
    current.push(line)
  })
  flush()
  return blocks
}

function parseTimestamp(value: string): number | null {
  const trimmed = value.trim()
  const short = /^(\d{2,}):(\d{2})\.(\d{1,3})$/.exec(trimmed)
  if (short) {
    const minutes = Number(short[1])
    const seconds = Number(short[2])
    if (seconds > 59) return null
    return (minutes * 60 + seconds) * 1_000 + Number(short[3].padEnd(3, "0"))
  }

  const long = /^(\d+):(\d{2}):(\d{2})[.,](\d{1,3})$/.exec(trimmed)
  if (!long) return null
  const hours = Number(long[1])
  const minutes = Number(long[2])
  const seconds = Number(long[3])
  if (minutes > 59 || seconds > 59) return null
  return ((hours * 60 + minutes) * 60 + seconds) * 1_000 + Number(long[4].padEnd(3, "0"))
}

function parseTimingLine(line: string): TimingLine | null {
  const match = /^(\S+)\s+-->\s+(\S+)(?:\s+(.*))?$/.exec(line.trim())
  if (!match) return null
  return {
    start: match[1],
    end: match[2],
    settings: match[3]?.trim() || undefined,
  }
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;")
}

function isSafeWebVttTag(value: string): boolean {
  return /^<\/?(?:b|i|u|ruby|rt)>$/i.test(value)
}

function sanitiseCueText(
  lines: string[],
  warnings: SubtitleDiagnostic[],
  cueIndex: number,
  line: number,
): string {
  const raw = lines.join("\n").split(String.fromCharCode(0)).join("")
  return raw
    .split(/(<[^>]*>)/g)
    .map((part) => {
      if (!part.startsWith("<")) return escapeHtml(part)
      if (/^<br\s*\/?>(?:$)/i.test(part)) return "\n"
      if (isSafeWebVttTag(part)) return part.toLowerCase()
      warnings.push(diagnostic("unsafe_markup", "字幕标签已转义，未允许的标记不会交给播放器解析。", cueIndex, line))
      return escapeHtml(part)
    })
    .join("")
}

function parseBlocks(
  blocks: Array<{ lines: string[]; startLine: number }>,
  format: SubtitleFormat,
  warnings: SubtitleDiagnostic[],
): ParsedCue[] {
  const cues: ParsedCue[] = []
  blocks.forEach((block, sourceOrder) => {
    const timingIndex = block.lines.findIndex((line) => format === "sbv"
      ? /^\S+\s*,\s*\S+$/.test(line.trim())
      : line.includes("-->"))
    if (timingIndex < 0) {
      warnings.push(diagnostic("malformed_cue", "字幕块缺少有效时间轴，已跳过。", sourceOrder, block.startLine))
      return
    }

    const timing = format === "sbv"
      ? parseSbvTimingLine(block.lines[timingIndex])
      : parseTimingLine(block.lines[timingIndex])
    if (timing === null) {
      warnings.push(diagnostic("invalid_timestamp", "字幕时间轴格式无效，已跳过。", sourceOrder, block.startLine + timingIndex))
      return
    }
    const startMs = parseTimestamp(timing.start)
    const endMs = parseTimestamp(timing.end)
    if (startMs === null || endMs === null) {
      warnings.push(diagnostic("invalid_timestamp", "字幕时间戳无效，已跳过。", sourceOrder, block.startLine + timingIndex))
      return
    }
    if (endMs <= startMs) {
      warnings.push(diagnostic("reversed_timestamp", "字幕结束时间必须晚于开始时间，已跳过。", sourceOrder, block.startLine + timingIndex))
      return
    }

    const textLines = block.lines.slice(timingIndex + 1)
    if (textLines.length === 0 || textLines.every((line) => line.trim() === "")) {
      warnings.push(diagnostic("empty_cue", "字幕文本为空，已跳过。", sourceOrder, block.startLine + timingIndex + 1))
      return
    }
    const text = sanitiseCueText(textLines, warnings, sourceOrder, block.startLine + timingIndex + 1).trim()
    if (!text) {
      warnings.push(diagnostic("empty_cue", "字幕文本为空，已跳过。", sourceOrder, block.startLine + timingIndex + 1))
      return
    }
    cues.push({
      startMs,
      endMs,
      text,
      settings: format === "vtt" ? timing.settings : undefined,
      sourceOrder,
      line: block.startLine + timingIndex,
    })
  })
  return cues
}

function parseSbvTimingLine(line: string): TimingLine | null {
  const match = /^(\S+)\s*,\s*(\S+)$/.exec(line.trim())
  if (!match) return null
  return { start: match[1], end: match[2] }
}

function parseVtt(
  text: string,
  warnings: SubtitleDiagnostic[],
): ParsedCue[] | null {
  const lines = splitLines(text)
  const firstNonEmpty = lines.findIndex((line) => line.trim() !== "")
  if (firstNonEmpty < 0 || !/^WEBVTT(?:[ \t].*)?$/.test(lines[firstNonEmpty].trim())) {
    warnings.push(diagnostic("invalid_header", "WebVTT 文件缺少有效 WEBVTT 头。", undefined, firstNonEmpty + 1))
    return null
  }

  const blocks = splitBlocks(lines.slice(firstNonEmpty + 1)).filter(({ lines: block }) => {
    const first = block[0]?.trim().toUpperCase() ?? ""
    return !first.startsWith("NOTE") && !first.startsWith("STYLE") && !first.startsWith("REGION")
  })
  return parseBlocks(
    blocks.map((block) => ({ ...block, startLine: block.startLine + firstNonEmpty + 1 })),
    "vtt",
    warnings,
  )
}

function formatTimestamp(milliseconds: number): string {
  const hours = Math.floor(milliseconds / 3_600_000)
  const minutes = Math.floor((milliseconds % 3_600_000) / 60_000)
  const seconds = Math.floor((milliseconds % 60_000) / 1_000)
  const millis = milliseconds % 1_000
  const pad = (value: number, width: number) => String(value).padStart(width, "0")
  return pad(hours, 2) + ":" + pad(minutes, 2) + ":" + pad(seconds, 2) + "." + pad(millis, 3)
}

function renderWebVtt(cues: ParsedCue[]): string {
  return [
    "WEBVTT",
    "",
    ...cues.flatMap((cue, index) => [
      String(index + 1),
      formatTimestamp(cue.startMs)
        + " --> "
        + formatTimestamp(cue.endMs)
        + (cue.settings ? " " + cue.settings : ""),
      cue.text,
      "",
    ]),
  ].join("\n")
}

/** Convert a bounded subtitle resource to browser-safe WebVTT. */
export function normalizeSubtitle(
  input: SubtitleInput,
  format: SubtitleFormat,
  options: SubtitleNormalizationOptions = {},
): SubtitleNormalizationResult {
  const limits = { ...DEFAULT_OPTIONS, ...options }
  const warnings: SubtitleDiagnostic[] = []

  if (!SUPPORTED_FORMATS.has(format)) {
    return {
      status: "rejected",
      format,
      warnings: [diagnostic("unsupported_format", "当前版本不支持该字幕格式。")],
    }
  }

  const bytes = inputBytes(input)
  if (bytes.byteLength > limits.maxBytes) {
    return {
      status: "rejected",
      format,
      warnings: [diagnostic("file_too_large", "字幕文件超过安全大小限制。")],
    }
  }
  if (bytes.byteLength === 0) {
    return {
      status: "rejected",
      format,
      warnings: [diagnostic("empty_input", "字幕文件为空。")],
    }
  }

  const decoded = decodeInput(bytes)
  if ("warning" in decoded) return { status: "rejected", format, warnings: [decoded.warning] }
  if (decoded.text.trim() === "") {
    return { status: "rejected", format, warnings: [diagnostic("empty_input", "字幕文件为空。")] }
  }

  const parsed = format === "vtt"
    ? parseVtt(decoded.text, warnings)
    : parseBlocks(splitBlocks(splitLines(decoded.text)), format, warnings)
  if (parsed === null) return { status: "rejected", format, warnings }

  const cues: ParsedCue[] = []
  let totalTextLength = 0
  for (const cue of parsed) {
    if (cues.length >= limits.maxCues) {
      warnings.push(diagnostic("cue_limit_exceeded", "字幕轨道超过 cue 数量限制，后续内容已拒绝。", cue.sourceOrder, cue.line))
      break
    }
    if (cue.text.length > limits.maxCueTextLength) {
      warnings.push(diagnostic("cue_text_too_large", "单条字幕超过文本长度限制，已跳过。", cue.sourceOrder, cue.line))
      continue
    }
    if (totalTextLength + cue.text.length > limits.maxTotalTextLength) {
      warnings.push(diagnostic("total_text_too_large", "字幕总文本超过安全预算，后续内容已跳过。", cue.sourceOrder, cue.line))
      continue
    }
    totalTextLength += cue.text.length
    cues.push(cue)
  }

  cues.sort((left, right) => left.startMs - right.startMs || left.endMs - right.endMs || left.sourceOrder - right.sourceOrder)
  for (let index = 1; index < cues.length; index += 1) {
    const previous = cues[index - 1]
    const current = cues[index]
    if (current.startMs < previous.endMs) {
      warnings.push(diagnostic("overlapping_cues", "字幕时间轴存在重叠，已保留并标记供诊断。", current.sourceOrder, current.line))
    }
  }

  if (cues.length === 0) {
    warnings.push(diagnostic("no_valid_cues", "没有可播放的有效字幕 cue。"))
    return { status: "rejected", format, warnings }
  }

  return {
    status: warnings.some((warning) => PARTIAL_WARNING_CODES.has(warning.code)) ? "partial" : "ok",
    format,
    webVtt: renderWebVtt(cues),
    cueCount: cues.length,
    warnings,
  }
}
