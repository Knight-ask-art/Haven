// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { MemoryRouter, Route, Routes, useNavigate } from "react-router"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { ComicWorkChapterCatalogDto } from "@/lib/ipc/generated/wire"
import type { EditionListItem } from "../lib/edition-mapper"
import { MediaDetailPage } from "./MediaDetailPage"
import comicWorkCatalogFixture from "../../../../../../contracts/ipc/v1/fixtures/comic/work-chapter-catalog.normal.json"

const comicWorkCatalog = comicWorkCatalogFixture as unknown as ComicWorkChapterCatalogDto

const gateway = vi.hoisted(() => ({
  createDownloadForMediaItem: vi.fn(),
  deleteOfflineDownload: vi.fn(),
  getMediaItemDownloadInfo: vi.fn(),
  revealOfflineDownload: vi.fn(),
  subscribeDownloadEvents: vi.fn(),
}))
const work = vi.hoisted(() => ({ getWorkDetail: vi.fn() }))
const editions = vi.hoisted(() => ({
  loadAllEditionsByWork: vi.fn(),
  mapEditionListToDetailItems: vi.fn((): EditionListItem[] => []),
}))
const comicCatalog = vi.hoisted(() => ({ getComicWorkChapterCatalog: vi.fn() }))
const progress = vi.hoisted(() => ({ resetProgress: vi.fn() }))
/** 让单个用例切换 work_get 投影出的媒介类型（生产分支按类型选择目录来源）。 */
const detailType = vi.hoisted(() => ({ value: "movie" as "movie" | "comic" }))

vi.mock("@/lib/ipc/runtime", () => ({
  getHavenClientMode: () => "tauri",
  getHavenClient: () => ({}),
  isTauriRuntime: () => true,
}))
vi.mock("../lib/media-detail-runtime-state", () => ({ resolveMediaDetailRuntimeState: () => "production" }))
vi.mock("../ipc/work-gateway", () => work)
vi.mock("../lib/work-detail-mapper", () => ({
  mapWorkDetailHeaderToMediaDetail: (header: { id: string }) => ({
    id: header.id, title: `作品 ${header.id}`, type: header.id === "periodical" ? "document" : detailType.value, year: 2026, backdropUrl: "cover", posterUrl: "cover",
    ...(header.id === "periodical" ? { categories: ["periodical"] } : {}),
    description: "", favorite: false,
    primaryAction: { kind: "playback", mediaItemId: `media-${header.id}`, labelHint: "start", locator: null },
  }),
}))
// normalizeEditionError must stay contract-faithful: production returns a HavenError whose
// `dto.retryable` the edition state machine reads, so an identity mock leaks raw errors.
vi.mock("../ipc/edition-gateway", async () => {
  const { toHavenError } = await import("@/lib/ipc/errors")
  return { ...editions, normalizeEditionError: toHavenError }
})
vi.mock("@/features/comic/ipc/comic-work-chapter-catalog-gateway", () => comicCatalog)
vi.mock("../lib/edition-mapper", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/edition-mapper")>()),
  mapEditionListToDetailItems: editions.mapEditionListToDetailItems,
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

/** 额外挂上漫画阅读器路由，用于断言章节是否真的发生导航。 */
function renderPageWithReader(initial = "/work/A") {
  return render(
    <MemoryRouter initialEntries={[initial]}>
      <Routes>
        <Route path="/work/:workId" element={<MediaDetailPage />} />
        <Route path="/comic/:mediaItemId" element={<div>漫画阅读器</div>} />
      </Routes>
    </MemoryRouter>
  )
}

async function openMore() {
  fireEvent.click(await screen.findByTitle("更多"))
}

/** 断言这些文案在 DOM 中按给定先后顺序出现，即它们在列表里的渲染顺序。 */
function expectRenderedOrder(texts: string[]) {
  const nodes = texts.map((text) => screen.getByText(text))
  for (let index = 1; index < nodes.length; index += 1) {
    expect(nodes[index - 1].compareDocumentPosition(nodes[index]) & Node.DOCUMENT_POSITION_FOLLOWING)
      .toBe(Node.DOCUMENT_POSITION_FOLLOWING)
  }
}

beforeEach(() => {
  vi.stubGlobal("confirm", vi.fn(() => true))
  detailType.value = "movie"
  work.getWorkDetail.mockImplementation(async (id: string) => ({ id }))
  editions.loadAllEditionsByWork.mockResolvedValue([])
  editions.mapEditionListToDetailItems.mockReturnValue([])
  comicCatalog.getComicWorkChapterCatalog.mockResolvedValue(comicWorkCatalog)
  gateway.getMediaItemDownloadInfo.mockResolvedValue(downloadable)
  gateway.subscribeDownloadEvents.mockResolvedValue(async () => undefined)
  gateway.deleteOfflineDownload.mockResolvedValue(undefined)
  progress.resetProgress.mockResolvedValue(undefined)
})
afterEach(() => { cleanup(); localStorage.clear(); vi.clearAllMocks(); vi.unstubAllGlobals() })

describe("MediaDetailPage download lifecycle", () => {
  it("does not pass an empty source to the native hero image while detail data is loading", () => {
    const pending = deferred<{ id: string }>()
    work.getWorkDetail.mockReturnValue(pending.promise)

    renderPage()

    expect(screen.queryByRole("img", { name: "作品信息暂不可用" })).toBeNull()
  })

  it("uses periodical labels when the Work category is periodical and the item is a document", async () => {
    renderPage("/work/periodical")

    expect(await screen.findByText("报刊资料")).toBeTruthy()
    expect(screen.getByRole("button", { name: "往期刊物与分册" })).toBeTruthy()
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

describe("MediaDetailPage comic chapter catalog", () => {
  it("replaces the generic edition list with backend Work chapters for a comic", async () => {
    detailType.value = "comic"
    renderPage()
    await screen.findByText("作品 A")

    await waitFor(() => expect(comicCatalog.getComicWorkChapterCatalog)
      .toHaveBeenCalledWith({ workId: "A", mediaItemId: null }))
    // 漫画生产路径只消费 Work 级目录，通用版本列表一次都不该发出。
    expect(editions.loadAllEditionsByWork).not.toHaveBeenCalled()
    // 章节来自后端目录，而不是通用版本列表。
    expect(await screen.findByText("幕间")).toBeTruthy()
    expect(await screen.findByText("第 2 卷 · 第 12.5 话")).toBeTruthy()
  })

  it("renders production comic chapters straight from the Work catalog DTO", async () => {
    detailType.value = "comic"
    renderPage()
    await screen.findByText("作品 A")
    await screen.findByText("幕间")

    // 状态、页数、进度、来源都按 DTO 逐项落到行上，而不是被压成“可打开的剧集行”。
    expect(screen.getByText("可阅读")).toBeTruthy()
    expect(screen.getByText("暂不可用")).toBeTruthy()
    expect(screen.getByText("仅外部来源")).toBeTruthy()
    expect(screen.getByText("第 2 卷 · 第 12.5 话")).toBeTruthy()
    expect(screen.getByText("24 页")).toBeTruthy()
    expect(screen.getByText("0 页")).toBeTruthy()
    expect(screen.getByText("页数未知")).toBeTruthy()
    expect(screen.getByText("1 个来源 · 可阅读")).toBeTruthy()
    expect(screen.getByText("1 个来源 · 暂不可用")).toBeTruthy()
    // 当前项与低置信度匹配提示同样来自 DTO。
    expect(screen.getByText("当前")).toBeTruthy()
    expect(screen.getByTestId(`comic-match-review-${comicWorkCatalog.chapters[2].mediaItemId}`)).toBeTruthy()
    expect(screen.queryByTestId(`comic-match-review-${comicWorkCatalog.chapters[0].mediaItemId}`)).toBeNull()
    // 进度取后端 progressRatio（0.4），不从通用版本列表推算。
    expect(screen.getByTestId(`comic-chapter-progress-${comicWorkCatalog.chapters[0].mediaItemId}`).textContent).toBe("40%")
  })

  it("does not navigate from chapters the backend marks as not openable", async () => {
    detailType.value = "comic"
    gateway.getMediaItemDownloadInfo.mockResolvedValue(offline)
    renderPageWithReader()
    await screen.findByText("作品 A")
    const unavailableTitle = await screen.findByText("暂不可用章节")
    const requestsBeforeClick = gateway.getMediaItemDownloadInfo.mock.calls.length

    fireEvent.click(unavailableTitle)

    expect(screen.queryByText("漫画阅读器")).toBeNull()
    expect(gateway.getMediaItemDownloadInfo).toHaveBeenCalledTimes(requestsBeforeClick)
    expect(await screen.findByText("当前章节暂不可打开")).toBeTruthy()
  })

  it("opens an openable chapter through the backend primary action", async () => {
    detailType.value = "comic"
    gateway.getMediaItemDownloadInfo.mockResolvedValue(offline)
    renderPageWithReader()
    await screen.findByText("作品 A")

    fireEvent.click(await screen.findByText("幕间"))

    expect(await screen.findByText("漫画阅读器")).toBeTruthy()
  })

  it("filters chapters by catalog.editions without re-deriving the chapter facts", async () => {
    detailType.value = "comic"
    renderPage()
    await screen.findByText("作品 A")
    await screen.findByText("幕间")

    const tablist = await screen.findByRole("tablist", { name: "漫画版本" })
    // 版本维度来自 catalog.editions（displayLabel + chapterCount），不从章节反推。
    expect(within(tablist).getByRole("tab", { name: /zh-hk \/ Fixture Scan Group/ }).textContent).toContain("2")
    fireEvent.click(within(tablist).getByRole("tab", { name: /en \/ Mirror label \/ grayscale/ }))

    expect(screen.getByText("外部章节")).toBeTruthy()
    expect(screen.queryByText("幕间")).toBeNull()
    expect(screen.queryByText("暂不可用章节")).toBeNull()
  })

  it("waits for the authoritative work type before choosing the comic catalog", async () => {
    detailType.value = "comic"
    const pending = deferred<{ id: string }>()
    work.getWorkDetail.mockReturnValue(pending.promise)
    renderPage()

    // work_get 未返回时类型未知：既不能猜 Work 目录，也不能先打通用版本列表。
    await waitFor(() => expect(work.getWorkDetail).toHaveBeenCalledWith("A"))
    expect(editions.loadAllEditionsByWork).not.toHaveBeenCalled()
    expect(comicCatalog.getComicWorkChapterCatalog).not.toHaveBeenCalled()

    pending.resolve({ id: "A" })

    await waitFor(() => expect(comicCatalog.getComicWorkChapterCatalog)
      .toHaveBeenCalledWith({ workId: "A", mediaItemId: null }))
    expect(editions.loadAllEditionsByWork).not.toHaveBeenCalled()
  })

  it("keeps the edition list path for non-comic works", async () => {
    renderPage()
    await screen.findByText("作品 A")

    await waitFor(() => expect(editions.loadAllEditionsByWork).toHaveBeenCalledWith("A"))
    expect(comicCatalog.getComicWorkChapterCatalog).not.toHaveBeenCalled()
  })

  it("does not issue the generic edition list when switching onto a comic work", async () => {
    const pendingB = deferred<{ id: string }>()
    work.getWorkDetail.mockImplementation(async (workId: string) => workId === "B" ? pendingB.promise : { id: workId })
    renderPage()
    await screen.findByText("作品 A")
    await waitFor(() => expect(editions.loadAllEditionsByWork).toHaveBeenCalledWith("A"))

    // A（电影）的媒介判定不得外溢给 B，B 的类型要等自己的 work_get。
    detailType.value = "comic"
    fireEvent.click(screen.getByRole("button", { name: "切到 B" }))
    await waitFor(() => expect(work.getWorkDetail).toHaveBeenCalledWith("B"))
    expect(editions.loadAllEditionsByWork).toHaveBeenCalledTimes(1)

    pendingB.resolve({ id: "B" })
    await waitFor(() => expect(comicCatalog.getComicWorkChapterCatalog)
      .toHaveBeenCalledWith({ workId: "B", mediaItemId: null }))
    expect(editions.loadAllEditionsByWork).toHaveBeenCalledTimes(1)
  })

  it("surfaces a failed comic catalog without falling back to the edition list", async () => {
    detailType.value = "comic"
    comicCatalog.getComicWorkChapterCatalog.mockRejectedValue(new Error("catalog unavailable"))
    renderPage()
    await screen.findByText("作品 A")

    await waitFor(() => expect(comicCatalog.getComicWorkChapterCatalog).toHaveBeenCalledTimes(1))
    expect(editions.loadAllEditionsByWork).not.toHaveBeenCalled()
    expect(screen.queryByText("幕间")).toBeNull()
  })

  it("keeps waiting on the same work for a comic after reset instead of issuing a generic edition list", async () => {
    detailType.value = "comic"
    const restarted = deferred<{ id: string }>()
    work.getWorkDetail
      .mockImplementationOnce(async (workId: string) => ({ id: workId }))
      .mockImplementationOnce(() => restarted.promise)
    renderPage()
    await screen.findByText("作品 A")
    await waitFor(() => expect(comicCatalog.getComicWorkChapterCatalog).toHaveBeenCalledTimes(1))

    await openMore()
    fireEvent.click(screen.getByRole("button", { name: "重置进度" }))
    await waitFor(() => expect(work.getWorkDetail).toHaveBeenCalledTimes(2))

    // 重开的那次 work_get 还没落地：权威类型未知，既不能拿旧类型继续打漫画目录，
    // 也不能退回通用版本列表（后者正是 detailSettledWorkId 只认作品 ID 时的竞态）。
    expect(editions.loadAllEditionsByWork).not.toHaveBeenCalled()
    expect(comicCatalog.getComicWorkChapterCatalog).toHaveBeenCalledTimes(1)

    restarted.resolve({ id: "A" })

    // 落地后仍按当前作品的权威类型重新加载，只走 Work 目录。
    await waitFor(() => expect(comicCatalog.getComicWorkChapterCatalog).toHaveBeenCalledTimes(2))
    expect(editions.loadAllEditionsByWork).not.toHaveBeenCalled()
  })
})

describe("MediaDetailPage content ordering", () => {
  it("keeps the backend chapter order for a production comic even when the sort preference is desc", async () => {
    detailType.value = "comic"
    localStorage.setItem("haven:ui:sort:A", "desc")
    renderPage()
    await screen.findByText("作品 A")

    // 生产漫画的章节顺序是后端事实：作品被设为 desc 后仍按 fixture 的 backendOrder
    // 排列（幕间 → 暂不可用章节 → 外部章节），不因本地排序偏好反转。
    await screen.findByText("幕间")
    expectRenderedOrder(["幕间", "暂不可用章节", "外部章节"])
    // 对生产漫画无效的排序开关不渲染，避免用户以为顺序已被改变；原位置改显示目录刷新状态。
    expect(screen.queryByTestId("work-sort-toggle")).toBeNull()
    expect(screen.getByTestId("comic-catalog-status").textContent).toBe("目录已同步")
  })

  it("still reverses non-comic editions by the persisted sort preference", async () => {
    editions.mapEditionListToDetailItems.mockReturnValue([
      { id: "e1", number: "EP 01", title: "第一集", durationOrPages: "45 分钟", primaryAction: null, mediaType: "episode" },
      { id: "e2", number: "EP 02", title: "第二集", durationOrPages: "45 分钟", primaryAction: null, mediaType: "episode" },
    ])
    localStorage.setItem("haven:ui:sort:A", "desc")
    renderPage()

    await screen.findByText("作品 A")
    await screen.findByText("第一集")
    expectRenderedOrder(["第二集", "第一集"])
    expect(screen.getByTestId("work-sort-toggle").getAttribute("aria-label")).toBe("倒序")
  })
})
