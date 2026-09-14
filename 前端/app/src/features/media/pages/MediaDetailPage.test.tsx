// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { MemoryRouter, Route, Routes, useNavigate } from "react-router"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { MediaDetailPage } from "./MediaDetailPage"

const gateway = vi.hoisted(() => ({
  createDownloadForMediaItem: vi.fn(),
  deleteOfflineDownload: vi.fn(),
  getMediaItemDownloadInfo: vi.fn(),
  revealOfflineDownload: vi.fn(),
  subscribeDownloadEvents: vi.fn(),
}))
const work = vi.hoisted(() => ({ getWorkDetail: vi.fn() }))
const editions = vi.hoisted(() => ({ loadAllEditionsByWork: vi.fn(), mapEditionListToDetailItems: vi.fn(() => []) }))
const progress = vi.hoisted(() => ({ resetProgress: vi.fn() }))

vi.mock("@/lib/ipc/runtime", () => ({ getHavenClientMode: () => "tauri" }))
vi.mock("../lib/media-detail-runtime-state", () => ({ resolveMediaDetailRuntimeState: () => "production" }))
vi.mock("../ipc/work-gateway", () => work)
vi.mock("../lib/work-detail-mapper", () => ({
  mapWorkDetailHeaderToMediaDetail: (header: { id: string }) => ({
    id: header.id, title: `作品 ${header.id}`, type: "movie", year: 2026, backdropUrl: "cover", posterUrl: "cover",
    description: "", favorite: false,
    primaryAction: { kind: "playback", mediaItemId: `media-${header.id}`, labelHint: "start", locator: null },
  }),
}))
vi.mock("../ipc/edition-gateway", () => ({ ...editions, normalizeEditionError: (error: unknown) => error }))
vi.mock("../lib/edition-mapper", () => ({
  mapEditionListToDetailItems: editions.mapEditionListToDetailItems,
  partitionEditionItems: () => [], toMediaDetailEpisodes: () => [],
}))
vi.mock("../ipc/favorite-gateway", () => ({ onFavoriteChanged: vi.fn(async () => () => undefined), setFavorite: vi.fn() }))
vi.mock("@/features/downloads/ipc/download-gateway", () => gateway)
vi.mock("@/features/progress/ipc/progress-gateway", () => progress)
vi.mock("@/components/ui/haven/ArtworkImage", () => ({ ArtworkImage: () => null }))
vi.mock("@/components/ui/haven/ShareCardModal", () => ({ ShareCardModal: () => null }))

const downloadable = { status: "idle", canDownload: true, hasOfflineResource: false, canOnlineRead: false, sourceResourceId: "source", taskId: null }
const queued = { ...downloadable, status: "queued", taskId: "task-a" }
const offline = { status: "downloaded", canDownload: true, hasOfflineResource: true, canOnlineRead: true, sourceResourceId: "source", taskId: "task-a" }

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((resolvePromise, rejectPromise) => { resolve = resolvePromise; reject = rejectPromise })
  return { promise, resolve, reject }
}

function Switch() {
  const navigate = useNavigate()
  return <button type="button" onClick={() => navigate("/work/B")}>切到 B</button>
}

function renderPage(initial = "/work/A") {
  return render(<MemoryRouter initialEntries={[initial]}><Switch /><Routes><Route path="/work/:workId" element={<MediaDetailPage />} /></Routes></MemoryRouter>)
}

async function openMore() {
  fireEvent.click(await screen.findByTitle("更多"))
}

beforeEach(() => {
  vi.stubGlobal("confirm", vi.fn(() => true))
  work.getWorkDetail.mockImplementation(async (id: string) => ({ id }))
  editions.loadAllEditionsByWork.mockResolvedValue([])
    gateway.getMediaItemDownloadInfo.mockResolvedValue(downloadable)
  gateway.subscribeDownloadEvents.mockResolvedValue(async () => undefined)
  gateway.deleteOfflineDownload.mockResolvedValue(undefined)
  progress.resetProgress.mockResolvedValue(undefined)
})
afterEach(() => { cleanup(); vi.clearAllMocks(); vi.unstubAllGlobals() })

describe("MediaDetailPage download lifecycle", () => {
  it("does not pass an empty source to the native hero image while detail data is loading", () => {
    const pending = deferred<{ id: string }>()
    work.getWorkDetail.mockReturnValue(pending.promise)

    renderPage()

    expect(screen.queryByRole("img", { name: "作品信息暂不可用" })).toBeNull()
  })

  it("refreshes the full download projection when creation returns completed", async () => {
    let created = false
    gateway.createDownloadForMediaItem.mockImplementation(async () => {
      created = true
      return { state: "completed" }
    })
    gateway.getMediaItemDownloadInfo.mockImplementation(async () => created ? offline : downloadable)
    renderPage()
    const downloadButton = await screen.findByLabelText("下载至本地")
    await waitFor(() => expect(downloadButton.hasAttribute("disabled")).toBe(false))
    fireEvent.click(downloadButton)

    expect(await screen.findByLabelText("已下载")).toBeTruthy()
    expect(gateway.getMediaItemDownloadInfo.mock.calls.length).toBeGreaterThanOrEqual(2)
  })

  it("re-enables the download action after creation fails", async () => {
    gateway.createDownloadForMediaItem.mockRejectedValue(new Error("disk unavailable"))
    renderPage()
    const downloadButton = await screen.findByLabelText("下载至本地")
    await waitFor(() => expect(downloadButton.hasAttribute("disabled")).toBe(false))

    fireEvent.click(downloadButton)

    expect(await screen.findByText("创建下载任务失败")).toBeTruthy()
    await waitFor(() => expect(screen.getByLabelText("下载至本地").hasAttribute("disabled")).toBe(false))
  })

  it("refreshes when a newly queued task completes before React commits its task id", async () => {
    let onEvent: ((event: { data: { taskId: string } }) => void) | undefined
    gateway.subscribeDownloadEvents.mockImplementation(async (callback: typeof onEvent) => {
      onEvent = callback
      return async () => undefined
    })
    gateway.getMediaItemDownloadInfo
      .mockResolvedValueOnce(downloadable)
      .mockResolvedValue(offline)
    gateway.createDownloadForMediaItem.mockImplementation(async () => {
      // A fast local task can emit its terminal event before command response
      // lets React commit `task-a` into component state.
      onEvent?.({ data: { taskId: "task-a" } })
      return { state: "queued", taskId: "task-a" }
    })
    renderPage()
    const downloadButton = await screen.findByLabelText("下载至本地")
    await waitFor(() => expect(onEvent).toBeDefined())
    await waitFor(() => expect(downloadButton.hasAttribute("disabled")).toBe(false))

    fireEvent.click(downloadButton)

    expect(await screen.findByLabelText("已下载")).toBeTruthy()
  })

  it("refreshes the full current download projection after a completed task event", async () => {
    let onEvent: ((event: { data: { taskId: string } }) => void) | undefined
    gateway.subscribeDownloadEvents.mockImplementation(async (callback: typeof onEvent) => { onEvent = callback; return async () => undefined })
    gateway.getMediaItemDownloadInfo.mockResolvedValueOnce(queued).mockResolvedValue(offline)
    renderPage()
    await screen.findByLabelText("下载至本地")
    await waitFor(() => expect(onEvent).toBeDefined())

    onEvent?.({ data: { taskId: "task-a" } })
    expect(await screen.findByLabelText("已下载")).toBeTruthy()
    await openMore()
    await waitFor(() => expect(screen.getByRole("button", { name: "删除离线内容" }).hasAttribute("disabled")).toBe(false))
  })

  it("ignores an unrelated download event while the current media task is queued", async () => {
    let onEvent: ((event: { data: { taskId: string } }) => void) | undefined
    gateway.subscribeDownloadEvents.mockImplementation(async (callback: typeof onEvent) => {
      onEvent = callback
      return async () => undefined
    })
    gateway.getMediaItemDownloadInfo.mockResolvedValue(queued)
    renderPage()
    await screen.findByLabelText("下载至本地")
    await waitFor(() => expect(onEvent).toBeDefined())

    const requestsBeforeEvent = gateway.getMediaItemDownloadInfo.mock.calls.length
    onEvent?.({ data: { taskId: "task-unrelated" } })

    expect(gateway.getMediaItemDownloadInfo).toHaveBeenCalledTimes(requestsBeforeEvent)
  })

  it("coalesces dense events for the current download task while a refresh is in flight", async () => {
    let onEvent: ((event: { data: { taskId: string } }) => void) | undefined
    const refresh = deferred<typeof queued>()
    gateway.subscribeDownloadEvents.mockImplementation(async (callback: typeof onEvent) => {
      onEvent = callback
      return async () => undefined
    })
    gateway.getMediaItemDownloadInfo
      .mockResolvedValueOnce(queued)
      .mockImplementation(() => refresh.promise)
    renderPage()
    await screen.findByLabelText("下载至本地")
    await waitFor(() => expect(onEvent).toBeDefined())

    const requestsBeforeEvents = gateway.getMediaItemDownloadInfo.mock.calls.length
    onEvent?.({ data: { taskId: "task-a" } })
    await Promise.resolve()
    onEvent?.({ data: { taskId: "task-a" } })
    await Promise.resolve()
    onEvent?.({ data: { taskId: "task-a" } })

    expect(gateway.getMediaItemDownloadInfo).toHaveBeenCalledTimes(requestsBeforeEvents + 1)

    refresh.resolve(queued)
    await waitFor(() => expect(gateway.getMediaItemDownloadInfo).toHaveBeenCalledTimes(requestsBeforeEvents + 2))
  })

  it("invalidates offline controls immediately when deletion succeeds and exposes refresh retry on failure", async () => {
    const refresh = deferred<typeof downloadable>()
    gateway.getMediaItemDownloadInfo
      .mockResolvedValueOnce(offline)
      .mockReturnValueOnce(refresh.promise)
      .mockResolvedValueOnce(downloadable)
    renderPage()
    expect(await screen.findByLabelText("已下载")).toBeTruthy()
    await openMore()
    fireEvent.click(screen.getByRole("button", { name: "删除离线内容" }))

    await waitFor(() => expect(gateway.deleteOfflineDownload).toHaveBeenCalledWith("task-a"))
    await openMore()
    expect(screen.getByRole("button", { name: "删除离线内容" }).hasAttribute("disabled")).toBe(true)

    refresh.reject(new Error("refresh failed"))
    expect(await screen.findByText("离线内容状态刷新失败，请重试")).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "重试" }))
    expect(await screen.findByLabelText("下载至本地")).toBeTruthy()
  })

  it("does not let a stale A deletion alter B after a route switch", async () => {
    const deleteA = deferred<void>()
    gateway.getMediaItemDownloadInfo.mockImplementation(async (mediaItemId: string) => mediaItemId === "media-A" ? offline : downloadable)
    gateway.deleteOfflineDownload.mockReturnValue(deleteA.promise)
    renderPage()
    expect(await screen.findByLabelText("已下载")).toBeTruthy()
    await openMore()
    fireEvent.click(screen.getByRole("button", { name: "删除离线内容" }))
    fireEvent.click(screen.getByRole("button", { name: "切到 B" }))
    expect(await screen.findByText("作品 B")).toBeTruthy()
    deleteA.resolve()
    await waitFor(() => expect(screen.getByLabelText("下载至本地")).toBeTruthy())
    expect(screen.queryByText("已删除离线内容")).toBeNull()
  })

  it("refreshes both detail and editions after reset", async () => {
    renderPage()
    await screen.findByText("作品 A")
    await openMore()
    fireEvent.click(screen.getByRole("button", { name: "重置进度" }))
    await waitFor(() => expect(work.getWorkDetail).toHaveBeenCalledTimes(2))
    expect(editions.loadAllEditionsByWork).toHaveBeenCalledTimes(2)
  })
})
