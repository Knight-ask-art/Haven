import { describe, expect, it } from "vitest"
import type { DownloadTaskDto } from "@/lib/ipc/generated/wire"
import { summarizeDownloadBatch } from "./DownloadBatchItem"

function task(id: string, progressRatio: number): DownloadTaskDto {
  return {
    schemaVersion: 1,
    taskId: id,
    batchId: "batch",
    workId: "work",
    editionId: "edition",
    mediaItemId: id,
    title: id,
    mediaType: "movie",
    category: "video",
    state: "downloading",
    progressRatio,
    sourceResourceId: "source",
    targetStorageId: "target",
    bytesDownloaded: 0,
    bytesTotal: null,
    speedBps: null,
    etaSeconds: null,
    offlineResourceId: null,
    posterUri: null,
    createdAt: "2026-09-06T00:00:00Z",
    updatedAt: "2026-09-06T00:00:00Z",
  }
}

describe("download batch summary", () => {
  it("averages child progress instead of only counting completed tasks", () => {
    const summary = summarizeDownloadBatch(Array.from({ length: 5 }, (_, index) => task(String(index), 0.5)))
    expect(summary.progress).toBe(50)
    expect(summary.active).toBe(5)
  })
})
