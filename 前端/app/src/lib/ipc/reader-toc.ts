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

/**
 * 严格自有键集合检查：键集合必须与生成 Wire DTO 的字段完全相等。
 *
 * `Object.keys` 只看可枚举字符串键，symbol 键和 `defineProperty` 造出的非枚举键
 * 都会被漏掉，`{...合法字段, [Symbol()]: x}` 这类结构会被当成闭合的 wire 数据透传。
 * `Reflect.ownKeys` 取全部自有键（字符串 + symbol，含不可枚举），任何非字符串键
 * 直接越界，未知键与缺失键由长度和逐项比较兜住。
 */
function hasExactFields(value: Record<string, unknown>, allowed: ReadonlySet<string>): boolean {
  const ownKeys = Reflect.ownKeys(value);
  return ownKeys.length === allowed.size
    && ownKeys.every((key) => typeof key === "string" && allowed.has(key));
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