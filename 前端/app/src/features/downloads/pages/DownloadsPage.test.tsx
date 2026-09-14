// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { MemoryRouter } from "react-router"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { NoticeContext } from "@/app/notice-center/notice-context"
import type { DownloadEvent, DownloadTaskDto } from "@/lib/ipc/generated/wire"
import { DownloadsPage } from "./DownloadsPage"

const {
  listDownloads,
  subscribeDownloadEvents,
  removeDownloadRecord,
  deleteOfflineDownload,
} = vi.hoisted(() => ({
  listDownloads: vi.fn<() => Promise<DownloadTaskDto[]>>(),
  subscribeDownloadEvents: vi.fn<(callback: (event: DownloadEvent) => void) => Promise<() => Promise<void>>>(),
  removeDownloadRecord: vi.fn<(taskId: string) => Promise<{ recordRemoved: boolean; offlineResourceRemoved: boolean }>>(),
  deleteOfflineDownload: vi.fn<(taskId: string) => Promise<{ recordRemoved: boolean; offlineResourceRemoved: boolean }>>(),
}))

vi.mock("../ipc/download-gateway", async (importOriginal) => {
  const original = await importOriginal<typeof import("../ipc/download-gateway")>()
  return {
    ...original,
    listDownloads,
    subscribeDownloadEvents,
    removeDownloadRecord,
    deleteOfflineDownload,
  }
})
vi.mock("@/lib/ipc/runtime", () => ({ getHavenClientMode: () => "tauri" }))
vi.mock("../components/DownloadTaskList", () => ({
  DownloadTaskList: ({ tasks, onRemoveRecord, onDeleteOffline }: { tasks: DownloadTaskDto[]; onRemoveRecord?: (taskId: string) => void; onDeleteOffline?: (taskId: string) => void }) => (
    <div>
      {tasks.map((task) => (
        <article key={task.taskId} data-testid={`task-${task.taskId}`}>
          {task.title}:{task.bytesDownloaded}
          <output data-testid={`offline-${task.taskId}`}>{task.offlineResourceId ?? "none"}</output>
          <button type="button" onClick={() => onRemoveRecord?.(task.taskId)}>删除记录 {task.taskId}</button>
          <button type="button" onClick={() => onDeleteOffline?.(task.taskId)}>删除离线 {task.taskId}</button>
        </article>
      ))}
    </div>
  ),
}))
vi.mock("../components/DownloadBatchItem", () => ({ DownloadBatchItem: () => null }))

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, resolve, reject }
}

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

function event(taskId: string, sequence: number, downloaded: number): DownloadEvent {
  return {
    operationId: taskId,
    sequence,
    at: `2026-09-07T00:00:0${sequence}.000Z`,
    kind: "updated",
    data: {
      taskId,
      state: downloaded === 100 ? "completed" : "downloading",
      offlineResourceId: downloaded === 100 ? `offline-${taskId}` : null,
      bytesTotal: 100,
      bytesDownloaded: downloaded,
      speedBps: null,
      etaSeconds: null,
      errorCode: null,
    },
  }
}

let receiveEvent: ((event: DownloadEvent) => void) | undefined

function renderPage() {
  return render(
    <NoticeContext.Provider value={{
      notices: [], push: () => "", dismiss: () => undefined, clear: () => undefined,
      resolveConfirm: () => undefined, confirm: async () => true,
    }}>
      <MemoryRouter initialEntries={["/downloads"]}><DownloadsPage /></MemoryRouter>
    </NoticeContext.Provider>,
  )
}

beforeEach(() => {
  receiveEvent = undefined
  listDownloads.mockReset()
  removeDownloadRecord.mockReset()
  deleteOfflineDownload.mockReset()
  subscribeDownloadEvents.mockImplementation(async (callback) => {
    receiveEvent = callback
    return async () => undefined
  })
})
afterEach(cleanup)

describe("DownloadsPage asynchronous refreshes", () => {
  it("merges a progress event into an in-flight list and releases the refresh control", async () => {
    const initial = task()
    const lateRefresh = deferred<DownloadTaskDto[]>()
    listDownloads.mockResolvedValueOnce([initial]).mockImplementationOnce(() => lateRefresh.promise)

    renderPage()
    await screen.findByTestId("task-task-1")
    fireEvent.click(screen.getByTitle("刷新下载任务"))
    await waitFor(() => expect(listDownloads).toHaveBeenCalledTimes(2))

    receiveEvent?.(event("task-1", 1, 70))
    lateRefresh.resolve([initial])

    await waitFor(() => expect(screen.getByTestId("task-task-1").textContent).toContain("下载一:70"))
    expect(screen.getByTitle("刷新下载任务").hasAttribute("disabled")).toBe(false)
  })

  it("keeps one unknown-task reconciliation alive across consecutive events until the task is listed", async () => {
    const firstList = deferred<DownloadTaskDto[]>()
    const reconciliation = deferred<DownloadTaskDto[]>()
    listDownloads.mockImplementationOnce(() => firstList.promise).mockImplementationOnce(() => reconciliation.promise)

    renderPage()
    await waitFor(() => expect(receiveEvent).toBeTypeOf("function"))
    receiveEvent?.(event("new-task", 1, 20))
    await waitFor(() => expect(listDownloads).toHaveBeenCalledTimes(2))
    receiveEvent?.(event("new-task", 2, 60))

    firstList.resolve([])
    reconciliation.resolve([task({ taskId: "new-task", title: "新下载", bytesDownloaded: 20 })])

    await waitFor(() => expect(screen.getByTestId("task-new-task").textContent).toContain("新下载:60"))
  })

  it("does not let a pre-operation refresh restore a successfully removed record", async () => {
    const initial = task()
    const staleRefresh = deferred<DownloadTaskDto[]>()
    listDownloads.mockResolvedValueOnce([initial]).mockImplementationOnce(() => staleRefresh.promise)
    removeDownloadRecord.mockResolvedValue({ recordRemoved: true, offlineResourceRemoved: false })

    renderPage()
    await screen.findByTestId("task-task-1")
    fireEvent.click(screen.getByTitle("刷新下载任务"))
    await waitFor(() => expect(listDownloads).toHaveBeenCalledTimes(2))
    fireEvent.click(screen.getByRole("button", { name: "删除记录 task-1" }))
    await waitFor(() => expect(screen.queryByTestId("task-task-1")).toBeNull())

    staleRefresh.resolve([initial])
    await waitFor(() => expect(screen.getByTitle("刷新下载任务").hasAttribute("disabled")).toBe(false))
    expect(screen.queryByTestId("task-task-1")).toBeNull()
  })

  it("does not restore an offline resource from a delayed completed event after deletion", async () => {
    const completed = task({ state: "completed", offlineResourceId: "offline-task-1", bytesDownloaded: 100 })
    listDownloads.mockResolvedValue([completed])
    deleteOfflineDownload.mockResolvedValue({ recordRemoved: false, offlineResourceRemoved: true })

    renderPage()
    await screen.findByTestId("task-task-1")
    expect(screen.getByTestId("offline-task-1").textContent).toBe("offline-task-1")
    fireEvent.click(screen.getByRole("button", { name: "删除离线 task-1" }))

    await waitFor(() => expect(screen.getByTestId("offline-task-1").textContent).toBe("none"))
    receiveEvent?.(event("task-1", 1, 100))

    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(screen.getByTestId("offline-task-1").textContent).toBe("none")
  })
})
