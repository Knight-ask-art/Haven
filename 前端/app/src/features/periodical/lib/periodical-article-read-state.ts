// 单篇文章的阅读能力状态。
//
// 能力只有一个来源：既有 download Feature 的 `getMediaItemsDownloadInfo` 投影
// （`canOnlineRead` / `hasOfflineResource` / `canDownload`，以及任务状态与 `taskId`）。
// 前端不解析 locator、不看 `isLocal`、不猜来源类型，也不拼任何 URL。
//
// 另一条独立事实是 Provider 的正文观察（`PeriodicalTreeDto.articles[].availability`）。
// 它是关于**来源**的陈述，不是本地阅读能力，因此只在没有真实可读路径时用来把
// 结论说清楚：`metadata_only` 是「来源只有元数据」，`unknown` 是「没有观察到」，
// 两者不能合并成同一句话。
//
// 九个状态必须彼此可辨，尤其：
//   - `unknown` 是「这次探测失败」，不是「不可用」；两者不能合并成同一句话。
//   - `unavailable` 只能说「当前不可阅读」；只有 Provider 明确观察到 `metadata_only`
//     时才允许说「仅有元数据」，否则就是替来源下没有依据的结论。
//   - `readable` 的两个来源是不同的阅读路径，文案必须分开：只有在线读取能力时说
//     「可在线阅读」，只有离线资源时说「可离线阅读」，两者都有时才并列。
//     把仅有离线资源的文章写成「可在线阅读」会指向一条并不存在的路径。
//   - Provider 的观察**永不**阻止已存在的本地内容：只要在线读取能力或离线资源
//     真实存在，就是 `readable`，与来源侧观察到什么无关。
//   - `download_only` 不是死状态：后端给了可下载来源，页面因此能执行「下载后阅读」。
//     它与 `preparing` / `downloading` / `download_failed` 一起构成下载生命周期，
//     四者都**不**可打开——创建成功不等于可读，只有重新读到的真实能力才升级成
//     `readable`。
//   - `metadata_only` 不只是一句文案，也是下载动作的边界：`canStartArticleDownload`
//     让迟到的点击与直接调用都无法为「来源明确没有正文」的一篇创建下载任务。
// 只有 `readable` 允许进入文章阅读器；其余状态一律不导航。

import type { MediaItemDownloadInfo } from "@/features/downloads/ipc/download-gateway"
import type { PeriodicalArticleDto } from "@/lib/ipc/generated/wire"

export type PeriodicalArticleReadState =
  | "loading"
  | "readable"
  | "preparing"
  | "downloading"
  | "download_failed"
  | "download_only"
  | "metadata_only"
  | "unavailable"
  | "unknown"

export interface PeriodicalArticleReadPresentation {
  state: PeriodicalArticleReadState
  label: string
  /** 唯一允许导航到 `/article/:mediaItemId` 的判据。 */
  canOpen: boolean
}

/**
 * 页面在这一篇上的下载动作事实。
 *
 * 它**不是**能力事实（那是 `info`）：`creating` 说的是「创建请求还在路上」，
 * `error` 说的是「上一次下载尝试失败了，原因是什么」。两者都只影响按钮此刻能做
 * 什么和怎么说，绝不参与「这篇文章能不能读」的判断——那只能由重新读到的真实能力
 * （在线读取能力或离线资源）决定。
 */
export interface ArticleDownloadAttempt {
  /** 创建下载任务的请求在途：此时必须禁用重复提交。 */
  creating: boolean
  /** 最近一次下载失败的可读原因；`null` 表示没有未处理的失败。 */
  error: string | null
}

export const NO_DOWNLOAD_ATTEMPT: ArticleDownloadAttempt = { creating: false, error: null }

export type CapabilityFacts = Pick<
  MediaItemDownloadInfo,
  "status" | "canDownload" | "hasOfflineResource" | "canOnlineRead" | "taskId"
>

/** Provider 的正文观察；缺省按「没有观察到」处理，绝不默认成 `metadata_only`。 */
export type ProviderContentAvailability = PeriodicalArticleDto["availability"]

/** 页面按 MediaItem 缓存的能力记录：事实（`info`）与「这次探测是否失败」分开。 */
export interface ArticleCapability {
  info: CapabilityFacts | null
  failed: boolean
}

/**
 * 当前仍然需要监听事件的那个下载任务。
 *
 * 只有状态投影仍说这一篇 `queued` 且带着 `taskId` 时，才存在一个「此刻真的在下载」的
 * 任务，页面才有理由为这个 taskId 保留事件归属。任务一旦进入终态，投影会重新落成
 * `downloaded`（离线资源已落地）或 `idle`：此时 `taskId` 要么为空、要么指向一个已经
 * 结束的任务，继续为它保留归属会让迟到的事件被当成当前任务、重新刷新这一行。
 *
 * 反过来，投影仍说 `queued` 时这个 taskId 就是有效的——哪怕它是页面在新一轮读取里
 * 重新读到的（新树读到同一个仍在下载的任务），事件也必须照常接收。
 *
 * 没有投影时返回 `null`：调用方在拿到真实事实之前不据此对账，既不能登记也不能注销。
 */
export function activeDownloadTaskId(info: CapabilityFacts | null | undefined): string | null {
  if (!info || info.status !== "queued") return null
  return info.taskId ?? null
}

export function resolvePeriodicalArticleReadState(
  info: CapabilityFacts | null | undefined,
  failed = false,
  providerAvailability: ProviderContentAvailability = "unknown",
  download: ArticleDownloadAttempt = NO_DOWNLOAD_ATTEMPT,
): PeriodicalArticleReadPresentation {
  if (failed) return { state: "unknown", label: "阅读能力读取失败", canOpen: false }
  if (!info) return { state: "loading", label: "正在读取阅读能力", canOpen: false }
  // 在线读取能力与离线资源都是可读路径，但它们是两条不同的路径，不能互相代称。
  // 真实的本地内容是最高优先级：Provider 的观察只描述来源，不能锁住用户已有的东西。
  if (info.canOnlineRead) {
    return {
      state: "readable",
      label: info.hasOfflineResource ? "可在线阅读 · 可离线阅读" : "可在线阅读",
      canOpen: true,
    }
  }
  if (info.hasOfflineResource) return { state: "readable", label: "可离线阅读", canOpen: true }
  // 来源已观察到正文不存在：这是关于来源的明确结论，比「当前不可阅读」更具体。
  // 它排在下载生命周期之前——承诺「下载后阅读」会指向一个并不存在的内容，
  // 因此 metadata_only 的文章既不展示下载状态，也不允许触发下载。
  if (providerAvailability === "metadata_only") {
    return { state: "metadata_only", label: "仅有元数据 · 正文未开放", canOpen: false }
  }
  // 已有排队/进行中的任务是最真实的进度事实，优先于页面自己记的「正在创建」。
  if (info.status === "queued") {
    return { state: "downloading", label: "下载中", canOpen: false }
  }
  if (download.creating) {
    return { state: "preparing", label: "正在创建下载任务", canOpen: false }
  }
  // 上一次下载失败：只要这一篇此刻仍可下载，失败就是可重试的处境，
  // 不能被说成「不可用」（那是结论），也不能被说成「已可阅读」。
  if (download.error !== null && info.canDownload) {
    return { state: "download_failed", label: download.error, canOpen: false }
  }
  if (info.canDownload) return { state: "download_only", label: "需要下载后阅读", canOpen: false }
  return { state: "unavailable", label: "当前文章暂不可阅读", canOpen: false }
}

/**
 * 下载动作边界的判据：只有这条为真时，页面才允许为这一篇创建下载任务。
 *
 * `resolvePeriodicalArticleReadState` 说得出「仅有元数据」还不够——动作一旦被调用就
 * 不再经过那个状态机，只要能力投影说 `canDownload`，下载任务就会真的建出来。因此
 * 动作入口必须自己再核一遍 Provider 的正文观察，竞态或直接调用都不例外：来源已经
 * 明确说正文不存在时，创建任务等于把一条并不存在的路径做成事实。
 *
 * 探测失败与缺失的能力事实同样不发起：那是「这次没拿到事实」，不是「可以下载」。
 * 反过来，来源没有观察到 metadata_only 时，能力投影的 `canDownload` 就照常生效——
 * 正常下载与失败后的重试都走这条路径。
 */
export function canStartArticleDownload(
  info: CapabilityFacts | null | undefined,
  failed: boolean,
  providerAvailability: ProviderContentAvailability,
): boolean {
  // 来源侧的结论压过能力投影：能下载不等于有正文可下载。
  if (providerAvailability === "metadata_only") return false
  if (failed) return false
  return info?.canDownload === true
}

/**
 * 文章按钮的可访问名称。
 *
 * 可访问名称必须说出「此刻真实能做的事」或「此刻真实的状态」：`readable` 的按钮
 * 做的是打开正文，`unknown` 的按钮做的是重试读取，`download_only` / `download_failed`
 * 的按钮做的是创建（或重试）下载，`preparing` / `downloading` 的按钮此刻什么都不做，
 * 其余状态根本没有可执行的动作。一律叫「打开」会让读屏用户以为按钮能打开一篇其实
 * 打不开的文章；把可执行的下载说成「打不开」也会让读屏用户错过唯一能做的事。
 *
 * 这里只管可访问名称；按钮上的可见文案与状态行仍由页面按同一份状态渲染。
 */
export function periodicalArticleAccessibleName(
  presentation: PeriodicalArticleReadPresentation,
  title: string,
): string {
  switch (presentation.state) {
    case "readable":
      return `打开《${title}》`
    case "unknown":
      return `重试读取《${title}》的阅读能力`
    case "loading":
      return `正在读取《${title}》的阅读能力`
    case "preparing":
      return `正在创建《${title}》的下载任务`
    case "downloading":
      return `《${title}》正在下载`
    case "download_failed":
      return `重试下载《${title}》`
    case "download_only":
      return `下载《${title}》后阅读`
    case "metadata_only":
      return `《${title}》仅有元数据，正文未开放`
    case "unavailable":
      return `《${title}》当前不可阅读`
  }
}

function capabilitiesFor(
  mediaItemIds: readonly string[],
  build: (mediaItemId: string) => ArticleCapability,
): Record<string, ArticleCapability> {
  const next: Record<string, ArticleCapability> = {}
  for (const mediaItemId of mediaItemIds) next[mediaItemId] = build(mediaItemId)
  return next
}

/** 批量查询在途：还没有任何能力事实，每一篇都是 `loading`。 */
export function pendingArticleCapabilities(
  mediaItemIds: readonly string[],
): Record<string, ArticleCapability> {
  return capabilitiesFor(mediaItemIds, () => ({ info: null, failed: false }))
}

/** 批量查询整体失败：每一篇都是「这次探测失败」，而不是「不可用」。 */
export function failedArticleCapabilities(
  mediaItemIds: readonly string[],
): Record<string, ArticleCapability> {
  return capabilitiesFor(mediaItemIds, () => ({ info: null, failed: true }))
}

/**
 * 批量投影落地。
 *
 * 投影里缺少某个请求 ID 时保持 `unknown`（`failed: true`）：缺条目说明这次没有拿到
 * 那一篇的事实，把它写成 `info: null, failed: false`（`loading`）或「无能力」的明确
 * 结论都是拿缺失当结论。只有真正读到条目才升级成明确判断。
 */
export function projectArticleCapabilities(
  mediaItemIds: readonly string[],
  projected: ReadonlyMap<string, CapabilityFacts | null | undefined>,
): Record<string, ArticleCapability> {
  return capabilitiesFor(mediaItemIds, (mediaItemId) => {
    const info = projected.get(mediaItemId)
    return info ? { info, failed: false } : { info: null, failed: true }
  })
}
