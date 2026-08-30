import type { ReaderTocResultDto, TocItemDto } from "./generated/wire";

const CANONICAL_UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const MAX_TOC_ITEMS = 8192;
const MAX_TOC_DEPTH = 255;
const RESULT_FIELDS = new Set(["schemaVersion", "sessionId", "items"]);
const ITEM_FIELDS = new Set(["id", "title", "depth", "progression"]);

export interface ReaderTocExpectation {
  sessionId: string;
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

export function isTocItemDto(value: unknown): value is TocItemDto {
  if (!isRecord(value) || !hasExactFields(value, ITEM_FIELDS)) return false;
  if (typeof value.id !== "string" || value.id.trim().length === 0) return false;
  if (typeof value.title !== "string" || value.title.trim().length === 0) return false;
  if (!Number.isSafeInteger(value.depth) || (value.depth as number) < 0 || (value.depth as number) > MAX_TOC_DEPTH) {
    return false;
  }
  const progression = value.progression;
  if (typeof progression !== "number" || !Number.isFinite(progression)) return false;
  return progression >= 0 && progression <= 1;
}

export function isReaderTocResultDto(
  value: unknown,
  expected?: ReaderTocExpectation,
): value is ReaderTocResultDto {
  if (!isRecord(value) || !hasExactFields(value, RESULT_FIELDS)) return false;
  if (value.schemaVersion !== 1) return false;
  if (!isCanonicalUuid(value.sessionId)) return false;
  if (expected && value.sessionId !== expected.sessionId) return false;
  if (!Array.isArray(value.items) || value.items.length > MAX_TOC_ITEMS) return false;
  return value.items.every((item) => isTocItemDto(item));
}