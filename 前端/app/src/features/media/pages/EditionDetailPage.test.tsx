// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { MemoryRouter, Route, Routes } from "react-router"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { EditionDetailDto, MediaItemSummaryDto, PrimaryActionDto } from "@/lib/ipc/generated/wire"
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

vi.mock("@/lib/ipc/runtime", () => ({ getHavenClientMode: () => "tauri" }))
vi.mock("../ipc/edition-gateway", () => ({
  getEdition: editionGateway.getEdition,
  normalizeEditionError: (error: unknown) => error,
}))
vi.mock("@/features/downloads/ipc/download-gateway", () => downloadGateway)

const action = (kind: PrimaryActionDto["kind"], mediaItemId: string): PrimaryActionDto => ({
  kind,
  labelHint: "start",
  editionId: "edition-1",
  mediaItemId,
  locator: null,
})

const capability = (overrides: Partial<{
  status: "idle" | "queued" | "downloaded"
  canDownload: boolean
  hasOfflineResource: boolean
  canOnlineRead: boolean
  sourceResourceId: string | null
  taskId: string | null
}> = {}) => ({
  status: "idle" as const,
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

  it("uses the article route selected by PrimaryActionDto", async () => {
    const article = item({
      mediaItemId: "media-article",
      title: "报刊文章",
      mediaType: "article",
      primaryAction: action("article", "media-article"),
    })
    editionGateway.getEdition.mockResolvedValue(detail([article]))
    downloadGateway.getMediaItemsDownloadInfo.mockResolvedValue(new Map([["media-article", capability({ canOnlineRead: true })]]))

    renderPage()

    await screen.findByText(/可在线阅读/)
    fireEvent.click(screen.getByRole("button", { name: "打开" }))
    expect(await screen.findByText("article route")).toBeTruthy()
  })
})
