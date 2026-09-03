import React, { useState } from "react"
import { 
  Play, Pause, ArrowLeft, Volume2, VolumeX,
  Maximize, Minimize, Layers, SkipForward, SkipBack,
  Bookmark, Captions, Check
} from "lucide-react"
import { cn } from "@/lib/utils"

interface VideoControlsProps {
  title: string
  subtitle?: string
  isPlaying: boolean
  currentTime: number
  duration: number
  volume: number
  isMuted: boolean
  playbackRate: number
  quality?: string
  isFullscreen: boolean
  showControls: boolean
  isBookmarked: boolean
  isBookmarkPending: boolean
  isBookmarkDisabled: boolean
  isBuffering?: boolean
  bufferedRanges?: Array<[number, number]>
  subtitleOptions?: VideoSubtitleOption[]
  selectedSubtitleId?: string | null
  subtitleLoading?: boolean
  subtitleError?: string | null
  onPlayPause: () => void
  onSeek: (time: number) => void
  onVolumeChange: (vol: number) => void
  onToggleMute: () => void
  onRateChange: (rate: number) => void
  onQualityChange?: (quality: string) => void
  onToggleFullscreen: () => void
  onBack: () => void
  onOpenEpisodes?: () => void
  onToggleBookmark: () => void
  onNextEpisode?: () => void
  onPrevEpisode?: () => void
  onSubtitleChange?: (subtitleId: string | null) => void
}

export interface VideoSubtitleOption {
  id: string
  label: string
  language: string | null
}

function formatTime(seconds: number): string {
  if (isNaN(seconds) || seconds < 0) return "00:00"
  const h = Math.floor(seconds / 3600)
  const m = Math.floor((seconds % 3600) / 60)
  const s = Math.floor(seconds % 60)
  const pad = (n: number) => (n < 10 ? `0${n}` : `${n}`)
  if (h > 0) {
    return `${pad(h)}:${pad(m)}:${pad(s)}`
  }
  return `${pad(m)}:${pad(s)}`
}

export function VideoControls({
  title,
  subtitle,
  isPlaying,
  currentTime,
  duration,
  volume,
  isMuted,
  playbackRate,
  quality,
  isFullscreen,
  showControls,
  isBookmarked,
  isBookmarkPending,
  isBookmarkDisabled,
  isBuffering = false,
  bufferedRanges = [],
  subtitleOptions = [],
  selectedSubtitleId = null,
  subtitleLoading = false,
  subtitleError = null,
  onPlayPause,
  onSeek,
  onVolumeChange,
  onToggleMute,
  onRateChange,
  onQualityChange,
  onToggleFullscreen,
  onBack,
  onOpenEpisodes,
  onToggleBookmark,
  onNextEpisode,
  onPrevEpisode,
  onSubtitleChange,
}: VideoControlsProps) {
  const [showSpeedMenu, setShowSpeedMenu] = useState(false)
  const [showQualityMenu, setShowQualityMenu] = useState(false)
  const [showSubtitleMenu, setShowSubtitleMenu] = useState(false)
  const [isHoveringProgress, setIsHoveringProgress] = useState(false)
  const [hoverTime, setHoverTime] = useState<number | null>(null)
  const [hoverPosition, setHoverPosition] = useState<number>(0)

  const progressPercent = duration > 0 ? (currentTime / duration) * 100 : 0
  // 已缓冲区间（灰色底）— 取最接近当前时间的已缓冲段
  const bufferedPercent = (() => {
    if (!duration || bufferedRanges.length === 0) return 0
    // 找到包含 currentTime 的区间或最近的已缓冲末端
    let maxEnd = 0
    for (const [start, end] of bufferedRanges) {
      if (start <= currentTime && currentTime <= end) return (end / duration) * 100
      if (end > maxEnd) maxEnd = end
    }
    return duration ? (maxEnd / duration) * 100 : 0
  })()

  const handleMouseMoveProgress = (e: React.MouseEvent<HTMLDivElement>) => {
    const rect = e.currentTarget.getBoundingClientRect()
    const pos = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width))
    setHoverPosition(e.clientX - rect.left)
    setHoverTime(pos * duration)
  }

  const handleProgressClick = (e: React.MouseEvent<HTMLDivElement>) => {
    const rect = e.currentTarget.getBoundingClientRect()
    const pos = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width))
    onSeek(pos * duration)
  }

  const rates = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0]
  const qualities = ["4K REMUX", "1080P 高清", "720P", "自动"]

  return (
    <div 
      className={cn(
        "absolute inset-0 z-30 flex flex-col justify-between p-6 select-none transition-opacity duration-300 pointer-events-none",
        showControls ? "opacity-100 pointer-events-auto" : "opacity-0"
      )}
    >
      {/* 深色顶部/底部风貌遮罩 */}
      <div className="absolute inset-x-0 top-0 h-[128px] bg-gradient-to-b from-black/90 via-black/50 to-transparent pointer-events-none -z-10" />
      <div className="absolute inset-x-0 bottom-0 h-44 bg-gradient-to-t from-black/95 via-black/60 to-transparent pointer-events-none -z-10" />

      {/* 
        ====================================================
        1. 顶栏 (Header HUD)
        ====================================================
      */}
      <div className="flex items-center justify-between w-full pt-[8px]">
        <div className="flex items-center gap-[16px]">
            <button
              onClick={onBack}
              aria-label="退出播放器"
              className="w-11 h-11 rounded-full bg-white/10 hover:bg-white/20 text-white backdrop-blur-md flex items-center justify-center transition-all hover:scale-105 active:scale-95 cursor-pointer border border-white/10"
              title="退出播放器"
            >
            <ArrowLeft className="w-5 h-5" />
          </button>

          <div className="flex flex-col">
            <h1 className="text-xl md:text-2xl font-bold text-white tracking-wide drop-shadow-md">
              {title}
            </h1>
            {subtitle && (
              <span className="text-xs md:text-sm font-semibold text-white/70">
                {subtitle}
              </span>
            )}
          </div>
        </div>

        {/* 顶栏右侧快捷按钮组 (已按需求移除画质标签) */}
        <div className="flex items-center gap-3" />
      </div>

      {/* 
        ====================================================
        2. 中央点击与无限制区域 (Center Action Feedback)
        ====================================================
      */}
      <div 
        onClick={onPlayPause}
        onDoubleClick={(e) => {
          e.preventDefault()
          onToggleFullscreen()
        }}
        className="flex-1 flex items-center justify-center cursor-pointer"
      >
        {!isPlaying && !isBuffering && (
          <div className="text-white drop-shadow-[0_10px_30px_rgba(0,0,0,0.9)] animate-in zoom-in-90 duration-200 hover:scale-110 transition-transform">
            <Play size={84} className="fill-current ml-[8px]" />
          </div>
        )}
        {isBuffering && isPlaying && (
          <div className="flex flex-col items-center gap-2 text-white">
            <div className="h-10 w-10 animate-spin rounded-full border-2 border-white/30 border-t-white" />
            <span className="text-xs font-semibold tracking-wide text-white/80">缓冲中…</span>
          </div>
        )}
      </div>

      {/* 
        ====================================================
        3. 底栏控制条 (Footer Controls HUD - Apple Style)
        ====================================================
      */}
      <div className="flex flex-col gap-3 w-full pb-[8px]">
        
        {/* 交互进度条 (Interactive Progress Bar) */}
        <div className="relative group/progress py-[8px] cursor-pointer">
          {/* 时间悬浮提示 */}
          {isHoveringProgress && hoverTime !== null && (
            <div 
              className="absolute bottom-[32px] -translate-x-1/2 px-[10px] py-1 rounded-lg bg-zinc-900/90 text-white text-xs font-bold shadow-xl border border-white/15 backdrop-blur-xl whitespace-nowrap pointer-events-none"
              style={{ left: `${hoverPosition}px` }}
            >
              {formatTime(hoverTime)}
            </div>
          )}

          {/* 轨槽 */}
          <div 
            onMouseEnter={() => setIsHoveringProgress(true)}
            onMouseLeave={() => setIsHoveringProgress(false)}
            onMouseMove={handleMouseMoveProgress}
            onClick={handleProgressClick}
            className="relative w-full h-1.5 group-hover/progress:h-3 rounded-full bg-white/20 overflow-hidden transition-all duration-200"
            role="slider"
            aria-label="播放进度"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round(progressPercent)}
            tabIndex={0}
            onKeyDown={(e) => {
              if (e.key === "ArrowLeft") { e.preventDefault(); onSeek(Math.max(0, currentTime - 5)) }
              if (e.key === "ArrowRight") { e.preventDefault(); onSeek(Math.min(duration, currentTime + 5)) }
              if (e.key === "Home") { e.preventDefault(); onSeek(0) }
              if (e.key === "End") { e.preventDefault(); onSeek(duration) }
            }}
          >
            {/* 已缓冲区间（灰色底） */}
            <div className="absolute inset-y-0 left-0 rounded-full bg-white/30" style={{ width: `${bufferedPercent}%` }} />
            {/* 已播放进度填充 */}
            <div 
              className="absolute inset-y-0 left-0 rounded-full bg-red-600 transition-all duration-100"
              style={{ width: `${progressPercent}%` }}
            >
              <div className="absolute right-0 top-1/2 -translate-y-1/2 w-3.5 h-3.5 rounded-full bg-white shadow-md scale-0 group-hover/progress:scale-100 transition-transform" />
            </div>
          </div>
        </div>

        {/* 控制按键列 */}
        <div className="flex items-center justify-between w-full pt-1">
          {/* 左侧播放与时间 */}
          <div className="flex items-center gap-[16px]">
            {/* 播放/暂停 */}
            <button
              onClick={onPlayPause}
              aria-label={isPlaying ? "暂停" : "播放"}
              className="w-10 h-10 rounded-full flex items-center justify-center text-white/90 hover:text-white hover:bg-white/15 backdrop-blur-md transition-all hover:scale-105 active:scale-95 cursor-pointer"
              title={isPlaying ? "暂停" : "播放"}
            >
              {isPlaying ? <Pause className="w-6 h-6 fill-current" /> : <Play className="w-6 h-6 fill-current ml-0.5" />}
            </button>

            {/* 上一集 / 下一集 (如果有) */}
            {onPrevEpisode && (
              <button
                onClick={onPrevEpisode}
                className="w-9 h-9 rounded-full flex items-center justify-center text-white/80 hover:text-white hover:bg-white/15 transition-all hover:scale-105 active:scale-95 cursor-pointer"
                title="上一集"
              >
                <SkipBack className="w-5 h-5 fill-current" />
              </button>
            )}
            {onNextEpisode && (
              <button
                onClick={onNextEpisode}
                className="w-9 h-9 rounded-full flex items-center justify-center text-white/80 hover:text-white hover:bg-white/15 transition-all hover:scale-105 active:scale-95 cursor-pointer"
                title="下一集"
              >
                <SkipForward className="w-5 h-5 fill-current" />
              </button>
            )}

            {/* 音量控制：使用原生 range，支持拖拽与键盘操作 */}
            <div className="flex items-center gap-3 group/vol pl-1">
              <button
                onClick={onToggleMute}
                aria-label={isMuted ? "取消静音" : "静音"}
                className="w-10 h-10 rounded-full flex items-center justify-center text-white/80 hover:text-white transition-colors cursor-pointer"
                title={isMuted ? "取消静音" : "静音"}
              >
                {isMuted || volume === 0 ? (
                  <VolumeX className="w-5 h-5 text-red-500" />
                ) : (
                  <Volume2 className="w-5 h-5" />
                )}
              </button>
              <input
                type="range"
                min="0"
                max="1"
                step="0.01"
                value={isMuted ? 0 : volume}
                onChange={(event) => onVolumeChange(Number(event.target.value))}
                aria-label="音量"
                className="h-1.5 w-[96px] shrink-0 cursor-pointer appearance-none rounded-full bg-white/20 accent-white sm:w-[128px]"
                style={{
                  background: `linear-gradient(to right, rgba(255,255,255,0.92) ${(isMuted ? 0 : volume) * 100}%, rgba(255,255,255,0.22) ${(isMuted ? 0 : volume) * 100}%)`,
                }}
              />
            </div>

            {/* 当前与总时间 */}
            <div className="text-xs font-medium tracking-wide text-white/90 pl-[8px]">
              <span>{formatTime(currentTime)}</span>
              <span className="mx-1.5 text-white/40">/</span>
              <span className="text-white/60">{formatTime(duration)}</span>
            </div>
          </div>

          {/* 右侧：无底色控制按钮组 (书签、选集、字幕、倍速、画质、全屏) */}
          <div className="flex items-center gap-1.5 relative shrink-0">
            <button
              type="button"
              onClick={onToggleBookmark}
              disabled={isBookmarkDisabled}
              aria-label={isBookmarked ? "删除当前时间书签" : "在当前时间添加书签"}
              aria-pressed={isBookmarked}
              className={cn(
                "w-10 h-10 rounded-full flex items-center justify-center transition-all border border-transparent",
                isBookmarked ? "text-red-500 bg-white/10" : "text-white/90 bg-transparent hover:bg-white/10",
                isBookmarkDisabled ? "cursor-not-allowed opacity-50" : "cursor-pointer hover:scale-105 active:scale-95",
              )}
              title={isBookmarkPending ? "正在保存书签" : (isBookmarkDisabled ? "书签功能暂不可用" : (isBookmarked ? "删除当前时间书签" : "在当前时间添加书签"))}
            >
              <Bookmark className={cn("w-5 h-5", isBookmarked && "fill-current")} />
            </button>
            
            {/* 选集按钮 */}
            {onOpenEpisodes && (
              <button
                onClick={onOpenEpisodes}
                aria-label="剧集列表"
                className={cn(
                  "h-10 px-[10px] rounded-full flex items-center gap-1.5 text-sm font-semibold tracking-wide transition-all cursor-pointer select-none",
                  "bg-transparent hover:bg-white/10 text-white border border-transparent",
                  "hover:scale-105 active:scale-95"
                )}
                title="剧集列表"
              >
                <Layers className="w-5 h-5" />
                <span>选集</span>
                </button>
              )}

            {/* 字幕选择器：外挂字幕和 HLS 字幕共用一个用户入口。 */}
            {onSubtitleChange && (subtitleOptions.length > 0 || subtitleLoading || subtitleError) && (
              <div className="relative">
                <button
                  type="button"
                  onClick={() => setShowSubtitleMenu(!showSubtitleMenu)}
                  aria-label="字幕"
                  aria-haspopup="menu"
                  aria-expanded={showSubtitleMenu}
                  className={cn(
                    "h-10 px-[10px] rounded-full flex items-center justify-center gap-1.5 text-sm font-semibold tracking-wide transition-all cursor-pointer select-none",
                    "bg-transparent hover:bg-white/10 text-white border border-transparent",
                    "hover:scale-105 active:scale-95",
                  )}
                  title={subtitleError ?? (subtitleLoading ? "正在加载字幕" : "字幕")}
                >
                  <Captions className="w-5 h-5" />
                  <span className="hidden sm:inline">字幕</span>
                </button>

                {showSubtitleMenu && (
                  <div
                    role="menu"
                    aria-label="字幕轨道"
                    className="absolute bottom-full right-0 mb-3 w-[190px] p-1.5 rounded-2xl bg-zinc-900/90 backdrop-blur-2xl border border-white/15 text-white flex flex-col gap-0.5 shadow-2xl z-50 animate-in fade-in zoom-in-95 duration-150"
                  >
                    <button
                      type="button"
                      role="menuitemradio"
                      aria-checked={selectedSubtitleId === null}
                      onClick={() => {
                        onSubtitleChange(null)
                        setShowSubtitleMenu(false)
                      }}
                      className={cn(
                        "flex items-center justify-between px-3 py-[8px] rounded-xl text-xs font-semibold transition-all cursor-pointer text-left",
                        selectedSubtitleId === null
                          ? "bg-white/20 text-white font-bold"
                          : "text-white/80 hover:bg-white/10 hover:text-white",
                      )}
                    >
                      <span>关闭字幕</span>
                      {selectedSubtitleId === null && <Check className="w-3.5 h-3.5 text-white" />}
                    </button>

                    {subtitleLoading && (
                      <div className="px-3 py-2 text-xs text-white/60">正在加载字幕…</div>
                    )}
                    {subtitleError && (
                      <div role="alert" className="px-3 py-2 text-xs leading-5 text-red-300">
                        {subtitleError}
                      </div>
                    )}

                    {subtitleOptions.map((option) => (
                      <button
                        type="button"
                        role="menuitemradio"
                        aria-checked={selectedSubtitleId === option.id}
                        key={option.id}
                        onClick={() => {
                          onSubtitleChange(option.id)
                          setShowSubtitleMenu(false)
                        }}
                        className={cn(
                          "flex items-center justify-between px-3 py-[8px] rounded-xl text-xs font-semibold transition-all cursor-pointer text-left",
                          selectedSubtitleId === option.id
                            ? "bg-white/20 text-white font-bold"
                            : "text-white/80 hover:bg-white/10 hover:text-white",
                        )}
                      >
                        <span className="truncate pr-2">{option.label}</span>
                        {selectedSubtitleId === option.id && <Check className="w-3.5 h-3.5 shrink-0 text-white" />}
                      </button>
                    ))}
                  </div>
                )}
              </div>
            )}

            {/* 倍速选择器 */}
            <div className="relative">
              <button
                onClick={() => setShowSpeedMenu(!showSpeedMenu)}
                aria-label="播放倍速"
                className={cn(
                  "h-10 px-[10px] rounded-full flex items-center justify-center text-sm font-semibold tracking-wide transition-all cursor-pointer select-none min-w-[64px]",
                  "bg-transparent hover:bg-white/10 text-white border border-transparent",
                  "hover:scale-105 active:scale-95"
                )}
                title="播放倍速"
              >
                {playbackRate === 1 ? "倍速" : `${playbackRate}x`}
              </button>

              {showSpeedMenu && (
                <div className="absolute bottom-full right-0 mb-3 w-[128px] p-1.5 rounded-2xl bg-zinc-900/90 backdrop-blur-2xl border border-white/15 text-white flex flex-col gap-0.5 shadow-2xl z-50 animate-in fade-in zoom-in-95 duration-150">
                  {rates.map((rate) => (
                    <button
                      key={rate}
                      onClick={() => {
                        onRateChange(rate)
                        setShowSpeedMenu(false)
                      }}
                      className={cn(
                        "flex items-center justify-between px-3 py-[8px] rounded-xl text-xs font-semibold transition-all cursor-pointer text-left",
                        playbackRate === rate 
                          ? "bg-white/20 text-white font-bold" 
                          : "text-white/80 hover:bg-white/10 hover:text-white"
                      )}
                    >
                      <span>{rate}x</span>
                      {playbackRate === rate && <Check className="w-3.5 h-3.5 text-white" />}
                    </button>
                  ))}
                </div>
              )}
            </div>

             {/* 画质切换菜单 */}
            {quality !== undefined && onQualityChange && (
              <div className="relative">
                <button
                  onClick={() => setShowQualityMenu(!showQualityMenu)}
                  aria-label="切换画质"
                  className={cn(
                    "h-10 px-[10px] rounded-full flex items-center justify-center text-sm font-semibold tracking-wide transition-all cursor-pointer select-none",
                    "bg-transparent hover:bg-white/10 text-white border border-transparent",
                    "hover:scale-105 active:scale-95"
                  )}
                  title="切换画质"
                >
                  {quality}
                </button>

                {showQualityMenu && (
                  <div className="absolute bottom-full right-0 mb-3 w-36 p-1.5 rounded-2xl bg-zinc-900/90 backdrop-blur-2xl border border-white/15 text-white flex flex-col gap-0.5 shadow-2xl z-50 animate-in fade-in zoom-in-95 duration-150">
                    {qualities.map((q) => (
                      <button
                        key={q}
                        onClick={() => {
                          onQualityChange(q)
                          setShowQualityMenu(false)
                        }}
                        className={cn(
                          "flex items-center justify-between px-3 py-[8px] rounded-xl text-xs font-semibold transition-all cursor-pointer text-left",
                          quality === q
                            ? "bg-white/20 text-white font-bold"
                            : "text-white/80 hover:bg-white/10 hover:text-white"
                        )}
                      >
                        <span>{q}</span>
                        {quality === q && <Check className="w-3.5 h-3.5 text-white" />}
                      </button>
                    ))}
                  </div>
                )}
              </div>
            )}

            {/* 全屏按钮 */}
            <button
              onClick={onToggleFullscreen}
              aria-label={isFullscreen ? "退出全屏" : "全屏"}
              className={cn(
                "w-10 h-10 rounded-full flex items-center justify-center text-white/90 hover:text-white transition-all cursor-pointer",
                "bg-transparent hover:bg-white/10 border border-transparent hover:scale-105 active:scale-95"
              )}
              title={isFullscreen ? "退出全屏" : "全屏"}
            >
              {isFullscreen ? <Minimize className="w-5 h-5" /> : <Maximize className="w-5 h-5" />}
            </button>

          </div>
        </div>

      </div>
    </div>
  )
}
