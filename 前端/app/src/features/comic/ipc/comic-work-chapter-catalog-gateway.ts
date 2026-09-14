// Work 级漫画章节目录 Gateway（React UI → Feature Hook/Action → 本模块 → HavenClient → Tauri 命令）。
//
// 只转交本地 Work/MediaItem 身份；远端章节 ID、Provider URL、页面授权都不参与
// 请求构造。响应必须通过与 Rust `deny_unknown_fields` DTO 对应的严格守卫，
// 并按请求身份回显校验，避免把别的作品目录当成当前事实。

import type { HavenClient } from "@/lib/ipc/client";
import { HavenError, toHavenError } from "@/lib/ipc/errors.js";
import type {
  ComicWorkChapterCatalogDto,
  ComicWorkChapterCatalogRequestDto,
} from "@/lib/ipc/generated/wire";
import { getHavenClient } from "@/lib/ipc/runtime.js";
import { isComicWorkChapterCatalogDto } from "@/lib/ipc/comic-work-chapter-catalog.js";

const CANONICAL_UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

export type ComicWorkCatalogTarget =
  | { workId: string; mediaItemId: null }
  | { workId: null; mediaItemId: string };

function invalidArgument(): HavenError {
  return new HavenError({
    code: "INVALID_ARGUMENT",
    userMessage: "漫画作品与章节必须二选一",
    retryable: false,
  });
}

function invalidCatalogResponse(): HavenError {
  return new HavenError({
    code: "COMIC_WORK_CATALOG_INVALID_RESPONSE",
    userMessage: "漫画作品目录不可用",
    retryable: false,
  });
}

/** 请求构造：Work 详情页用 workId，Reader 用当前 Session 的 mediaItemId。 */
export function comicWorkCatalogRequest(target: ComicWorkCatalogTarget): ComicWorkChapterCatalogRequestDto {
  return { workId: target.workId, mediaItemId: target.mediaItemId };
}

function assertTarget(target: ComicWorkCatalogTarget): void {
  if ((target.workId === null) === (target.mediaItemId === null)) throw invalidArgument();
  const id = target.workId ?? target.mediaItemId;
  if (id === null || !CANONICAL_UUID_PATTERN.test(id)) throw invalidArgument();
}

async function loadWorkCatalog(
  target: ComicWorkCatalogTarget,
  invoke: (request: ComicWorkChapterCatalogRequestDto) => Promise<ComicWorkChapterCatalogDto>,
): Promise<ComicWorkChapterCatalogDto> {
  assertTarget(target);
  const request = comicWorkCatalogRequest(target);
  let result: unknown;
  try {
    result = await invoke(request);
  } catch (error) {
    throw toHavenError(error);
  }
  const expected = target.workId !== null ? { workId: target.workId } : { mediaItemId: target.mediaItemId };
  if (!isComicWorkChapterCatalogDto(result, expected)) throw invalidCatalogResponse();
  return result;
}

export async function getComicWorkChapterCatalog(
  target: ComicWorkCatalogTarget,
  client: Pick<HavenClient, "comicWorkChapterCatalogGet"> = getHavenClient(),
): Promise<ComicWorkChapterCatalogDto> {
  return loadWorkCatalog(target, (request) => client.comicWorkChapterCatalogGet(request));
}

export async function refreshComicWorkChapterCatalog(
  target: ComicWorkCatalogTarget,
  client: Pick<HavenClient, "comicWorkChapterCatalogRefresh"> = getHavenClient(),
): Promise<ComicWorkChapterCatalogDto> {
  return loadWorkCatalog(target, (request) => client.comicWorkChapterCatalogRefresh(request));
}
