import { useCallback, useEffect, useRef, useState } from "react"
import {
  X, Download, Heart, Tv, ArrowUpDown, ChevronRight, Check, LoaderCircle
} from "lucide-react"
import { cn } from "@/lib/utils"
import { useCastDiscovery } from "@/features/player/hooks/useCastDiscovery"
import { playCast, stopCast } from "@/features/cast/ipc/cast-gateway"

export interface EpisodeItem {
  id: string
  number: string | number
  title: string
  durationOrPages?: string
  progress?: number
  /** 仅演示数据使用；生产下载状态一律来自 `downloadBadgeOf` 的权威投影。 */
  isDownloaded?: boolean
  thumbnail?: string
}

/** 单集下载展示结论（由 player hook 从权威下载投影推出，组件只渲染）。 */
export type EpisodeDownloadBadge = "downloaded" | "queued" | "creating" | null

export type EpisodeDownloadActionState =
  | { state: "downloaded" }
  | { state: "queued" }
  | { state: "creating" }
  | { state: "actionable"; canStart: boolean; reason: string | null; error: string | null }
  | { state: "unknown"; error: string | null }

const NO_DOWNLOAD_STATE: EpisodeDownloadActionState = { state: "unknown", error: null }

function disabledStyle(disabled: boolean): string {
  return disabled
    ? "text-muted-foreground/50 cursor-not-allowed"
    : "text-muted-foreground hover:text-foreground cursor-pointer"
}

interface EpisodeDrawerProps {
  isOpen: boolean
  onClose: () => void
  episodes: EpisodeItem[]
  currentEpisodeId?: string
  onSelectEpisode: (episode: EpisodeItem) => void
  onOpenDetails?: () => void
  mediaTitle?: string
  secondaryTitle?: string
  mediaYear?: number
  mediaDescription?: string
  castMediaItemId?: string
  /** 收藏是 Work 的事实；null 表示当前没有权威 Work（只读）。 */
  isFavorited?: boolean | null
  isFavoritePending?: boolean
  favoriteError?: string | null
  favoriteDisabledReason?: string | null
  onToggleFavorite?: () => void
  onReloadFavorite?: () => void
  /** 由 player hook 提供的每集真实下载状态（仅 production）。 */
  downloadStateOf?: (episodeId: string) => EpisodeDownloadActionState
  downloadBadgeOf?: (episodeId: string) => EpisodeDownloadBadge
  /** 下载本集 / 下载全集；返回本批真实结果文案（无失败时为 null）。 */
  onDownloadCurrent?: () => Promise<string | null>
  onDownloadAll?: () => Promise<string | null>
  onReloadDownloads?: () => void
  downloadUnavailableReason?: string | null
  downloadPending?: boolean
  downloadError?: string | null
}

export function EpisodeDrawer({
  isOpen,
  onClose,
  episodes,
  currentEpisodeId,
  onSelectEpisode,
  onOpenDetails,
  mediaTitle = "未知作品",
  secondaryTitle = "",
  mediaYear,
  mediaDescription = "",
  castMediaItemId,
  isFavorited = null,
  isFavoritePending = false,
  favoriteError = null,
  favoriteDisabledReason = null,
  onToggleFavorite,
  onReloadFavorite,
  downloadStateOf,
  downloadBadgeOf,
  onDownloadCurrent,
  onDownloadAll,
  onReloadDownloads,
  downloadUnavailableReason = null,
  downloadPending = false,
  downloadError = null,
}: EpisodeDrawerProps) {
  const [isAscending, setIsAscending] = useState<boolean>(() => {
    try {
      const key = `haven:ui:drawer-sort:${mediaTitle || "unknown"}`
      return localStorage.getItem(key) !== "desc"
    } catch { return true }
  })
  useEffect(() => {
    try {
      const key = `haven:ui:drawer-sort:${mediaTitle || "unknown"}`
      localStorage.setItem(key, isAscending ? "asc" : "desc")
    } catch { /* ignore */ }
  }, [isAscending, mediaTitle])
  const [isDownloadDialogOpen, setIsDownloadDialogOpen] = useState<boolean>(false)
  // 本页最近一次「下载」批次的结果文案（真实失败 / 部分失败才出现）。
  const [downloadResultText, setDownloadResultText] = useState<string | null>(null)
  const [isCastDialogOpen, setIsCastDialogOpen] = useState<boolean>(false)
  const [selectedCastDevice, setSelectedCastDevice] = useState<string | null>(null)
  const [isCasting, setIsCasting] = useState<boolean>(false)
  const [castError, setCastError] = useState<string | null>(null)
  const [isCastPending, setIsCastPending] = useState<boolean>(false)
  const castDiscovery = useCastDiscovery(isCastDialogOpen, 5000)

  // 投屏动作的同步闸门与归属。
  //
  // `castPendingRef` 是同步闩锁：React 的 pending state 要等下一次渲染才生效，同一帧里的
  // 第二次点击读不到它，所以「开始 / 停止」是否已有 play/stop 收尾在途只认 ref。
  // 闩锁只在 play/stop 真正收尾后才释放（即使代已失效），旧动作在途期间绝不放行
  // 新动作，避免 B 先开始、迟到的 A 再被清理。
  // `castGenerationRef` 是本次动作的归属：关闭、切集、换视频、卸载都会让它失效，迟到的
  // 成功 / 失败回调据此丢弃，不会把旧的 castSessionId 写回新的一集。
  // `currentCastSessionRef` 始终指向「当前有效的投屏会话」（唯一权威来源），清理与替换
  // 只依据它，从而不会误停同设备上的新会话。
  // `castStopInFlightRef` 去重重复停止：同一会话的停止请求只会真正调用一次 stopCast。
  const castPendingRef = useRef<boolean>(false)
  const castGenerationRef = useRef<number>(0)
  const currentCastSessionRef = useRef<string | null>(null)
  const castStopInFlightRef = useRef<Map<string, Promise<boolean>>>(new Map())
  const castMounted = useRef<boolean>(true)
  const castOpenRef = useRef<boolean>(isOpen)
  castOpenRef.current = isOpen

  /** 只让代失效；闩锁保持到在途 play/stop 收尾。 */
  const invalidateCastGeneration = useCallback(() => {
    castGenerationRef.current += 1
  }, [])

  /** 释放同步闩锁（只应由真正收尾的 play/stop 调用）。 */
  const releaseCastLatch = useCallback(() => {
    castPendingRef.current = false
    if (castMounted.current) setIsCastPending(false)
  }, [])

  useEffect(() => {
    // StrictMode 会重放 setup：必须重新标记已挂载，否则后续迟到回调全部被丢弃。
    castMounted.current = true
    return () => { castMounted.current = false }
  }, [])

  /** 停止指定会话；同一会话的并发停止共享同一个 promise。返回是否成功。 */
  const requestCastStop = useCallback((sessionId: string): Promise<boolean> => {
    const existing = castStopInFlightRef.current.get(sessionId)
    if (existing) return existing
    const request = stopCast(sessionId)
      .then(() => true, () => false)
      .finally(() => { castStopInFlightRef.current.delete(sessionId) })
    castStopInFlightRef.current.set(sessionId, request)
    return request
  }, [])

  /**
   * 停止当前归属的会话。成功或确认失败后都按 sessionId 清掉当前引用（不依赖代，
   * 代已变化也不会阻止释放闩锁）；失败时保留可重试的归属并在弹窗内提示。
   */
  const stopCurrentCast = useCallback(async (failureMessage: string | null) => {
    const sessionId = currentCastSessionRef.current
    if (sessionId === null) return
    const stopped = await requestCastStop(sessionId)
    if (stopped) {
      if (currentCastSessionRef.current === sessionId) {
        currentCastSessionRef.current = null
        if (castMounted.current) setIsCasting(false)
      }
      return
    }
    // 失败：保留 currentCastSessionRef 作为可重试的归属，下次停止 / 卸载会再次尝试。
    if (castMounted.current && failureMessage !== null && castOpenRef.current) {
      setCastError(failureMessage)
    }
  }, [requestCastStop])

  // 关闭侧栏：停止当前投屏（真实清理）。只对「打开 → 关闭」这一转变生效，
  // 首次挂载时并不存在需要停止的会话。清理同样必须占用同步闩锁：在途的
  // play/stop 收尾前不放行新的开始。
  const wasCastOpenRef = useRef<boolean>(isOpen)
  useEffect(() => {
    const wasOpen = wasCastOpenRef.current
    wasCastOpenRef.current = isOpen
    if (!wasOpen || isOpen) return
    invalidateCastGeneration()
    if (currentCastSessionRef.current === null) return
    if (castPendingRef.current) {
      // 已有动作在途（play 或 stop）：由它收尾；若它随后建立了会话，本代已失效，
      // 迟到的成功会自行停止；若它正在停止，重复请求会复用同一个 stop。
      void stopCurrentCast(null)
      return
    }
    castPendingRef.current = true
    setIsCastPending(true)
    void stopCurrentCast("停止投屏失败，请重新打开投屏窗口重试").finally(releaseCastLatch)
  }, [invalidateCastGeneration, isOpen, releaseCastLatch, stopCurrentCast])

  const previousEpisodeIdRef = useRef<string | undefined>(currentEpisodeId)
  // 切集 / 换视频：当前会话、在途的开始动作一并作废；旧会话仍按归属停止，
  // 停止收尾（或复用已在途的停止）前保持闩锁。
  const previousCastMediaItemIdRef = useRef<string | undefined>(castMediaItemId)
  useEffect(() => {
    const changed = previousEpisodeIdRef.current !== currentEpisodeId
      || previousCastMediaItemIdRef.current !== castMediaItemId
    previousEpisodeIdRef.current = currentEpisodeId
    previousCastMediaItemIdRef.current = castMediaItemId
    if (!changed) return
    invalidateCastGeneration()
    setCastError(null)
    if (currentCastSessionRef.current === null) return
    if (castPendingRef.current) {
      void stopCurrentCast(null)
      return
    }
    castPendingRef.current = true
    setIsCastPending(true)
    void stopCurrentCast("停止上一集投屏失败，请在投屏窗口重试").finally(releaseCastLatch)
  }, [currentEpisodeId, castMediaItemId, invalidateCastGeneration, releaseCastLatch, stopCurrentCast])

  // 离开播放页时撤销当前授权；在途开始请求由其迟到结果分支收尾。
  useEffect(() => {
    return () => {
      castMounted.current = false
      castGenerationRef.current += 1
      castPendingRef.current = false
      const sessionId = currentCastSessionRef.current
      currentCastSessionRef.current = null
      if (sessionId) void requestCastStop(sessionId)
    }
  }, [requestCastStop])

  const runCastToggle = async () => {
    // 同步闸门：上一次 play/stop 收尾未完成时不允许再开始新的动作。
    if (castPendingRef.current) return
    const generation = castGenerationRef.current
    castPendingRef.current = true
    setIsCastPending(true)
    setCastError(null)
    if (currentCastSessionRef.current !== null) {
      // 停止路径：清引用只在会话身份匹配时发生，且不依赖代是否仍有效。
      await stopCurrentCast("停止投屏失败，请重试")
      releaseCastLatch()
      return
    }
    if (!selectedDevice?.supported || !castMediaItemId) {
      releaseCastLatch()
      return
    }
    try {
      const res = await playCast(castMediaItemId, selectedDevice.id)
      if (!castMounted.current || castGenerationRef.current !== generation) {
        // 迟到的成功：授权属于已失效的页面（关闭 / 切集 / 卸载），必须真正释放，
        // 否则设备上会留下一个无人管理的投屏会话。
        const stopped = await requestCastStop(res.castSessionId)
        if (!stopped && castMounted.current) {
          currentCastSessionRef.current = res.castSessionId
          setIsCasting(true)
          setCastError("停止投屏失败，请重试")
        }
        return
      }
      currentCastSessionRef.current = res.castSessionId
      setIsCasting(true)
    } catch (e: unknown) {
      if (!castMounted.current || castGenerationRef.current !== generation) {
        // 没有返回授权句柄；旧页面不接管这次失败。
        return
      }
      setCastError(e instanceof Error ? e.message : "投屏失败，请检查设备是否在线")
    } finally {
      // 无论代是否变化，play 都已收尾，必须释放闩锁；迟到的成功已在上面自行停止。
      releaseCastLatch()
    }
  }


  const displayEpisodes = isAscending ? episodes : [...episodes].reverse()
  const currentEpisode = episodes.find((episode) => episode.id === currentEpisodeId) ?? episodes[0]
  const castDevices = castDiscovery.devices.map((d) => ({
    id: d.deviceId,
    name: d.friendlyName,
    supported: d.protocol === "dlna",
    detail: `${d.protocol === "chromecast" ? "Chromecast · 暂不支持控制" : "DLNA"} · ${d.ip}${d.modelName ? ` · ${d.modelName}` : ""}`,
  }))

  const selectedDevice = castDevices.find((device) => device.id === selectedCastDevice)

  // 下载入口的真实状态：只依据 hook 给出的权威投影，组件自身不推断「已下载」。
  const currentDownloadState = currentEpisode
    ? (downloadStateOf?.(currentEpisode.id) ?? NO_DOWNLOAD_STATE)
    : null
  const drawerDownloadReady = downloadStateOf !== undefined && onDownloadAll !== undefined
  const currentCanDownload = currentDownloadState !== null
    && drawerDownloadReady
    && downloadUnavailableReason === null
    && (currentDownloadState.state === "unknown"
      || currentDownloadState.state === "actionable" && currentDownloadState.canStart)
  const downloadButtonDisabled = downloadUnavailableReason !== null
    || !drawerDownloadReady || episodes.length === 0 || downloadPending
  // 下载不可用（无真实服务 / 无权威状态）时必须可辨：禁用并给出说明，而不是假成功。
  const drawerDownloadUnavailableReason = downloadUnavailableReason
    ?? (!drawerDownloadReady ? "当前运行环境没有本地下载服务，无法创建真实下载任务。" : null)
  const downloadActionLabel = currentDownloadState === null
    ? "下载"
    : currentDownloadState.state === "downloaded"
      ? "已下载"
      : currentDownloadState.state === "queued"
        ? "下载中"
        : currentDownloadState.state === "creating"
          ? "提交中"
          : "下载"
  const downloadActionHint = downloadUnavailableReason
    ?? (currentDownloadState?.state === "actionable" ? currentDownloadState.reason : null)
  const currentDownloadError = currentDownloadState?.state === "unknown" || currentDownloadState?.state === "actionable"
    ? currentDownloadState.error : null

  const runDownloadTarget = async (target: "all" | "current") => {
    if (!drawerDownloadReady || downloadUnavailableReason !== null || (target === "current" && !currentCanDownload)) return
    const action = target === "all" ? onDownloadAll : onDownloadCurrent
    if (!action) return
    setDownloadResultText(null)
    try {
      setDownloadResultText(await action())
    } catch (cause) {
      setDownloadResultText(cause instanceof Error ? cause.message : "创建下载任务失败，请稍后重试。")
    } finally {
      // 关闭弹窗：结果只反映真实创建（排队），不代表下载完成。
      setIsDownloadDialogOpen(false)
    }
  }

  // 所有 Hook 都已在上面按固定顺序执行完毕；这里只决定是否渲染。
  if (!isOpen) return null

  return (
    <aside className="w-[min(460px,34vw)] min-w-[320px] h-full bg-white dark:bg-zinc-950 text-foreground border-l border-black/10 dark:border-white/10 flex flex-col shrink-0 select-none shadow-2xl z-30 transition-all duration-300">
      
      {/* 
        ====================================================
        1. 顶栏 (显示二级标题，如“第01集 / 湖底的秘密尸体”)
        ====================================================
      */}
      <div className="flex items-center justify-between px-6 pt-5 pb-3 border-b border-black/5 dark:border-white/10 shrink-0">
        <div className="flex items-center gap-[8px]">
          <span className="text-lg font-extrabold text-foreground pb-1 relative after:absolute after:bottom-0 after:left-0 after:right-0 after:h-0.5 after:bg-foreground">
            {secondaryTitle}
          </span>
        </div>

        <button
          onClick={onClose}
          aria-label="关闭侧栏"
          className="w-10 h-10 rounded-full flex items-center justify-center text-muted-foreground hover:text-foreground hover:bg-black/5 dark:hover:bg-white/10 transition-all cursor-pointer"
          title="关闭侧栏"
        >
          <X className="w-6 h-6" />
        </button>
      </div>

      {/* 
        ====================================================
        2. 内容滚动区
        ====================================================
      */}
      <div className="flex-1 overflow-y-auto scrollbar-none p-6 flex flex-col gap-6">
        
        {/* 标题与简介卡片 */}
        <div className="flex flex-col gap-3">
          <div className="flex items-start justify-between gap-[8px]">
            <h1 className="text-2xl font-black tracking-tight text-foreground leading-tight">
              {mediaTitle}
            </h1>
            <button
              type="button"
              onClick={onOpenDetails}
              className="flex items-center text-xs font-bold text-muted-foreground hover:text-foreground shrink-0 mt-1"
            >
              <span>简介</span>
              <ChevronRight className="w-3.5 h-3.5" />
            </button>
          </div>

          {/* 标签 Badge 组（真实元数据，缺省不占位） */}
          {(mediaYear || mediaDescription) && (
            <div className="flex flex-wrap items-center gap-[8px]">
              {mediaYear && (
                <span className="px-[8px] py-0.5 rounded bg-black/5 dark:bg-white/10 text-xs font-semibold text-muted-foreground">
                  {mediaYear}
                </span>
              )}
              <span className="px-[8px] py-0.5 rounded bg-black/5 dark:bg-white/10 text-xs font-semibold text-muted-foreground">
                TV
              </span>
            </div>
          )}

          {/* 简短概述（空描述不占位） */}
          {mediaDescription && (
            <p className="text-xs text-muted-foreground line-clamp-2 leading-relaxed font-medium mt-1">
              {mediaDescription}
            </p>
          )}

          <div className="flex items-center gap-[8px] text-xs font-semibold text-muted-foreground">
            <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" />
            <span>自动选择可播放源</span>
          </div>
        </div>

        {/* 快捷功能工具栏 */}
        <div className="grid grid-cols-3 gap-[8px] p-3 rounded-2xl bg-black/5 dark:bg-white/5 border border-black/5 dark:border-white/5">
          <button
            onClick={() => setIsDownloadDialogOpen(true)}
            disabled={!drawerDownloadReady || downloadUnavailableReason !== null || downloadButtonDisabled || downloadPending}
            title={downloadActionHint ?? drawerDownloadUnavailableReason ?? undefined}
            className={cn(
              "flex flex-col items-center justify-center gap-1 py-1.5 transition-colors rounded-xl",
              currentDownloadState?.state === "downloaded"
                ? "text-emerald-500"
                : currentDownloadState?.state === "queued" || currentDownloadState?.state === "creating"
                  ? "text-blue-500"
                  : disabledStyle(!drawerDownloadReady || downloadUnavailableReason !== null || downloadButtonDisabled || downloadPending)
            )}
            aria-label="下载"
            aria-disabled={!drawerDownloadReady || downloadUnavailableReason !== null || downloadButtonDisabled || downloadPending}
          >
            <Download className={cn("w-5 h-5", downloadPending && "opacity-60")} />
            <span className="text-[11px] font-semibold">
              {downloadPending ? "提交中" : downloadActionLabel}
            </span>
          </button>
          <button
            onClick={() => onToggleFavorite?.()}
            disabled={isFavorited === null || isFavoritePending || favoriteDisabledReason !== null || !onToggleFavorite}
            title={favoriteDisabledReason ?? "收藏"}
            aria-pressed={isFavorited === true}
            className={cn(
              "flex flex-col items-center justify-center gap-1 py-1.5 transition-colors rounded-xl",
              isFavorited === true
                ? "text-red-500"
                : disabledStyle(isFavorited === null || isFavoritePending || favoriteDisabledReason !== null || !onToggleFavorite)
            )}
          >
            <Heart className={cn("w-5 h-5", isFavorited === true && "fill-current")} />
            <span className="text-[11px] font-semibold">{isFavoritePending ? "保存中" : "收藏"}</span>
          </button>
          <button
            onClick={() => setIsCastDialogOpen(true)}
            className={cn(
              "flex flex-col items-center justify-center gap-1 py-1.5 transition-colors cursor-pointer",
              isCasting ? "text-emerald-500" : "text-muted-foreground hover:text-foreground"
            )}
            aria-label="投屏"
          >
            <Tv className="w-5 h-5" />
            <span className="text-[11px] font-semibold">{isCastPending ? "处理中" : isCasting ? "已投屏" : "投屏"}</span>
          </button>
        </div>

        {(favoriteError || downloadError || currentDownloadError || downloadResultText || drawerDownloadUnavailableReason) && (
          <div className="flex flex-col gap-1 text-xs leading-relaxed">
            {favoriteError && <div className="flex items-center gap-2 text-red-600 dark:text-red-400">
              <p role="alert">{favoriteError}</p>
              {onReloadFavorite && <button type="button" onClick={onReloadFavorite} disabled={isFavoritePending} className="shrink-0 underline disabled:opacity-50">重新读取</button>}
            </div>}
            {drawerDownloadUnavailableReason && <p className="text-muted-foreground">{drawerDownloadUnavailableReason}</p>}
            {(downloadError || currentDownloadError) && <div className="flex items-center gap-2 text-red-600 dark:text-red-400">
              <p role="alert">{downloadError ?? currentDownloadError}</p>
              {onReloadDownloads && <button type="button" onClick={onReloadDownloads} disabled={downloadPending} className="shrink-0 underline disabled:opacity-50">重新读取</button>}
            </div>}
            {downloadResultText && <p className="text-amber-600 dark:text-amber-400">{downloadResultText}</p>}
          </div>
        )}

        {/* 选集 Selector Grid (带集数与二级标题) */}
        <div className="flex flex-col gap-3 pt-[8px]">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-[8px]">
              <h3 className="text-sm font-bold text-foreground">选集</h3>
              <button 
                onClick={() => setIsAscending(!isAscending)}
                className="flex items-center gap-1 text-xs font-semibold text-muted-foreground hover:text-foreground transition-colors cursor-pointer"
              >
                <ArrowUpDown className="w-3 h-3" />
                <span>{isAscending ? "正序" : "倒序"}</span>
              </button>
            </div>
            <span className="text-xs font-semibold text-muted-foreground">
              05|周五上午
            </span>
          </div>

          {/* 集数与二级标题列表/网格 */}
          <div className="grid grid-cols-1 gap-[12px] sm:grid-cols-2">
            {displayEpisodes.map((ep) => {
              const isSelected = ep.id === currentEpisodeId
              const isWatched = ep.progress === 100
              const hasProgress = ep.progress !== undefined && ep.progress > 0 && ep.progress < 100
              const downloadBadge = downloadBadgeOf?.(ep.id) ?? null

              return (
                <button
                  key={ep.id}
                  onClick={() => onSelectEpisode(ep)}
                  className={cn(
                    "relative min-h-[44px] px-3.5 py-[8px] rounded-xl text-xs transition-all cursor-pointer border flex flex-col justify-center text-left overflow-hidden",
                    isSelected
                      ? "bg-blue-500/10 text-blue-500 dark:text-blue-400 border-blue-500/40 shadow-sm font-bold"
                      : isWatched
                        ? "bg-green-500/10 text-green-700 dark:text-green-400 border-green-500/20 font-medium"
                        : "bg-black/5 dark:bg-white/5 text-foreground/80 border-transparent hover:bg-black/10 dark:hover:bg-white/10 font-medium"
                  )}
                  title={`${ep.number} ${ep.title}${downloadBadge === "downloaded" ? " · 已下载" : downloadBadge ? " · 下载中" : ""}${isWatched ? " · 已看" : hasProgress ? ` · ${Math.round(ep.progress!)}%` : ""}`}
                >
                  <span className="flex items-center gap-1 font-bold text-xs truncate w-full">
                    {isWatched && <Check className="h-3 w-3 shrink-0" />}
                    {ep.number}
                    {downloadBadge && (
                      <span className={cn(
                        "ml-auto shrink-0 rounded px-1 text-[10px] font-semibold",
                        downloadBadge === "downloaded"
                          ? "bg-emerald-500/15 text-emerald-600 dark:text-emerald-400"
                          : "bg-blue-500/15 text-blue-600 dark:text-blue-400"
                      )}>
                        {downloadBadge === "downloaded" ? "已下载" : downloadBadge === "creating" ? "提交中" : "下载中"}
                      </span>
                    )}
                  </span>
                  {ep.title && (
                    <span className="text-[11px] opacity-75 truncate w-full mt-0.5">
                      {ep.title}
                    </span>
                  )}
                  {hasProgress && (
                    <span className="absolute bottom-0 left-0 h-0.5 bg-primary/60" style={{ width: `${ep.progress}%` }} />
                  )}
                </button>
              )
            })}
          </div>
        </div>
      </div>

      {isDownloadDialogOpen && (
        <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/40 p-5 backdrop-blur-sm">
          <div
            role="dialog"
            aria-modal="true"
            aria-labelledby="download-dialog-title"
            className="w-full max-w-[420px] rounded-3xl border border-black/10 bg-white p-5 text-foreground shadow-2xl dark:border-white/10 dark:bg-zinc-900"
          >
            <div className="flex items-start justify-between gap-[16px]">
              <div>
                <p className="text-xs font-semibold uppercase tracking-[0.16em] text-muted-foreground">离线下载</p>
                <h2 id="download-dialog-title" className="mt-1 text-xl font-black tracking-tight">下载内容</h2>
                <p className="mt-1 text-sm text-muted-foreground">{mediaTitle}</p>
              </div>
              <button
                type="button"
                onClick={() => setIsDownloadDialogOpen(false)}
                aria-label="关闭下载弹窗"
                className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-black/5 hover:text-foreground dark:hover:bg-white/10"
              >
                <X className="h-5 w-5" />
              </button>
            </div>

            <div className="mt-5 space-y-3">
              <button
                type="button"
                onClick={() => void runDownloadTarget("all")}
                disabled={!drawerDownloadReady || downloadUnavailableReason !== null || downloadPending}
                className="flex w-full items-center justify-between rounded-2xl border border-black/10 p-[16px] text-left transition-colors hover:border-foreground/40 hover:bg-black/[0.03] disabled:cursor-not-allowed disabled:opacity-50 dark:border-white/10 dark:hover:bg-white/[0.04]"
              >
                <span className="flex items-center gap-3">
                  <span className="flex h-10 w-10 items-center justify-center rounded-xl bg-black/5 dark:bg-white/10">
                    <Download className="h-5 w-5" />
                  </span>
                  <span>
                    <span className="block text-sm font-bold">下载全集</span>
                    <span className="mt-0.5 block text-xs text-muted-foreground">共 {episodes.length} 集</span>
                  </span>
                </span>
                <ChevronRight className="h-[16px] w-[16px] text-muted-foreground" />
              </button>

              <button
                type="button"
                onClick={() => void runDownloadTarget("current")}
                disabled={!currentCanDownload || downloadPending}
                className="flex w-full items-center justify-between rounded-2xl border border-black/10 p-[16px] text-left transition-colors hover:border-foreground/40 hover:bg-black/[0.03] disabled:cursor-not-allowed disabled:opacity-50 dark:border-white/10 dark:hover:bg-white/[0.04]"
              >
                <span className="flex items-center gap-3">
                  <span className="flex h-10 w-10 items-center justify-center rounded-xl bg-black/5 dark:bg-white/10">
                    <Download className="h-5 w-5" />
                  </span>
                  <span>
                    <span className="block text-sm font-bold">下载本集</span>
                    <span className="mt-0.5 block text-xs text-muted-foreground">
                      {currentEpisode?.number ?? secondaryTitle} · {currentEpisode?.title ?? "当前播放内容"}
                    </span>
                  </span>
                </span>
                <ChevronRight className="h-[16px] w-[16px] text-muted-foreground" />
              </button>
            </div>

            {currentDownloadState?.state === "actionable" && currentDownloadState.reason && (
              <p className="mt-3 text-xs text-muted-foreground">{currentDownloadState.reason}</p>
            )}
            <p className="mt-[16px] text-xs leading-relaxed text-muted-foreground">
              {drawerDownloadReady
                ? "栖阅会为每一集选择可下载的来源并加入下载队列；加入队列后仍需在下载页查看完成进度。"
                : "当前运行环境没有本地下载服务，下载功能不可用。"}
            </p>
          </div>
        </div>
      )}

      {isCastDialogOpen && (
        <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/40 p-5 backdrop-blur-sm">
          <div
            role="dialog"
            aria-modal="true"
            aria-labelledby="cast-dialog-title"
            className="w-full max-w-[420px] rounded-3xl border border-black/10 bg-white p-5 text-foreground shadow-2xl dark:border-white/10 dark:bg-zinc-900"
          >
            <div className="flex items-start justify-between gap-[16px]">
              <div>
                <p className="text-xs font-semibold uppercase tracking-[0.16em] text-muted-foreground">Haven Cast</p>
                <h2 id="cast-dialog-title" className="mt-1 text-xl font-black tracking-tight">投屏到设备</h2>
                <p className="mt-1 text-sm text-muted-foreground">选择一个设备继续播放</p>
              </div>
              <button
                type="button"
                onClick={() => setIsCastDialogOpen(false)}
                aria-label="关闭投屏弹窗"
                className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-black/5 hover:text-foreground dark:hover:bg-white/10"
              >
                <X className="h-5 w-5" />
              </button>
            </div>

            <div className="mt-5 space-y-[8px]">
              {castDiscovery.scanning && castDevices.length === 0 && (
                <div className="flex items-center gap-2 rounded-xl bg-black/[0.04] px-3.5 py-3 text-xs text-muted-foreground dark:bg-white/[0.06]">
                  <LoaderCircle className="h-3.5 w-3.5 animate-spin" />
                  <span>正在发现附近设备…</span>
                  <button type="button" onClick={() => castDiscovery.refresh()} className="ml-auto text-xs font-semibold text-primary">刷新</button>
                </div>
              )}
              {!castDiscovery.scanning && castDevices.length === 0 && (
                <div className="rounded-xl border border-dashed border-black/10 bg-white/60 px-4 py-6 text-center dark:border-white/10 dark:bg-white/[0.04]">
                  <p className="text-sm font-semibold">未发现可用设备</p>
                  <p className="mt-1 text-xs leading-relaxed text-muted-foreground">请确认电视与电脑在同一 Wi-Fi，并放行 Windows 防火墙：允许 haven-tauri 通过专用网络（UDP 3339-3438 / TCP 3500-4499）。</p>
                  <button type="button" onClick={() => castDiscovery.refresh()} className="mt-3 rounded-full border px-3.5 py-1.5 text-xs font-semibold">重新发现</button>
                </div>
              )}
              {castDevices.map((device) => {
                const isSelected = selectedCastDevice === device.id
                return (
                  <button
                    key={device.id}
                    type="button"
                    disabled={!device.supported}
                    onClick={() => setSelectedCastDevice(device.id)}
                    aria-pressed={isSelected}
                    className={cn(
                      "flex w-full items-center justify-between rounded-2xl border p-3.5 text-left transition-colors disabled:cursor-not-allowed disabled:opacity-50",
                      isSelected
                        ? "border-foreground/50 bg-black/[0.04] dark:bg-white/[0.08]"
                        : "border-black/10 hover:border-foreground/30 dark:border-white/10 dark:hover:bg-white/[0.04]"
                    )}
                  >
                    <span className="flex items-center gap-3">
                      <span className="flex h-10 w-10 items-center justify-center rounded-xl bg-black/5 dark:bg-white/10">
                        <Tv className="h-5 w-5" />
                      </span>
                      <span>
                        <span className="block text-sm font-bold">{device.name}</span>
                        <span className="mt-0.5 block text-xs text-muted-foreground">{device.detail}</span>
                      </span>
                    </span>
                    {isSelected && <Check className="h-5 w-5" />}
                  </button>
                )
              })}
            </div>

            {castDiscovery.error && (
              <p className="mt-3 text-xs text-amber-600 dark:text-amber-400">{castDiscovery.error.dto.userMessage}</p>
            )}
            {castError && <p className="mt-2 text-xs text-red-600">{castError}</p>}

            <div className="mt-[16px] flex items-center gap-[8px] text-xs text-muted-foreground">
              <span>首次投屏需放行防火墙，端口动态选择 3500-4499 首可用。</span>
            </div>

            <button
              type="button"
              disabled={(!isCasting && (!selectedDevice?.supported || !castMediaItemId)) || isCastPending}
              aria-busy={isCastPending}
              onClick={() => { void runCastToggle() }}
              className="mt-5 flex h-11 w-full items-center justify-center gap-2 rounded-full bg-foreground px-[16px] text-sm font-bold text-background transition-opacity hover:opacity-85 disabled:cursor-not-allowed disabled:opacity-40"
            >
              {isCastPending && <LoaderCircle className="h-4 w-4 animate-spin" />}
              {isCastPending ? (isCasting ? "正在停止投屏…" : "正在开始投屏…") : (isCasting ? "停止投屏" : "开始投屏")}
            </button>
          </div>
        </div>
      )}
    </aside>
  )
}
