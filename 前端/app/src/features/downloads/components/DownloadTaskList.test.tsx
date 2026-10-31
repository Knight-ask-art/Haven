// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import type { DownloadTaskDto } from "@/lib/ipc/generated/wire"
import { DownloadTaskList } from "./DownloadTaskList"

/**
 * 空态必须说清是哪一种"空"（审计 B04）。
 *
 * 这个组件拿到的可能只是**筛选后**的列表：没有筛选上下文时，空数组一律渲染
 * 「当前没有下载任务」会在筛选无匹配时否认整个下载库还有任务。调用方有筛选时
 * 必须传入语境空态，组件照原样渲染它；调用方在别处已有可见行（批次分组）时传 `null`，
 * 组件什么空态都不写。
 */
function task(overrides: Partial<DownloadTaskDto> = {}): DownloadTaskDto {
  return {
    schemaVersion: 1,
    taskId: "task-1",
    workId: "work-1",
    editionId: "edition-1",
    mediaItemId: "media-1",
    sourceResourceId: "source-1",
    targetStorageId: "storage-1",
    offlineResourceId: null,
    title: "下载一",
    mediaType: "book",
    category: "book",
    posterUri: null,
    state: "downloading",
    bytesTotal: 100,
    bytesDownloaded: 10,
    speedBps: null,
    etaSeconds: null,
    progressRatio: 0.1,
    createdAt: "2026-09-07T00:00:00.000Z",
    updatedAt: "2026-09-07T00:00:00.000Z",
    ...overrides,
  }
}

afterEach(cleanup)

describe("DownloadTaskList empty state", () => {
  it("falls back to the whole-library empty state when no context is given", () => {
    render(<DownloadTaskList tasks={[]} />)

    expect(screen.getByText("当前没有下载任务")).toBeTruthy()
    expect(screen.getByText("从作品详情页选择内容后会出现在这里。")).toBeTruthy()
  })

  it("renders the caller's filtered empty state instead of claiming the library is empty", () => {
    render(<DownloadTaskList tasks={[]} emptyState={<p>没有符合当前筛选的下载任务</p>} />)

    expect(screen.getByText("没有符合当前筛选的下载任务")).toBeTruthy()
    expect(screen.queryByText("当前没有下载任务")).toBeNull()
  })

  it("does not render any empty state while tasks are visible", () => {
    render(
      <DownloadTaskList
        tasks={[task()]}
        emptyState={<p>没有符合当前筛选的下载任务</p>}
      />,
    )

    expect(screen.getByText("下载一")).toBeTruthy()
    expect(screen.queryByText("没有符合当前筛选的下载任务")).toBeNull()
    expect(screen.queryByText("当前没有下载任务")).toBeNull()
  })

  it("stays silent when the caller passes null (visible rows live elsewhere, e.g. batch groups)", () => {
    render(<DownloadTaskList tasks={[]} emptyState={null} />)

    expect(screen.queryByText("当前没有下载任务")).toBeNull()
    expect(screen.queryByText("从作品详情页选择内容后会出现在这里。")).toBeNull()
  })
})
