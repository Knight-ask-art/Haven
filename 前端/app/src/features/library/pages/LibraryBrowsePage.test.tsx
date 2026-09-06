// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { MemoryRouter, Route, Routes } from "react-router"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { LibraryMediaItemData } from "../components/MediaItem"
import type { LibraryBrowsePage, LibraryBrowseQuery } from "../ipc/gateway"
import { LibraryBrowsePage as BrowsePage } from "./LibraryBrowsePage"

const { getLibraryBrowsePage } = vi.hoisted(() => ({
  getLibraryBrowsePage: vi.fn<(cursor: string | null, query: LibraryBrowseQuery) => Promise<LibraryBrowsePage>>(),
}))

vi.mock("../ipc/gateway", async (importOriginal) => {
  const original = await importOriginal<typeof import("../ipc/gateway")>()
  return { ...original, getLibraryBrowsePage }
})
vi.mock("@/lib/ipc/runtime", () => ({ isTauriRuntime: () => true }))
vi.mock("@/lib/ipc/events", () => ({
  onFavoriteChanged: vi.fn(async () => () => undefined),
  onLibraryChanged: vi.fn(async () => () => undefined),
}))
vi.mock("../components/MediaItem", () => ({
  MediaItem: ({ item }: { item: LibraryMediaItemData }) => <article>{item.title}</article>,
}))

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, resolve, reject }
}

function item(id: string, title: string, year = 2026): LibraryMediaItemData {
  return { id, title, type: "movie", year, imageUrl: "", favorite: false }
}

function renderPage(initialEntry: string) {
  return render(
    <MemoryRouter initialEntries={[initialEntry]}>
      <Routes>
        <Route path="/library/:category" element={<BrowsePage />} />
      </Routes>
    </MemoryRouter>,
  )
}

beforeEach(() => {
  getLibraryBrowsePage.mockReset()
})
afterEach(() => {
  cleanup()
})

describe("LibraryBrowsePage pagination races", () => {
  it("stops filtered auto-pagination after failure until the user retries", async () => {
    getLibraryBrowsePage
      .mockResolvedValueOnce({ items: [item("first", "第一页")], nextCursor: "cursor-200", total: 2 })
      .mockRejectedValueOnce(new Error("下一页失败"))
      .mockResolvedValueOnce({ items: [item("second", "重试成功")], nextCursor: null, total: 2 })

    renderPage("/library/video?year=2026")

    const retry = await screen.findByRole("button", { name: "重试继续" })
    expect(getLibraryBrowsePage).toHaveBeenCalledTimes(2)
    await new Promise((resolve) => setTimeout(resolve, 20))
    expect(getLibraryBrowsePage).toHaveBeenCalledTimes(2)

    fireEvent.click(retry)
    expect(await screen.findByText("重试成功")).toBeTruthy()
    expect(getLibraryBrowsePage).toHaveBeenCalledTimes(3)
    expect(getLibraryBrowsePage.mock.calls[2]?.[0]).toBe("cursor-200")
  })

  it("does not inherit A cursor or accept A late page after switching to B", async () => {
    const lateA = deferred<LibraryBrowsePage>()
    const firstB = deferred<LibraryBrowsePage>()
    getLibraryBrowsePage.mockImplementation((cursor, query) => {
      if (query.query === "A" && cursor === null) {
        return Promise.resolve({ items: [item("a-0", "A 首屏")], nextCursor: "cursor-600", total: 3 })
      }
      if (query.query === "A" && cursor === "cursor-600") return lateA.promise
      if (query.query === "B" && cursor === null) return firstB.promise
      return Promise.reject(new Error(`unexpected request ${query.query}/${cursor}`))
    })

    renderPage("/library/video?q=A")
    await screen.findByText("A 首屏")
    fireEvent.click(screen.getByRole("button", { name: "加载更多" }))
    await waitFor(() => expect(getLibraryBrowsePage).toHaveBeenCalledTimes(2))

    fireEvent.change(screen.getByRole("textbox", { name: "搜索影视库" }), { target: { value: "B" } })
    await waitFor(() => expect(getLibraryBrowsePage).toHaveBeenCalledTimes(3))
    expect(getLibraryBrowsePage.mock.calls[2]).toEqual([null, expect.objectContaining({ query: "B" })])
    expect(getLibraryBrowsePage.mock.calls).not.toContainEqual(["cursor-600", expect.objectContaining({ query: "B" })])

    lateA.resolve({ items: [item("a-late", "A 迟到页")], nextCursor: null, total: 3 })
    firstB.resolve({ items: [item("b-0", "B 首屏")], nextCursor: "cursor-b", total: 2 })
    expect(await screen.findByText("B 首屏")).toBeTruthy()
    expect(screen.queryByText("A 迟到页")).toBeNull()
    expect(getLibraryBrowsePage).toHaveBeenCalledTimes(3)
  })
})
