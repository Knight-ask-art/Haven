// 选集抽屉的动作接线：权威收藏 + 每集真实下载任务。
//
// 只有 production 运行时读取 IPC 事实；mock 预览保持演示边界，不伪造生产状态。
// 所有异步结果都经代际令牌回写，旧代 / 旧快照 / 旧读取一律丢弃；事件订阅在卸载 / 换作用域时退订。

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { HavenError } from "@/lib/ipc/errors"
import type { DownloadEvent } from "@/lib/ipc/generated/wire"
import type { HavenClientMode } from "@/lib/ipc/runtime"
import {
  type EpisodeDownloadViews,
  type FavoriteWriteResult,
  readEpisodeDownloadViews,
  readWorkFavorite,
  startEpisodeDownload,
  subscribeEpisodeDownloadEvents,
  subscribeFavoriteChanged,
  writeWorkFavorite,
} from "../lib/episode-actions"
import {
  type EpisodeDownloadOutcome,
  type EpisodeDownloadSummary,
  type EpisodeDownloadView,
  failedDownloadView,
  summarizeEpisodeDownloads,
} from "../lib/episode-actions-state"
import { downloadErrorMessage } from "@/features/downloads/lib/download-error"

export const DRAWER_ACTION_MODE: HavenClientMode = "tauri"

export interface UsePlayerEpisodeActionsInput {
  /**
   * 是否接线真实动作。抽屉只在 production 播放器里被渲染；在其它运行环境
   * （浏览器预览）保持只读，不伪造生产状态。
   */
  enabled: boolean
  /** 抽屉当前展示的集 id（顺序即「下载全集」的顺序）。 */
  episodeIds: readonly string[]
  /** 收藏所属的 Work；未就绪时为 null。 */
  workId: string | null
  /** 当前集；用于「下载本集」。 */
  currentEpisodeId: string | null
  /** 入口未就绪的原因（如播放会话未建立）；非 null 时所有创建动作禁用并给出说明。 */
  unavailableReason: string | null
}

export interface UsePlayerEpisodeActionsResult {
  favorite: {
    isFavorite: boolean
    loading: boolean
    saving: boolean
    canWrite: boolean
    error: string | null
    toggle: () => Promise<void>
    reload: () => Promise<void>
  }
  downloads: {
    viewOf: (mediaItemId: string) => EpisodeDownloadView
    pending: boolean
    error: string | null
    downloadCurrent: () => Promise<EpisodeDownloadSummary | null>
    downloadAll: () => Promise<EpisodeDownloadSummary | null>
    reload: () => Promise<void>
  }
}

const NO_FACT: EpisodeDownloadView = { kind: "unknown", canDownload: true }

export function usePlayerEpisodeActions(
  mode: HavenClientMode,
  input: UsePlayerEpisodeActionsInput,
): UsePlayerEpisodeActionsResult {
  const production = mode === DRAWER_ACTION_MODE && input.enabled
  const { workId, currentEpisodeId, unavailableReason } = input
  // 数组身份每次渲染都可能变化，用 key 作为 effect 依赖。
  const episodeIdsKey = input.episodeIds.join("\n")
  const episodeIds = useMemo(
    () => (episodeIdsKey === "" ? [] : episodeIdsKey.split("\n")),
    [episodeIdsKey],
  )

  const mountedRef = useRef(true)
  useEffect(() => {
    mountedRef.current = true
    return () => { mountedRef.current = false }
  }, [])

  // ---------- 收藏 ----------
  // 每次读取 / 事件各占一个 epoch；只有最新 epoch 才能回写，旧读不能覆盖后到的事件。
  // 收藏只按 Work 事实，换 Work / 卸载即换代。
  const favoriteEpochRef = useRef(0)
  const [favorite, setFavoriteState] = useState(false)
  const [favoriteLoading, setFavoriteLoading] = useState(false)
  const [favoriteSaving, setFavoriteSaving] = useState(false)
  const [favoriteCanWrite, setFavoriteCanWrite] = useState(false)
  const [favoriteError, setFavoriteError] = useState<string | null>(null)
  const favoriteRef = useRef(false)
  favoriteRef.current = favorite
  const favoriteWriteScopeRef = useRef(0)
  const favoriteWritePendingRef = useRef(false)

  const reloadFavorite = useCallback(async () => {
    const epoch = ++favoriteEpochRef.current
    const current = () => favoriteEpochRef.current === epoch && mountedRef.current
    setFavoriteError(null)
    if (!production || workId === null) return
    setFavoriteLoading(true)
    try {
      const truth = await readWorkFavorite(workId)
      if (!current() || truth.workId !== workId) return
      setFavoriteState(truth.favorite)
      setFavoriteCanWrite(true)
    } catch {
      if (!current()) return
      setFavoriteCanWrite(false)
      setFavoriteError("无法读取收藏状态，请重新读取。")
    } finally {
      if (current()) setFavoriteLoading(false)
    }
  }, [production, workId])

  useEffect(() => {
    favoriteWriteScopeRef.current += 1
    favoriteWritePendingRef.current = false
    setFavoriteState(false)
    setFavoriteCanWrite(false)
    setFavoriteSaving(false)
    setFavoriteLoading(false)
    void reloadFavorite()
    return () => {
      favoriteEpochRef.current += 1
      favoriteWriteScopeRef.current += 1
    }
  }, [reloadFavorite])

  useEffect(() => {
    if (!production || workId === null) return undefined
    let disposed = false
    let unsubscribe: (() => void) | null = null
    void subscribeFavoriteChanged((truth) => {
      if (disposed || workId !== truth.workId) return
      void reloadFavorite()
    }).then((dispose) => {
      if (disposed) dispose()
      else unsubscribe = dispose
    })
    return () => {
      disposed = true
      unsubscribe?.()
    }
  }, [production, reloadFavorite, workId])

  const toggleFavorite = useCallback(async () => {
    if (!production || workId === null || favoriteLoading || favoriteWritePendingRef.current || !favoriteCanWrite) return
    const scope = favoriteWriteScopeRef.current
    favoriteWritePendingRef.current = true
    const previous = favoriteRef.current
    const next = !previous
    setFavoriteError(null)
    setFavoriteSaving(true)
    // 乐观切换；以写入结果为最终结论，失败回滚到 previous。
    setFavoriteState(next)
    let result: FavoriteWriteResult
    try {
      result = await writeWorkFavorite(workId, next, previous)
    } catch {
      result = { favorite: previous, error: new HavenError({ code: "INTERNAL_ERROR", userMessage: "收藏操作失败，请稍后重试", retryable: false }) }
    }
    if (favoriteWriteScopeRef.current !== scope || !mountedRef.current) return
    favoriteEpochRef.current += 1
    setFavoriteState(result.favorite)
    setFavoriteLoading(false)
    favoriteWritePendingRef.current = false
    setFavoriteSaving(false)
    if (result.error !== null) setFavoriteError(result.error.dto.userMessage)
    else void reloadFavorite()
  }, [favoriteCanWrite, favoriteLoading, production, reloadFavorite, workId])

  // ---------- 下载 ----------
  // session 中为本页意图（创建中 / 刚排队 / 创建失败），info 中为权威投影。
  const [sessionViews, setSessionViews] = useState<ReadonlyMap<string, EpisodeDownloadView>>(new Map())
  const [infoViews, setInfoViews] = useState<ReadonlyMap<string, EpisodeDownloadView>>(new Map())
  const [downloadPending, setDownloadPending] = useState(false)
  const [downloadError, setDownloadError] = useState<string | null>(null)
  const sessionRef = useRef<ReadonlyMap<string, EpisodeDownloadView>>(sessionViews)
  sessionRef.current = sessionViews
  const infoRef = useRef<ReadonlyMap<string, EpisodeDownloadView>>(infoViews)
  infoRef.current = infoViews
  const taskOwnersRef = useRef(new Map<string, string>())
  // 作用域代际：换作品 / 换集序 / 卸载 / 开关 production 即换代，旧代结果一律丢弃。
  // 每集另有读取 epoch：同一代内后发的刷新覆盖先发的，旧快照不能覆盖新快照。
  const downloadScopeRef = useRef(0)
  const readEpochRef = useRef(new Map<string, number>())
  const inFlightRef = useRef(false)

  const patchSession = useCallback((entries: ReadonlyMap<string, EpisodeDownloadView>, clear: readonly string[] = []) => {
    const next = new Map(sessionRef.current)
    for (const id of clear) next.delete(id)
    for (const [id, view] of entries) next.set(id, view)
    sessionRef.current = next
    setSessionViews(next)
  }, [])

  /**
   * 读取真实投影并回写。`refreshIds` 是这次重读的范围；只有这些集才解除本页意图。
   * 投影里缺条目 = 这次没拿到事实：不拿它注销任务归属。
   */
  const refreshViews = useCallback(async (scope: number, refreshIds: readonly string[]) => {
    if (!production || refreshIds.length === 0) return
    const epochs = new Map<string, number>()
    for (const id of refreshIds) {
      const epoch = (readEpochRef.current.get(id) ?? 0) + 1
      readEpochRef.current.set(id, epoch)
      epochs.set(id, epoch)
    }
    let views: EpisodeDownloadViews
    try {
      views = await readEpisodeDownloadViews(refreshIds)
    } catch (cause) {
      if (downloadScopeRef.current !== scope) return
      const message = toMessage(cause)
      const failed = new Map<string, EpisodeDownloadView>()
      for (const [id, epoch] of epochs) {
        if (readEpochRef.current.get(id) === epoch) failed.set(id, failedDownloadView(message))
      }
      for (const [id, view] of failed) {
        infoRef.current = new Map(infoRef.current).set(id, view)
      }
      setInfoViews(new Map(infoRef.current))
      patchSession(new Map(), [...failed.keys()])
      return
    }
    if (downloadScopeRef.current !== scope) return
    // 归属对账：投影仍报告该集在排队且 taskId 一致时保留；被后发刷新持有的集留给后发结论。
    for (const [taskId, owner] of [...taskOwnersRef.current]) {
      if (readEpochRef.current.get(owner) !== epochs.get(owner)) continue
      const view = views.get(owner)
      if (!views.has(owner)) continue
      if (view?.kind === "info" && view.status === "queued" && view.taskId === taskId) continue
      taskOwnersRef.current.delete(taskId)
    }
    const nextInfo = new Map(infoRef.current)
    const applied: string[] = []
    for (const [mediaItemId, view] of views) {
      if (readEpochRef.current.get(mediaItemId) !== epochs.get(mediaItemId)) continue
      nextInfo.set(mediaItemId, view)
      applied.push(mediaItemId)
      if (view.kind === "info" && view.status === "queued" && view.taskId) {
        taskOwnersRef.current.set(view.taskId, mediaItemId)
      }
    }
    for (const [id, epoch] of epochs) {
      if (!views.has(id) && readEpochRef.current.get(id) === epoch) {
        nextInfo.set(id, failedDownloadView("未能读取该集下载状态，请重新读取。"))
        applied.push(id)
      }
    }
    infoRef.current = nextInfo
    setInfoViews(nextInfo)
    patchSession(new Map(), applied)
  }, [patchSession, production])

  // 换作用域：旧事实、旧归属、旧意图、旧读取一并发作废，并读取这一代的真实投影。
  // 状态只在挂载期间被修改；卸载与作用域切换都只作废，不再写。
  const [downloadSubscriptionScope, setDownloadSubscriptionScope] = useState(0)
  useEffect(() => {
    downloadScopeRef.current += 1
    const scope = downloadScopeRef.current
    taskOwnersRef.current = new Map()
    readEpochRef.current = new Map()
    sessionRef.current = new Map()
    infoRef.current = new Map()
    inFlightRef.current = false
    setDownloadSubscriptionScope(scope)
    setSessionViews(new Map())
    setInfoViews(new Map())
    setDownloadError(null)
    setDownloadPending(false)
    if (production && episodeIds.length > 0) void refreshViews(scope, episodeIds)
    return () => {
      downloadScopeRef.current += 1
      taskOwnersRef.current = new Map()
      readEpochRef.current = new Map()
      inFlightRef.current = false
    }
    // episodeIds 由 episodeIdsKey 派生。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workId, episodeIdsKey, production, refreshViews])

  useEffect(() => {
    if (!production || downloadSubscriptionScope === 0) return
    const scope = downloadSubscriptionScope
    const onEvent = (event: DownloadEvent) => {
      if (downloadScopeRef.current !== scope) return
      const owner = taskOwnersRef.current.get(event.data.taskId)
      const { state, errorCode } = event.data
      if (state !== "completed" && state !== "failed" && state !== "cancelled" && state !== "interrupted") return
      if (owner && (state === "failed" || state === "interrupted")) {
        // 终态失败只带稳定错误码，交给受控码表翻译。
        setDownloadError(errorCode ? downloadErrorMessage(errorCode) : "下载任务需要处理，请重试。")
      }
      // 事件使该集在途读取失效；这里只作废，后续重读才是权威回写。
      // 终态可能早于首次归属读取。未知任务只触发回源，不从事件猜测所属剧集。
      void refreshViews(scope, owner ? [owner] : episodeIds)
    }
    let disposed = false
    let unsubscribe: (() => Promise<void>) | null = null
    void subscribeEpisodeDownloadEvents(onEvent)
      .then((dispose) => {
        if (disposed) return dispose()
        unsubscribe = dispose
        // 订阅建立后回读，覆盖初次查询与监听建立之间的事件窗口。
        void refreshViews(scope, episodeIds)
      })
      .catch(() => undefined)
    return () => {
      disposed = true
      // 退订是异步 IPC；失败只影响监听释放，不影响已卸载页面。
      unsubscribe?.().catch(() => undefined)
    }
  }, [production, downloadSubscriptionScope, episodeIds, refreshViews])

  const runDownloads = useCallback(async (ids: string[]): Promise<EpisodeDownloadSummary | null> => {
    if (!production || unavailableReason !== null || ids.length === 0 || inFlightRef.current) return null
    const scope = downloadScopeRef.current
    const same = () => downloadScopeRef.current === scope && mountedRef.current
    inFlightRef.current = true
    setDownloadPending(true)
    setDownloadError(null)
    const outcomes = new Map<string, EpisodeDownloadOutcome>()
    // 这次落到「创建中」的集：离开创建中时以真实投影为准。
    const touched: string[] = []
    try {
      for (const mediaItemId of ids) {
        if (!same()) return null
        const known = infoRef.current.get(mediaItemId)
        const intent = sessionRef.current.get(mediaItemId)
        if (known?.kind === "info") {
          if (known.status === "downloaded") { outcomes.set(mediaItemId, { outcome: "already", state: "downloaded" }); continue }
          if (known.status === "queued") { outcomes.set(mediaItemId, { outcome: "already", state: "queued" }); continue }
          if (!known.canDownload) {
            outcomes.set(mediaItemId, { outcome: "blocked", reason: "当前剧集没有可保存到离线位置的资源" })
            continue
          }
        }
        if (intent?.kind === "queued" || intent?.kind === "creating") {
          outcomes.set(mediaItemId, { outcome: "already", state: "queued" })
          continue
        }
        touched.push(mediaItemId)
        patchSession(new Map([[mediaItemId, { kind: "creating", taskId: null } satisfies EpisodeDownloadView]]))
        // 逐集创建：前面失败不阻断后面，失败逐条记入结果。
        const result = await startEpisodeDownload(mediaItemId)
        if (!same()) return null
        if (result.outcome === "started") {
          taskOwnersRef.current.set(result.taskId, mediaItemId)
          outcomes.set(mediaItemId, { outcome: "started" })
          // 创建成功只代表已排队。
          patchSession(new Map([[mediaItemId, { kind: "queued", taskId: result.taskId, canDownload: true } satisfies EpisodeDownloadView]]))
        } else if (result.outcome === "already") {
          // 后端说已下载 / 已排队：按真实结论落一个明确状态，而不是继续显示创建中。
          outcomes.set(mediaItemId, { outcome: "already", state: result.state })
          if (result.taskId) taskOwnersRef.current.set(result.taskId, mediaItemId)
          patchSession(new Map([[mediaItemId, {
            kind: "info", status: result.state, canDownload: true, taskId: result.taskId,
          } satisfies EpisodeDownloadView]]))
        } else if (result.outcome === "blocked") {
          outcomes.set(mediaItemId, { outcome: "blocked", reason: result.reason })
          patchSession(new Map([[mediaItemId, { kind: "info", status: "idle", canDownload: false, taskId: null } satisfies EpisodeDownloadView]]))
        } else {
          outcomes.set(mediaItemId, { outcome: "failed", message: result.message })
          patchSession(new Map([[mediaItemId, { kind: "create_failed", message: result.message, canDownload: true } satisfies EpisodeDownloadView]]))
        }
      }
      const summary = summarizeEpisodeDownloads(ids, outcomes)
      // 本批所有处理过的集都以这次重读的投影为准，解除创建中 / 陈旧意图；重读失败时
      // 只解除意图，不把失败读成不可下载。
      if (touched.length > 0) await refreshViews(scope, touched)
      if (!same()) return summary
      if (summary.failed > 0) {
        const first = summary.failures[0]
        setDownloadError(
          summary.failed === 1 ? first.message : `${summary.failed} 集下载创建失败：${first.message}`,
        )
      } else if (summary.blocked > 0) {
        setDownloadError(`有 ${summary.blocked} 集没有可保存到离线位置的资源，未加入下载。`)
      }
      return summary
    } finally {
      // 旧作用域的 finally 不能清掉新作用域的 busy。
      if (downloadScopeRef.current === scope) {
        inFlightRef.current = false
        if (mountedRef.current) setDownloadPending(false)
      }
    }
  }, [patchSession, production, refreshViews, unavailableReason])

  const downloadAll = useCallback(() => runDownloads([...episodeIds]), [episodeIds, runDownloads])
  const reloadDownloads = useCallback(async () => {
    if (!production) return
    setDownloadError(null)
    await refreshViews(downloadScopeRef.current, episodeIds)
  }, [episodeIds, production, refreshViews])
  const downloadCurrent = useCallback(
    () => (currentEpisodeId ? runDownloads([currentEpisodeId]) : Promise.resolve(null)),
    [currentEpisodeId, runDownloads],
  )

  const viewOf = useCallback((mediaItemId: string): EpisodeDownloadView => (
    sessionRef.current.get(mediaItemId) ?? infoRef.current.get(mediaItemId) ?? NO_FACT
  ), [])

  return {
    favorite: {
      isFavorite: favorite,
      loading: favoriteLoading,
      saving: favoriteSaving,
      canWrite: production && favoriteCanWrite,
      error: favoriteError,
      toggle: toggleFavorite,
      reload: reloadFavorite,
    },
    downloads: {
      viewOf,
      pending: downloadPending,
      error: downloadError,
      downloadCurrent,
      downloadAll,
      reload: reloadDownloads,
    },
  }
}

function toMessage(cause: unknown): string {
  return cause instanceof Error && cause.message ? cause.message : "无法读取下载状态，请稍后重试"
}
