import type { HavenClient } from "@/lib/ipc/client";
import { HavenError, toHavenError } from "@/lib/ipc/errors.js";
import type { ComicPageManifestDto, SessionOpenResultDto } from "@/lib/ipc/generated/wire";
import { getHavenClient } from "@/lib/ipc/runtime.js";
import { isComicPageManifestDto } from "@/lib/ipc/comic-page-manifest.js";

const SESSION_ID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

function invalidArgument(): HavenError {
  return new HavenError({
    code: "INVALID_ARGUMENT",
    userMessage: "漫画会话无效",
    retryable: false,
  });
}

function invalidManifestResponse(): HavenError {
  return new HavenError({
    code: "COMIC_MANIFEST_INVALID_RESPONSE",
    userMessage: "漫画页面清单不可用",
    retryable: false,
  });
}

export async function getComicPageManifest(
  session: SessionOpenResultDto,
  client: HavenClient = getHavenClient(),
): Promise<ComicPageManifestDto> {
  if (
    session.engine !== "comic"
    || session.contentUri !== null
    || !SESSION_ID_PATTERN.test(session.sessionId)
    || session.mediaItemId.trim().length === 0
  ) throw invalidArgument();

  let result: unknown;
  try {
    result = await client.comicPageManifestGet({ sessionId: session.sessionId });
  } catch (error) {
    throw toHavenError(error);
  }
  if (!isComicPageManifestDto(result, {
    sessionId: session.sessionId,
    mediaItemId: session.mediaItemId,
  })) throw invalidManifestResponse();
  return result;
}
