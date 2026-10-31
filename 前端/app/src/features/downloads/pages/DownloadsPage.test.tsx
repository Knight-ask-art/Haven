// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { MemoryRouter, Route, Routes, useLocation } from "react-router"
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
vi.mock("../components/DownloadTaskList", async (importOriginal) => {
  // 空态是这次要验证的行为本身：必须渲染真实组件的空态分支，不能用一个只画 tasks 的
  // 替身把它藏起来。只有非空列表用替身（需要直接点每行上的管理动作）。
  const actual = await importOriginal<typeof import("../components/DownloadTaskList")>()
  return {
    DownloadTaskList: (props: Parameters<typeof actual.DownloadTaskList>[0]) => props.tasks.length === 0
      ? actual.DownloadTaskList(props)
      : (
        <div>
          {props.tasks.map((task) => (
            <article key={task.taskId} data-testid={`task-${task.taskId}`}>
              {task.title}:{task.bytesDownloaded}
              <output data-testid={`offline-${task.taskId}`}>{task.offlineResourceId ?? "none"}</output>
              <button type="button" onClick={() => props.onRemoveRecord?.(task.taskId)}>删除记录 {task.taskId}</button>
              <button type="button" onClick={() => props.onDeleteOffline?.(task.taskId)}>删除离线 {task.taskId}</button>
            </article>
          ))}
        </div>
      ),
  }
})
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

/** 把当前 URL 的查询串写进 DOM，用来断言"清除筛选只动了这两项参数"。 */
function LocationProbe() {
  const location = useLocation()
  return <output data-testid="location-probe">{location.search.replace(/^\?/, "")}</output>
}

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
  it("uses clear storage and file-import copy", async () => {
    listDownloads.mockResolvedValueOnce([])

    renderPage()

    expect(await screen.findByText(/空间不足时，请释放空间后手动重试失败的任务。/)).toBeTruthy()
    expect(screen.getByText("当前版本暂不支持拖拽导入本地文件")).toBeTruthy()
    expect(screen.queryByText(/WaitingForSpace|P1 预览|拖拽文件到此处可导入/)).toBeNull()
  })

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

/**
 * 筛选无匹配 ≠ 整个下载库没有任务（审计 B04）。
 *
 * 库里明明有需处理的任务、页头也这样统计，用户只是把状态切到「已完成」：显示
 * 「当前没有下载任务 / 从作品详情页选择内容」是在否认页头自己刚说过的事实。
 */
describe("DownloadsPage filtered empty state", () => {
  const failed = task({ taskId: "task-failed", title: "失败的下载", state: "failed" })

  it("says the filter has no matches and offers to clear it, without claiming the library is empty", async () => {
    listDownloads.mockResolvedValue([failed])

    renderPage()
    await screen.findByText(/1 个需处理/)
    fireEvent.click(screen.getByRole("tab", { name: "已完成" }))

    const empty = await screen.findByTestId("downloads-filter-empty")
    expect(empty.textContent).toContain("没有符合当前筛选的下载任务")
    expect(empty.textContent).toContain("已完成")
    expect(screen.queryByText("当前没有下载任务")).toBeNull()
    expect(screen.queryByText("从作品详情页选择内容后会出现在这里。")).toBeNull()

    fireEvent.click(screen.getByRole("button", { name: "清除筛选" }))
    await waitFor(() => expect(screen.getByTestId("task-task-failed")).toBeTruthy())
    expect(screen.queryByTestId("downloads-filter-empty")).toBeNull()
    expect(screen.getByRole("tab", { name: "全部任务" }).getAttribute("aria-selected")).toBe("true")
  })

  it("names a non-matching search query and keeps sorting unrelated", async () => {
    listDownloads.mockResolvedValue([failed])

    renderPage()
    await screen.findByTestId("task-task-failed")
    fireEvent.change(screen.getByPlaceholderText("搜索下载任务"), { target: { value: "不存在的标题" } })

    const empty = await screen.findByTestId("downloads-filter-empty")
    expect(empty.textContent).toContain("不存在的标题")

    fireEvent.click(screen.getByRole("button", { name: "清除筛选" }))
    await waitFor(() => expect(screen.getByTestId("task-task-failed")).toBeTruthy())
    expect((screen.getByPlaceholderText("搜索下载任务") as HTMLInputElement).value).toBe("")
  })

  it("still shows the whole-library empty state when there really are no tasks", async () => {
    listDownloads.mockResolvedValue([])

    renderPage()
    await waitFor(() => expect(screen.getByText("当前没有下载任务")).toBeTruthy())
    expect(screen.queryByTestId("downloads-filter-empty")).toBeNull()
  })

  /**
   * 批次行本身就是可见内容：单条列表空着时不能在其下方再断言「当前没有下载任务」。
   *
   * 例如把状态切到「已完成」后只剩批次里的行——页头与批次都列着任务，列表却说自己全空。
   */
  it("does not claim the library is empty underneath visible batch rows", async () => {
    const batched = task({ taskId: "task-batched", title: "批次里的下载", state: "failed", batchId: "batch-1" })
    listDownloads.mockResolvedValue([batched])

    renderPage()
    await screen.findByText(/1 个需处理/)

    // 批次分组可见（页头计数列出 1 个批次），单条列表没有行。
    await waitFor(() => expect(screen.getByText(/1 个批次/)).toBeTruthy())
    expect(screen.queryByTestId("task-task-batched")).toBeNull()
    expect(screen.queryByText("当前没有下载任务")).toBeNull()
    expect(screen.queryByText("从作品详情页选择内容后会出现在这里。")).toBeNull()
  })

  it("clearing filters keeps unrelated URL params and the local sort, and clears the local query", async () => {
    listDownloads.mockResolvedValue([
      task({ taskId: "task-a", title: "甲书", category: "book", state: "failed", createdAt: "2026-09-01T00:00:00.000Z" }),
      task({ taskId: "task-b", title: "乙书", category: "video", state: "completed", createdAt: "2026-09-02T00:00:00.000Z" }),
    ])

    render(
      <NoticeContext.Provider value={{
        notices: [], push: () => "", dismiss: () => undefined, clear: () => undefined,
        resolveConfirm: () => undefined, confirm: async () => true,
      }}>
        <MemoryRouter initialEntries={["/downloads?storage=storage-1&category=video&status=failed"]}>
          <Routes>
            {/* 探针与页面同级：URL 一变它就重渲染，用来断言查询串的最终形态。 */}
            <Route path="/downloads" element={<><DownloadsPage /><LocationProbe /></>} />
          </Routes>
        </MemoryRouter>
      </NoticeContext.Provider>,
    )
    // 排序切到「名称」：清除筛选不该把它复位成「最近添加」。
    fireEvent.click(await screen.findByRole("button", { name: "最近添加" }))
    expect(screen.getByRole("button", { name: "名称" })).toBeTruthy()

    // 分类 + 搜索词都无匹配，给出筛选空态。
    fireEvent.change(screen.getByPlaceholderText("搜索下载任务"), { target: { value: "不存在的标题" } })
    await screen.findByTestId("downloads-filter-empty")

    fireEvent.click(screen.getByRole("button", { name: "清除筛选" }))
    await waitFor(() => expect(screen.getByTestId("task-task-a")).toBeTruthy())
    expect(screen.getByTestId("task-task-b")).toBeTruthy()
    expect((screen.getByPlaceholderText("搜索下载任务") as HTMLInputElement).value).toBe("")
    expect(screen.getByRole("button", { name: "名称" })).toBeTruthy()
    // 无关参数原样保留，只有分类/状态被清掉。
    const params = new URLSearchParams(screen.getByTestId("location-probe").textContent ?? "")
    expect(params.get("storage")).toBe("storage-1")
    expect(params.get("category")).toBeNull()
    expect(params.get("status")).toBeNull()
  })
})
