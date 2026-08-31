import type {
  SourceCategoryDto,
  SourceDescriptorDto,
  SourceKindDto,
  SourceModeDto,
} from "@/lib/ipc/generated/wire"

export const SOURCE_CATEGORY_ORDER: readonly SourceCategoryDto[] = [
  "video",
  "book",
  "comic",
  "periodical",
]

export const SOURCE_CATEGORY_LABELS: Record<SourceCategoryDto, string> = {
  video: "影视",
  book: "图书",
  comic: "漫画",
  periodical: "报刊文章",
}
export const SOURCE_CATEGORY_DESCRIPTIONS: Record<SourceCategoryDto, string> = {
  video: "影视作品的资料、播放和下载来源",
  book: "电子书目录、书库和下载来源",
  comic: "漫画作品资料与后续漫画来源",
  periodical: "报刊、文章和资料类内容来源",
}

export const SOURCE_MODE_LABELS: Record<SourceModeDto, string> = {
  collection: "聚合来源",
  single: "单一来源",
}

export const SOURCE_MODE_DESCRIPTIONS: Record<SourceModeDto, string> = {
  collection: "一个入口可以包含多个上游来源",
  single: "一个入口对应一个 Provider 或目录",
}

export const SOURCE_KIND_LABELS: Record<SourceKindDto, string> = {
  search: "可搜索",
  online_read: "在线打开",
  offline_download: "保存本地",
}

export function sourceCategoryLabel(category: SourceCategoryDto): string {
  return SOURCE_CATEGORY_LABELS[category]
}

export function sourceModeLabel(mode: SourceModeDto): string {
  return SOURCE_MODE_LABELS[mode]
}

export function sourceCapabilityLabels(kinds: readonly SourceKindDto[]): string[] {
  return kinds.map((kind) => SOURCE_KIND_LABELS[kind])
}

export function sourceHasCapability(
  source: Pick<SourceDescriptorDto, "kinds">,
  capability: SourceKindDto,
): boolean {
  return source.kinds.includes(capability)
}

/** 只有这些来源的端点由用户配置；固定 Provider 的地址由后端拥有。 */
export function sourceUsesConfiguredEndpoint(sourceId: string): boolean {
  return sourceId === "cms10" || sourceId === "m3u" || sourceId.startsWith("custom_")
}

export function sourceMatchesCategory(
  source: Pick<SourceDescriptorDto, "categories">,
  category: SourceCategoryDto | "all",
): boolean {
  return category === "all" || source.categories.includes(category)
}

export function groupSourcesByMode(
  sources: readonly SourceDescriptorDto[],
): { collection: SourceDescriptorDto[]; single: SourceDescriptorDto[] } {
  return {
    collection: sources.filter((source) => source.mode === "collection"),
    single: sources.filter((source) => source.mode === "single"),
  }
}
