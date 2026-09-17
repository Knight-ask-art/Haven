// 期刊层级（`periodical_tree_get`）的严格 Wire 守卫。
//
// 后端已经按 Repository 顺序组装好 期刊 → 卷 → 期 → 文章，前端只做形状与
// 归属闭合校验，**绝不重排、不去重、不按标题猜测归属**。字段集合与 Rust 侧
// `deny_unknown_fields` DTO 一一对应：多一个字段（例如任何传输事实）即越界，
// 少一个字段也不成立——Rust 的 Option 字段没有 `skip_serializing_if`，
// 空值只会显式序列化成 null。
//
// 各字段的边界与 `haven-domain::periodical` 的实体校验对齐（`identity_text` 的
// 512 字符身份文本上限、`Issn::parse` 的 mod-11 校验位与规范 `NNNN-NNNX` 形态、
// `publication_date_year` 的真实日历日、`Periodical::validate` 的 ISSN 身份不变量、
// `page_text` 的 64 字符无空白页码、卷年份 1000..=2999、`u32` 序号、`Doi::parse`
// 的形态），因此这里拒绝的输入本来也无法进入后端持久化。
// 额外多出的一层是前端加固：URL / 本地路径 / 凭据一类传输事实在报刊层级里没有
// 任何合法位置（正文地址由既有 Session/grant 路径单独承担），出现即视为
// Provider 原始响应或签名地址泄漏，整体判为不可信。
//
// `availability` 是 Provider 的正文观察，取值集合闭合为三个字面量。它不是本地
// 阅读能力（真实可读路径由既有 Resource/Session 能力决定），但页面要靠它把
// 「来源只有元数据」与「没有观察到」分开，因此缺字段、近似词或大小写变体都必须
// 整体判为不可信，而不是回退成 `unknown`。

import type {
  PageRangeDto,
  PeriodicalArticleDto,
  PeriodicalDto,
  PeriodicalIssueDto,
  PeriodicalTreeDto,
  PeriodicalVolumeDto,
} from "./generated/wire"

const CANONICAL_UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
/** 后端 `Issn::parse` 只产出规范形式 `NNNN-NNNX`（末位为数字或 X）。 */
const ISSN_PATTERN = /^\d{4}-\d{3}[\dX]$/
const DATE_PART_PATTERN = /^\d{1,2}$/
const WHITESPACE_PATTERN = /\s/
const DOI_FORBIDDEN_PATTERN = /[<>"\\]/

/** 与后端 `MAX_IDENTITY_TEXT` 对齐：标题、出版方、卷标、期号原文、来源身份共用。 */
const MAX_IDENTITY_TEXT = 512
/** 与后端 `MAX_PAGE_TEXT` 对齐：页码文本更短，且不允许空白。 */
const MAX_PAGE_TEXT = 64
/** 与后端 `Doi::parse` 对齐。 */
const MAX_DOI_TEXT = 256

/**
 * 集合规模上限。后端不做截断（整棵树一次性返回），所以这是纯前端的内存/渲染
 * 保护：真实期刊远达不到这些量级（卷按年计、期按年内发行计、期均文章有限）。
 */
const MAX_VOLUMES = 512
const MAX_ISSUES_PER_VOLUME = 512
const MAX_ARTICLES_PER_ISSUE = 1_000

/** 与后端 `u32` 序号字段对齐。 */
const MAX_ORDINAL = 4_294_967_295
/** 卷号/期号的有限非负上界（后端只要求有限且非负，这里是前端量级保护）。 */
const MAX_VOLUME_OR_ISSUE_NUMBER = 1_000_000
/** 与后端 `PeriodicalVolume::validate` 对齐。 */
const MIN_YEAR = 1_000
const MAX_YEAR = 2_999

/**
 * 传输事实模式：`scheme://` 地址、`data:` 载荷、绝对/UNC/回溯路径、带凭据的
 * 查询串、认证头前缀。
 *
 * 其中 `://` 与 `data:` 与后端 `identity_text` 完全一致；路径与凭据是前端加固。
 * 刻意不把裸斜杠当作地址——DOI（`10.1038/xxx`）与页码区间（`S1-S5`）都含斜杠。
 */
const TRANSPORT_FACT_PATTERN =
  /(?:[a-z][a-z0-9+.-]*:\/\/)|(?:^data:)|(?:^[\\/])|(?:^[a-z]:[\\/])|(?:\\\\)|(?:(?:^|[\\/])\.\.(?:[\\/]|$))|(?:[?&;,](?:cookie|authorization|auth|token|access_token|api[_-]?key|apikey|signature|sig|secret|password|credential|session|sessionid|session_id)=)|(?:^(?:bearer|basic)\s)/i

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

/**
 * 严格键集合：`Reflect.ownKeys` 覆盖字符串键、Symbol 键与不可枚举的自有键，
 * 因此运行时多挂上的 Symbol 不会被 `Object.keys` 静默忽略——未知 Symbol 同样是
 * 越界字段，而不是「看不见就不算」。任何非字符串键都直接判为越界。
 */
function hasExactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const actual = Reflect.ownKeys(value)
  if (actual.length !== keys.length) return false
  const expected = new Set<string>(keys)
  return actual.every((key) => typeof key === "string" && expected.has(key))
}

function hasNoControlCharacters(value: string): boolean {
  return !Array.from(value).some((character) => {
    const codePoint = character.codePointAt(0) ?? 0
    return codePoint <= 0x1f || codePoint === 0x7f
  })
}

/** 身份/展示文本：非空、有长度上限、无控制字符、且不携带任何传输事实。 */
function isSafeText(value: unknown, maxLength: number): value is string {
  return typeof value === "string"
    && value.trim().length > 0
    && value.length <= maxLength
    && hasNoControlCharacters(value)
    && !TRANSPORT_FACT_PATTERN.test(value)
}

function isNullableSafeText(value: unknown, maxLength: number): value is string | null {
  return value === null || isSafeText(value, maxLength)
}

/** 页码文本：在身份文本之上再要求不含任何空白（后端 `page_text` 语义）。 */
function isPageText(value: unknown): value is string {
  return isSafeText(value, MAX_PAGE_TEXT) && !WHITESPACE_PATTERN.test(value)
}

function isNullablePageText(value: unknown): value is string | null {
  return value === null || isPageText(value)
}

function isCanonicalUuid(value: unknown): value is string {
  return typeof value === "string" && CANONICAL_UUID_PATTERN.test(value)
}

/** 有限的非负数值：拒绝 NaN/Infinity、负数与离谱量级（卷号/期号）。 */
function isNullableNonNegativeNumber(value: unknown): value is number | null {
  return value === null || (
    typeof value === "number"
    && Number.isFinite(value)
    && value >= 0
    && value <= MAX_VOLUME_OR_ISSUE_NUMBER
  )
}

function isOrdinal(value: unknown): value is number {
  return typeof value === "number"
    && Number.isSafeInteger(value)
    && value >= 0
    && value <= MAX_ORDINAL
}

function isNullableOrdinal(value: unknown): value is number | null {
  return value === null || isOrdinal(value)
}

function isNullableYear(value: unknown): value is number | null {
  return value === null || (
    typeof value === "number"
    && Number.isSafeInteger(value)
    && value >= MIN_YEAR
    && value <= MAX_YEAR
  )
}

/**
 * 与后端 `Issn::parse` 一致的 mod-11 校验位：8 位按权重 8..1 加权求和
 * （`X` 记 10），必须能被 11 整除。调用前已由 `ISSN_PATTERN` 保证
 * 只有前 7 位是数字、末位是数字或 `X`。
 */
function hasValidIssnCheckDigit(value: string): boolean {
  const digits = value.replace("-", "")
  let sum = 0
  for (let index = 0; index < digits.length; index += 1) {
    const character = digits[index]
    const digit = character === "X" ? 10 : Number(character)
    if (!Number.isInteger(digit)) return false
    sum += digit * (8 - index)
  }
  return sum % 11 === 0
}

function isNullableIssn(value: unknown): value is string | null {
  return value === null
    || (typeof value === "string" && ISSN_PATTERN.test(value) && hasValidIssnCheckDigit(value))
}

/** 与后端 `days_in_month` 一致：按真实月份天数（含 4/100/400 闰年规则）判定。 */
function daysInMonth(year: number, month: number): number {
  if (month === 2) {
    return (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0 ? 29 : 28
  }
  return [4, 6, 9, 11].includes(month) ? 30 : 31
}

/** 发行日期精度：`YYYY` / `YYYY-MM` / `YYYY-MM-DD`；月份与日允许来源式不补零写法。 */
function isPublicationDate(value: unknown): value is string {
  if (typeof value !== "string") return false
  const parts = value.split("-")
  if (parts.length < 1 || parts.length > 3) return false
  if (!/^\d{4}$/.test(parts[0])) return false
  const year = Number(parts[0])
  if (year < MIN_YEAR || year > MAX_YEAR) return false
  let monthValue: number | undefined
  const month = parts[1]
  if (month !== undefined) {
    if (!DATE_PART_PATTERN.test(month)) return false
    monthValue = Number(month)
    if (monthValue < 1 || monthValue > 12) return false
  }
  const day = parts[2]
  if (day !== undefined) {
    if (!DATE_PART_PATTERN.test(day) || monthValue === undefined) return false
    // 与后端 `publication_date_year` 一致：日必须落在该年该月的真实天数内，
    // 因此 `2024-02-31`、`2023-02-29` 这类形态合法但不存在的日期一律拒绝。
    const dayValue = Number(day)
    if (dayValue < 1 || dayValue > daysInMonth(year, monthValue)) return false
  }
  return true
}

function isNullablePublicationDate(value: unknown): value is string | null {
  return value === null || isPublicationDate(value)
}

/** DOI：后端 `Doi::parse` 只存形如 `10.<registrant>/<suffix>` 的规范化文本。 */
function isDoi(value: unknown): value is string {
  if (!isSafeText(value, MAX_DOI_TEXT)) return false
  if (WHITESPACE_PATTERN.test(value) || DOI_FORBIDDEN_PATTERN.test(value)) return false
  if (!value.startsWith("10.")) return false
  const slash = value.indexOf("/", 3)
  return slash > 3 && slash < value.length - 1
}

function isPageRange(value: unknown): value is PageRangeDto {
  if (!isRecord(value) || !hasExactKeys(value, ["start", "end"])) return false
  return isPageText(value.start) && isNullablePageText(value.end)
}

/**
 * Provider 正文观察的闭合取值集合。
 *
 * 与后端 `PeriodicalArticleAvailabilityDto` 一一对应，且刻意**不做**任何归一化：
 * `full_text` / `metadata_only` / `unknown` 是三个不同的事实，任何近似词
 * （`readable`、`full`、大小写变体）都必须整体判为不可信。
 */
const ARTICLE_AVAILABILITY_VALUES = ["full_text", "metadata_only", "unknown"] as const

function isArticleAvailability(value: unknown): value is PeriodicalArticleDto["availability"] {
  return typeof value === "string"
    && (ARTICLE_AVAILABILITY_VALUES as readonly string[]).includes(value)
}

function isArticle(value: unknown, issueId: string): value is PeriodicalArticleDto {
  if (!isRecord(value) || !hasExactKeys(value, [
    "id",
    "issueId",
    "mediaItemId",
    "ordinal",
    "title",
    "doi",
    "pageRange",
    "sourceKey",
    "remoteArticleId",
    "availability",
  ])) return false

  const id = value.id
  const parentIssueId = value.issueId
  // 文章必须归属于本期：跨期引用说明响应被拼接或错配，不能当成当前事实。
  if (!isCanonicalUuid(id) || !isCanonicalUuid(parentIssueId) || parentIssueId !== issueId) {
    return false
  }

  return isCanonicalUuid(value.mediaItemId)
    && isNullableOrdinal(value.ordinal)
    && isSafeText(value.title, MAX_IDENTITY_TEXT)
    && (value.doi === null || isDoi(value.doi))
    && (value.pageRange === null || isPageRange(value.pageRange))
    && isSafeText(value.sourceKey, MAX_IDENTITY_TEXT)
    && isSafeText(value.remoteArticleId, MAX_IDENTITY_TEXT)
    // 缺字段或取值越界都不是「默认 unknown」：那是把缺失当结论，页面会据此
    // 把一篇只有元数据的文章显示成没有观察到。
    && isArticleAvailability(value.availability)
}

function isIssue(value: unknown, volumeId: string): value is PeriodicalIssueDto {
  if (!isRecord(value) || !hasExactKeys(value, [
    "id",
    "volumeId",
    "label",
    "number",
    "publicationDate",
    "ordinal",
    "articles",
  ])) return false

  const id = value.id
  const parentVolumeId = value.volumeId
  if (!isCanonicalUuid(id) || !isCanonicalUuid(parentVolumeId) || parentVolumeId !== volumeId) {
    return false
  }

  if (!isNullableSafeText(value.label, MAX_IDENTITY_TEXT)) return false
  if (!isNullableNonNegativeNumber(value.number)) return false
  if (!isNullablePublicationDate(value.publicationDate)) return false
  if (!isOrdinal(value.ordinal)) return false
  if (!Array.isArray(value.articles) || value.articles.length > MAX_ARTICLES_PER_ISSUE) return false

  const articleIds = new Set<string>()
  return value.articles.every((article) => {
    if (!isArticle(article, id)) return false
    // 同期内重复 ID 说明层级被重复展开，渲染键与进度归属都会失真。
    if (articleIds.has(article.id)) return false
    articleIds.add(article.id)
    return true
  })
}

function isVolume(value: unknown, periodicalId: string): value is PeriodicalVolumeDto {
  if (!isRecord(value) || !hasExactKeys(value, [
    "id",
    "periodicalId",
    "label",
    "number",
    "year",
    "ordinal",
    "issues",
  ])) return false

  const id = value.id
  const parentPeriodicalId = value.periodicalId
  if (!isCanonicalUuid(id) || !isCanonicalUuid(parentPeriodicalId) || parentPeriodicalId !== periodicalId) {
    return false
  }

  if (!isNullableSafeText(value.label, MAX_IDENTITY_TEXT)) return false
  if (!isNullableNonNegativeNumber(value.number)) return false
  if (!isNullableYear(value.year)) return false
  if (!isOrdinal(value.ordinal)) return false
  if (!Array.isArray(value.issues) || value.issues.length > MAX_ISSUES_PER_VOLUME) return false

  const issueIds = new Set<string>()
  return value.issues.every((issue) => {
    if (!isIssue(issue, id)) return false
    if (issueIds.has(issue.id)) return false
    issueIds.add(issue.id)
    return true
  })
}

function isPeriodical(value: unknown, workId: string): value is PeriodicalDto {
  if (!isRecord(value) || !hasExactKeys(value, [
    "id",
    "workId",
    "title",
    "issnPrint",
    "issnElectronic",
    "publisher",
  ])) return false

  const id = value.id
  const parentWorkId = value.workId
  // 期刊必须就是本树 Work 的期刊，否则整棵树属于别的作品。
  if (!isCanonicalUuid(id) || !isCanonicalUuid(parentWorkId) || parentWorkId !== workId) return false

  if (!isSafeText(value.title, MAX_IDENTITY_TEXT)) return false

  const issnPrint = value.issnPrint
  const issnElectronic = value.issnElectronic
  if (!isNullableIssn(issnPrint) || !isNullableIssn(issnElectronic)) return false
  // 后端 `Periodical::validate`（040 迁移不变量）：期刊至少需要一个 ISSN 身份，
  // 且 print 与 electronic 若是同一个值就不再是两个身份，不能当成两个。
  if (issnPrint === null && issnElectronic === null) return false
  if (issnPrint !== null && issnPrint === issnElectronic) return false

  return isNullableSafeText(value.publisher, MAX_IDENTITY_TEXT)
}

/**
 * `periodical_tree_get` 响应守卫。
 *
 * `expected.workId` 存在时必须原样回显，避免把别的作品的期刊层级当成当前事实。
 * Work 存在但没有卷是合法的成功结果（`volumes: []`），不是错误。
 */
export function isPeriodicalTreeDto(
  value: unknown,
  expected?: { workId?: string },
): value is PeriodicalTreeDto {
  if (!isRecord(value) || !hasExactKeys(value, [
    "schemaVersion",
    "workId",
    "periodical",
    "volumes",
  ])) return false

  if (value.schemaVersion !== 1) return false

  const workId = value.workId
  if (!isCanonicalUuid(workId)) return false
  if (expected?.workId !== undefined && workId !== expected.workId) return false

  const periodical = value.periodical
  if (!isPeriodical(periodical, workId)) return false

  if (!Array.isArray(value.volumes) || value.volumes.length > MAX_VOLUMES) return false

  const volumeIds = new Set<string>()
  return value.volumes.every((volume) => {
    if (!isVolume(volume, periodical.id)) return false
    // 同层重复卷 ID 会让用户看到重复层级，也让展开状态互相覆盖。
    if (volumeIds.has(volume.id)) return false
    volumeIds.add(volume.id)
    return true
  })
}
