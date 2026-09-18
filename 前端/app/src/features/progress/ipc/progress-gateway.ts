import { getHavenClient } from "@/lib/ipc/runtime.js"
import { HavenError, toHavenError } from "@/lib/ipc/errors.js"
import type { HavenClient } from "@/lib/ipc/client"
import type {
  CompletionWire,
  LocatorDto,
  ProgressSaveRequest,
  ProgressMarkCompletedRequest,
  ProgressSaveResult,
} from "@/lib/ipc/generated/wire"

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
const COMPLETIONS: readonly CompletionWire[] = ["not_started", "in_progress", "completed", "abandoned"]
const INVALID_RESPONSE = {
  code: "PROGRESS_INVALID_RESPONSE",
  userMessage: "进度保存响应不可用，请稍后重试",
  retryable: false,
} as const

function isCanonicalUuid(value: unknown): value is string {
  return typeof value === "string" && UUID.test(value) && value === value.toLowerCase()
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value)
}

function isNullableString(value: unknown): value is string | null {
  return value === null || typeof value === "string"
}

/**
 * 自有键精确集合检查：键集合必须与生成的 Wire DTO 完全相等。
 *
 * `Object.keys` 只看可枚举字符串键，symbol 键和非枚举键都会被漏掉，
 * `{...合法字段, [Symbol()]: x}` 或 `defineProperty(..., { enumerable: false })`
 * 这类结构会被当成合法 wire 数据透传。`Reflect.ownKeys` 取到的全部自有键
 * （字符串 + symbol，含不可枚举）必须与预期键一一对应，因此未知键、缺失键、
 * symbol 键、非枚举键一律拒绝。
 */
function hasExactOwnKeys(value: object, expected: readonly string[]): boolean {
  const ownKeys = Reflect.ownKeys(value)
  if (ownKeys.length !== expected.length) return false
  return ownKeys.every((key) => typeof key === "string" && expected.includes(key))
}

/**
 * Locator 顶层必须是恰好 `version`/`kind`/`data` 三个自有键的对象。
 *
 * 旧守卫只检查这三个键"存在且类型可用"，因此 `{version, kind, data, extra}`
 * 这类带未知键的结构会被当成合法 Locator 继续透传。这里补上外层闭合检查：
 * 缺键和未知键都必须拒绝，且必须是自己持有的键，不能靠原型链补齐。
 */
const LOCATOR_ENVELOPE_KEYS: readonly string[] = ["version", "kind", "data"]

/** 文本锚点与五种 data 形状的字段集合，逐字对应 `generated/wire.ts` 的 DTO。 */
const TEXT_ANCHOR_KEYS: readonly string[] = ["exact", "prefix", "suffix"]
const VIDEO_LOCATOR_KEYS: readonly string[] = ["positionMs"]
const BOOK_LOCATOR_KEYS: readonly string[] = ["publicationResource", "progression", "textAnchor", "formatLocator"]
const PDF_LOCATOR_KEYS: readonly string[] = ["pageIndex", "x", "y", "zoom", "textAnchor"]
const COMIC_LOCATOR_KEYS: readonly string[] = ["chapterItemId", "pageIndex", "pageProgression"]
const ARTICLE_LOCATOR_KEYS: readonly string[] = ["blockId", "progression", "textAnchor"]

function isTextAnchor(value: unknown): boolean {
  if (value === null || typeof value !== "object") return false
  if (!hasExactOwnKeys(value, TEXT_ANCHOR_KEYS)) return false
  const anchor = value as Record<string, unknown>
  return isNullableString(anchor.exact) && isNullableString(anchor.prefix) && isNullableString(anchor.suffix)
}

function isNullableTextAnchor(value: unknown): boolean {
  return value === null || isTextAnchor(value)
}

/**
 * Locator 守卫：外层信封与内容体都闭合。
 *
 * Rust 侧这些结构体都没有 `serde(default)` 或 `skip_serializing_if`，Option 字段
 * 一律显式序列化成 null，所以五种 data 形状的字段集合既不允许多余字段，也不允许
 * 省略字段；判定边界仍与旧守卫逐字一致，只补键集合闭合。
 */
function isLocator(value: unknown): value is LocatorDto {
  if (typeof value !== "object" || value === null) return false
  const locator = value as Record<string, unknown>
  if (!hasExactOwnKeys(locator, LOCATOR_ENVELOPE_KEYS)) return false
  if (locator.version !== 1 || typeof locator.kind !== "string" || typeof locator.data !== "object" || locator.data === null) {
    return false
  }
  const data = locator.data as Record<string, unknown>
  switch (locator.kind) {
    case "video":
      return hasExactOwnKeys(data, VIDEO_LOCATOR_KEYS)
        && isFiniteNumber(data.positionMs) && Number.isInteger(data.positionMs) && data.positionMs >= 0
    case "book":
      return hasExactOwnKeys(data, BOOK_LOCATOR_KEYS)
        && typeof data.publicationResource === "string" && isNullableString(data.formatLocator)
        && (data.progression === null || isFiniteNumber(data.progression)) && isNullableTextAnchor(data.textAnchor)
    case "pdf":
      return hasExactOwnKeys(data, PDF_LOCATOR_KEYS)
        && isFiniteNumber(data.pageIndex) && Number.isInteger(data.pageIndex) && data.pageIndex >= 0
        && [data.x, data.y, data.zoom].every((v) => v === null || isFiniteNumber(v))
        && isNullableTextAnchor(data.textAnchor)
    case "comic":
      return hasExactOwnKeys(data, COMIC_LOCATOR_KEYS)
        && isCanonicalUuid(data.chapterItemId)
        && isFiniteNumber(data.pageIndex) && Number.isInteger(data.pageIndex) && data.pageIndex >= 0
        && (data.pageProgression === null || isFiniteNumber(data.pageProgression))
    case "article":
      return hasExactOwnKeys(data, ARTICLE_LOCATOR_KEYS)
        && isNullableString(data.blockId) && (data.progression === null || isFiniteNumber(data.progression))
        && isNullableTextAnchor(data.textAnchor)
    default:
      return false
  }
}

function isRequest(value: unknown): value is ProgressSaveRequest {
  if (typeof value !== "object" || value === null) return false
  const request = value as Record<string, unknown>
  return isCanonicalUuid(request.mediaItemId)
    && isLocator(request.locator)
    && (request.locator.kind !== "comic" || request.locator.data.chapterItemId === request.mediaItemId)
    && (request.completion === null || (typeof request.completion === "string" && COMPLETIONS.includes(request.completion as CompletionWire)))
    && (request.expectedRevision === null || typeof request.expectedRevision === "string")
}

function isResult(value: unknown): value is ProgressSaveResult {
  if (typeof value !== "object" || value === null) return false
  const result = value as Record<string, unknown>
  return typeof result.revision === "string" && result.revision.length > 0
}

function isMarkCompletedRequest(value: unknown): value is ProgressMarkCompletedRequest {
  if (typeof value !== "object" || value === null) return false
  const request = value as Record<string, unknown>
  return isCanonicalUuid(request.mediaItemId) && isLocator(request.initialLocator)
    && ((request.initialLocator as { kind?: string; data?: { chapterItemId?: unknown } }).kind !== "comic"
      || (request.initialLocator as { data: { chapterItemId?: unknown } }).data.chapterItemId === request.mediaItemId)
}

/** Saves a progress locator through the typed client and fails closed on malformed wire data. */
export async function saveProgress(
  request: ProgressSaveRequest,
  client: HavenClient = getHavenClient(),
): Promise<ProgressSaveResult> {
  if (!isRequest(request)) {
    throw new HavenError({ code: "INVALID_ARGUMENT", userMessage: "进度参数无效", retryable: false })
  }
  let result: ProgressSaveResult
  try {
    result = await client.progressSave(request)
  } catch (error) {
    throw toHavenError(error)
  }
  if (!isResult(result)) throw new HavenError(INVALID_RESPONSE)
  return result
}

/** Marks a media item completed without replaying a stale list snapshot over existing progress. */
export async function markCompletedProgress(
  request: ProgressMarkCompletedRequest,
  client: HavenClient = getHavenClient(),
): Promise<ProgressSaveResult> {
  if (!isMarkCompletedRequest(request)) {
    throw new HavenError({ code: "INVALID_ARGUMENT", userMessage: "完成进度参数无效", retryable: false })
  }
  let result: ProgressSaveResult
  try {
    result = await client.progressMarkCompleted(request)
  } catch (error) {
    throw toHavenError(error)
  }
  if (!isResult(result)) throw new HavenError(INVALID_RESPONSE)
  return result
}

export { isCanonicalUuid, isLocator, isRequest, isResult }

/** Reset progress through the feature gateway; the page never calls the client directly. */
export async function resetProgress(mediaItemId: string, client: HavenClient = getHavenClient()): Promise<void> {
  await client.progressReset({ mediaItemId })
}
