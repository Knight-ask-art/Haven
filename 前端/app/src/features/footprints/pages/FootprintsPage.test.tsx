// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { MemoryRouter, useLocation } from "react-router"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { FootprintActionCard } from "../ipc/footprints-gateway"
import { FootprintsPage } from "./FootprintsPage"

const {
  getContinueFootprintItems, getRecentActivityFootprintItems, getFavoriteFootprintItems, getMarkerFootprintCards,
  getMediaItemDownloadInfo, subscribeDownloadEvents, createDownloadForMediaItem, deleteOfflineDownload,
} = vi.hoisted(() => ({
  getContinueFootprintItems: vi.fn<() => Promise<FootprintActionCard[]>>(),
  getRecentActivityFootprintItems: vi.fn(), getFavoriteFootprintItems: vi.fn(), getMarkerFootprintCards: vi.fn(),
  getMediaItemDownloadInfo: vi.fn(), subscribeDownloadEvents: vi.fn(), createDownloadForMediaItem: vi.fn(), deleteOfflineDownload: vi.fn(),
}))

vi.mock("../ipc/footprints-gateway", async (importOriginal) => ({
  ...await importOriginal<typeof import("../ipc/footprints-gateway")>(),
  getContinueFootprintItems, getRecentActivityFootprintItems, getFavoriteFootprintItems, getMarkerFootprintCards,
}))
vi.mock("@/features/downloads/ipc/download-gateway", () => ({
  getMediaItemDownloadInfo, subscribeDownloadEvents,
  createDownloadForMediaItem, deleteOfflineDownload, revealOfflineDownload: vi.fn(),
}))
vi.mock("@/lib/ipc/runtime", () => ({ getHavenClientMode: () => "tauri" }))
vi.mock("@/lib/ipc/events", () => ({ onFavoriteChanged: vi.fn(async () => () => undefined) }))
vi.mock("@/features/media/ipc/favorite-gateway", () => ({ setFavorite: vi.fn() }))
vi.mock("@/features/home/components/HavenStage", () => ({
  HavenStage: ({
    title,
    onPrimaryAction,
    onAction,
    isDownloaded,
    canManageOffline,
  }: {
    title: string
    onPrimaryAction: () => void
    onAction?: (action: string) => void
    isDownloaded?: boolean
    canManageOffline?: boolean
  }) => (
    <section>
      <button type="button" onClick={onPrimaryAction}>打开 {title}</button>
      <button type="button" onClick={() => onAction?.("download")}>下载 {title}</button>
      <output data-testid="hero-offline">{String(isDownloaded)}</output>
      {canManageOffline && <button type="button" onClick={() => onAction?.("delete")}>删除离线内容</button>}
    </section>
  ),
}))
vi.mock("@/features/home/components/ContentShelf", () => ({
  ContentShelf: ({ title, items }: { title: string; items: Array<FootprintActionCard> }) => <section aria-label={title}>{items.map((item) => <button key={item.id} type="button" onClick={item.onClick}>打开 {item.title}</button>)}</section>,
}))
vi.mock("@/components/ui/haven/ArtworkImage", () => ({ ArtworkImage: () => null }))

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((resolvePromise) => { resolve = resolvePromise })
  return { promise, resolve }
}

function card(id: string, title: string, activeAt: string): FootprintActionCard {
  return {
    id, workId: `work-${id}`, mediaItemId: `media-${id}`, title, subtitle: "继续阅读", imageUrl: "", favorite: false,
    progress: 50, lastActiveAt: activeAt,
    primaryAction: {
      kind: "reader",
      labelHint: "continue",
      mediaItemId: `media-${id}`,
      editionId: `edition-${id}`,
      locator: null,
    },
  } as FootprintActionCard
}

function Location() { return <output data-testid="location">{useLocation().pathname}</output> }

beforeEach(() => {
  getFavoriteFootprintItems.mockReset().mockResolvedValue([])
  getRecentActivityFootprintItems.mockReset().mockResolvedValue([])
  getMarkerFootprintCards.mockReset().mockResolvedValue([])
  getContinueFootprintItems.mockReset()
  getMediaItemDownloadInfo.mockReset()
  createDownloadForMediaItem.mockReset()
  subscribeDownloadEvents.mockReset().mockResolvedValue(async () => undefined)
  deleteOfflineDownload.mockReset().mockResolvedValue(undefined)
  vi.spyOn(window, "confirm").mockReturnValue(true)
})
afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

describe("FootprintsPage opening races", () => {
  it("only lets the most recently clicked footprint navigate after capability checks resolve", async () => {
    const b = card("b", "B 项", "2026-09-07T00:00:02.000Z")
    const c = card("c", "C 项", "2026-09-07T00:00:01.000Z")
    const lateB = deferred<{ canOnlineRead: boolean; hasOfflineResource: boolean; canDownload: boolean }>()
    getContinueFootprintItems.mockResolvedValue([b, c])
    getMediaItemDownloadInfo
      .mockResolvedValueOnce({ canOnlineRead: true, hasOfflineResource: false, canDownload: false })
      .mockImplementationOnce(() => lateB.promise)
      .mockResolvedValueOnce({ canOnlineRead: true, hasOfflineResource: false, canDownload: false })

    render(<MemoryRouter initialEntries={["/footprints"]}><FootprintsPage /><Location /></MemoryRouter>)
    // The Hero's initial capability projection is independent from the two
    // user actions below. Wait for it so the deferred result belongs to B.
    await waitFor(() => expect(getMediaItemDownloadInfo).toHaveBeenCalledTimes(1))
    fireEvent.click(await screen.findByRole("button", { name: "打开 B 项" }))
    fireEvent.click(screen.getByRole("button", { name: "打开 C 项" }))
    await waitFor(() => expect(screen.getByTestId("location").textContent).toBe("/reader/media-c"))

    lateB.resolve({ canOnlineRead: true, hasOfflineResource: false, canDownload: false })
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(screen.getByTestId("location").textContent).toBe("/reader/media-c")
  })

  it("coalesces dense events for the current Hero task while a refresh is in flight", async () => {
    let onEvent: ((event: { data: { taskId: string } }) => void) | undefined
    const refresh = deferred<{ canOnlineRead: boolean; hasOfflineResource: boolean; canDownload: boolean; taskId: string | null }>()
    const hero = card("hero", "Hero 项", "2026-09-07T00:00:03.000Z")
    getContinueFootprintItems.mockResolvedValue([hero])
    getMediaItemDownloadInfo
      .mockResolvedValueOnce({ canOnlineRead: false, hasOfflineResource: false, canDownload: true, taskId: "task-hero" })
      .mockImplementation(() => refresh.promise)
    subscribeDownloadEvents.mockImplementation(async (callback: typeof onEvent) => {
      onEvent = callback
      return async () => undefined
    })

    render(<MemoryRouter initialEntries={["/footprints"]}><FootprintsPage /></MemoryRouter>)
    await waitFor(() => expect(onEvent).toBeDefined())

    const requestsBeforeEvents = getMediaItemDownloadInfo.mock.calls.length
    onEvent?.({ data: { taskId: "task-hero" } })
    await Promise.resolve()
    onEvent?.({ data: { taskId: "task-hero" } })
    await Promise.resolve()
    onEvent?.({ data: { taskId: "task-hero" } })

    expect(getMediaItemDownloadInfo).toHaveBeenCalledTimes(requestsBeforeEvents + 1)

    refresh.resolve({ canOnlineRead: false, hasOfflineResource: false, canDownload: true, taskId: "task-hero" })
    await waitFor(() => expect(getMediaItemDownloadInfo).toHaveBeenCalledTimes(requestsBeforeEvents + 2))
  })

  it("does not let a stale event refresh restore Hero offline state after deletion", async () => {
    let onEvent: ((event: { data: { taskId: string } }) => void) | undefined
    const staleRefresh = deferred<{ canOnlineRead: boolean; hasOfflineResource: boolean; canDownload: boolean; taskId: string | null }>()
    const hero = card("hero", "Hero 项", "2026-09-07T00:00:03.000Z")
    const offlineInfo = { canOnlineRead: false, hasOfflineResource: true, canDownload: true, taskId: "task-hero" }
    const removedInfo = { canOnlineRead: false, hasOfflineResource: false, canDownload: true, taskId: null }
    getContinueFootprintItems.mockResolvedValue([hero])
    getMediaItemDownloadInfo
      .mockResolvedValueOnce(offlineInfo)
      .mockImplementationOnce(() => staleRefresh.promise)
      .mockResolvedValueOnce(removedInfo)
      .mockResolvedValueOnce(offlineInfo)
    subscribeDownloadEvents.mockImplementation(async (callback: typeof onEvent) => {
      onEvent = callback
      return async () => undefined
    })

    render(<MemoryRouter initialEntries={["/footprints"]}><FootprintsPage /></MemoryRouter>)
    await screen.findByRole("button", { name: "删除离线内容" })
    await waitFor(() => expect(onEvent).toBeDefined())

    onEvent?.({ data: { taskId: "task-hero" } })
    await Promise.resolve()
    onEvent?.({ data: { taskId: "task-hero" } })
    fireEvent.click(screen.getByRole("button", { name: "删除离线内容" }))

    await waitFor(() => expect(deleteOfflineDownload).toHaveBeenCalledWith("task-hero"))
    await waitFor(() => expect(screen.getByTestId("hero-offline").textContent).toBe("false"))

    staleRefresh.resolve(offlineInfo)
    await new Promise((resolve) => setTimeout(resolve, 0))

    expect(screen.getByTestId("hero-offline").textContent).toBe("false")
    expect(screen.queryByRole("button", { name: "删除离线内容" })).toBeNull()
  })

  it("keeps a delete read-back authoritative when an old download event arrives during it", async () => {
    let onEvent: ((event: { data: { taskId: string } }) => void) | undefined
    let staleEventDispatched = false
    const deleteReadBack = deferred<{ canOnlineRead: boolean; hasOfflineResource: boolean; canDownload: boolean; taskId: string | null }>()
    const hero = card("hero", "Hero 项", "2026-09-07T00:00:03.000Z")
    const offlineInfo = { canOnlineRead: false, hasOfflineResource: true, canDownload: true, taskId: "task-hero" }
    const removedInfo = { canOnlineRead: false, hasOfflineResource: false, canDownload: true, taskId: null }
    getContinueFootprintItems.mockResolvedValue([hero])
    getMediaItemDownloadInfo
      .mockResolvedValueOnce(offlineInfo)
      .mockImplementationOnce(() => {
        queueMicrotask(() => {
          staleEventDispatched = true
          onEvent?.({ data: { taskId: "task-hero" } })
        })
        return deleteReadBack.promise
      })
      .mockResolvedValueOnce(offlineInfo)
    subscribeDownloadEvents.mockImplementation(async (callback: typeof onEvent) => {
      onEvent = callback
      return async () => undefined
    })

    render(<MemoryRouter initialEntries={["/footprints"]}><FootprintsPage /></MemoryRouter>)
    await screen.findByRole("button", { name: "删除离线内容" })
    await waitFor(() => expect(onEvent).toBeDefined())
    fireEvent.click(screen.getByRole("button", { name: "删除离线内容" }))
    await waitFor(() => expect(staleEventDispatched).toBe(true))
    expect(getMediaItemDownloadInfo).toHaveBeenCalledTimes(2)

    deleteReadBack.resolve(removedInfo)

    await waitFor(() => expect(screen.getByTestId("hero-offline").textContent).toBe("false"))
    expect(screen.queryByRole("button", { name: "删除离线内容" })).toBeNull()
  })

  it("accepts a later Hero event when deleting the offline task itself fails", async () => {
    let onEvent: ((event: { data: { taskId: string } }) => void) | undefined
    const hero = card("hero", "Hero 项", "2026-09-07T00:00:03.000Z")
    const offlineInfo = { canOnlineRead: false, hasOfflineResource: true, canDownload: true, taskId: "task-hero" }
    const removedInfo = { canOnlineRead: false, hasOfflineResource: false, canDownload: true, taskId: null }
    getContinueFootprintItems.mockResolvedValue([hero])
    getMediaItemDownloadInfo
      .mockResolvedValueOnce(offlineInfo)
      .mockResolvedValueOnce(removedInfo)
    deleteOfflineDownload.mockRejectedValueOnce(new Error("delete failed"))
    subscribeDownloadEvents.mockImplementation(async (callback: typeof onEvent) => {
      onEvent = callback
      return async () => undefined
    })

    render(<MemoryRouter initialEntries={["/footprints"]}><FootprintsPage /></MemoryRouter>)
    fireEvent.click(await screen.findByRole("button", { name: "删除离线内容" }))
    await screen.findByText("操作失败，请重试")
    await waitFor(() => expect(onEvent).toBeDefined())

    onEvent?.({ data: { taskId: "task-hero" } })

    await waitFor(() => expect(screen.getByTestId("hero-offline").textContent).toBe("false"))
    expect(screen.queryByRole("button", { name: "删除离线内容" })).toBeNull()
  })

  it("keeps Hero offline state invalidated and offers a retry when deletion refresh fails", async () => {
    const hero = card("hero", "Hero 项", "2026-09-07T00:00:03.000Z")
    const offlineInfo = { canOnlineRead: false, hasOfflineResource: true, canDownload: true, taskId: "task-hero" }
    const removedInfo = { canOnlineRead: false, hasOfflineResource: false, canDownload: true, taskId: null }
    getContinueFootprintItems.mockResolvedValue([hero])
    getMediaItemDownloadInfo
      .mockResolvedValueOnce(offlineInfo)
      .mockRejectedValueOnce(new Error("refresh failed"))
      .mockResolvedValueOnce(removedInfo)

    render(<MemoryRouter initialEntries={["/footprints"]}><FootprintsPage /></MemoryRouter>)
    fireEvent.click(await screen.findByRole("button", { name: "删除离线内容" }))

    await screen.findByText("离线内容已删除，状态刷新失败")
    expect(screen.getByTestId("hero-offline").textContent).toBe("false")
    const retry = screen.getByRole("button", { name: "重试" })
    fireEvent.click(retry)

    await waitFor(() => expect(getMediaItemDownloadInfo).toHaveBeenCalledTimes(3))
    expect(screen.getByTestId("hero-offline").textContent).toBe("false")
  })

  it("reconciles a completion event that arrives before the create-download response", async () => {
    let onEvent: ((event: { data: { taskId: string } }) => void) | undefined
    const hero = card("hero", "Hero 项", "2026-09-07T00:00:03.000Z")
    const idleInfo = { canOnlineRead: false, hasOfflineResource: false, canDownload: true, taskId: null }
    const queuedInfo = { canOnlineRead: false, hasOfflineResource: false, canDownload: true, taskId: "task-hero" }
    const completedInfo = { canOnlineRead: false, hasOfflineResource: true, canDownload: true, taskId: "task-hero" }
    getContinueFootprintItems.mockResolvedValue([hero])
    getMediaItemDownloadInfo
      .mockResolvedValueOnce(idleInfo)
      .mockResolvedValueOnce(idleInfo)
      .mockResolvedValueOnce(queuedInfo)
      .mockResolvedValueOnce(completedInfo)
    subscribeDownloadEvents.mockImplementation(async (callback: typeof onEvent) => {
      onEvent = callback
      return async () => undefined
    })
    createDownloadForMediaItem.mockImplementation(async () => {
      onEvent?.({ data: { taskId: "task-hero" } })
      return { taskId: "task-hero", state: "downloading" }
    })

    render(<MemoryRouter initialEntries={["/footprints"]}><FootprintsPage /></MemoryRouter>)
    await waitFor(() => expect(onEvent).toBeDefined())
    await waitFor(() => expect(getMediaItemDownloadInfo).toHaveBeenCalledTimes(1))
    fireEvent.click(screen.getByRole("button", { name: "下载 Hero 项" }))

    await waitFor(() => expect(screen.getByTestId("hero-offline").textContent).toBe("true"))
    expect(getMediaItemDownloadInfo).toHaveBeenCalledTimes(4)
  })

  it("keeps observing a created task when its first read-back is an older idle snapshot", async () => {
    let onEvent: ((event: { data: { taskId: string } }) => void) | undefined
    const hero = card("hero", "Hero 项", "2026-09-07T00:00:03.000Z")
    const idleInfo = { canOnlineRead: false, hasOfflineResource: false, canDownload: true, taskId: null }
    const completedInfo = { canOnlineRead: false, hasOfflineResource: true, canDownload: true, taskId: "task-hero" }
    getContinueFootprintItems.mockResolvedValue([hero])
    getMediaItemDownloadInfo
      .mockResolvedValueOnce(idleInfo)
      .mockResolvedValueOnce(idleInfo)
      .mockResolvedValueOnce(idleInfo)
      .mockResolvedValueOnce(completedInfo)
    subscribeDownloadEvents.mockImplementation(async (callback: typeof onEvent) => {
      onEvent = callback
      return async () => undefined
    })
    createDownloadForMediaItem.mockResolvedValue({ taskId: "task-hero", state: "downloading" })

    render(<MemoryRouter initialEntries={["/footprints"]}><FootprintsPage /></MemoryRouter>)
    await waitFor(() => expect(onEvent).toBeDefined())
    fireEvent.click(screen.getByRole("button", { name: "下载 Hero 项" }))
    await waitFor(() => expect(getMediaItemDownloadInfo).toHaveBeenCalledTimes(3))

    onEvent?.({ data: { taskId: "task-hero" } })

    await waitFor(() => expect(screen.getByTestId("hero-offline").textContent).toBe("true"))
  })
})
