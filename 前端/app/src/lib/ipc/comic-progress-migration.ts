import type {
  ComicChapterEvidenceDto,
  ComicChapterEvidenceKindDto,
  ComicChapterMatchDto,
  ComicChapterMatchKindDto,
  ComicChapterSourceIdentityDto,
  ComicPageMappingConfidenceDto,
  ComicPageMappingStrategyDto,
  ComicPageMigrationDto,
  ComicPageProgressRemapRequestDto,
  ComicProgressMigrationModeDto,
  ComicProgressMigrationRequestDto,
  ComicProgressMigrationResultDto,
  ComicProgressMigrationRevertRequestDto,
  ComicProgressMigrationRevertResultDto,
  ComicProgressMigrationStatusDto,
  ComicMatchConfidenceDto,
} from "./generated/wire"

const CANONICAL_UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
const MAX_OPAQUE_VALUE_LENGTH = 4096
const MAX_REVISION_LENGTH = 256
const MAX_PAGE_IDENTITIES = 5000

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function hasExactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const actual = Object.keys(value).sort()
  const expected = [...keys].sort()
  return actual.length === expected.length && actual.every((key, index) => key === expected[index])
}

function isSafeOpaque(value: unknown, maxLength = MAX_OPAQUE_VALUE_LENGTH): value is string {
  if (typeof value !== "string") return false
  const trimmed = value.trim()
  return trimmed.length > 0
    && trimmed.length <= maxLength
    && !trimmed.includes("://")
    && !trimmed.toLowerCase().startsWith("data:")
    && !Array.from(trimmed).some((character) => {
      const codePoint = character.codePointAt(0) ?? 0
      return codePoint <= 0x1f || codePoint === 0x7f
    })
}

function isCanonicalUuid(value: unknown): value is string {
  return typeof value === "string" && CANONICAL_UUID_PATTERN.test(value)
}

function isSourceIdentity(value: unknown): value is ComicChapterSourceIdentityDto {
  if (!isRecord(value) || !hasExactKeys(value, ["sourceId", "remoteWorkId", "remoteChapterId"])) {
    return false
  }
  return isSafeOpaque(value.sourceId)
    && isSafeOpaque(value.remoteWorkId)
    && isSafeOpaque(value.remoteChapterId)
}

export function isComicChapterSourceIdentityDto(
  value: unknown,
): value is ComicChapterSourceIdentityDto {
  return isSourceIdentity(value)
}

function isRevision(value: unknown): value is string {
  return isSafeOpaque(value, MAX_REVISION_LENGTH)
}

export function isComicProgressMigrationRequestDto(
  value: unknown,
): value is ComicProgressMigrationRequestDto {
  return isRecord(value)
    && hasExactKeys(value, ["source", "target", "allowTargetOverwrite"])
    && isSourceIdentity(value.source)
    && isSourceIdentity(value.target)
    && typeof value.allowTargetOverwrite === "boolean"
}

export function isComicPageProgressRemapRequestDto(
  value: unknown,
): value is ComicPageProgressRemapRequestDto {
  return isRecord(value)
    && hasExactKeys(value, ["sessionId", "expectedRevision"])
    && isCanonicalUuid(value.sessionId)
    && (value.expectedRevision === null || isRevision(value.expectedRevision))
}

export function isComicProgressMigrationRevertRequestDto(
  value: unknown,
): value is ComicProgressMigrationRevertRequestDto {
  return isRecord(value)
    && hasExactKeys(value, ["migrationId", "expectedAppliedRevision"])
    && isCanonicalUuid(value.migrationId)
    && isRevision(value.expectedAppliedRevision)
}

function isOneOf<T extends string>(value: unknown, allowed: readonly T[]): value is T {
  return typeof value === "string" && allowed.includes(value as T)
}

function isMigrationStatus(value: unknown): value is ComicProgressMigrationStatusDto {
  return isOneOf(value, [
    "unchanged",
    "applied",
    "shared_content",
    "suggested",
    "no_source_progress",
    "target_progress_preserved",
    "no_target_page",
  ])
}

function isMatchKind(value: unknown): value is ComicChapterMatchKindDto {
  return isOneOf(value, [
    "same_remote_chapter",
    "same_content",
    "same_logical_chapter_variant",
    "candidate",
    "unrelated",
  ])
}

function isMatchConfidence(value: unknown): value is ComicMatchConfidenceDto {
  return isOneOf(value, ["high", "medium", "low"])
}

function isMigrationMode(value: unknown): value is ComicProgressMigrationModeDto {
  return isOneOf(value, ["shared", "one_time", "suggested", "none"])
}

function isEvidenceKind(value: unknown): value is ComicChapterEvidenceKindDto {
  return isOneOf(value, [
    "same_remote_identity",
    "authoritative_content_key",
    "edition_compatible",
    "edition_conflict",
    "exact_page_identity",
    "partial_page_identity",
    "matching_chapter_metadata",
    "weak_chapter_metadata",
  ])
}

function isSafeCount(value: unknown): value is number {
  return typeof value === "number"
    && Number.isSafeInteger(value)
    && value >= 0
    && value <= MAX_PAGE_IDENTITIES
}

function isEvidence(value: unknown): value is ComicChapterEvidenceDto {
  if (!isRecord(value) || !hasExactKeys(value, ["kind", "matched"]) || !isEvidenceKind(value.kind)) {
    return false
  }
  const countEvidence = value.kind === "exact_page_identity" || value.kind === "partial_page_identity"
  return countEvidence ? isSafeCount(value.matched) : value.matched === null
}

function isMatch(value: unknown): value is ComicChapterMatchDto {
  return isRecord(value)
    && hasExactKeys(value, ["kind", "confidence", "progressMigration", "evidence"])
    && isMatchKind(value.kind)
    && isMatchConfidence(value.confidence)
    && isMigrationMode(value.progressMigration)
    && Array.isArray(value.evidence)
    && value.evidence.length <= MAX_PAGE_IDENTITIES
    && value.evidence.every(isEvidence)
}

export function isComicChapterMatchDto(value: unknown): value is ComicChapterMatchDto {
  return isMatch(value)
}

function isMappingConfidence(value: unknown): value is ComicPageMappingConfidenceDto {
  return isOneOf(value, ["high", "medium", "low"])
}

function isMappingStrategy(value: unknown): value is ComicPageMappingStrategyDto {
  return isOneOf(value, [
    "stable_key",
    "content_fingerprint",
    "reordered_anchor",
    "nearest_surviving_page",
    "proportional_fallback",
    "no_target",
  ])
}

function isPageMigration(value: unknown): value is ComicPageMigrationDto {
  return isRecord(value)
    && hasExactKeys(value, ["targetPageIndex", "confidence", "strategy", "reversible"])
    && (value.targetPageIndex === null || isSafeCount(value.targetPageIndex))
    && isMappingConfidence(value.confidence)
    && isMappingStrategy(value.strategy)
    && typeof value.reversible === "boolean"
}

export function isComicProgressMigrationResultDto(
  value: unknown,
): value is ComicProgressMigrationResultDto {
  if (!isRecord(value) || !hasExactKeys(value, [
    "status",
    "matchResult",
    "pageMigration",
    "snapshotId",
    "appliedRevision",
  ])) return false

  if (!isMigrationStatus(value.status)
    || !(value.matchResult === null || isMatch(value.matchResult))
    || !isPageMigration(value.pageMigration)
    || !(value.snapshotId === null || isCanonicalUuid(value.snapshotId))
    || !(value.appliedRevision === null || isRevision(value.appliedRevision))) {
    return false
  }

  // `applied` is the only status that mutates Progress and therefore must expose
  // both rollback identity and the CAS revision used for the write.
  if (value.status === "applied") {
    return value.snapshotId !== null && value.appliedRevision !== null
  }
  return value.snapshotId === null && value.appliedRevision === null
}

export function isComicProgressMigrationRevertResultDto(
  value: unknown,
): value is ComicProgressMigrationRevertResultDto {
  return isRecord(value)
    && hasExactKeys(value, ["reverted"])
    && typeof value.reverted === "boolean"
}
