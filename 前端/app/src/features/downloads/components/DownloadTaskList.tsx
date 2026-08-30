import { HavenIcon } from "@/components/ui/haven/HavenIcon"
import type { DownloadTaskDto } from "@/lib/ipc/generated/wire"
import { DownloadTaskItem } from "./DownloadTaskItem"

export function DownloadTaskList({
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
}: {
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
}) {
  if (tasks.length === 0) {
    return (
      <div className="flex min-h-[160px] flex-col items-center justify-center text-center text-muted-foreground">
        <HavenIcon symbol="check-circle" size={32} className="mb-[12px] opacity-60" />
        <p className="text-sm font-semibold text-foreground">当前没有下载任务</p>
        <p className="mt-[4px] text-xs">从作品详情页选择内容后会出现在这里。</p>
      </div>
    )
  }

  return (
    <div className="flex flex-col gap-[10px] pb-[24px]">
      {tasks.map((task) => (
        <DownloadTaskItem
          key={task.taskId}
          task={task}
          pending={pendingTaskIds?.has(task.taskId)}
          onPause={onPause}
          onResume={onResume}
          onCancel={onCancel}
          onRetry={onRetry}
          onOpen={onOpen}
          onRemoveRecord={onRemoveRecord}
          onDeleteOffline={onDeleteOffline}
          onRevealOffline={onRevealOffline}
        />
      ))}
    </div>
  )
}
