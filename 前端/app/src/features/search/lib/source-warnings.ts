// 来源健康度明细状态（V2-H 收尾批次）。
// 纯函数：从搜索 Channel 事件流累计来源警告（源名 · 安全文案 / 稳定码），
// 供 SourceCandidates 折叠明细列表消费；不触碰 IPC 与 DOM。

import type { SearchSourceEvent } from "@/lib/ipc/generated/wire"

/** 来源警告明细条目。 */
export interface SourceWarning {
  sourceId: string
  code: string
  message: string | null
}

/** 来源 ID → 安全显示名（内置预设中文名；自定义源保留原样）。 */
const SOURCE_DISPLAY_NAMES: Record<string, string> = {
  tvmaze: "TVMaze",
  bangumi: "Bangumi",
  anilist: "AniList",
  itunes: "iTunes Search",
  gutenberg: "Project Gutenberg",
  archive: "Internet Archive",
  mangadex: "MangaDex",
  arxiv: "arXiv",
  crossref: "Crossref",
  openalex: "OpenAlex",
  cms10: "CMS10",
  m3u: "M3U 播放列表",
  opds_gutenberg: "古腾堡计划（OPDS）",
}

export function sourceDisplayName(sourceId: string): string {
  return SOURCE_DISPLAY_NAMES[sourceId] ?? sourceId
}

/**
 * 从 warning 事件累积明细；非 warning 事件返回原数组不变。
 * message 缺失时回退展示稳定 code（安全兜底，不含 URL/路径）。
 */
export function accumulateWarning(
  warnings: SourceWarning[],
  event: SearchSourceEvent,
): SourceWarning[] {
  if (event.kind !== "warning") return warnings
  return [
    ...warnings,
    {
      sourceId: event.data.sourceId ?? "unknown",
      code: event.data.code ?? "UNKNOWN",
      message: event.data.message ?? null,
    },
  ]
}

/** 明细行文案：优先安全 userMessage，缺失回退稳定 code。 */
export function warningLineText(warning: SourceWarning): string {
  return warning.message ?? warning.code
}
