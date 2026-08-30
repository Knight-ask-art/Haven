import type { ComicPageManifestDto } from "./generated/wire";

const CANONICAL_UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const COMIC_PAGE_URI_PATTERN = /^haven-resource:\/\/comic-page\/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$/;
const MAX_COMIC_PAGES = 5_000;
const MANIFEST_FIELDS = new Set(["schemaVersion", "sessionId", "mediaItemId", "pageCount", "pages"]);
const PAGE_FIELDS = new Set(["pageId", "pageIndex", "availability", "contentUri"]);

export interface ComicPageManifestExpectation {
  sessionId: string;
  mediaItemId: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasExactFields(value: Record<string, unknown>, allowed: ReadonlySet<string>): boolean {
  const keys = Object.keys(value);
  return keys.length === allowed.size && keys.every((key) => allowed.has(key));
}

function isCanonicalUuid(value: unknown): value is string {
  return typeof value === "string" && CANONICAL_UUID_PATTERN.test(value);
}

export function isComicPageManifestDto(
  value: unknown,
  expected?: ComicPageManifestExpectation,
): value is ComicPageManifestDto {
  if (!isRecord(value) || !hasExactFields(value, MANIFEST_FIELDS)) return false;
  if (
    value.schemaVersion !== 1
    || !isCanonicalUuid(value.sessionId)
    || typeof value.mediaItemId !== "string"
    || value.mediaItemId.trim().length === 0
    || !Number.isSafeInteger(value.pageCount)
    || (value.pageCount as number) < 0
    || (value.pageCount as number) > MAX_COMIC_PAGES
    || !Array.isArray(value.pages)
    || value.pageCount !== value.pages.length
  ) return false;
  if (expected && (
    value.sessionId !== expected.sessionId
    || value.mediaItemId !== expected.mediaItemId
  )) return false;

  const pageIds = new Set<string>();
  const grantIds = new Set<string>();
  for (const [pageIndex, pageValue] of value.pages.entries()) {
    if (!isRecord(pageValue) || !hasExactFields(pageValue, PAGE_FIELDS)) return false;
    if (!isCanonicalUuid(pageValue.pageId) || pageIds.has(pageValue.pageId)) return false;
    if (pageValue.pageIndex !== pageIndex) return false;
    pageIds.add(pageValue.pageId);

    if (pageValue.availability === "unavailable") {
      if (pageValue.contentUri !== null) return false;
      continue;
    }
    if (pageValue.availability !== "ready" || typeof pageValue.contentUri !== "string") {
      return false;
    }
    const match = COMIC_PAGE_URI_PATTERN.exec(pageValue.contentUri);
    if (!match || grantIds.has(match[1])) return false;
    grantIds.add(match[1]);
  }
  for (const pageId of pageIds) {
    if (grantIds.has(pageId)) return false;
  }
  return true;
}
