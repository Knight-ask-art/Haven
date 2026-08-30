import { useState, useRef, useEffect } from "react"
import { useNavigate } from "react-router"
import { cn } from "@/lib/utils"
import { Play, Heart, Download, Ellipsis, Info, RotateCcw, Folder, Share2, Trash2 } from "lucide-react"
import { ShareCardModal } from "@/components/ui/haven/ShareCardModal"

export interface HavenStageProps {
  id: string
  title: string
  originalTitle?: string
  metadata: string
  description: string
  backdropUrl: string
  primaryActionLabel: string
  isDownloaded?: boolean
  onPrimaryAction?: () => void
  onAction?: (action: string) => void
}

export function HavenStage({
  id = "2",
  title,
  originalTitle,
  metadata,
  description,
  backdropUrl,
  primaryActionLabel,
  isDownloaded = true,
  onPrimaryAction,
  onAction
}: HavenStageProps) {
  const navigate = useNavigate()
  const isEmpty = !title && !backdropUrl
  const [isMoreMenuOpen, setIsMoreMenuOpen] = useState(false)
  const [isShareModalOpen, setIsShareModalOpen] = useState(false)
  const [isFavorite, setIsFavorite] = useState(false)
  const [downloadState, setDownloadState] = useState(isDownloaded)
  const [toastMessage, setToastMessage] = useState<string | null>(null)
  const menuRef = useRef<HTMLDivElement>(null)

  // 点击外部关闭菜单
  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(event.target as Node)) {
        setIsMoreMenuOpen(false)
      }
    }
    if (isMoreMenuOpen) {
      document.addEventListener("mousedown", handleClickOutside)
    }
    return () => {
      document.removeEventListener("mousedown", handleClickOutside)
    }
  }, [isMoreMenuOpen])

  return (
    <section className="relative w-full h-[70vh] min-h-[580px] max-h-[850px] flex items-end overflow-hidden">
      {/* 极简苹果风 Toast 提示弹窗 */}
      {toastMessage && (
        <div className="fixed top-[32px] left-1/2 -translate-x-1/2 z-[100] flex items-center gap-3 px-6 py-3.5 rounded-full bg-zinc-950/90 dark:bg-white/95 text-white dark:text-zinc-950 text-base font-bold shadow-2xl backdrop-blur-2xl border border-white/10 dark:border-black/10 animate-in fade-in slide-in-from-top-[16px] duration-300">
          <Download className="w-6 h-6 text-emerald-400 dark:text-emerald-600 shrink-0" />
          <span>{toastMessage}</span>
        </div>
      )}

      {/* 沉浸式背景图 */}
      <div className="absolute inset-0 z-0 select-none">
        <img 
          src={backdropUrl || "https://images.unsplash.com/photo-1518709268805-4e9042af9f23?q=80&w=2560&auto=format&fit=crop"} 
          alt={title || "Haven"} 
          className="w-full h-full object-cover object-top filter brightness-[0.95]"
          draggable={false}
        />
        {/* 深色渐变遮罩，使底部平滑融入页面底色 */}
        <div className="absolute inset-0 bg-gradient-to-t from-background via-background/50 to-transparent" />
        {/* 左侧的额外遮罩，保证文字在复杂背景下的可读性 */}
        <div className="absolute inset-0 bg-gradient-to-r from-background via-background/75 to-transparent/10" />
      </div>

      {/* 舞台内容区域：增加左侧边距 (pl-[64px] md:pl-[96px] lg:pl-[128px]) 创造更宽阔的呼吸感 */}
      <div className="relative z-10 w-full pl-[48px] md:pl-[80px] lg:pl-[120px] pr-[32px] md:pr-[64px] pb-[72px] md:pb-[96px] flex flex-col gap-[16px] md:gap-[20px] max-w-4xl">
        {isEmpty ? (
          /* 没有播放记录时的 Hero 空状态展示 */
          <div className="flex flex-col gap-[16px]">
            <span className="text-muted-foreground text-xs md:text-sm font-bold tracking-[0.2em] uppercase">
              足迹 · FOOTPRINTS
            </span>
            <h1 className="text-4xl md:text-6xl lg:text-[72px] leading-tight font-black tracking-tighter text-foreground">
              记录您的每一次探索
            </h1>
            <p className="text-sm md:text-base text-foreground/70 max-w-xl leading-relaxed font-medium">
              当您开始阅读图书、看漫画或播放视频时，未完成的进度与历史记录将实时沉淀在此。
            </p>
            <div className="flex items-center gap-[16px] mt-[16px]">
              <a
                href="/library"
                className={cn(
                  "flex items-center gap-[8px] px-[28px] h-[48px] md:h-[54px] rounded-full bg-foreground text-background font-bold text-sm md:text-base shadow-xl transition-transform hover:scale-105"
                )}
              >
                前往媒体库
              </a>
            </div>
          </div>
        ) : (
          /* 有播放记录时的标准展示 */
          <>
            <div className="flex flex-col gap-1.5">
              {originalTitle && (
                <span className="text-muted-foreground text-[11px] md:text-[13px] font-bold tracking-[0.2em] uppercase">
                  {originalTitle}
                </span>
              )}
              <h1 className="text-5xl md:text-7xl lg:text-[96px] leading-none font-black tracking-tighter text-foreground drop-shadow-sm">
                {title}
              </h1>
            </div>

            <div className="flex items-center gap-3 text-xs md:text-sm text-muted-foreground font-semibold mt-1">
              {metadata}
            </div>

            <p className="text-sm md:text-base text-foreground/80 line-clamp-2 md:line-clamp-3 leading-relaxed max-w-2xl text-pretty mt-1 font-medium">
              {description}
            </p>

            {/* 操作区：增大上下边距拉开呼吸感 */}
            <div className="flex items-center gap-[16px] mt-[32px]">
              <button 
          onClick={onPrimaryAction || (() => navigate(`/work/${id || "2"}`))}
                className={cn(
                  "flex items-center gap-[8px] px-[32px] h-[64px] rounded-full bg-foreground text-background font-bold text-sm md:text-base cursor-pointer",
                  "transition-all duration-300 hover:scale-105 active:scale-95 shadow-xl shadow-black/10 hover:opacity-90",
                  "outline-none focus-visible:ring-4 focus-visible:ring-foreground/50 focus-visible:ring-offset-0"
                )}
              >
                <Play className="w-5 h-5 fill-current" />
                {primaryActionLabel}
              </button>

              {/* 次级操作组 */}
              <div className="flex items-center gap-[16px] relative" ref={menuRef}>
                <button
                  onClick={() => {
                    setIsFavorite(!isFavorite)
                    onAction?.('heart')
                  }}
                  title={isFavorite ? "已收藏" : "加入收藏"}
                  className={cn(
                    "flex items-center justify-center w-[64px] h-[64px] rounded-full cursor-pointer",
                    "bg-black/5 dark:bg-white/10 backdrop-blur-md border border-black/5 dark:border-white/10",
                    "transition-all duration-300 hover:scale-105 active:scale-95",
                    isFavorite 
                      ? "text-red-500 bg-red-500/10 dark:bg-red-500/20 border-red-500/30" 
                      : "text-foreground hover:bg-black/10 dark:hover:bg-white/20",
                    "outline-none focus-visible:ring-4 focus-visible:ring-foreground/50 focus-visible:ring-offset-0"
                  )}
                >
                  <Heart className={cn("w-[24px] h-[24px] transition-transform duration-200", isFavorite && "fill-current scale-110")} />
                </button>

                <button
                  onClick={() => {
                    const next = !downloadState
                    setDownloadState(next)
                    onAction?.('download')
                    setToastMessage(next ? "已进入下载队列" : "已从下载队列中移除")
                    setTimeout(() => setToastMessage(null), 2500)
                  }}
                  title={downloadState ? "已下载" : "下载至本地"}
                  className={cn(
                    "flex items-center justify-center w-[64px] h-[64px] rounded-full cursor-pointer",
                    "bg-black/5 dark:bg-white/10 backdrop-blur-md border border-black/5 dark:border-white/10",
                    "transition-all duration-300 hover:scale-105 active:scale-95",
                    downloadState 
                      ? "text-emerald-500 bg-emerald-500/10 dark:bg-emerald-500/20 border-emerald-500/30" 
                      : "text-foreground hover:bg-black/10 dark:hover:bg-white/20",
                    "outline-none focus-visible:ring-4 focus-visible:ring-foreground/50 focus-visible:ring-offset-0"
                  )}
                >
                  <Download className={cn("w-[24px] h-[24px] transition-transform duration-200", downloadState && "scale-110")} />
                </button>

                {/* 更多按钮及其弹出菜单 */}
                <div className="relative">
                  <button
                    onClick={() => setIsMoreMenuOpen(!isMoreMenuOpen)}
                    title="更多选项"
                    className={cn(
                      "flex items-center justify-center w-[64px] h-[64px] rounded-full",
                      "bg-black/5 dark:bg-white/10 backdrop-blur-md border border-black/5 dark:border-white/10",
                      "text-foreground hover:bg-black/10 dark:hover:bg-white/20",
                      "transition-all duration-300 hover:scale-105 active:scale-95",
                      isMoreMenuOpen && "bg-black/20 dark:bg-white/30 scale-105",
                      "outline-none focus-visible:ring-4 focus-visible:ring-foreground/50 focus-visible:ring-offset-0"
                    )}
                  >
                    <Ellipsis className="w-[24px] h-[24px]" />
                  </button>

                  {/* Apple 风格毛玻璃弹出菜单 */}
                  {isMoreMenuOpen && (
                    <div className={cn(
                      "absolute bottom-full mb-3 left-0 md:left-auto md:right-0 w-[248px] p-[10px] rounded-2xl z-50 shadow-2xl",
                      "bg-white/80 dark:bg-zinc-900/85 backdrop-blur-2xl border border-black/10 dark:border-white/15",
                      "flex flex-col gap-[4px] animate-in fade-in zoom-in-95 duration-150"
                    )}>
                      <button
                        onClick={() => { 
                          setIsMoreMenuOpen(false); 
                          onAction?.('info');
                          navigate(`/work/${id}`);
                        }}
                        className="flex min-h-[48px] items-center justify-center gap-[16px] rounded-xl px-[16px] py-[12px] text-sm md:text-base font-semibold text-foreground hover:bg-black/5 dark:hover:bg-white/10 transition-colors text-center"
                      >
                        <Info className="w-[20px] h-[20px] shrink-0 text-muted-foreground" />
                        查看详细信息
                      </button>

                      <button
                        onClick={() => { setIsMoreMenuOpen(false); onAction?.('reset') }}
                        className="flex min-h-[48px] items-center justify-center gap-[16px] rounded-xl px-[16px] py-[12px] text-sm md:text-base font-semibold text-foreground hover:bg-black/5 dark:hover:bg-white/10 transition-colors text-center"
                      >
                        <RotateCcw className="w-[20px] h-[20px] shrink-0 text-muted-foreground" />
                        重置播放进度
                      </button>

                      {isDownloaded && (
                        <button
                          onClick={() => { setIsMoreMenuOpen(false); onAction?.('folder') }}
                          className="flex min-h-[48px] items-center justify-center gap-[16px] rounded-xl px-[16px] py-[12px] text-sm md:text-base font-semibold text-foreground hover:bg-black/5 dark:hover:bg-white/10 transition-colors text-center"
                        >
                          <Folder className="w-[20px] h-[20px] shrink-0 text-muted-foreground" />
                          在文件夹中定位
                        </button>
                      )}

                      <button
                        onClick={() => { setIsMoreMenuOpen(false); setIsShareModalOpen(true); onAction?.('share') }}
                        className="flex min-h-[48px] items-center justify-center gap-[16px] rounded-xl px-[16px] py-[12px] text-sm md:text-base font-semibold text-foreground hover:bg-black/5 dark:hover:bg-white/10 transition-colors text-center"
                      >
                        <Share2 className="w-[20px] h-[20px] shrink-0 text-muted-foreground" />
                        生成分享卡片
                      </button>

                      <div className="h-[1px] bg-black/5 dark:bg-white/10 my-1" />

                      <button
                        onClick={() => { setIsMoreMenuOpen(false); onAction?.('delete') }}
                        className="flex min-h-[48px] items-center justify-center gap-[16px] rounded-xl px-[16px] py-[12px] text-sm md:text-base font-semibold text-destructive hover:bg-destructive/10 transition-colors text-center"
                      >
                        <Trash2 className="w-[20px] h-[20px] shrink-0 text-destructive" />
                        清理本地缓存
                      </button>
                    </div>
                  )}
                </div>
              </div>
            </div>
          </>
        )}
      </div>

      {/* Apple 风格分享卡片弹窗 */}
      <ShareCardModal
        isOpen={isShareModalOpen}
        onClose={() => setIsShareModalOpen(false)}
        data={{
          title,
          originalTitle,
          metadata,
          description,
          backdropUrl
        }}
      />
    </section>
  )
}
