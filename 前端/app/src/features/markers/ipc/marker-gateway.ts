// Marker Gateway（FE-MARKER-001：标记创建的唯一数据通道，禁止散落 invoke）。
// 契约 §23.2：workId/editionId 由后端从 MediaItem 推导，前端只提交 mediaItemId + Locator。
// 后端按 MediaType 校验 Locator kind（LOCATOR_KIND_INCOMPATIBLE），因此各消费页
// 必须提交与自身媒介一致的 Locator（video/book/comic/article）。
//
// 环境分流（对齐 library/footprints gateway 裁决）：
// - 浏览器 dev = 演示环境 → 不发 IPC，由调用方保留既有 localStorage 演示行为。
// - Tauri WebView = 生产环境 → 真实 marker_create。

import type { HavenClient } from "@/lib/ipc/client"
import type { LocatorDto, MarkerCreateRequest, MarkerDto, MarkerTypeDto, TextAnchorDto } from "@/lib/ipc/generated/wire"
import { getHavenClient, isTauriRuntime } from "@/lib/ipc/runtime"
import { HavenError, toHavenError } from "@/lib/ipc/errors"
import { isCanonicalUuid, isLocator } from "@/features/progress/ipc/progress-gateway"

const MARKER_TYPES: readonly MarkerTypeDto[] = ["bookmark", "highlight", "note", "scene", "quote", "image"]

const INVALID_RESPONSE = {
  code: "MARKER_INVALID_RESPONSE",
  userMessage: "标记响应不可用，请稍后重试",
  retryable: false,
} as const

function isNullableString(value: unknown): value is string | null {
  return value === null || typeof value === "string"
}

function isRequest(value: unknown): value is MarkerCreateRequest {
  if (typeof value !== "object" || value === null) return false
  const request = value as Record<string, unknown>
  return isCanonicalUuid(request.mediaItemId)
    && isLocator(request.locator)
    // comic Locator 的 chapterItemId 必须与 mediaItemId 一致（与 progress 同规则）。
    && (request.locator.kind !== "comic" || request.locator.data.chapterItemId === request.mediaItemId)
    && typeof request.markerType === "string"
    && MARKER_TYPES.includes(request.markerType as MarkerTypeDto)
    && isNullableString(request.title)
    && isNullableString(request.excerpt)
    && isNullableString(request.note)
}

function isMarkerDto(value: unknown): value is MarkerDto {
  if (typeof value !== "object" || value === null) return false
  const dto = value as Record<string, unknown>
  return typeof dto.markerId === "string" && dto.markerId.length > 0
    && typeof dto.mediaItemId === "string" && dto.mediaItemId.length > 0
    && typeof dto.workId === "string" && typeof dto.editionId === "string"
    && isLocator(dto.locator)
    && typeof dto.markerType === "string" && MARKER_TYPES.includes(dto.markerType as MarkerTypeDto)
    && isNullableString(dto.title) && isNullableString(dto.excerpt) && isNullableString(dto.note)
    && typeof dto.createdAt === "string" && typeof dto.updatedAt === "string"
}

/** 创建标记；对畸形 wire 数据 fail closed（与 progress/session gateway 同规则）。 */
export async function createMarker(
  request: MarkerCreateRequest,
  client: HavenClient = getHavenClient(),
): Promise<MarkerDto> {
  if (!isRequest(request)) {
    throw new HavenError({ code: "INVALID_ARGUMENT", userMessage: "标记参数无效", retryable: false })
  }
  let result: MarkerDto
  try {
    result = await client.markerCreate(request)
  } catch (error) {
    throw toHavenError(error)
  }
  if (!isMarkerDto(result)) throw new HavenError(INVALID_RESPONSE)
  return result
}

/** 删除标记（软删除墓碑语义，契约 §23.2）。 */
export async function deleteMarker(
  markerId: string,
  client: HavenClient = getHavenClient(),
): Promise<boolean> {
  if (typeof markerId !== "string" || markerId.length === 0) {
    throw new HavenError({ code: "INVALID_ARGUMENT", userMessage: "标记标识无效", retryable: false })
  }
  try {
    return await client.markerDelete({ markerId })
  } catch (error) {
    throw toHavenError(error)
  }
}

/** 列出某 MediaItem 的标记（阅读器内书签面板）。 */
export async function listMarkers(
  mediaItemId: string,
  client: HavenClient = getHavenClient(),
): Promise<MarkerDto[]> {
  if (!isCanonicalUuid(mediaItemId)) {
    throw new HavenError({ code: "INVALID_ARGUMENT", userMessage: "媒体条目无效", retryable: false })
  }
  try {
    return await client.markerList({ mediaItemId })
  } catch (error) {
    throw toHavenError(error)
  }
}

// ---- Locator 构造器（各消费页提交与自身媒介一致的 kind）----

/** `haven-resource://text/<mediaItemId>`：0.1 本地文本资源标识（与进度控制器同源）。 */
function publicationResourceOf(mediaItemId: string): string {
  return `haven-resource://text/${mediaItemId}`
}

export function bookMarkerLocator(mediaItemId: string, progression: number): LocatorDto {
  return {
    version: 1,
    kind: "book",
    data: { publicationResource: publicationResourceOf(mediaItemId), progression, textAnchor: null, formatLocator: null },
  }
}

/** pageIndex 为 0-based（与 comic 进度控制器一致；UI 的 1-based 由调用方换算）。 */
export function comicMarkerLocator(mediaItemId: string, pageIndex: number, pageProgression: number | null = null): LocatorDto {
  return { version: 1, kind: "comic", data: { chapterItemId: mediaItemId, pageIndex, pageProgression } }
}

export function articleMarkerLocator(
  blockId: string | null,
  progression: number,
  textAnchor: TextAnchorDto | null = null,
): LocatorDto {
  return { version: 1, kind: "article", data: { blockId, progression, textAnchor } }
}

export function videoMarkerLocator(positionMs: number): LocatorDto {
  return { version: 1, kind: "video", data: { positionMs: Math.max(0, Math.round(positionMs)) } }
}

export { isTauriRuntime }
