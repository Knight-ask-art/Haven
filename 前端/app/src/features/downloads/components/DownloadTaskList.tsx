import { HavenIcon } from "@/components/ui/haven/HavenIcon"
import type { ReactNode } from "react"
import type { DownloadTaskDto } from "@/lib/ipc/generated/wire"
import { DownloadTaskItem } from "./DownloadTaskItem"

export function DownloadTaskList({
  tasks,
  pendingTaskIds,
  emptyState,
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
  /**
   * 列表为空时说什么。
   *
   * 缺省是全库空态。调用方在有筛选时**必须**换成语境空态：页面已有失败任务、用户只是
   * 把状态切到「已完成」时，显示「当前没有下载任务 / 从作品详情页选择内容」是在说整个
   * 下载库都是空的，而页头的计数就摆在那里。
   *
   * 传 `null` 表示"本列表没有可见行时什么也不说"：调用方在别处（例如批次分组）已经画出了
   * 可见行时用它——否则本列表会在那些行下面再断言一次"当前没有下载任务"。
   */
  emptyState?: ReactNode
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
    // 显式 `null` = 调用方另有可见行，不写任何空态。
    if (emptyState === null) return null
    if (emptyState !== undefined) return <>{emptyState}</>
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
