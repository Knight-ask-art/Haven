// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { MemoryRouter, useLocation } from "react-router"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { HistoryCardProps } from "../ipc/footprints-gateway"
import { HistoryPage } from "./HistoryPage"

const { getRecentActivityFootprintItems, getMediaItemDownloadInfo } = vi.hoisted(() => ({
  getRecentActivityFootprintItems: vi.fn<() => Promise<HistoryCardProps[]>>(),
  getMediaItemDownloadInfo: vi.fn(),
}))

vi.mock("../ipc/footprints-gateway", async (importOriginal) => ({
  ...await importOriginal<typeof import("../ipc/footprints-gateway")>(),
  getRecentActivityFootprintItems,
}))
vi.mock("@/features/downloads/ipc/download-gateway", () => ({ getMediaItemDownloadInfo }))
vi.mock("@/lib/ipc/runtime", () => ({ getHavenClientMode: () => "tauri" }))
vi.mock("@/components/ui/haven/ArtworkImage", () => ({ ArtworkImage: ({ alt }: { alt: string }) => <img alt={alt} /> }))

function Location() {
  return <output data-testid="location">{useLocation().pathname}</output>
}

function historyItem(): HistoryCardProps {
  return {
    id: "history-1", workId: "work-1", mediaItemId: "media-1", title: "只能下载的图书", subtitle: "刚刚",
    imageUrl: "", lastActiveAt: "2026-09-07T00:00:00.000Z", primaryAction: {
      kind: "reader", labelHint: "continue", mediaItemId: "media-1", editionId: "edition-1", locator: null,
    },
  }
}

beforeEach(() => {
  getRecentActivityFootprintItems.mockReset()
  getMediaItemDownloadInfo.mockReset()
})
afterEach(cleanup)

describe("HistoryPage production opening", () => {
  it("uses the same capability gate as Footprints instead of directly navigating", async () => {
    getRecentActivityFootprintItems.mockResolvedValue([historyItem()])
    getMediaItemDownloadInfo.mockResolvedValue({ canOnlineRead: false, hasOfflineResource: false, canDownload: true })

    render(<MemoryRouter initialEntries={["/footprints/history"]}><HistoryPage /><Location /></MemoryRouter>)
    fireEvent.click(await screen.findByRole("button", { name: /只能下载的图书/ }))

    expect(await screen.findByText("只能下载的图书该内容需要下载后阅读")).toBeTruthy()
    expect(screen.getByTestId("location").textContent).toBe("/footprints/history")
    expect(getMediaItemDownloadInfo).toHaveBeenCalledWith("media-1")
  })
})
