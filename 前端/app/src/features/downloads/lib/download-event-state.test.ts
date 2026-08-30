import { describe, expect, it } from "vitest"

import type { DownloadEvent, DownloadTaskDto } from "@/lib/ipc/generated/wire"
import {
  acceptDownloadEvent,
  applyDownloadEvent,
  createDownloadEventState,
  forgetDownloadEventsForTask,
  mergeLatestDownloadEvents,
} from "./download-event-state"

const BASE_TASK: DownloadTaskDto = {
  schemaVersion: 1,
  taskId: "task-1",
  workId: "work-1",
  editionId: "edition-1",
  mediaItemId: "media-1",
  sourceResourceId: "resource-source",
  targetStorageId: "storage-1",
  offlineResourceId: null,
  title: "测试下载",
  mediaType: "book",
  category: "book",
  posterUri: null,
  state: "queued",
  bytesTotal: 100,
  bytesDownloaded: 0,
  speedBps: null,
  etaSeconds: null,
  progressRatio: 0,
  createdAt: "2026-08-20T00:00:00.000Z",
  updatedAt: "2026-08-20T00:00:00.000Z",
}

function event(sequence: number, overrides: Partial<DownloadEvent["data"]> = {}): DownloadEvent {
  return {
    operationId: "task-1",
    sequence,
    at: `2026-08-20T00:00:0${sequence}.000Z`,
    kind: "updated",
    data: {
      taskId: "task-1",
      state: "downloading",
      offlineResourceId: null,
      bytesTotal: 100,
      bytesDownloaded: 40,
      speedBps: 20,
      etaSeconds: 3,
      errorCode: null,
      ...overrides,
    },
  }
}

describe("download event state", () => {
  it("rejects duplicate and out-of-order sequences for one operation", () => {
    const state = createDownloadEventState()

    expect(acceptDownloadEvent(state, event(2))).toBe(true)
    expect(acceptDownloadEvent(state, event(2))).toBe(false)
    expect(acceptDownloadEvent(state, event(1))).toBe(false)
    expect(acceptDownloadEvent(state, event(3))).toBe(true)
  })

  it("patches progress, speed and ETA from the latest channel event", () => {
    const [task] = applyDownloadEvent([BASE_TASK], event(1))

    expect(task).toMatchObject({
      state: "downloading",
      bytesDownloaded: 40,
      progressRatio: 0.4,
      speedBps: 20,
      etaSeconds: 3,
    })
  })

  it("records the offline resource and clears transfer metrics on completion", () => {
    const completed = event(2, {
      state: "completed",
      offlineResourceId: "resource-offline",
      bytesDownloaded: 100,
      speedBps: null,
      etaSeconds: null,
    })
    const [task] = applyDownloadEvent([BASE_TASK], completed)

    expect(task).toMatchObject({
      state: "completed",
      offlineResourceId: "resource-offline",
      progressRatio: 1,
      speedBps: null,
      etaSeconds: null,
    })
  })

  it("keeps a newer channel snapshot when an older list query finishes", () => {
    const state = createDownloadEventState()
    const latest = event(3, { bytesDownloaded: 75 })
    expect(acceptDownloadEvent(state, latest)).toBe(true)

    const [task] = mergeLatestDownloadEvents([BASE_TASK], state)
    expect(task.bytesDownloaded).toBe(75)
    expect(task.updatedAt).toBe(latest.at)
  })

  it("does not restore a forgotten offline resource from a cached terminal event", () => {
    const state = createDownloadEventState()
    const completed = event(2, {
      state: "completed",
      offlineResourceId: "resource-offline",
      bytesDownloaded: 100,
    })
    expect(acceptDownloadEvent(state, completed)).toBe(true)
    forgetDownloadEventsForTask(state, BASE_TASK.taskId)

    const [task] = mergeLatestDownloadEvents([BASE_TASK], state)
    expect(task.offlineResourceId).toBeNull()
    expect(task.state).toBe("queued")
  })

  it("keeps a stable worker error code on failed events for the notice layer", () => {
    const state = createDownloadEventState()
    const failed = event(4, {
      state: "failed",
      errorCode: "DOWNLOAD_DISK_SPACE_LOW",
    })

    expect(acceptDownloadEvent(state, failed)).toBe(true)
    expect(state.latestByTask.get(BASE_TASK.taskId)?.data.errorCode).toBe("DOWNLOAD_DISK_SPACE_LOW")
  })
})
