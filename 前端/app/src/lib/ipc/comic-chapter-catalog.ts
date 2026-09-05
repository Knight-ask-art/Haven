import type {
  ComicChapterAvailabilityDto,
  ComicChapterCatalogDto,
  ComicChapterCatalogItemDto,
  ComicChapterCatalogRefreshStateDto,
  ComicChapterSourceStatusDto,
  ComicEditionFacetKindDto,
  ComicEditionProfileDto,
  ComicRegisteredChapterCatalogDto,
  ComicRegisteredChapterCatalogItemDto,
  ComicScanGroupKindDto,
} from "./generated/wire";

const CANONICAL_UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const MAX_CHAPTERS = 500;
const MAX_TEXT_LENGTH = 512;
const MAX_TIMESTAMP_LENGTH = 64;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasExactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  return actual.length === expected.length && actual.every((key, index) => key === expected[index]);
}

function isSafeText(value: unknown, maxLength = MAX_TEXT_LENGTH): value is string {
  return (
    typeof value === "string"
    && value.length > 0
    && value.length <= maxLength
    && !Array.from(value).some((character) => {
      const codePoint = character.codePointAt(0) ?? 0;
      return codePoint <= 0x1f || codePoint === 0x7f;
    })
  );
}

function isNullableText(value: unknown, maxLength = MAX_TEXT_LENGTH): value is string | null {
  return value === null || isSafeText(value, maxLength);
}

function isNullableFiniteNumber(value: unknown): value is number | null {
  return value === null || (typeof value === "number" && Number.isFinite(value));
}

function isNullablePageCount(value: unknown): value is number | null {
  return value === null || (
    typeof value === "number"
    && Number.isInteger(value)
    && value >= 0
    && value <= MAX_CHAPTERS
  );
}

function isFacetKind(value: unknown): value is ComicEditionFacetKindDto {
  return value === "unknown" || value === "known" || value === "not_applicable";
}

function isScanGroupKind(value: unknown): value is ComicScanGroupKindDto {
  return value === "unknown"
    || value === "content_line"
    || value === "mirror_label"
    || value === "not_applicable";
}

function isAvailability(value: unknown): value is ComicChapterAvailabilityDto {
  return value === "available"
    || value === "temporarily_unavailable"
    || value === "external_only"
    || value === "unknown";
}

function isSourceStatus(value: unknown): value is ComicChapterSourceStatusDto {
  return value === "available"
    || value === "temporarily_unavailable"
    || value === "external_only"
    || value === "unknown"
    || value === "missing";
}

function isColorMode(value: unknown): value is ComicEditionProfileDto["colorMode"] {
  return value === "unknown" || value === "full_color" || value === "grayscale" || value === "mixed";
}

function isFacetValue(value: unknown, kind: unknown): value is string | null {
  if (!isFacetKind(kind)) return false;
  if (kind === "known") return isSafeText(value);
  return value === null;
}

function isScanGroupValue(value: unknown, kind: unknown): value is string | null {
  if (!isScanGroupKind(kind)) return false;
  if (kind === "content_line" || kind === "mirror_label") return isSafeText(value);
  return value === null;
}

function isEditionProfile(value: unknown): value is ComicEditionProfileDto {
  if (!isRecord(value) || !hasExactKeys(value, [
    "language",
    "languageKind",
    "translationLine",
    "translationLineKind",
    "scanGroup",
    "scanGroupKind",
    "colorMode",
  ])) return false;

  return isFacetValue(value.language, value.languageKind)
    && isFacetValue(value.translationLine, value.translationLineKind)
    && isScanGroupValue(value.scanGroup, value.scanGroupKind)
    && isColorMode(value.colorMode);
}

export function isComicEditionProfileDto(value: unknown): value is ComicEditionProfileDto {
  return isEditionProfile(value)
}

function isChapter(value: unknown): value is ComicChapterCatalogItemDto {
  if (!isRecord(value) || !hasExactKeys(value, [
    "remoteChapterId",
    "chapterNumber",
    "volumeNumber",
    "title",
    "pageCount",
    "publishedAt",
    "updatedAt",
    "availability",
    "editionProfile",
  ])) return false;

  return typeof value.remoteChapterId === "string"
    && CANONICAL_UUID_PATTERN.test(value.remoteChapterId)
    && isNullableFiniteNumber(value.chapterNumber)
    && isNullableFiniteNumber(value.volumeNumber)
    && isNullableText(value.title)
    && isNullablePageCount(value.pageCount)
    && isNullableText(value.publishedAt, MAX_TIMESTAMP_LENGTH)
    && isNullableText(value.updatedAt, MAX_TIMESTAMP_LENGTH)
    && isAvailability(value.availability)
    && isEditionProfile(value.editionProfile);
}

function isSafeGeneration(value: unknown): value is number {
  return typeof value === "number"
    && Number.isSafeInteger(value)
    && value >= 0;
}

function isRefreshState(value: unknown): value is ComicChapterCatalogRefreshStateDto {
  if (!isRecord(value) || !hasExactKeys(value, [
    "generation",
    "fetchedAt",
    "total",
    "truncated",
  ])) return false;

  return isSafeGeneration(value.generation)
    && isSafeText(value.fetchedAt, MAX_TIMESTAMP_LENGTH)
    && (value.total === null || (
      typeof value.total === "number"
      && Number.isInteger(value.total)
      && value.total >= 0
    ))
    && typeof value.truncated === "boolean";
}

function isRegisteredChapter(value: unknown): value is ComicRegisteredChapterCatalogItemDto {
  if (!isRecord(value) || !hasExactKeys(value, [
    "mediaItemId",
    "sourceId",
    "remoteWorkId",
    "remoteChapterId",
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
  ])) return false;

  return typeof value.mediaItemId === "string"
    && CANONICAL_UUID_PATTERN.test(value.mediaItemId)
    && typeof value.sourceId === "string"
    && isSafeText(value.sourceId)
    && typeof value.remoteWorkId === "string"
    && CANONICAL_UUID_PATTERN.test(value.remoteWorkId)
    && typeof value.remoteChapterId === "string"
    && CANONICAL_UUID_PATTERN.test(value.remoteChapterId)
    && isNullableFiniteNumber(value.chapterNumber)
    && isNullableFiniteNumber(value.volumeNumber)
    && isNullableText(value.title)
    && isNullablePageCount(value.pageCount)
    && typeof value.sourceOrder === "number"
    && Number.isInteger(value.sourceOrder)
    && value.sourceOrder >= 0
    && value.sourceOrder <= MAX_CHAPTERS
    && isSourceStatus(value.availability)
    && isNullableText(value.publishedAt, MAX_TIMESTAMP_LENGTH)
    && isNullableText(value.sourceUpdatedAt, MAX_TIMESTAMP_LENGTH)
    && (value.lastSeenGeneration === null || isSafeGeneration(value.lastSeenGeneration))
    && isEditionProfile(value.editionProfile);
}

export function isComicChapterCatalogDto(
  value: unknown,
  expected?: { sourceId?: string; remoteWorkId?: string },
): value is ComicChapterCatalogDto {
  if (!isRecord(value) || !hasExactKeys(value, [
    "schemaVersion",
    "sourceId",
    "remoteWorkId",
    "fetchedAt",
    "total",
    "truncated",
    "chapters",
  ])) return false;

  if (
    value.schemaVersion !== 1
    || value.sourceId !== "mangadex"
    || typeof value.remoteWorkId !== "string"
    || !CANONICAL_UUID_PATTERN.test(value.remoteWorkId)
    || !isSafeText(value.fetchedAt, MAX_TIMESTAMP_LENGTH)
    || !(value.total === null || (
      typeof value.total === "number"
      && Number.isInteger(value.total)
      && value.total >= 0
    ))
    || typeof value.truncated !== "boolean"
    || !Array.isArray(value.chapters)
    || value.chapters.length > MAX_CHAPTERS
  ) return false;

  if (expected?.sourceId !== undefined && value.sourceId !== expected.sourceId) return false;
  if (expected?.remoteWorkId !== undefined && value.remoteWorkId !== expected.remoteWorkId) return false;

  const ids = new Set<string>();
  return value.chapters.every((chapter) => {
    if (!isChapter(chapter) || ids.has(chapter.remoteChapterId)) return false;
    ids.add(chapter.remoteChapterId);
    return true;
  });
}

export function isComicRegisteredChapterCatalogDto(
  value: unknown,
  expected?: { sourceId?: string; remoteWorkId?: string },
): value is ComicRegisteredChapterCatalogDto {
  if (!isRecord(value) || !hasExactKeys(value, [
    "schemaVersion",
    "sourceId",
    "remoteWorkId",
    "refreshState",
    "chapters",
  ])) return false;

  if (
    value.schemaVersion !== 1
    || value.sourceId !== "mangadex"
    || typeof value.remoteWorkId !== "string"
    || !CANONICAL_UUID_PATTERN.test(value.remoteWorkId)
    || !(value.refreshState === null || isRefreshState(value.refreshState))
    || !Array.isArray(value.chapters)
    || value.chapters.length > MAX_CHAPTERS
  ) return false;

  if (expected?.sourceId !== undefined && value.sourceId !== expected.sourceId) return false;
  if (expected?.remoteWorkId !== undefined && value.remoteWorkId !== expected.remoteWorkId) return false;

  const ids = new Set<string>();
  return value.chapters.every((chapter) => {
    if (!isRegisteredChapter(chapter)) return false;
    if (chapter.sourceId !== value.sourceId || chapter.remoteWorkId !== value.remoteWorkId) return false;
    if (ids.has(chapter.remoteChapterId)) return false;
    ids.add(chapter.remoteChapterId);
    return true;
  });
}
