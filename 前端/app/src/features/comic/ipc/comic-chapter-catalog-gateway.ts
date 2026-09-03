import type { HavenClient } from "@/lib/ipc/client";
import { HavenError, toHavenError } from "@/lib/ipc/errors.js";
import type {
  ComicChapterCatalogDto,
  ComicChapterCatalogGetRequest,
  ComicRegisteredChapterCatalogDto,
} from "@/lib/ipc/generated/wire";
import { getHavenClient } from "@/lib/ipc/runtime.js";
import {
  isComicChapterCatalogDto,
  isComicRegisteredChapterCatalogDto,
} from "@/lib/ipc/comic-chapter-catalog.js";

const CANONICAL_UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

function invalidArgument(): HavenError {
  return new HavenError({
    code: "INVALID_ARGUMENT",
    userMessage: "漫画来源作品无效",
    retryable: false,
  });
}

function invalidCatalogResponse(): HavenError {
  return new HavenError({
    code: "COMIC_CATALOG_INVALID_RESPONSE",
    userMessage: "漫画章节目录不可用",
    retryable: false,
  });
}

function invalidRegisteredCatalogResponse(): HavenError {
  return new HavenError({
    code: "COMIC_REGISTERED_CATALOG_INVALID_RESPONSE",
    userMessage: "已登记漫画章节目录不可用",
    retryable: false,
  });
}

export async function getComicChapterCatalog(
  request: ComicChapterCatalogGetRequest,
  client: Pick<HavenClient, "comicChapterCatalogGet"> = getHavenClient(),
): Promise<ComicChapterCatalogDto> {
  if (
    request.sourceId !== "mangadex"
    || !CANONICAL_UUID_PATTERN.test(request.remoteWorkId)
  ) throw invalidArgument();

  let result: unknown;
  try {
    result = await client.comicChapterCatalogGet(request);
  } catch (error) {
    throw toHavenError(error);
  }
  if (!isComicChapterCatalogDto(result, request)) throw invalidCatalogResponse();
  return result;
}

export async function refreshComicChapterCatalog(
  request: ComicChapterCatalogGetRequest,
  client: Pick<HavenClient, "comicChapterCatalogRefresh"> = getHavenClient(),
): Promise<ComicChapterCatalogDto> {
  if (
    request.sourceId !== "mangadex"
    || !CANONICAL_UUID_PATTERN.test(request.remoteWorkId)
  ) throw invalidArgument();

  let result: unknown;
  try {
    result = await client.comicChapterCatalogRefresh(request);
  } catch (error) {
    throw toHavenError(error);
  }
  if (!isComicChapterCatalogDto(result, request)) throw invalidCatalogResponse();
  return result;
}

export async function getRegisteredComicChapterCatalog(
  request: ComicChapterCatalogGetRequest,
  client: Pick<HavenClient, "comicChapterCatalogRegisteredGet"> = getHavenClient(),
): Promise<ComicRegisteredChapterCatalogDto> {
  if (
    request.sourceId !== "mangadex"
    || !CANONICAL_UUID_PATTERN.test(request.remoteWorkId)
  ) throw invalidArgument();

  let result: unknown;
  try {
    result = await client.comicChapterCatalogRegisteredGet(request);
  } catch (error) {
    throw toHavenError(error);
  }
  if (!isComicRegisteredChapterCatalogDto(result, request)) {
    throw invalidRegisteredCatalogResponse();
  }
  return result;
}
