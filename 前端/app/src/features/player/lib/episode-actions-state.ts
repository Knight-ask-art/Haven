// 选集抽屉动作（收藏 / 下载）的纯状态规则。
//
// 这里只回答「从权威事实推出什么」：收藏来自 Work Header，下载来自下载投影，
// 创建任务只说明任务已排队，绝不提前写成「已下载」。

import type { MediaItemDownloadInfo } from "@/features/downloads/ipc/download-gateway"

/** 单集的下载展示状态：意图（待提交）或权威投影。 */
export type EpisodeDownloadView =
  /** 尚未拿到投影：不禁用（用户可点击，由创建前的实时判定兜底）。 */
  | { kind: "unknown"; canDownload: boolean }
  /** 已拿到权威投影。`canDownload` 是后端能力，不是本地推断。 */
  | { kind: "info"; status: "idle" | "queued" | "downloaded"; canDownload: boolean; taskId: string | null }
  /** 投影失败：既不是「可下载」也不是「不可下载」，只是这次没读到事实。 */
  | { kind: "failed"; message: string; canDownload: boolean }
  /** 本页发起的创建请求在途。 */
  | { kind: "creating"; taskId: null }
  /**
   * 本页创建成功、任务刚排队（或已被事件认领）。不能读成 downloaded：
   * 之后以重新读到的投影为准。
   */
  | { kind: "queued"; taskId: string; canDownload: boolean }
  /** 本页为这一集发起的创建失败。 */
  | { kind: "create_failed"; message: string; canDownload: boolean }

export function viewFromDownloadInfo(info: MediaItemDownloadInfo): EpisodeDownloadView {
  return {
    kind: "info",
    status: info.status,
    canDownload: info.canDownload,
    taskId: info.status === "queued" ? info.taskId : null,
  }
}

export function failedDownloadView(message: string): EpisodeDownloadView {
  return { kind: "failed", message, canDownload: false }
}

export type EpisodeDownloadPresentation =
  | { state: "downloaded" }
  | { state: "queued" }
  | { state: "creating" }
  | { state: "actionable"; canStart: boolean; reason: string | null; error: string | null }
  | { state: "unknown"; error: string | null }

/**
 * 把单集视图收敛为抽屉可渲染的结论。
 *
 * - `downloaded` 只来自权威投影的 `downloaded`（离线资源已落地）或本页已确认完成；
 * - `queued` 覆盖权威 `queued` 与本页 `creating` / `queued`（创建成功仅代表排队）；
 * - 投影缺事实时返回 `actionable` 且 `canStart=true`：事实缺失不是不可用的结论，
 *   真正的拒绝由创建动作按点击那一刻的后端投影判定。
 */
export function presentEpisodeDownload(view: EpisodeDownloadView, unavailableReason: string | null): EpisodeDownloadPresentation {
  if (unavailableReason !== null) return { state: "actionable", canStart: false, reason: unavailableReason, error: null }
  switch (view.kind) {
    case "info":
      if (view.status === "downloaded") return { state: "downloaded" }
      if (view.status === "queued") return { state: "queued" }
      return {
        state: "actionable",
        canStart: view.canDownload,
        reason: view.canDownload ? null : "当前剧集没有可保存到离线位置的资源",
        error: null,
      }
    case "queued":
      return { state: "queued" }
    case "creating":
      return { state: "creating" }
    case "create_failed":
      return {
        state: "actionable",
        canStart: view.canDownload,
        reason: view.canDownload ? null : "当前剧集没有可保存到离线位置的资源",
        error: view.message,
      }
    case "failed":
      return { state: "unknown", error: view.message }
    case "unknown":
      return { state: "actionable", canStart: true, reason: null, error: null }
  }
}

/** 创建前的实时判定：没有真实可下载来源就绝不发起。 */
export function canStartEpisodeDownload(info: MediaItemDownloadInfo | null | undefined): boolean {
  // 没读到投影时不据此拦截：gateway 会按同一套规则重新选源并在没有来源时明确失败。
  if (info === null || info === undefined) return true
  return info.canDownload
}

export type EpisodeDownloadOutcome =
  | { outcome: "started" }
  | { outcome: "already"; state: "downloaded" | "queued" }
  | { outcome: "blocked"; reason: string }
  | { outcome: "failed"; message: string }

export interface EpisodeDownloadSummary {
  started: number
  already: number
  blocked: number
  failed: number
  total: number
  failures: Array<{ mediaItemId: string; message: string }>
}

export function summarizeEpisodeDownloads(ids: readonly string[], outcomes: ReadonlyMap<string, EpisodeDownloadOutcome>): EpisodeDownloadSummary {
  const summary: EpisodeDownloadSummary = { started: 0, already: 0, blocked: 0, failed: 0, total: 0, failures: [] }
  for (const id of ids) {
    const result = outcomes.get(id)
    if (!result) continue
    summary.total += 1
    switch (result.outcome) {
      case "started": summary.started += 1; break
      case "already": summary.already += 1; break
      case "blocked": summary.blocked += 1; break
      case "failed":
        summary.failed += 1
        summary.failures.push({ mediaItemId: id, message: result.message })
        break
    }
  }
  return summary
}

/** 徽标：只有权威投影说已落地才写「已下载」。 */
export function episodeBadgeLabel(presentation: EpisodeDownloadPresentation): "downloaded" | "queued" | "creating" | null {
  switch (presentation.state) {
    case "downloaded": return "downloaded"
    case "queued": return "queued"
    case "creating": return "creating"
    case "unknown":
    case "actionable":
      return null
  }
}

/** 本批结果的用户可见摘要；本批没有新动作时返回 null，失败 / 部分失败逐项写明。 */
export function summarizeEpisodeDownloadsText(summary: EpisodeDownloadSummary): string | null {
  const parts: string[] = []
  if (summary.started > 0) parts.push(`已加入下载队列 ${summary.started} 集`)
  if (summary.already > 0) parts.push(`${summary.already} 集已下载或排队中`)
  if (summary.failed > 0) {
    const first = summary.failures[0]
    parts.push(summary.failed === 1 ? `1 集创建失败（${first.message}）` : `${summary.failed} 集创建失败（${first.message} 等）`)
  }
  if (summary.blocked > 0) parts.push(`${summary.blocked} 集没有可下载资源`)
  if (parts.length === 0) return null
  return `${parts.join("，")}。`
}
