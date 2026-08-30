import { HavenError } from "@/lib/ipc/errors"
import { isTauriRuntime } from "@/lib/ipc/runtime.js"
import { SESSION_URI_PATTERN } from "./session-gateway"

const BYTE_RANGE_PATTERN = /^bytes=(\d*)-(\d*)$/
const ALLOWED_CONTENT_TYPES = new Set([
  "application/epub+zip",
  "application/vnd.comicbook+zip",
  "application/pdf",
  "audio/flac",
  "audio/mpeg",
  "audio/mp4",
  "image/jpeg",
  "image/png",
  "image/webp",
  "text/plain",
  "text/markdown",
  "text/html",
  "video/mp4",
  "video/webm",
  "video/x-matroska",
])

export interface SessionResourcePayload {
  bytes: ArrayBuffer
  contentType: string
  partial: boolean
}

/** Successful settled states; callers map thrown HavenError retryability into the shared six-state model. */
export type ResourceFetchResult =
  | ({ kind: "data" } & SessionResourcePayload)
  | ({ kind: "empty" } & SessionResourcePayload)

export interface ResourceFetchOptions {
  range?: string
  signal?: AbortSignal
}

function resourceError(code: string, userMessage: string): HavenError {
  return new HavenError({ code, userMessage, retryable: false })
}

function isValidByteRange(range: string): boolean {
  const match = BYTE_RANGE_PATTERN.exec(range)
  if (!match) return false
  const start = match[1] === "" ? null : Number(match[1])
  const end = match[2] === "" ? null : Number(match[2])
  if (start === null && end === null) return false
  if (start === null) return end !== null && Number.isSafeInteger(end) && end > 0
  return Number.isSafeInteger(start)
    && start >= 0
    && (end === null || (Number.isSafeInteger(end) && end >= start))
}

function normalizedAllowedContentType(value: string | null): string | null {
  const contentType = value?.split(";", 1)[0]?.trim().toLowerCase() ?? ""
  if (!contentType || contentType === "application/octet-stream") return null
  return ALLOWED_CONTENT_TYPES.has(contentType) ? contentType : null
}

function errorForHttpStatus(status: number): HavenError {
  switch (status) {
    case 403:
      return resourceError("SECURITY_POLICY_DENIED", "没有读取该资源的权限")
    case 404:
      return resourceError("RESOURCE_NOT_FOUND", "资源不存在")
    case 410:
      return resourceError("RESOURCE_UNAVAILABLE", "资源会话已失效")
    case 413:
      return resourceError("FORMAT_UNSUPPORTED", "资源超过当前版本的 32 MiB 大小限制")
    case 416:
      return resourceError("INVALID_ARGUMENT", "资源读取范围无效")
    default:
      return resourceError("INTERNAL_ERROR", "资源读取失败，请稍后重试")
  }
}

function fetchableSessionUri(contentUri: string): string {
  const windowsWebView = isTauriRuntime()
    && typeof navigator !== "undefined"
    && /Windows/i.test(navigator.userAgent)
  if (!windowsWebView) return contentUri

  return contentUri.replace(
    "haven-resource://session/",
    "http://haven-resource.session/",
  )
}

export async function fetchSessionResource(
  contentUri: string,
  options: ResourceFetchOptions = {},
): Promise<ResourceFetchResult> {
  if (!SESSION_URI_PATTERN.test(contentUri)) {
    throw resourceError("INVALID_ARGUMENT", "资源地址无效")
  }
  if (options.range !== undefined && !isValidByteRange(options.range)) {
    throw resourceError("INVALID_ARGUMENT", "资源读取范围无效")
  }

  let response: Response
  try {
    response = await fetch(fetchableSessionUri(contentUri), {
      headers: options.range === undefined ? undefined : { Range: options.range },
      signal: options.signal,
    })
  } catch (error) {
    if (error instanceof DOMException && error.name === "AbortError") {
      throw resourceError("OPERATION_CANCELLED", "资源读取已取消")
    }
    throw resourceError("RESOURCE_UNAVAILABLE", "资源暂时无法读取")
  }

  if (!response.ok) {
    throw errorForHttpStatus(response.status)
  }

  const contentType = normalizedAllowedContentType(response.headers.get("Content-Type"))
  if (contentType === null) {
    throw resourceError("FORMAT_UNSUPPORTED", "资源格式不受支持")
  }

  let bytes: ArrayBuffer
  try {
    bytes = await response.arrayBuffer()
  } catch {
    throw resourceError("RESOURCE_UNAVAILABLE", "资源暂时无法读取")
  }

  const payload: SessionResourcePayload = {
    bytes,
    contentType,
    partial: response.status === 206,
  }
  return bytes.byteLength === 0
    ? { kind: "empty", ...payload }
    : { kind: "data", ...payload }
}

export { SESSION_URI_PATTERN as SESSION_RESOURCE_URI_PATTERN }
