// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { MemoryRouter, Route, Routes, useLocation } from "react-router"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { LibraryMediaItemData } from "../components/MediaItem"
import { LibraryPage } from "./LibraryPage"

const { getLibraryBrowseItems, markLibraryItemCompleted } = vi.hoisted(() => ({
  getLibraryBrowseItems: vi.fn<() => Promise<LibraryMediaItemData[]>>(),
  markLibraryItemCompleted: vi.fn<(item: LibraryMediaItemData) => Promise<void>>(),
}))

vi.mock("../ipc/gateway", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../ipc/gateway")>()),
  getLibraryBrowseItems,
  markLibraryItemCompleted,
}))
vi.mock("@/lib/ipc/runtime", () => ({ isTauriRuntime: () => true, getHavenClientMode: () => "tauri" }))
vi.mock("@/lib/ipc/events", () => ({
  onFavoriteChanged: vi.fn(async () => () => undefined),
  onLibraryChanged: vi.fn(async () => () => undefined),
}))
vi.mock("../components/LibraryTVExpandedSidebar", () => ({
  LibraryTVExpandedSidebar: ({ onSelectCategory }: { onSelectCategory: (category: string) => void }) => (
    <nav>
      <button type="button" onClick={() => onSelectCategory("all")}>全部</button>
      <button type="button" onClick={() => onSelectCategory("book")}>图书</button>
    </nav>
  ),
}))
vi.mock("../components/LibraryTVHeroInfo", () => ({ LibraryTVHeroInfo: () => null }))

function item(id: string, type: string): LibraryMediaItemData {
  return {
    id,
    title: id,
    type,
    year: 2026,
    imageUrl: "cover",
    progressMediaItemId: `${id}-media`,
    progressLocator: { version: 1, kind: "video", data: { positionMs: 0 } },
  }
}

function Location() {
  const location = useLocation()
  return <output data-testid="location">{location.pathname}{location.search}</output>
}

function renderPage() {
  return render(
    <MemoryRouter initialEntries={["/library?category=all"]}>
      <Routes>
        <Route path="/library" element={<LibraryPage />} />
        <Route path="/work/:id" element={<Location />} />
      </Routes>
      <Location />
    </MemoryRouter>,
  )
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((resolvePromise) => { resolve = resolvePromise })
  return { promise, resolve }
}

beforeEach(() => {
  getLibraryBrowseItems.mockResolvedValue([item("movie-a", "movie"), item("book-a", "book")])
  markLibraryItemCompleted.mockResolvedValue(undefined)
})
afterEach(() => cleanup())

describe("LibraryPage batch selection lifecycle", () => {
  it("selects cards from all-category shelves instead of navigating, and clears the selection on category changes", async () => {
    renderPage()
    await screen.findByText("movie-a")

    fireEvent.click(screen.getByRole("button", { name: "批量操作" }))
    fireEvent.click(screen.getByRole("button", { name: /^movie-a / }))

    expect(screen.getByText("已选 1 项")).toBeTruthy()
    expect(screen.getByTestId("location").textContent).toBe("/library?category=all")

    fireEvent.click(screen.getByRole("button", { name: "图书" }))
    expect(await screen.findByText("全部 图书 资源")).toBeTruthy()
    expect(screen.getByText("已选 0 项")).toBeTruthy()

    fireEvent.click(screen.getAllByRole("button", { name: "选择 book-a" })[0]!)
    fireEvent.click(screen.getByRole("button", { name: "标记已看" }))
    await waitFor(() => expect(markLibraryItemCompleted).toHaveBeenCalledWith(expect.objectContaining({ id: "book-a" })))
    expect(await screen.findByRole("button", { name: "批量操作" })).toBeTruthy()
  })

  it("keeps only failed selections for retry and blocks category changes or exit while saving", async () => {
    const pending = deferred<void>()
    markLibraryItemCompleted.mockReturnValueOnce(pending.promise)
    renderPage()
    await screen.findByText("movie-a")

    fireEvent.click(screen.getByRole("button", { name: "批量操作" }))
    fireEvent.click(screen.getByRole("button", { name: /^movie-a / }))
    fireEvent.click(screen.getByRole("button", { name: "标记已看" }))

    expect(screen.getByRole("button", { name: "退出批量" }).hasAttribute("disabled")).toBe(true)
    fireEvent.click(screen.getByRole("button", { name: "图书" }))
    expect(screen.queryByText("全部 图书 资源")).toBeNull()

    pending.resolve()
    expect(await screen.findByRole("button", { name: "批量操作" })).toBeTruthy()
  })

  it("retains failed selections for an explicit retry and clears them only after success", async () => {
    markLibraryItemCompleted.mockRejectedValueOnce(new Error("保存失败"))
    renderPage()
    await screen.findByText("movie-a")

    fireEvent.click(screen.getByRole("button", { name: "批量操作" }))
    fireEvent.click(screen.getByRole("button", { name: /^movie-a / }))
    fireEvent.click(screen.getByRole("button", { name: "标记已看" }))

    expect(await screen.findByText("0 项已标记，1 项失败，请重试")).toBeTruthy()
    expect(screen.getByText("已选 1 项")).toBeTruthy()
    expect(screen.getByRole("button", { name: "标记已看" })).toBeTruthy()
  })
})
