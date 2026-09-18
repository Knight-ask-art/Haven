// 期刊层级的纯展示投影：把后端已排序的事实翻译成文案，**不做任何排序、去重或归属推断**。
//
// 页面按 `volumes → issues → articles` 的数组顺序直接渲染；这里的函数只负责把
// 单个节点上的字段变成人读文本，且只用来源真实给出的字段：
//   - 卷/期标题优先用来源标签（`label`），其次用来源给出的号（`number`），
//     都没有时才退回到后端返回顺序里的位置，且必须写成「第 N 项」这种位置文案
//     （`卷（第 2 项）`）：`ordinal` 是数组下标，写成「第 N 卷」会被读成来源卷号。
//   - 缺字段一律返回 null，由调用方决定不渲染，而不是补一个看起来像事实的默认值。
//   - `sourceKey` 与 `remoteArticleId` 是幂等/归属核对用的身份，不是展示事实，
//     任何情况下都不进入文案或路由。

import type {
  PageRangeDto,
  PeriodicalArticleDto,
  PeriodicalIssueDto,
  PeriodicalTreeDto,
  PeriodicalVolumeDto,
} from "@/lib/ipc/generated/wire"

const YEAR_PATTERN = /^\d{4}$/
const DATE_PART_PATTERN = /^\d{1,2}$/
/** 与 Wire 守卫 `isPeriodicalTreeDto` / 后端 `PeriodicalVolume::validate` 一致的年份边界。 */
const MIN_YEAR = 1_000
const MAX_YEAR = 2_999

/**
 * 与 Wire 守卫 `isPeriodicalTreeDto` 和后端 `publication_date_year` 一致的
 * 真实日历天数（含 4/100/400 闰年规则）。
 */
function daysInMonth(year: number, month: number): number {
  if (month === 2) {
    return (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0 ? 29 : 28
  }
  return [4, 6, 9, 11].includes(month) ? 30 : 31
}

/**
 * 发行日期（`YYYY` / `YYYY-MM` / `YYYY-MM-DD`，月份与日允许来源式不补零写法）。
 *
 * 精度只展开到来源给出的层级：`2024` 不会变成 `2024 年 1 月`。形状不可识别、
 * 年份越界、或日期在该年该月并不存在（`2024-02-31`、`2023-02-29`）时返回 null——
 * 展示层宁可留白，也不猜一个日期。这里的判定与 Wire 守卫逐条对齐：能被守卫放行的
 * 值一定渲染得出来，而守卫会拒绝的值不会在这里被补成看起来合法的日期。
 */
export function formatPublicationDate(value: string | null | undefined): string | null {
  if (typeof value !== "string") return null
  const parts = value.split("-")
  if (parts.length < 1 || parts.length > 3) return null
  const [year, month, day] = parts
  if (!YEAR_PATTERN.test(year)) return null
  const yearNumber = Number(year)
  if (yearNumber < MIN_YEAR || yearNumber > MAX_YEAR) return null
  if (month === undefined) return `${year} 年`
  if (!DATE_PART_PATTERN.test(month)) return null
  const monthNumber = Number(month)
  if (monthNumber < 1 || monthNumber > 12) return null
  if (day === undefined) return `${year} 年 ${monthNumber} 月`
  if (!DATE_PART_PATTERN.test(day)) return null
  const dayNumber = Number(day)
  if (dayNumber < 1 || dayNumber > daysInMonth(yearNumber, monthNumber)) return null
  return `${year} 年 ${monthNumber} 月 ${dayNumber} 日`
}

/** 卷标题：来源标签 → 来源卷号 → 后端顺序位置。 */
export function volumeHeading(volume: PeriodicalVolumeDto): string {
  if (volume.label !== null && volume.label !== "") return volume.label
  if (volume.number !== null) return `第 ${volume.number} 卷`
  // 没有来源标签也没有来源卷号时，`ordinal` 只是后端数组里的位置，不是来源声明的
  // 卷号：写成「第 N 卷」会被读成来源编号，因此只声明它在本树中的位置。
  return `卷（第 ${volume.ordinal + 1} 项）`
}

/** 卷的附加事实（年份）。没有年份时返回 null，不显示占位。 */
export function volumeMeta(volume: PeriodicalVolumeDto): string | null {
  return volume.year === null ? null : `${volume.year} 年`
}

/** 期标题：来源标签 → 来源期号 → 后端顺序位置。 */
export function issueHeading(issue: PeriodicalIssueDto): string {
  if (issue.label !== null && issue.label !== "") return issue.label
  if (issue.number !== null) return `第 ${issue.number} 期`
  // 同上：`ordinal` 是后端返回顺序里的位置，不是来源声明的期号。
  return `期（第 ${issue.ordinal + 1} 项）`
}

/** 期的附加事实（发行日期）。 */
export function issueMeta(issue: PeriodicalIssueDto): string | null {
  return formatPublicationDate(issue.publicationDate)
}

/** 页码区间：只在来源给了结束页时才是区间，否则就是单页。 */
export function formatPageRange(pageRange: PageRangeDto | null): string | null {
  if (!pageRange) return null
  return pageRange.end === null ? pageRange.start : `${pageRange.start}–${pageRange.end}`
}

/** 文章序号：来源没给时返回 null（后端不猜测，前端也不补）。 */
export function articleOrdinalLabel(ordinal: number | null): string | null {
  return ordinal === null ? null : `第 ${ordinal} 篇`
}

/** 文章元信息行：序号 · 页码 · DOI，只拼接真实存在的部分。 */
export function articleMetaText(article: PeriodicalArticleDto): string {
  const parts: string[] = []
  const ordinal = articleOrdinalLabel(article.ordinal)
  if (ordinal !== null) parts.push(ordinal)
  const pages = formatPageRange(article.pageRange)
  if (pages !== null) parts.push(`页码 ${pages}`)
  if (article.doi !== null) parts.push(`DOI ${article.doi}`)
  return parts.join(" · ")
}

export interface IssnEntry {
  key: "print" | "electronic"
  label: string
  value: string
}

/** 只列出后端确实返回的 ISSN 身份；print 与 electronic 不互相推断。 */
export function issnEntries(
  issnPrint: string | null,
  issnElectronic: string | null,
): IssnEntry[] {
  const entries: IssnEntry[] = []
  if (issnPrint !== null) entries.push({ key: "print", label: "ISSN（印刷版）", value: issnPrint })
  if (issnElectronic !== null) {
    entries.push({ key: "electronic", label: "ISSN（电子版）", value: issnElectronic })
  }
  return entries
}

/**
 * 批量读取阅读能力用的 MediaItem 身份列表。
 *
 * 保持后端顺序并去掉重复引用：同一篇正文被多个层级引用时只查一次，但不改变
 * 任何渲染顺序（渲染仍按原数组遍历）。
 */
export function collectArticleMediaItemIds(tree: PeriodicalTreeDto): string[] {
  const seen = new Set<string>()
  const ids: string[] = []
  for (const volume of tree.volumes) {
    for (const issue of volume.issues) {
      for (const article of issue.articles) {
        if (seen.has(article.mediaItemId)) continue
        seen.add(article.mediaItemId)
        ids.push(article.mediaItemId)
      }
    }
  }
  return ids
}
