import type { DownloadTaskDto } from "@/lib/ipc/generated/wire"
import { cn } from "@/lib/utils"

interface DownloadBatchItemProps {
  batchId: string
  title: string
  tasks: DownloadTaskDto[]
}

export function DownloadBatchItem({ title, tasks }: DownloadBatchItemProps) {
  const total = tasks.length
  const completed = tasks.filter((t) => t.state === "completed").length
  const failed = tasks.filter((t) => t.state === "failed").length
  const downloading = tasks.filter((t) => t.state === "downloading" || t.state === "queued").length
  const progress = total === 0 ? 0 : Math.round((completed / total) * 100)
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
        {downloading > 0 ? `${downloading} 正在下载` : isCompleted ? "全部完成" : `${failed > 0 ? `${failed} 失败` : ""}`}
      </div>
      <details className="mt-3">
        <summary className="cursor-pointer text-xs text-muted-foreground hover:text-foreground">展开 {total} 个子任务</summary>
        <ul className="mt-2 space-y-1">
          {tasks.map((task) => (
            <li key={task.taskId} className="flex items-center justify-between text-xs">
              <span className="truncate" title={task.title}>
                {task.title}
              </span>
              <span className={cn("ml-2 shrink-0 rounded px-1.5 py-0.5", task.state === "completed" ? "bg-green-100 text-green-700" : task.state === "failed" ? "bg-red-100 text-red-700" : "bg-muted")}>
                {task.state}
              </span>
            </li>
          ))}
        </ul>
      </details>
    </div>
  )
}
