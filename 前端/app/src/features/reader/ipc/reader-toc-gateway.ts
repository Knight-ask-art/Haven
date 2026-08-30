import type { HavenClient } from "@/lib/ipc/client";
import { HavenError, toHavenError } from "@/lib/ipc/errors.js";
import type { ReaderTocResultDto, SessionOpenResultDto } from "@/lib/ipc/generated/wire";
import { getHavenClient } from "@/lib/ipc/runtime.js";
import { isReaderTocResultDto } from "@/lib/ipc/reader-toc.js";

const SESSION_ID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

function invalidArgument(): HavenError {
  return new HavenError({
    code: "INVALID_ARGUMENT",
    userMessage: "阅读会话无效",
    retryable: false,
  });
}

function invalidTocResponse(): HavenError {
  return new HavenError({
    code: "READER_TOC_INVALID_RESPONSE",
    userMessage: "章节目录不可用",
    retryable: false,
  });
}

export async function getReaderToc(
  session: SessionOpenResultDto,
  client: HavenClient = getHavenClient(),
): Promise<ReaderTocResultDto> {
  if (
    session.engine !== "reader"
    || !SESSION_ID_PATTERN.test(session.sessionId)
    || session.mediaItemId.trim().length === 0
  ) throw invalidArgument();

  let result: unknown;
  try {
    result = await client.readerTocGet({ sessionId: session.sessionId });
  } catch (error) {
    throw toHavenError(error);
  }
  if (!isReaderTocResultDto(result, { sessionId: session.sessionId })) throw invalidTocResponse();
  return result;
}