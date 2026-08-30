import { getHavenClient } from "@/lib/ipc/runtime.js"
import { HavenError, toHavenError } from "@/lib/ipc/errors.js"
import type { HavenClient } from "@/lib/ipc/client"
import type {
  CompletionWire,
  LocatorDto,
  ProgressSaveRequest,
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

function isTextAnchor(value: unknown): boolean {
  if (value === null || typeof value !== "object") return false
  const anchor = value as Record<string, unknown>
  return isNullableString(anchor.exact) && isNullableString(anchor.prefix) && isNullableString(anchor.suffix)
}

function isNullableTextAnchor(value: unknown): boolean {
  return value === null || isTextAnchor(value)
}

function isLocator(value: unknown): value is LocatorDto {
  if (typeof value !== "object" || value === null) return false
  const locator = value as Record<string, unknown>
  if (locator.version !== 1 || typeof locator.kind !== "string" || typeof locator.data !== "object" || locator.data === null) {
    return false
  }
  const data = locator.data as Record<string, unknown>
  switch (locator.kind) {
    case "video":
      return isFiniteNumber(data.positionMs) && Number.isInteger(data.positionMs) && data.positionMs >= 0
    case "book":
      return typeof data.publicationResource === "string" && isNullableString(data.formatLocator)
        && (data.progression === null || isFiniteNumber(data.progression)) && isNullableTextAnchor(data.textAnchor)
    case "pdf":
      return isFiniteNumber(data.pageIndex) && Number.isInteger(data.pageIndex) && data.pageIndex >= 0
        && [data.x, data.y, data.zoom].every((v) => v === null || isFiniteNumber(v))
        && isNullableTextAnchor(data.textAnchor)
    case "comic":
      return isCanonicalUuid(data.chapterItemId)
        && isFiniteNumber(data.pageIndex) && Number.isInteger(data.pageIndex) && data.pageIndex >= 0
        && (data.pageProgression === null || isFiniteNumber(data.pageProgression))
    case "article":
      return isNullableString(data.blockId) && (data.progression === null || isFiniteNumber(data.progression))
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

export { isCanonicalUuid, isLocator, isRequest, isResult }
