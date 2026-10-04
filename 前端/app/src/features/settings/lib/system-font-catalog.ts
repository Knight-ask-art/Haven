import { isSafeUiFontFamilyName } from "@/lib/ipc/settings-wire"

export type SystemFontEntry = {
  family: string
  fullName: string
  postscriptName: string
  searchIndex: string
}

export type LocalFontRecord = {
  family: string
  fullName?: string
  postscriptName?: string
}

export type SystemFontCatalogResult =
  | { status: "ready"; fonts: SystemFontEntry[] }
  | { status: "unsupported" }
  | { status: "denied" }
  | { status: "error" }

type LocalFontQuery = () => Promise<LocalFontRecord[]>

type LocalFontWindow = Window & {
  queryLocalFonts?: LocalFontQuery
}

function normalizeSearchKey(value: string): string {
  return value.toLocaleLowerCase().replace(/[\s._()+&'-]+/g, "")
}

function createEntry(record: LocalFontRecord, toPinyin: (value: string) => string): SystemFontEntry | null {
  const family = record.family.trim()
  if (!isSafeUiFontFamilyName(family)) return null
  const fullName = record.fullName?.trim() || family
  const postscriptName = record.postscriptName?.trim() || family
  const names = [family, fullName, postscriptName]
  const searchIndex = [...new Set(names.flatMap((name) => [normalizeSearchKey(name), toPinyin(name)]))]
    .filter(Boolean)
    .join(" ")
  return { family, fullName, postscriptName, searchIndex }
}

/**
 * 由用户主动点击后调用本机字体访问 API；不支持或拒绝授权时返回明确状态，不伪造字体清单。
 */
export async function querySystemFontCatalog(
  query: LocalFontQuery | undefined = typeof window === "undefined"
    ? undefined
    : (window as LocalFontWindow).queryLocalFonts?.bind(window),
): Promise<SystemFontCatalogResult> {
  if (!query) return { status: "unsupported" }
  try {
    const records = await query()
    if (records.length === 0) return { status: "ready", fonts: [] }
    // 拼音词库较大，只在用户主动打开本机字体选择器后加载，避免增加应用首屏包体。
    const { pinyin } = await import("pinyin-pro")
    const toPinyin = (value: string) =>
      normalizeSearchKey(pinyin(value, { toneType: "none", nonZh: "consecutive" }))
    const byFamily = new Map<string, SystemFontEntry>()
    for (const record of records) {
      const entry = createEntry(record, toPinyin)
      if (!entry) continue
      const key = normalizeSearchKey(entry.family)
      const previous = byFamily.get(key)
      if (!previous || entry.fullName.length > previous.fullName.length) byFamily.set(key, entry)
    }
    const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" })
    return {
      status: "ready",
      fonts: [...byFamily.values()].sort((left, right) => collator.compare(left.family, right.family)),
    }
  } catch (error) {
    const name = typeof error === "object" && error !== null && "name" in error
      ? String(error.name)
      : ""
    return name === "NotAllowedError" || name === "SecurityError"
      ? { status: "denied" }
      : { status: "error" }
  }
}

/** 同时匹配原文字、英文名和无声调拼音。 */
export function searchSystemFonts(fonts: readonly SystemFontEntry[], query: string): SystemFontEntry[] {
  const trimmed = query.trim()
  if (!trimmed) return [...fonts]
  const key = normalizeSearchKey(trimmed)
  return fonts.filter((font) => font.searchIndex.includes(key))
}
