import type {
  ComicChapterSourceCandidateDto,
  ComicChapterSourceCandidatesDto,
  ComicChapterSourceCandidatesGetRequestDto,
  ComicChapterSourceStatusDto,
} from "./generated/wire"
import {
  isComicChapterMatchDto,
  isComicChapterSourceIdentityDto,
} from "./comic-progress-migration"
import { isComicEditionProfileDto } from "./comic-chapter-catalog"

const CANONICAL_UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
const MAX_CANDIDATES = 500
const MAX_TEXT_LENGTH = 512
const MAX_TIMESTAMP_LENGTH = 64

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function hasExactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const actual = Object.keys(value).sort()
  const expected = [...keys].sort()
  return actual.length === expected.length && actual.every((key, index) => key === expected[index])
}

function isSafeText(value: unknown, maxLength = MAX_TEXT_LENGTH): value is string {
  return typeof value === "string"
    && value.length > 0
    && value.length <= maxLength
    && !Array.from(value).some((character) => {
      const codePoint = character.codePointAt(0) ?? 0
      return codePoint <= 0x1f || codePoint === 0x7f
    })
}

function isNullableText(value: unknown, maxLength = MAX_TEXT_LENGTH): value is string | null {
  return value === null || isSafeText(value, maxLength)
}

function isNullableFiniteNumber(value: unknown): value is number | null {
  return value === null || (typeof value === "number" && Number.isFinite(value))
}

function isSafePageCount(value: unknown): value is number | null {
  return value === null || (
    typeof value === "number"
    && Number.isSafeInteger(value)
    && value >= 0
    && value <= MAX_CANDIDATES
  )
}

function isSafeSourceStatus(value: unknown): value is ComicChapterSourceStatusDto {
  return value === "available"
    || value === "temporarily_unavailable"
    || value === "external_only"
    || value === "unknown"
    || value === "missing"
}

function isCandidate(value: unknown): value is ComicChapterSourceCandidateDto {
  if (!isRecord(value) || !hasExactKeys(value, [
    "source",
    "mediaItemId",
    "chapterNumber",
    "volumeNumber",
    "title",
    "pageCount",
    "sourceOrder",
    "availability",
    "publishedAt",
    "sourceUpdatedAt",
    "lastSeenGeneration",
    "editionProfile",
    "matchResult",
  ])) return false

  return isComicChapterSourceIdentityDto(value.source)
    && typeof value.mediaItemId === "string"
    && CANONICAL_UUID_PATTERN.test(value.mediaItemId)
    && isNullableFiniteNumber(value.chapterNumber)
    && isNullableFiniteNumber(value.volumeNumber)
    && isNullableText(value.title)
    && isSafePageCount(value.pageCount)
    && typeof value.sourceOrder === "number"
    && Number.isSafeInteger(value.sourceOrder)
    && value.sourceOrder >= 0
    && value.sourceOrder <= MAX_CANDIDATES
    && isSafeSourceStatus(value.availability)
    && isNullableText(value.publishedAt, MAX_TIMESTAMP_LENGTH)
    && isNullableText(value.sourceUpdatedAt, MAX_TIMESTAMP_LENGTH)
    && (value.lastSeenGeneration === null || (
      typeof value.lastSeenGeneration === "number"
      && Number.isSafeInteger(value.lastSeenGeneration)
      && value.lastSeenGeneration >= 0
    ))
    && isComicEditionProfileDto(value.editionProfile)
    && isComicChapterMatchDto(value.matchResult)
}

export function isComicChapterSourceCandidatesGetRequestDto(
  value: unknown,
): value is ComicChapterSourceCandidatesGetRequestDto {
  return isRecord(value)
    && hasExactKeys(value, ["source"])
    && isComicChapterSourceIdentityDto(value.source)
}

export function isComicChapterSourceCandidatesDto(
  value: unknown,
  expected?: ComicChapterSourceCandidatesGetRequestDto,
): value is ComicChapterSourceCandidatesDto {
  if (!isRecord(value) || !hasExactKeys(value, [
    "schemaVersion",
    "source",
    "currentMediaItemId",
    "candidates",
    "truncated",
  ])) return false

  if (
    value.schemaVersion !== 1
    || !isComicChapterSourceIdentityDto(value.source)
    || typeof value.currentMediaItemId !== "string"
    || !CANONICAL_UUID_PATTERN.test(value.currentMediaItemId)
    || !Array.isArray(value.candidates)
    || value.candidates.length > MAX_CANDIDATES
    || typeof value.truncated !== "boolean"
  ) return false

  if (expected && !sameSourceIdentity(value.source, expected.source)) return false
  return value.candidates.every(isCandidate)
}

function sameSourceIdentity(
  left: ComicChapterSourceCandidatesDto["source"],
  right: ComicChapterSourceCandidatesGetRequestDto["source"],
): boolean {
  return left.sourceId === right.sourceId
    && left.remoteWorkId === right.remoteWorkId
    && left.remoteChapterId === right.remoteChapterId
}
