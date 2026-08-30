import type { HavenClient } from "@/lib/ipc/client"
import { HavenError, toHavenError } from "@/lib/ipc/errors"
import type { ReaderSearchResultDto, SessionOpenResultDto } from "@/lib/ipc/generated/wire"
import { getHavenClient } from "@/lib/ipc/runtime"

const SESSION_ID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/

function invalidArgument(): HavenError {
  return new HavenError({
    code: "INVALID_ARGUMENT",
    userMessage: "阅读会话无效",
    retryable: false,
  })
}

function invalidSearchResponse(): HavenError {
  return new HavenError({
    code: "READER_SEARCH_INVALID_RESPONSE",
    userMessage: "检索结果不可用",
    retryable: false,
  })
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function isReaderSearchResultDto(value: unknown): boolean {
  if (!isRecord(value)) return false
  if (value.schemaVersion !== 1) return false
  if (typeof value.sessionId !== "string" || !SESSION_ID_PATTERN.test(value.sessionId)) return false
  if (!Array.isArray(value.hits)) return false
  return value.hits.every((hit: unknown) => {
    if (!isRecord(hit as Record<string, unknown>)) return false
    const h = hit as Record<string, unknown>
    return (
      typeof h.chapterId === "string" &&
      typeof h.chapterTitle === "string" &&
      typeof h.chapterIndex === "number" &&
      typeof h.paragraphIndex === "number" &&
      typeof h.progressionInChapter === "number" &&
      typeof h.score === "number" &&
      isRecord(h.textAnchor as Record<string, unknown>)
    )
  })
}

export async function searchReaderContent(
  session: SessionOpenResultDto,
  query: string,
  client: HavenClient = getHavenClient(),
): Promise<ReaderSearchResultDto> {
  if (
    session.engine !== "reader" ||
    !SESSION_ID_PATTERN.test(session.sessionId) ||
    session.mediaItemId.trim().length === 0
  ) {
    throw invalidArgument()
  }
  const trimmed = query.trim()
  if (!trimmed || trimmed.length > 128) {
    throw invalidArgument()
  }
  let result: unknown
  try {
    result = await client.readerSearch({ sessionId: session.sessionId, query: trimmed })
  } catch (error) {
    throw toHavenError(error)
  }
  if (!isReaderSearchResultDto(result) || (result as ReaderSearchResultDto).sessionId !== session.sessionId) {
    throw invalidSearchResponse()
  }
  return result as ReaderSearchResultDto
}