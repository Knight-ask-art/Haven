import { HavenIcon } from "@/components/ui/haven/HavenIcon"
import { ArtworkImage } from "@/components/ui/haven/ArtworkImage"
import type { DownloadTaskDto } from "@/lib/ipc/generated/wire"
import { defaultCoverCategoryForMediaType } from "@/lib/default-cover"
import { cn } from "@/lib/utils"
import { Ellipsis, RefreshCw } from "lucide-react"
import { useState } from "react"

interface DownloadTaskItemProps {
  task: DownloadTaskDto
  pending?: boolean
  onPause?: (id: string) => void
  onResume?: (id: string) => void
  onCancel?: (id: string) => void
  onRetry?: (id: string) => void
  onOpen?: (task: DownloadTaskDto) => void
  onRemoveRecord?: (id: string) => void
  onDeleteOffline?: (id: string) => void
  onRevealOffline?: (id: string) => void
}

const STATE_LABELS: Record<DownloadTaskDto["state"], string> = {
  queued: "等待下载",
  resolving: "正在准备",
  downloading: "正在下载",
  paused: "已暂停",
  verifying: "正在校验",
  completed: "下载完成",
  failed: "下载失败",
  cancelled: "已取消",
  interrupted: "下载中断",
}

const TYPE_LABELS: Record<DownloadTaskDto["mediaType"], string> = {
  movie: "影片",
  series: "剧集",
  episode: "单集",
  book: "图书",
  document: "资料",
  comic: "漫画",
  article: "文章",
  audio: "音频",
  unknown: "文件",
}

export function DownloadTaskItem({
  task,
  pending = false,
  onPause,
  onResume,
  onCancel,
  onRetry,
  onOpen,
  onRemoveRecord,
  onDeleteOffline,
  onRevealOffline,
}: DownloadTaskItemProps) {
  const [menuOpen, setMenuOpen] = useState(false)
  const progress = task.state === "completed"
    ? 100
    : Math.round((task.progressRatio ?? 0) * 100)
  const canPause = ["queued", "resolving", "downloading"].includes(task.state)
  const canResume = task.state === "paused" || task.state === "interrupted"
  const canRetry = task.state === "failed" || task.state === "cancelled"
  const canCancel = task.state !== "completed" && task.state !== "cancelled" && task.state !== "verifying"
  const isError = task.state === "failed" || task.state === "interrupted"
  const canRemoveRecord = task.state === "completed" || task.state === "failed" || task.state === "cancelled"
  const canManageOffline = task.state === "completed" && task.offlineResourceId !== null

  return (
    <article className="group flex min-h-[128px] items-center gap-[16px] rounded-lg border border-border/60 bg-background p-[16px] shadow-sm transition-shadow hover:shadow-md md:gap-[24px] md:p-[20px]">
      <button
        type="button"
        disabled={!task.workId && !task.mediaItemId}
        onClick={() => onOpen?.(task)}
        className="flex h-[96px] w-[68px] shrink-0 items-center justify-center overflow-hidden rounded-md bg-muted disabled:cursor-default"
        title={task.workId || task.mediaItemId ? "打开内容" : undefined}
      >
        <ArtworkImage
          src={task.posterUri}
          alt=""
          fallbackCategory={defaultCoverCategoryForMediaType(task.category)}
          fallbackSeed={task.mediaItemId ?? task.workId ?? task.taskId}
          className="h-full w-full object-cover"
          loading="lazy"
        />
      </button>

      <div className="flex min-w-0 flex-1 flex-col self-stretch py-[2px]">
        <div className="flex items-start justify-between gap-[12px]">
          <div className="min-w-0">
            <button
              type="button"
              disabled={!task.workId && !task.mediaItemId}
              onClick={() => onOpen?.(task)}
              className="line-clamp-2 text-left text-sm font-bold leading-snug text-foreground hover:underline disabled:no-underline md:text-base"
            >
              {task.title}
            </button>
            <p className="mt-[4px] text-xs font-medium text-muted-foreground">
              {TYPE_LABELS[task.mediaType]}
            </p>
          </div>

          <div className="flex h-[36px] shrink-0 items-center gap-[4px]">
            {canPause && (
              <button
                type="button"
                disabled={pending}
                onClick={() => onPause?.(task.taskId)}
                className="flex h-[36px] w-[36px] items-center justify-center rounded-full bg-muted text-foreground transition-colors hover:bg-muted/70 disabled:opacity-40"
                title="暂停"
              >
                <HavenIcon symbol="pause" size={18} />
              </button>
            )}
            {canResume && (
              <button
                type="button"
                disabled={pending}
                onClick={() => onResume?.(task.taskId)}
                className="flex h-[36px] w-[36px] items-center justify-center rounded-full bg-muted text-foreground transition-colors hover:bg-muted/70 disabled:opacity-40"
                title="继续"
              >
                <HavenIcon symbol="play" size={18} />
              </button>
            )}
            {canRetry && (
              <button
                type="button"
                disabled={pending}
                onClick={() => onRetry?.(task.taskId)}
                className="flex h-[36px] w-[36px] items-center justify-center rounded-full bg-muted text-foreground transition-colors hover:bg-muted/70 disabled:opacity-40"
                title="重试"
              >
                <HavenIcon symbol={RefreshCw} size={18} />
              </button>
            )}
            {canCancel && (
              <button
                type="button"
                disabled={pending}
                onClick={() => onCancel?.(task.taskId)}
                className="flex h-[36px] w-[36px] items-center justify-center rounded-full text-destructive transition-colors hover:bg-destructive/10 disabled:opacity-40"
                title="取消"
              >
                <HavenIcon symbol="x" size={18} />
              </button>
            )}
            {(canRemoveRecord || canManageOffline) && (
              <div className="relative">
                <button
                  type="button"
                  disabled={pending}
                  onClick={() => setMenuOpen((open) => !open)}
                  className="flex h-[36px] w-[36px] items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:opacity-40"
                  title="更多操作"
                  aria-label="更多操作"
                  aria-expanded={menuOpen}
                >
                  <Ellipsis className="h-[18px] w-[18px]" aria-hidden="true" />
                </button>
                {menuOpen && (
                  <div className="absolute right-0 top-[42px] z-20 min-w-[160px] rounded-lg border border-border bg-background p-1 shadow-lg">
                    {canManageOffline && (
                      <>
                        <button
                          type="button"
                          onClick={() => { setMenuOpen(false); onRevealOffline?.(task.taskId) }}
                          className="block w-full rounded-md px-3 py-2 text-left text-xs font-semibold hover:bg-muted"
                        >
                          在文件夹中显示
                        </button>
                        <button
                          type="button"
                          onClick={() => { setMenuOpen(false); onDeleteOffline?.(task.taskId) }}
                          className="block w-full rounded-md px-3 py-2 text-left text-xs font-semibold text-destructive hover:bg-destructive/10"
                        >
                          删除离线内容
                        </button>
                      </>
                    )}
                    {canRemoveRecord && (
                      <button
                        type="button"
                        onClick={() => { setMenuOpen(false); onRemoveRecord?.(task.taskId) }}
                        className="block w-full rounded-md px-3 py-2 text-left text-xs font-semibold hover:bg-muted"
                      >
                        从列表移除
                      </button>
                    )}
                  </div>
                )}
              </div>
            )}
          </div>
        </div>

        <div className="mt-auto pt-[12px]">
          <div className="mb-[7px] flex items-center justify-between gap-[12px] text-xs font-semibold">
            <span className={cn(
              isError ? "text-destructive" : task.state === "completed" ? "text-emerald-600 dark:text-emerald-400" : "text-primary",
            )}>
              {pending ? "正在处理..." : task.state === "completed" && !task.offlineResourceId ? "离线内容已删除" : STATE_LABELS[task.state]}
            </span>
            <span className="truncate font-mono text-[11px] text-muted-foreground">
              {formatBytes(task.bytesDownloaded)} / {formatBytes(task.bytesTotal)} · {progress}%
            </span>
          </div>
          {task.state === "downloading" && (task.speedBps !== null || task.etaSeconds !== null) && (
            <div className="mb-[7px] flex items-center gap-[14px] text-[11px] text-muted-foreground">
              {task.speedBps !== null && <span>当前速度 {formatBytes(task.speedBps)}/秒</span>}
              {task.etaSeconds !== null && <span>预计剩余 {formatDuration(task.etaSeconds)}</span>}
            </div>
          )}
          <div className="h-[5px] w-full overflow-hidden rounded-full bg-muted">
            <div
              className={cn(
                "h-full rounded-full transition-[width] duration-300",
                isError ? "bg-destructive" : task.state === "completed" ? "bg-emerald-500" : "bg-primary",
              )}
              style={{ width: `${progress}%` }}
            />
          </div>
        </div>
      </div>
    </article>
  )
}

function formatBytes(value: number | null): string {
  if (value === null) return "未知"
  if (value < 1024) return `${value} B`
  const units = ["KB", "MB", "GB", "TB"]
  let amount = value / 1024
  let unit = units[0]
  for (let index = 1; index < units.length && amount >= 1024; index += 1) {
    amount /= 1024
    unit = units[index]
  }
  return `${amount >= 10 ? amount.toFixed(1) : amount.toFixed(2)} ${unit}`
}

function formatDuration(seconds: number): string {
  if (seconds < 60) return `${seconds} 秒`
  const minutes = Math.floor(seconds / 60)
  const remainingSeconds = seconds % 60
  if (minutes < 60) return remainingSeconds ? `${minutes} 分 ${remainingSeconds} 秒` : `${minutes} 分钟`
  const hours = Math.floor(minutes / 60)
  const remainingMinutes = minutes % 60
  return remainingMinutes ? `${hours} 小时 ${remainingMinutes} 分` : `${hours} 小时`
}
