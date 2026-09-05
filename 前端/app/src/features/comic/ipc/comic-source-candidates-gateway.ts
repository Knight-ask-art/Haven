import type { HavenClient } from "@/lib/ipc/client"
import { HavenError, toHavenError } from "@/lib/ipc/errors.js"
import type {
  ComicChapterSourceCandidatesDto,
  ComicChapterSourceCandidatesGetRequestDto,
} from "@/lib/ipc/generated/wire"
import {
  isComicChapterSourceCandidatesDto,
  isComicChapterSourceCandidatesGetRequestDto,
} from "@/lib/ipc/comic-source-candidates"
import { getHavenClient } from "@/lib/ipc/runtime.js"

function invalidArgument(): HavenError {
  return new HavenError({
    code: "INVALID_ARGUMENT",
    userMessage: "漫画章节来源候选请求无效",
    retryable: false,
  })
}
function invalidResponse(): HavenError {
  return new HavenError({
    code: "COMIC_SOURCE_CANDIDATES_INVALID_RESPONSE",
    userMessage: "漫画章节来源候选不可用",
    retryable: false,
  })
}

export async function getComicChapterSourceCandidates(
  request: ComicChapterSourceCandidatesGetRequestDto,
  client: Pick<HavenClient, "comicChapterSourceCandidatesGet"> = getHavenClient(),
): Promise<ComicChapterSourceCandidatesDto> {
  if (!isComicChapterSourceCandidatesGetRequestDto(request)) throw invalidArgument()

  let result: unknown
  try {
    result = await client.comicChapterSourceCandidatesGet(request)
  } catch (error) {
    throw toHavenError(error)
  }
  if (!isComicChapterSourceCandidatesDto(result, request)) throw invalidResponse()
  return result
}
