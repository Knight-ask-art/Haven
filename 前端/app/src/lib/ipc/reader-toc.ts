import type { ReaderTocResultDto, TocItemDto } from "./generated/wire";

const CANONICAL_UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const MAX_TOC_ITEMS = 8192;
const MAX_TOC_DEPTH = 255;
const RESULT_FIELDS = new Set(["schemaVersion", "sessionId", "items"]);
/**
 * `TocItemDto` 的五个自有键。Rust DTO 的 `fragment` 是 `#[serde(default)]` 的
 * `Option<String>`（ts-rs 导出为 `fragment?: string`），因此键可以缺席，也可以是
 * 字符串或显式 `null`。这里不用静态集合做长度相等判断：缺席与删除一个键在运行时
 * 无法区分，长度检查只会把合法的缺席条目误判为越界。
 */
const ITEM_FIELDS: ReadonlySet<string> = new Set(["id", "title", "depth", "fragment", "progression"]);
const ITEM_REQUIRED_FIELDS = ["id", "title", "depth", "progression"] as const;

/**
 * 与后端 `resolve_toc_href` 的锚点校验对齐：percent-decode 后 `trim`，
 * 空串被丢弃，超过 512 字符或含控制字符、`/\?#<>"'` 即拒绝。
 */
const MAX_TOC_FRAGMENT_CHARS = 512;
const FORBIDDEN_FRAGMENT_PATTERN = /[/\\?#<>"']/;

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

/**
 * 自有键必须全部落在允许集合内，且所有必需键都在；`fragment` 是唯一的可选键。
 * 非字符串键（Symbol、不可枚举自有钱）一律越界。
 */
function hasAllowedFields(
  value: Record<string, unknown>,
  allowed: ReadonlySet<string>,
  required: readonly string[],
): boolean {
  const ownKeys = Reflect.ownKeys(value);
  return ownKeys.every((key) => typeof key === "string" && allowed.has(key))
    && required.every((key) => ownKeys.includes(key));
}

/**
 * 文档内锚点：缺席与 `null` 都表示「章节起点」（同一个语义），字符串必须与
 * 后端产物同形——已 `trim`、非空、不超过 512 字符、无控制字符、无 `/\?#<>"'`。
 */
function isTocFragment(value: unknown): value is string | null | undefined {
  if (value === undefined || value === null) return true;
  if (typeof value !== "string") return false;
  if (value.length === 0 || value !== value.trim()) return false;
  if (Array.from(value).length > MAX_TOC_FRAGMENT_CHARS) return false;
  return !Array.from(value).some((character) => {
    const codePoint = character.codePointAt(0) ?? 0;
    return codePoint <= 0x1f || codePoint === 0x7f || FORBIDDEN_FRAGMENT_PATTERN.test(character);
  });
}

export function isTocItemDto(value: unknown): value is TocItemDto {
  if (!isRecord(value) || !hasAllowedFields(value, ITEM_FIELDS, ITEM_REQUIRED_FIELDS)) return false;
  if (typeof value.id !== "string" || value.id.trim().length === 0) return false;
  if (typeof value.title !== "string" || value.title.trim().length === 0) return false;
  if (!Number.isSafeInteger(value.depth) || (value.depth as number) < 0 || (value.depth as number) > MAX_TOC_DEPTH) {
    return false;
  }
  if (!isTocFragment(value.fragment)) return false;
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