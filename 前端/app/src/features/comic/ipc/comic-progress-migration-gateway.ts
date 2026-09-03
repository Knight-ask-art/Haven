import type { HavenClient } from "@/lib/ipc/client"
import { HavenError, toHavenError } from "@/lib/ipc/errors.js"
import type {
  ComicPageProgressRemapRequestDto,
  ComicProgressMigrationRequestDto,
  ComicProgressMigrationResultDto,
  ComicProgressMigrationRevertRequestDto,
  ComicProgressMigrationRevertResultDto,
} from "@/lib/ipc/generated/wire"
import {
  isComicPageProgressRemapRequestDto,
  isComicProgressMigrationRequestDto,
  isComicProgressMigrationResultDto,
  isComicProgressMigrationRevertRequestDto,
  isComicProgressMigrationRevertResultDto,
} from "@/lib/ipc/comic-progress-migration.js"
import { getHavenClient } from "@/lib/ipc/runtime.js"

function invalidArgument(): HavenError {
  return new HavenError({
    code: "INVALID_ARGUMENT",
    userMessage: "漫画进度迁移请求无效",
    retryable: false,
  })
}
function invalidMigrationResponse(): HavenError {
  return new HavenError({
    code: "COMIC_PROGRESS_MIGRATION_INVALID_RESPONSE",
    userMessage: "漫画进度迁移结果不可用",
    retryable: false,
  })
}

function invalidRevertResponse(): HavenError {
  return new HavenError({
    code: "COMIC_PROGRESS_REVERT_INVALID_RESPONSE",
    userMessage: "漫画进度撤销结果不可用",
    retryable: false,
  })
}

export async function migrateComicProgress(
  request: ComicProgressMigrationRequestDto,
  client: Pick<HavenClient, "comicProgressMigrate"> = getHavenClient(),
): Promise<ComicProgressMigrationResultDto> {
  if (!isComicProgressMigrationRequestDto(request)) throw invalidArgument()

  let result: unknown
  try {
    result = await client.comicProgressMigrate(request)
  } catch (error) {
    throw toHavenError(error)
  }
  if (!isComicProgressMigrationResultDto(result)) throw invalidMigrationResponse()
  return result
}

export async function remapComicProgress(
  request: ComicPageProgressRemapRequestDto,
  client: Pick<HavenClient, "comicProgressRemap"> = getHavenClient(),
): Promise<ComicProgressMigrationResultDto> {
  if (!isComicPageProgressRemapRequestDto(request)) throw invalidArgument()

  let result: unknown
  try {
    result = await client.comicProgressRemap(request)
  } catch (error) {
    throw toHavenError(error)
  }
  if (!isComicProgressMigrationResultDto(result)) throw invalidMigrationResponse()
  return result
}

export async function revertComicProgress(
  request: ComicProgressMigrationRevertRequestDto,
  client: Pick<HavenClient, "comicProgressRevert"> = getHavenClient(),
): Promise<ComicProgressMigrationRevertResultDto> {
  if (!isComicProgressMigrationRevertRequestDto(request)) throw invalidArgument()

  let result: unknown
  try {
    result = await client.comicProgressRevert(request)
  } catch (error) {
    throw toHavenError(error)
  }
  if (!isComicProgressMigrationRevertResultDto(result)) throw invalidRevertResponse()
  return result
}
