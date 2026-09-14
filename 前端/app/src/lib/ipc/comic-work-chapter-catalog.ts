// Work 级漫画目录的严格 Wire 守卫。
//
// 后端已经负责章节顺序、当前章节、上一章/下一章和可打开状态；前端只做形状
// 与语义闭合校验，绝不按章节号或标题重新推导这些事实。这里的字段集合与
// Rust 侧的 `deny_unknown_fields` DTO 一一对应，任何多余字段都视为越界。

import type {
  ComicCatalogRefreshOutcomeStatusDto,
  ComicCatalogRefreshReceiptDto,
  ComicChapterAggregateStatusDto,
  ComicChapterSourceStatusDto,
  ComicChapterSourceSummaryDto,
  ComicWorkCatalogStatusDto,
  ComicWorkChapterCatalogDto,
  ComicWorkChapterDto,
  ComicWorkEditionDto,
  CompletionWire,
  LocatorDto,
  ProgressSummaryDto,
} from "./generated/wire";
import { isComicEditionProfileDto } from "./comic-chapter-catalog.js";
import { isComicChapterMatchDto, isComicChapterSourceIdentityDto } from "./comic-progress-migration.js";

const CANONICAL_UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const MAX_TEXT_LENGTH = 512;
const MAX_TIMESTAMP_LENGTH = 64;
const MAX_EDITIONS = 128;
const MAX_CHAPTERS = 500;
const MAX_PAGE_COUNT = 10_000;
const MAX_SOURCES_PER_CHAPTER = 64;
const MAX_REFRESH_RECEIPTS = 64;
const MAX_REVISION_LENGTH = 256;
const MAX_KEYFRAME_URI_LENGTH = 8192;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasExactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  return actual.length === expected.length && actual.every((key, index) => key === expected[index]);
}

function isOneOf<T extends string>(value: unknown, allowed: readonly T[]): value is T {
  return typeof value === "string" && allowed.includes(value as T);
}

function isCanonicalUuid(value: unknown): value is string {
  return typeof value === "string" && CANONICAL_UUID_PATTERN.test(value);
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

function isNullableUuid(value: unknown): value is string | null {
  return value === null || isCanonicalUuid(value);
}

function isNullableFiniteNumber(value: unknown): value is number | null {
  return value === null || (typeof value === "number" && Number.isFinite(value));
}

function isNullablePageCount(value: unknown): value is number | null {
  return value === null || (
    typeof value === "number"
    && Number.isInteger(value)
    && value >= 0
    && value <= MAX_PAGE_COUNT
  );
}

function isSafeIndex(value: unknown, max: number): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 && value <= max;
}

function isNullableSafeIndex(value: unknown, max: number): value is number | null {
  return value === null || isSafeIndex(value, max);
}

function isAttachmentStatus(value: unknown): value is ComicChapterSourceStatusDto {
  return isOneOf(value, [
    "available",
    "temporarily_unavailable",
    "external_only",
    "unknown",
    "missing",
  ]);
}

function isAggregateStatus(value: unknown): value is ComicChapterAggregateStatusDto {
  return isAttachmentStatus(value);
}

function isWorkCatalogStatus(value: unknown): value is ComicWorkCatalogStatusDto {
  return isOneOf(value, ["never_synced", "synced", "refresh_failed", "truncated"]);
}

function isRefreshOutcomeStatus(value: unknown): value is ComicCatalogRefreshOutcomeStatusDto {
  return isOneOf(value, [
    "never_synced",
    "succeeded",
    "temporarily_unavailable",
    "external_only",
    "unknown",
    "refresh_failed",
    "truncated",
  ]);
}

function isCompletion(value: unknown): value is CompletionWire {
  return isOneOf(value, ["not_started", "in_progress", "completed", "abandoned"]);
}

function isProgressRatio(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 1;
}

function isNullableRatio(value: unknown): value is number | null {
  return value === null || isProgressRatio(value);
}

/**
 * 文本锚点。Rust `TextAnchorDto` 三个 Option 字段都没有 `serde(default)` 或
 * `skip_serializing_if`，因此键恒存在、缺失即非法；空值只能显式写成 null。
 */
function isTextAnchor(value: unknown): boolean {
  if (value === null || !isRecord(value)) return false;
  if (!hasExactKeys(value, ["exact", "prefix", "suffix"])) return false;
  return isNullableText(value.exact) && isNullableText(value.prefix) && isNullableText(value.suffix);
}

function isNullableTextAnchor(value: unknown): boolean {
  return value === null || isTextAnchor(value);
}

/**
 * Locator 数据体。五种 data 形状各自与生成的 `*LocatorDto` 字段集合严格相等：
 * Rust 侧这些结构体都没有 `serde(default)`，Option 字段也一律显式序列化成
 * null，所以闭集里既不允许多余字段，也不允许省略字段。
 *
 * 外层信封同样闭合：`LocatorDto` 的手写 Serialize 只写 `version` / `kind` /
 * `data` 三个键，因此键集合必须严格相等——多一个字段即越界，少一个字段也不
 * 成立（缺失的 `version`/`kind` 会落到类型判断，缺失的 `data` 会落到下面的
 * 记录判断）。
 */
function isLocator(value: unknown): value is LocatorDto {
  if (!isRecord(value) || !hasExactKeys(value, ["version", "kind", "data"])) return false;
  if (value.version !== 1 || typeof value.kind !== "string") return false;
  const data: unknown = value.data;
  if (!isRecord(data)) return false;
  switch (value.kind) {
    case "video":
      return hasExactKeys(data, ["positionMs"])
        && typeof data.positionMs === "number"
        && Number.isInteger(data.positionMs)
        && data.positionMs >= 0;
    case "book":
      return hasExactKeys(data, ["publicationResource", "progression", "textAnchor", "formatLocator"])
        && typeof data.publicationResource === "string"
        && isNullableFiniteNumber(data.progression)
        && isNullableTextAnchor(data.textAnchor)
        && isNullableText(data.formatLocator);
    case "pdf":
      return hasExactKeys(data, ["pageIndex", "x", "y", "zoom", "textAnchor"])
        && typeof data.pageIndex === "number"
        && Number.isInteger(data.pageIndex)
        && data.pageIndex >= 0
        && isNullableFiniteNumber(data.x)
        && isNullableFiniteNumber(data.y)
        && isNullableFiniteNumber(data.zoom)
        && isNullableTextAnchor(data.textAnchor);
    case "comic":
      return hasExactKeys(data, ["chapterItemId", "pageIndex", "pageProgression"])
        && isCanonicalUuid(data.chapterItemId)
        && typeof data.pageIndex === "number"
        && Number.isInteger(data.pageIndex)
        && data.pageIndex >= 0
        && isNullableFiniteNumber(data.pageProgression);
    case "article":
      return hasExactKeys(data, ["blockId", "progression", "textAnchor"])
        && isNullableText(data.blockId)
        && isNullableFiniteNumber(data.progression)
        && isNullableTextAnchor(data.textAnchor);
    default:
      return false;
  }
}

/**
 * 章节进度摘要。`progressRatio` 是派生展示值，`revision` 是写入 CAS token。
 *
 * Rust `ProgressSummaryDto` 只声明了 `#[serde(default)]`，没有
 * `skip_serializing_if`，因此 `None` 会被显式序列化成 `keyframeUri: null`；
 * 省略（前端本地构造）与显式 null（后端真实投影）都必须接受，非空字符串
 * 仍走同一套文本安全边界。
 */
function isChapterProgress(value: unknown): value is ProgressSummaryDto {
  if (!isRecord(value)) return false;
  const hasKeyframeUri = Object.prototype.hasOwnProperty.call(value, "keyframeUri");
  const keys = ["mediaItemId", "completion", "progressRatio", "revision", "updatedAt", "locator"];
  if (hasKeyframeUri) keys.push("keyframeUri");
  if (!hasExactKeys(value, keys)) return false;
  return isCanonicalUuid(value.mediaItemId)
    && isCompletion(value.completion)
    && isNullableRatio(value.progressRatio)
    && isSafeText(value.revision, MAX_REVISION_LENGTH)
    && isSafeText(value.updatedAt, MAX_TIMESTAMP_LENGTH)
    && isLocator(value.locator)
    && (!hasKeyframeUri
      || value.keyframeUri === null
      || isSafeText(value.keyframeUri, MAX_KEYFRAME_URI_LENGTH));
}

function isSourceSummary(value: unknown): value is ComicChapterSourceSummaryDto {
  if (!isRecord(value) || !hasExactKeys(value, [
    "source",
    "status",
    "mirrorLabel",
    "observedAt",
    "sourceOrder",
  ])) return false;

  return isComicChapterSourceIdentityDto(value.source)
    && isAttachmentStatus(value.status)
    && isNullableText(value.mirrorLabel)
    && isNullableText(value.observedAt, MAX_TIMESTAMP_LENGTH)
    && isSafeIndex(value.sourceOrder, MAX_SOURCES_PER_CHAPTER);
}

function isWorkEdition(value: unknown): value is ComicWorkEditionDto {
  if (!isRecord(value) || !hasExactKeys(value, [
    "editionId",
    "profile",
    "displayLabel",
    "chapterCount",
  ])) return false;

  return isCanonicalUuid(value.editionId)
    && isComicEditionProfileDto(value.profile)
    && isSafeText(value.displayLabel)
    && isSafeIndex(value.chapterCount, MAX_CHAPTERS);
}

function isWorkChapter(value: unknown): value is ComicWorkChapterDto {
  if (!isRecord(value) || !hasExactKeys(value, [
    "mediaItemId",
    "editionId",
    "subjectId",
    "chapterNumber",
    "volumeNumber",
    "title",
    "publishedAt",
    "pageCount",
    "status",
    "canOpen",
    "sources",
    "matchResult",
    "progress",
    "previousMediaItemId",
    "nextMediaItemId",
    "backendOrder",
  ])) return false;

  return isCanonicalUuid(value.mediaItemId)
    && isCanonicalUuid(value.editionId)
    && isNullableUuid(value.subjectId)
    && isNullableFiniteNumber(value.chapterNumber)
    && isNullableFiniteNumber(value.volumeNumber)
    && isNullableText(value.title)
    && isNullableText(value.publishedAt, MAX_TIMESTAMP_LENGTH)
    && isNullablePageCount(value.pageCount)
    && isAggregateStatus(value.status)
    && typeof value.canOpen === "boolean"
    && Array.isArray(value.sources)
    && value.sources.length <= MAX_SOURCES_PER_CHAPTER
    && value.sources.every(isSourceSummary)
    && (value.matchResult === null || isComicChapterMatchDto(value.matchResult))
    && (value.progress === null || isChapterProgress(value.progress))
    && isNullableUuid(value.previousMediaItemId)
    && isNullableUuid(value.nextMediaItemId)
    && isSafeIndex(value.backendOrder, MAX_CHAPTERS);
}

function isRefreshReceipt(value: unknown): value is ComicCatalogRefreshReceiptDto {
  if (!isRecord(value) || !hasExactKeys(value, [
    "refreshId",
    "workId",
    "sourceId",
    "remoteWorkId",
    "status",
    "generationBefore",
    "generationAfter",
    "observedFrom",
    "observedTo",
    "truncated",
    "retainedPreviousCatalog",
    "errorCode",
    "observedAt",
  ])) return false;

  return isCanonicalUuid(value.refreshId)
    && isCanonicalUuid(value.workId)
    && isSafeText(value.sourceId)
    && isSafeText(value.remoteWorkId, MAX_TEXT_LENGTH)
    && isRefreshOutcomeStatus(value.status)
    && isSafeIndex(value.generationBefore, Number.MAX_SAFE_INTEGER)
    && isNullableSafeIndex(value.generationAfter, Number.MAX_SAFE_INTEGER)
    && isNullableText(value.observedFrom, MAX_TIMESTAMP_LENGTH)
    && isNullableText(value.observedTo, MAX_TIMESTAMP_LENGTH)
    && typeof value.truncated === "boolean"
    && typeof value.retainedPreviousCatalog === "boolean"
    && isNullableText(value.errorCode, MAX_TEXT_LENGTH)
    && isSafeText(value.observedAt, MAX_TIMESTAMP_LENGTH);
}

/**
 * Work 级漫画章节目录。请求可以是 `workId` 或 `mediaItemId` 之一，
 * 对应的身份必须原样回显，避免把别的作品/章节的目录当成当前事实。
 */
export function isComicWorkChapterCatalogDto(
  value: unknown,
  expected?: { workId?: string; mediaItemId?: string },
): value is ComicWorkChapterCatalogDto {
  if (!isRecord(value) || !hasExactKeys(value, [
    "schemaVersion",
    "workId",
    "currentMediaItemId",
    "editions",
    "chapters",
    "refreshStatus",
    "lastObservedAt",
    "truncated",
    "refreshReceipts",
  ])) return false;

  if (
    value.schemaVersion !== 1
    || !isCanonicalUuid(value.workId)
    || !isNullableUuid(value.currentMediaItemId)
    || !Array.isArray(value.editions)
    || value.editions.length > MAX_EDITIONS
    || !Array.isArray(value.chapters)
    || value.chapters.length > MAX_CHAPTERS
    || !isWorkCatalogStatus(value.refreshStatus)
    || !isNullableText(value.lastObservedAt, MAX_TIMESTAMP_LENGTH)
    || typeof value.truncated !== "boolean"
    || !Array.isArray(value.refreshReceipts)
    || value.refreshReceipts.length > MAX_REFRESH_RECEIPTS
  ) return false;

  if (expected?.workId !== undefined && value.workId !== expected.workId) return false;
  if (
    expected?.mediaItemId !== undefined
    && value.currentMediaItemId !== expected.mediaItemId
  ) return false;

  if (!value.refreshReceipts.every(isRefreshReceipt)) return false;

  const editionIds = new Set<string>();
  for (const edition of value.editions) {
    if (!isWorkEdition(edition) || editionIds.has(edition.editionId)) return false;
    editionIds.add(edition.editionId);
  }

  const mediaItemIds = new Set<string>();
  return value.chapters.every((chapter) => {
    if (!isWorkChapter(chapter)) return false;
    // 后端保证章节只引用本次返回的 Edition；否则版本筛选无法确定语义。
    if (!editionIds.has(chapter.editionId)) return false;
    if (mediaItemIds.has(chapter.mediaItemId)) return false;
    mediaItemIds.add(chapter.mediaItemId);
    return true;
  });
}
