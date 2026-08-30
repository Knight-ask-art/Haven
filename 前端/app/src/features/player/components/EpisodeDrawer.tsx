import { useEffect, useState } from "react"
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
  isDownloaded?: boolean
  thumbnail?: string
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
  const [isFavorited, setIsFavorited] = useState<boolean>(false)
  const [isDownloaded, setIsDownloaded] = useState<boolean>(false)
  const [isDownloadDialogOpen, setIsDownloadDialogOpen] = useState<boolean>(false)
  const [isCastDialogOpen, setIsCastDialogOpen] = useState<boolean>(false)
  const [selectedCastDevice, setSelectedCastDevice] = useState<string | null>(null)
  const [isCasting, setIsCasting] = useState<boolean>(false)
  const [castSessionId, setCastSessionId] = useState<string | null>(null)
  const [castError, setCastError] = useState<string | null>(null)
  const castDiscovery = useCastDiscovery(isCastDialogOpen, 5000)

  useEffect(() => {
    if (!isOpen && castSessionId) {
      void stopCast(castSessionId).catch(() => undefined)
      setCastSessionId(null)
      setIsCasting(false)
    }
  }, [isOpen, castSessionId])

  // 切集自动停止：当前集变化且正在投屏 → 停止旧会话
  useEffect(() => {
    if (isCasting && castSessionId) {
      void stopCast(castSessionId).catch(() => undefined)
      setCastSessionId(null)
      setIsCasting(false)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentEpisodeId])

  // 离开播放页自动停止：卸载时清理
  useEffect(() => {
    return () => {
      if (castSessionId) void stopCast(castSessionId).catch(() => undefined)
    }
  }, [castSessionId])

  if (!isOpen) return null

  const displayEpisodes = isAscending ? episodes : [...episodes].reverse()
  const currentEpisode = episodes.find((episode) => episode.id === currentEpisodeId) ?? episodes[0]
  const castDevices = castDiscovery.devices.map((d) => ({
    id: d.deviceId,
    name: d.friendlyName,
    detail: `${d.protocol === "chromecast" ? "Chromecast" : "DLNA"} · ${d.ip}${d.modelName ? ` · ${d.modelName}` : ""}`,
  }))

  const chooseDownloadTarget = (target: "all" | "current") => {
    setIsDownloaded(true)
    setIsDownloadDialogOpen(false)
    // 当前 UI 只负责表达下载意图，实际队列由 Download Engine 接管。
    void target
  }

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
            className={cn(
              "flex flex-col items-center justify-center gap-1 py-1.5 transition-colors cursor-pointer",
              isDownloaded ? "text-emerald-500" : "text-muted-foreground hover:text-foreground"
            )}
            aria-label="下载"
          >
            <Download className="w-5 h-5" />
            <span className="text-[11px] font-semibold">{isDownloaded ? "已下载" : "下载"}</span>
          </button>
          <button 
            onClick={() => setIsFavorited(!isFavorited)}
            className={cn(
              "flex flex-col items-center justify-center gap-1 py-1.5 transition-colors cursor-pointer",
              isFavorited ? "text-red-500" : "text-muted-foreground hover:text-foreground"
            )}
          >
            <Heart className={cn("w-5 h-5", isFavorited && "fill-current")} />
            <span className="text-[11px] font-semibold">{isFavorited ? "已收藏" : "已收藏"}</span>
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
            <span className="text-[11px] font-semibold">{isCasting ? "已投屏" : "投屏"}</span>
          </button>
        </div>

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
                  title={`${ep.number} ${ep.title}${isWatched ? " · 已看" : hasProgress ? ` · ${Math.round(ep.progress!)}%` : ""}`}
                >
                  <span className="flex items-center gap-1 font-bold text-xs truncate w-full">
                    {isWatched && <Check className="h-3 w-3 shrink-0" />}
                    {ep.number}
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
                onClick={() => chooseDownloadTarget("all")}
                className="flex w-full items-center justify-between rounded-2xl border border-black/10 p-[16px] text-left transition-colors hover:border-foreground/40 hover:bg-black/[0.03] dark:border-white/10 dark:hover:bg-white/[0.04]"
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
                onClick={() => chooseDownloadTarget("current")}
                className="flex w-full items-center justify-between rounded-2xl border border-black/10 p-[16px] text-left transition-colors hover:border-foreground/40 hover:bg-black/[0.03] dark:border-white/10 dark:hover:bg-white/[0.04]"
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

            <p className="mt-[16px] text-xs leading-relaxed text-muted-foreground">
              栖阅会自动选择可播放源并加入下载队列，无需手动切换线路。
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
                    onClick={() => setSelectedCastDevice(device.id)}
                    aria-pressed={isSelected}
                    className={cn(
                      "flex w-full items-center justify-between rounded-2xl border p-3.5 text-left transition-colors",
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
              disabled={(!selectedCastDevice && !isCasting) || (!castMediaItemId && !isCasting)}
              onClick={async () => {
                if (isCasting && castSessionId) {
                  try { await stopCast(castSessionId) } catch { /* ignore */ }
                  setIsCasting(false)
                  setCastSessionId(null)
                  return
                }
                if (!selectedCastDevice || !castMediaItemId) return
                setCastError(null)
                try {
                  const res = await playCast(castMediaItemId, selectedCastDevice)
                  setCastSessionId(res.castSessionId)
                  setIsCasting(true)
                } catch (e: unknown) {
                  const msg = e instanceof Error ? e.message : "投屏失败，请检查设备是否在线"
                  setCastError(msg)
                }
              }}
              className="mt-5 flex h-11 w-full items-center justify-center rounded-full bg-foreground px-[16px] text-sm font-bold text-background transition-opacity hover:opacity-85 disabled:cursor-not-allowed disabled:opacity-40"
            >
              {isCasting ? "停止投屏" : "开始投屏"}
            </button>
          </div>
        </div>
      )}
    </aside>
  )
}
