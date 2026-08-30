import { getHavenClient, isTauriRuntime } from "@/lib/ipc/runtime"
import { toHavenError, HavenError } from "@/lib/ipc/errors"
import type { EditionDetailDto, EditionGetRequest, EditionListByWorkRequest, EditionListByWorkResultDto } from "./edition-wire"
import { isEditionListByWorkResult } from "../lib/edition-mapper"

export const EDITION_LIST_LIMIT = 200

export function editionListRequest(workId: string, cursor: string | null = null): EditionListByWorkRequest {
  return { workId, cursor, limit: EDITION_LIST_LIMIT }
}

export function editionGetRequest(editionId: string): EditionGetRequest {
  return { editionId }
}

export async function getEdition(editionId: string): Promise<EditionDetailDto> {
  if (!isTauriRuntime()) {
    return {
      schemaVersion: 1,
      editionId,
      workId: "",
      title: "",
      subtitle: null,
      mediaType: "unknown",
      releaseDate: null,
      language: null,
      region: null,
      publisherOrStudio: null,
      description: null,
      items: [],
    }
  }
  const result = await getHavenClient().editionGet(editionGetRequest(editionId))
  if (!isEditionDetail(result)) {
    throw new HavenError({ code: "INVALID_RESPONSE", userMessage: "版本详情格式无效", retryable: false })
  }
  return result
}

/** Production uses edition_list_by_work; browser demo deliberately retains its curated catalogue. */
export async function getEditionListByWork(
  workId: string,
  cursor: string | null = null,
): Promise<EditionListByWorkResultDto> {
  if (!isTauriRuntime()) {
    return { schemaVersion: 1, items: [], nextCursor: null, total: 0, revision: null }
  }
  const result = await getHavenClient().editionListByWork(editionListRequest(workId, cursor))
  if (!isEditionListByWorkResult(result)) {
    throw new HavenError({
      code: "INVALID_RESPONSE",
      userMessage: "版本信息格式无效",
      retryable: false,
    })
  }
  return result
}

/** Consume every opaque cursor exactly once; a repeated cursor is a terminal protocol error. */
export async function loadAllEditionsByWork(workId: string): Promise<EditionListByWorkResultDto> {
  const items: EditionListByWorkResultDto["items"] = []
  const seen = new Set<string>()
  let cursor: string | null = null
  for (;;) {
    const page = await getEditionListByWork(workId, cursor)
    if (page.items.some((item) => item.workId !== workId)) {
      throw new HavenError({ code: "INVALID_RESPONSE", userMessage: "版本信息与作品不匹配", retryable: false })
    }
    items.push(...page.items)
    if (page.nextCursor === null) return { ...page, items }
    if (seen.has(page.nextCursor)) {
      throw new HavenError({ code: "INVALID_RESPONSE", userMessage: "版本信息分页游标无效", retryable: false })
    }
    seen.add(page.nextCursor)
    cursor = page.nextCursor
  }
}

export function normalizeEditionError(error: unknown): HavenError {
  return toHavenError(error)
}

function isEditionDetail(value: unknown): value is EditionDetailDto {
  if (typeof value !== "object" || value === null) return false
  const result = value as Record<string, unknown>
  return result.schemaVersion === 1 && typeof result.editionId === "string" && typeof result.workId === "string" && Array.isArray(result.items)
}
