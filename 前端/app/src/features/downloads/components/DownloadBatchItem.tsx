import type { DownloadTaskDto } from "@/lib/ipc/generated/wire"
import { cn } from "@/lib/utils"
import { DownloadTaskList } from "./DownloadTaskList"

interface DownloadBatchItemProps {
  batchId: string
  title: string
  tasks: DownloadTaskDto[]
  pendingTaskIds?: ReadonlySet<string>
  onPause?: (id: string) => void
  onResume?: (id: string) => void
  onCancel?: (id: string) => void
  onRetry?: (id: string) => void
  onOpen?: (task: DownloadTaskDto) => void
  onRemoveRecord?: (id: string) => void
  onDeleteOffline?: (id: string) => void
  onRevealOffline?: (id: string) => void
}

const ACTIVE_STATES = new Set<DownloadTaskDto["state"]>(["queued", "resolving", "downloading", "paused", "verifying"])

export function summarizeDownloadBatch(tasks: DownloadTaskDto[]) {
  const total = tasks.length
  const completed = tasks.filter((task) => task.state === "completed").length
  const failed = tasks.filter((task) => task.state === "failed" || task.state === "interrupted").length
  const active = tasks.filter((task) => ACTIVE_STATES.has(task.state)).length
  const progress = total === 0 ? 0 : Math.round(tasks.reduce((sum, task) => (
    sum + (task.state === "completed" ? 1 : Math.max(0, Math.min(1, task.progressRatio ?? 0)))
  ), 0) / total * 100)
  return { total, completed, failed, active, progress }
}

export function DownloadBatchItem({
  title,
  tasks,
  pendingTaskIds,
  onPause,
  onResume,
  onCancel,
  onRetry,
  onOpen,
  onRemoveRecord,
  onDeleteOffline,
  onRevealOffline,
}: DownloadBatchItemProps) {
  const { total, completed, failed, active, progress } = summarizeDownloadBatch(tasks)
  const isCompleted = completed === total && total > 0
  const isFailed = failed > 0 && completed + failed === total

  return (
    <div className="rounded-xl border bg-card p-4 shadow-sm">
      <div className="flex items-center justify-between">
        <h3 className="font-semibold truncate" title={title}>
          {title}
        </h3>
        <span className="text-xs text-muted-foreground">
          {completed}/{total} {isCompleted ? "已完成" : isFailed ? "部分失败" : `${progress}%`}
        </span>
      </div>
      <div className="mt-2 h-2 w-full rounded-full bg-muted">
        <div
          className={cn("h-2 rounded-full transition-all", isCompleted ? "bg-green-500" : isFailed ? "bg-amber-500" : "bg-primary")}
          style={{ width: `${progress}%` }}
        />
      </div>
      <div className="mt-2 text-xs text-muted-foreground">
        {active > 0 ? `${active} 正在处理` : isCompleted ? "全部完成" : `${failed > 0 ? `${failed} 失败` : ""}`}
      </div>
      <details className="mt-3">
        <summary className="cursor-pointer text-xs text-muted-foreground hover:text-foreground">展开 {total} 个子任务</summary>
        <div className="mt-3">
          <DownloadTaskList
            tasks={tasks}
            pendingTaskIds={pendingTaskIds}
            onPause={onPause}
            onResume={onResume}
            onCancel={onCancel}
            onRetry={onRetry}
            onOpen={onOpen}
            onRemoveRecord={onRemoveRecord}
            onDeleteOffline={onDeleteOffline}
            onRevealOffline={onRevealOffline}
          />
        </div>
      </details>
    </div>
  )
}
