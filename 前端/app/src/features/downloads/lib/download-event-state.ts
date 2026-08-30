import type { DownloadEvent, DownloadTaskDto } from "@/lib/ipc/generated/wire"

export interface DownloadEventState {
  latestByTask: Map<string, DownloadEvent>
  sequenceByOperation: Map<string, number>
}

export function createDownloadEventState(): DownloadEventState {
  return {
    latestByTask: new Map(),
    sequenceByOperation: new Map(),
  }
}

export function acceptDownloadEvent(state: DownloadEventState, event: DownloadEvent): boolean {
  const previous = state.sequenceByOperation.get(event.operationId) ?? 0
  if (event.sequence <= previous) return false
  state.sequenceByOperation.set(event.operationId, event.sequence)
  state.latestByTask.set(event.data.taskId, event)
  return true
}

export function applyDownloadEvent(
  tasks: DownloadTaskDto[],
  event: DownloadEvent,
): DownloadTaskDto[] {
  return tasks.map((task) => task.taskId === event.data.taskId
    ? patchDownloadTask(task, event)
    : task)
}

export function mergeLatestDownloadEvents(
  tasks: DownloadTaskDto[],
  state: DownloadEventState,
): DownloadTaskDto[] {
  return tasks.map((task) => {
    const event = state.latestByTask.get(task.taskId)
    return event && event.at >= task.updatedAt ? patchDownloadTask(task, event) : task
  })
}

export function forgetDownloadEventsForTask(state: DownloadEventState, taskId: string): void {
  state.latestByTask.delete(taskId)
}

function patchDownloadTask(task: DownloadTaskDto, event: DownloadEvent): DownloadTaskDto {
  const bytesTotal = event.data.bytesTotal
  return {
    ...task,
    state: event.data.state,
    offlineResourceId: event.data.offlineResourceId,
    bytesTotal,
    bytesDownloaded: event.data.bytesDownloaded,
    progressRatio: bytesTotal && bytesTotal > 0
      ? Math.min(1, event.data.bytesDownloaded / bytesTotal)
      : null,
    speedBps: event.data.speedBps,
    etaSeconds: event.data.etaSeconds,
    updatedAt: event.at,
  }
}
