// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { MemoryRouter, Route, Routes } from "react-router"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { EditionDetailDto, MediaItemSummaryDto, PrimaryActionDto } from "@/lib/ipc/generated/wire"
import type { MediaItemDownloadInfo } from "@/features/downloads/ipc/download-gateway"
import { EditionDetailPage } from "./EditionDetailPage"

const editionGateway = vi.hoisted(() => ({
  getEdition: vi.fn(),
}))
const downloadGateway = vi.hoisted(() => ({
  createDownloadForMediaItem: vi.fn(),
  getMediaItemDownloadInfo: vi.fn(),
  getMediaItemsDownloadInfo: vi.fn(),
  subscribeDownloadEvents: vi.fn(),
}))

const resumeClient = vi.hoisted(() => ({
  downloadResume: vi.fn(async ({ taskId }: { taskId: string }) => ({ taskId, state: "queued" })),
  downloadRetry: vi.fn(async ({ taskId }: { taskId: string }) => ({ taskId, state: "queued" })),
}))
vi.mock("@/lib/ipc/runtime", () => ({ getHavenClientMode: () => "tauri" }))
vi.mock("../ipc/edition-gateway", () => ({
  getEdition: editionGateway.getEdition,
  normalizeEditionError: (error: unknown) => error,
}))
vi.mock("@/features/downloads/ipc/download-gateway", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/features/downloads/ipc/download-gateway")>()
  return {
    ...actual,
    ...downloadGateway,
    // Paused / interrupted go through the REAL resume / retry selection, whose
    // IPC is faked by `resumeClient`; idle creates through the mocked create so
    // the handler's real mediaItemId stays visible to assertions.
    startDownloadFromProjection: (
      info: { status: string },
      mediaItemId: string,
      client?: unknown,
    ) => (
      info.status === "paused" || info.status === "interrupted"
        ? actual.startDownloadFromProjection(info as never, mediaItemId, (client ?? resumeClient) as never)
        : downloadGateway.createDownloadForMediaItem(mediaItemId)
    ),
  }
})

const action = (kind: PrimaryActionDto["kind"], mediaItemId: string): PrimaryActionDto => ({
  kind,
  labelHint: "start",
  editionId: "edition-1",
  mediaItemId,
  locator: null,
})

const capability = (overrides: Partial<MediaItemDownloadInfo> = {}): MediaItemDownloadInfo => ({
  status: "idle" as const,
  taskState: null,
  offlineResourceId: null,
  canDownload: false,
  hasOfflineResource: false,
  canOnlineRead: false,
  sourceResourceId: null,
  taskId: null,
  ...overrides,
})

function item(overrides: Partial<MediaItemSummaryDto> = {}): MediaItemSummaryDto {
  return {
    mediaItemId: "media-1",
    editionId: "edition-1",
    title: "第一篇内容",
    mediaType: "document",
    indexLabel: "第 1 项",
    durationMs: null,
    pageCount: 12,
    chapterCount: null,
    publishedAt: "2024-08-01",
    status: "available",
    availableResourceCount: 1,
    seasonNumber: null,
    episodeNumber: null,
    progress: null,
    primaryAction: action("reader", "media-1"),
    ...overrides,
  }
}

function detail(items: MediaItemSummaryDto[]): EditionDetailDto {
  return {
    schemaVersion: 1,
    editionId: "edition-1",
    workId: "work-1",
    title: "国家地理 · 2024 年 8 月号",
    subtitle: "深海与极地专题",
    mediaType: "document",
    releaseDate: "2024-08-01",
    language: "zh-CN",
    region: "CN",
    publisherOrStudio: "编辑部",
    description: "报刊资料测试版本",
    items,
  }
}

function renderPage() {
  return render(
    <MemoryRouter initialEntries={["/edition/edition-1"]}>
      <Routes>
        <Route path="/edition/:editionId" element={<EditionDetailPage />} />
        <Route path="/reader/:mediaItemId" element={<p>reader route</p>} />
        <Route path="/article/:mediaItemId" element={<p>article route</p>} />
      </Routes>
    </MemoryRouter>,
  )
}

beforeEach(() => {
  editionGateway.getEdition.mockResolvedValue(detail([item()]))
  downloadGateway.getMediaItemDownloadInfo.mockResolvedValue(capability())
  downloadGateway.getMediaItemsDownloadInfo.mockResolvedValue(new Map([["media-1", capability()]]))
  downloadGateway.subscribeDownloadEvents.mockResolvedValue(async () => undefined)
  downloadGateway.createDownloadForMediaItem.mockResolvedValue({ state: "queued", taskId: "task-1" })
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe("EditionDetailPage reading actions", () => {
  it("closes the event gap after a delayed subscription becomes ready", async () => {
    let ready!: (cleanup: () => Promise<void>) => void
    downloadGateway.subscribeDownloadEvents.mockImplementation(() => new Promise((resolve) => { ready = resolve }))
    let current = capability({ status: "queued", canDownload: true, taskId: "task-1" })
    downloadGateway.getMediaItemsDownloadInfo.mockResolvedValue(new Map([["media-1", current]]))
    downloadGateway.getMediaItemDownloadInfo.mockImplementation(async () => current)
    renderPage()
    await screen.findByText("下载中")
    current = capability({ status: "downloaded", hasOfflineResource: true, taskId: "task-1" })
    ready(async () => undefined)
    expect(await screen.findByText(/已下载/)).toBeTruthy()
    expect(screen.getByRole("button", { name: "打开" }).hasAttribute("disabled")).toBe(false)
  })

  it("does not let the initial batch overwrite a terminal-event refresh", async () => {
    let release!: (value: Map<string, ReturnType<typeof capability>>) => void
    const queued = capability({ status: "queued", canDownload: true, taskId: "task-1" })
    let current = queued
    let onEvent!: (event: { data: { taskId: string; state: string } }) => void
    downloadGateway.getMediaItemsDownloadInfo.mockImplementation(() => new Promise((resolve) => { release = resolve }))
    downloadGateway.getMediaItemDownloadInfo.mockImplementation(async () => current)
    downloadGateway.subscribeDownloadEvents.mockImplementation(async (callback: typeof onEvent) => {
      onEvent = callback
      return async () => undefined
    })
    renderPage()
    await waitFor(() => expect(onEvent).toBeDefined())
    current = capability({ status: "downloaded", hasOfflineResource: true, taskId: "task-1" })
    onEvent({ data: { taskId: "task-1", state: "completed" } })
    release(new Map([["media-1", queued]]))
    expect(await screen.findByText(/已下载/)).toBeTruthy()
    expect(screen.getByRole("button", { name: "打开" }).hasAttribute("disabled")).toBe(false)
  })

  it("loads capability for every media item and opens an online-readable document", async () => {
    const readable = item({
      mediaItemId: "media-readable",
      title: "在线资料",
      primaryAction: action("reader", "media-readable"),
    })
    const unavailable = item({
      mediaItemId: "media-unavailable",
      title: "不可用资料",
      primaryAction: null,
    })
    editionGateway.getEdition.mockResolvedValue(detail([readable, unavailable]))
    downloadGateway.getMediaItemsDownloadInfo.mockResolvedValue(new Map([
      ["media-readable", capability({ canOnlineRead: true })],
      ["media-unavailable", capability()],
    ]))
    downloadGateway.getMediaItemDownloadInfo.mockImplementation(async (id: string) => capability({ canOnlineRead: id === "media-readable" }))

    renderPage()

    await waitFor(() => expect(downloadGateway.getMediaItemsDownloadInfo).toHaveBeenCalledWith([
      "media-readable",
      "media-unavailable",
    ]))
    expect(screen.getByText(/可在线阅读/)).toBeTruthy()
    expect(screen.getByText(/当前内容暂不可用/)).toBeTruthy()

    fireEvent.click(screen.getByRole("button", { name: "打开" }))
    expect(await screen.findByText("reader route")).toBeTruthy()
  })

  it("offers download-first reading for a download-only item and never creates a reader route", async () => {
    const downloadOnly = item({
      mediaItemId: "media-download-only",
      title: "需要下载的期号",
      primaryAction: action("reader", "media-download-only"),
    })
    editionGateway.getEdition.mockResolvedValue(detail([downloadOnly]))
    let current = capability({ canDownload: true })
    downloadGateway.getMediaItemsDownloadInfo.mockResolvedValue(new Map([["media-download-only", current]]))
    downloadGateway.getMediaItemDownloadInfo.mockImplementation(async () => current)
    downloadGateway.createDownloadForMediaItem.mockImplementation(async () => {
      current = capability({ status: "queued", canDownload: true, taskId: "task-download-only" })
      return { state: "queued", taskId: "task-download-only" }
    })

    renderPage()

    expect(await screen.findByText(/需要下载后阅读/)).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "下载后阅读" }))

    await waitFor(() => expect(downloadGateway.createDownloadForMediaItem).toHaveBeenCalledWith("media-download-only"))
    expect(await screen.findByText("下载中")).toBeTruthy()
    expect(screen.queryByText("reader route")).toBeNull()
  })

  it("resumes a paused task on the same id instead of creating and never says 下载中", async () => {
    const row = item({ mediaItemId: "media-paused", title: "暂停的期号", primaryAction: action("reader", "media-paused") })
    editionGateway.getEdition.mockResolvedValue(detail([row]))
    let current = capability({
      status: "paused",
      taskState: "paused",
      canDownload: false,
      canOnlineRead: true,
      taskId: "task-paused",
    })
    downloadGateway.getMediaItemsDownloadInfo.mockResolvedValue(new Map([["media-paused", current]]))
    downloadGateway.getMediaItemDownloadInfo.mockImplementation(async () => current)

    renderPage()

    expect(await screen.findByText(/下载已暂停/)).toBeTruthy()
    expect(screen.queryByText("下载中")).toBeNull()
    resumeClient.downloadResume.mockImplementationOnce(async ({ taskId }) => {
      current = capability({ status: "queued", taskState: "queued", taskId })
      return { taskId, state: "queued" }
    })
    fireEvent.click(screen.getByRole("button", { name: "继续下载" }))
    await waitFor(() => expect(resumeClient.downloadResume).toHaveBeenCalledWith({ taskId: "task-paused" }))
    expect(resumeClient.downloadRetry).not.toHaveBeenCalled()
    expect(downloadGateway.createDownloadForMediaItem).not.toHaveBeenCalled()
    expect(await screen.findByText("下载中")).toBeTruthy()
  })

  it("retries an interrupted task on the same id instead of creating", async () => {
    const row = item({ mediaItemId: "media-stopped", title: "中断的期号", primaryAction: action("reader", "media-stopped") })
    editionGateway.getEdition.mockResolvedValue(detail([row]))
    const current = capability({
      status: "interrupted",
      taskState: "interrupted",
      canDownload: false,
      canOnlineRead: true,
      taskId: "task-stopped",
    })
    downloadGateway.getMediaItemsDownloadInfo.mockResolvedValue(new Map([["media-stopped", current]]))
    downloadGateway.getMediaItemDownloadInfo.mockImplementation(async () => current)

    renderPage()

    fireEvent.click(await screen.findByRole("button", { name: "重试下载" }))
    await waitFor(() => expect(resumeClient.downloadRetry).toHaveBeenCalledWith({ taskId: "task-stopped" }))
    expect(resumeClient.downloadResume).not.toHaveBeenCalled()
    expect(downloadGateway.createDownloadForMediaItem).not.toHaveBeenCalled()
  })

  it("uses the article route selected by PrimaryActionDto", async () => {
    const article = item({
      mediaItemId: "media-article",
      title: "报刊文章",
      mediaType: "article",
      primaryAction: action("article", "media-article"),
    })
    editionGateway.getEdition.mockResolvedValue(detail([article]))
    downloadGateway.getMediaItemsDownloadInfo.mockResolvedValue(new Map([["media-article", capability({ canOnlineRead: true })]]))
    downloadGateway.getMediaItemDownloadInfo.mockResolvedValue(capability({ canOnlineRead: true }))

    renderPage()

    await screen.findByText(/可在线阅读/)
    fireEvent.click(screen.getByRole("button", { name: "打开" }))
    expect(await screen.findByText("article route")).toBeTruthy()
  })

  it("marks only the row missing from the batch projection as a retryable failure", async () => {
    // 批量投影里少了 media-broken：这一篇这次没读到能力事实。它必须落成可重试的失败，
    // 而不是永远停在 loading（读不出事实）或「当前内容暂不可用」（把缺失当结论）。
    const broken = item({
      mediaItemId: "media-broken",
      title: "读取失败的资料",
      indexLabel: "第 1 项",
      primaryAction: action("reader", "media-broken"),
    })
    const readable = item({
      mediaItemId: "media-readable",
      title: "在线资料",
      indexLabel: "第 2 项",
      primaryAction: action("reader", "media-readable"),
    })
    editionGateway.getEdition.mockResolvedValue(detail([broken, readable]))
    downloadGateway.getMediaItemsDownloadInfo.mockResolvedValue(new Map([
      ["media-readable", capability({ canOnlineRead: true })],
    ]))
    downloadGateway.getMediaItemDownloadInfo.mockImplementation(async (id: string) => {
      if (id === "media-broken") throw new Error("capability unavailable")
      return capability({ canOnlineRead: true })
    })

    renderPage()

    expect(await screen.findByText("第 1 项 · 12 页 · 能力读取失败")).toBeTruthy()
    // 成功项保留真实能力：一项失败不牵连整批，也不影响这一项原本的可读路径。
    expect(screen.getByText("第 2 项 · 12 页 · 可在线阅读")).toBeTruthy()
    // 失败项不是「正在读取」，也不会被写成明确不可用。
    expect(screen.queryByText(/正在读取能力/)).toBeNull()
    expect(screen.queryByText("当前内容暂不可用")).toBeNull()

    downloadGateway.getMediaItemDownloadInfo.mockResolvedValue(capability({ canOnlineRead: true }))
    fireEvent.click(screen.getByRole("button", { name: "重试" }))
    await waitFor(() => expect(downloadGateway.getMediaItemDownloadInfo).toHaveBeenCalledWith("media-broken"))
    // 单篇重查拿到真实能力后，这一行回到可打开状态。
    expect(await screen.findByText("第 1 项 · 12 页 · 可在线阅读")).toBeTruthy()
    expect(screen.getAllByRole("button", { name: "打开" })).toHaveLength(2)
  })
})
