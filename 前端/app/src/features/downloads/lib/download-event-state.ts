import type { DownloadEvent, DownloadTaskDto } from "@/lib/ipc/generated/wire"

export interface DownloadEventState {
  latestByTask: Map<string, DownloadEvent>
  sequenceByOperation: Map<string, number>
  offlineResourceRemovedTaskIds: Set<string>
}

export function createDownloadEventState(): DownloadEventState {
  return {
    latestByTask: new Map(),
    sequenceByOperation: new Map(),
    offlineResourceRemovedTaskIds: new Set(),
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
  state: DownloadEventState,
): DownloadTaskDto[] {
  return tasks.map((task) => task.taskId === event.data.taskId
    ? event.at >= task.updatedAt
      ? patchDownloadTask(task, event, state.offlineResourceRemovedTaskIds.has(task.taskId))
      : task
    : task)
}

export function mergeDownloadTask(tasks: DownloadTaskDto[], updated: DownloadTaskDto): DownloadTaskDto[] {
  return tasks.map((task) => task.taskId === updated.taskId
    ? updated.updatedAt >= task.updatedAt ? updated : task
    : task)
}

export function mergeLatestDownloadEvents(
  tasks: DownloadTaskDto[],
  state: DownloadEventState,
): DownloadTaskDto[] {
  return tasks.map((task) => {
    const event = state.latestByTask.get(task.taskId)
    return event && event.at >= task.updatedAt
      ? patchDownloadTask(task, event, state.offlineResourceRemovedTaskIds.has(task.taskId))
      : clearRemovedOfflineResource(task, state)
  })
}

/**
 * Applies a list response without discarding a newer local action or channel
 * event that arrived while the request was in flight.
 */
export function mergeDownloadList(
  current: DownloadTaskDto[],
  listed: DownloadTaskDto[],
  state: DownloadEventState,
): DownloadTaskDto[] {
  const currentByTaskId = new Map(current.map((task) => [task.taskId, task]))
  const merged = listed.map((listedTask) => {
    const task = clearRemovedOfflineResource(listedTask, state)
    const currentTask = currentByTaskId.get(task.taskId)
    return currentTask ? mergeDownloadTask([task], currentTask)[0] : task
  })
  return mergeLatestDownloadEvents(merged, state)
}

export function forgetDownloadEventsForTask(state: DownloadEventState, taskId: string): void {
  state.latestByTask.delete(taskId)
  state.offlineResourceRemovedTaskIds.delete(taskId)
}

export function markOfflineResourceDeleted(state: DownloadEventState, taskId: string): void {
  state.latestByTask.delete(taskId)
  state.offlineResourceRemovedTaskIds.add(taskId)
}

function clearRemovedOfflineResource(task: DownloadTaskDto, state: DownloadEventState): DownloadTaskDto {
  return state.offlineResourceRemovedTaskIds.has(task.taskId)
    ? { ...task, offlineResourceId: null }
    : task
}

function patchDownloadTask(
  task: DownloadTaskDto,
  event: DownloadEvent,
  offlineResourceRemoved: boolean,
): DownloadTaskDto {
  const bytesTotal = event.data.bytesTotal
  return {
    ...task,
    state: event.data.state,
    offlineResourceId: offlineResourceRemoved ? null : event.data.offlineResourceId,
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
