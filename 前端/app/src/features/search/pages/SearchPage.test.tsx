// @vitest-environment jsdom

import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react"
import { MemoryRouter, Route, Routes, useLocation } from "react-router"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { HavenClientMode } from "@/lib/ipc/runtime"
import type { SearchSourceEvent, WorkCardDto } from "@/lib/ipc/generated/wire"
import type { LocalSearchResult } from "../ipc/search-gateway"
import type { startSourceSearch as StartSourceSearch } from "../ipc/source-search-gateway"
import libraryFixture from "../../../../../../contracts/ipc/v1/fixtures/library/list.normal.json"
import { SearchPage } from "./SearchPage"

const mocks = vi.hoisted(() => ({
  mode: "mock" as HavenClientMode,
  searchLocalLibrary: vi.fn(),
  startSourceSearch: vi.fn<typeof StartSourceSearch>(),
  cancelSourceSearch: vi.fn(),
  importSourceWork: vi.fn(),
  recordSearchHistory: vi.fn(),
}))

vi.mock("@/lib/ipc/runtime", () => ({ getHavenClientMode: () => mocks.mode }))
vi.mock("../ipc/search-gateway", () => ({ searchLocalLibrary: mocks.searchLocalLibrary }))
vi.mock("../ipc/source-search-gateway", () => ({
  startSourceSearch: mocks.startSourceSearch,
  cancelSourceSearch: mocks.cancelSourceSearch,
  importSourceWork: mocks.importSourceWork,
}))
vi.mock("../ipc/search-history-gateway", () => ({
  listSearchHistory: async () => [],
  getSearchHistorySetting: async () => true,
  recordSearchHistory: mocks.recordSearchHistory,
  clearSearchHistory: async () => [],
  removeSearchHistory: async () => [],
}))
vi.mock("../hooks/use-trending-boards", () => ({
  useTrendingBoards: () => ({ boards: { boards: [] }, status: "success", error: null, retry: vi.fn() }),
}))
vi.mock("@/components/ui/haven/ArtworkImage", () => ({
  ArtworkImage: ({ alt }: { alt: string }) => <span role="img" aria-label={alt} />,
}))

const localItems: LocalSearchResult[] = [
  { id: "work-book", title: "三体", originalTitle: "三体", description: "文化大革命时期，军方探秘工程寻找地外文明。", category: "book", year: 2008, rating: 9.4 },
  { id: "work-video", title: "三体剧集", category: "video", year: 2023, rating: 8.7 },
]

function event(kind: SearchSourceEvent["kind"], works: WorkCardDto[] = [], sourceId: string | null = null): SearchSourceEvent {
  return {
    operationId: "op-test",
    sequence: 1,
    at: "2026-10-04T00:00:00Z",
    kind,
    data: { sourceId, works, code: null, message: null },
  }
}

function candidate(workId = "content-candidate-test", title = "来源里的三体"): WorkCardDto {
  return { ...libraryFixture.items[0], workId, title } as WorkCardDto
}

function LocationProbe() {
  const location = useLocation()
  return <output data-testid="location">{location.pathname}{location.search}</output>
}

async function renderPage(entry = "/search?q=三体") {
  let result!: ReturnType<typeof render>
  await act(async () => {
    result = render(
      <MemoryRouter initialEntries={[entry]}>
        <LocationProbe />
        <Routes>
          <Route path="/search" element={<SearchPage />} />
          <Route path="/work/:id" element={<p>作品详情</p>} />
        </Routes>
      </MemoryRouter>,
    )
  })
  return result
}

beforeEach(() => {
  mocks.mode = "mock"
  mocks.searchLocalLibrary.mockReset().mockResolvedValue(localItems)
  mocks.startSourceSearch.mockReset().mockImplementation(async (_query, onEvent) => {
    onEvent(event("started"))
    onEvent(event("completed"))
    return { operationId: "op-test", taskId: "task-test", alreadyRunning: false }
  })
  mocks.cancelSourceSearch.mockReset().mockResolvedValue(undefined)
  mocks.importSourceWork.mockReset().mockResolvedValue({ workId: "imported-work" })
  mocks.recordSearchHistory.mockReset().mockResolvedValue([])
})

afterEach(() => cleanup())

describe("SearchPage result presentation and existing actions", () => {
  it("shows the query, local and source sections in mock preview instead of a blank page", async () => {
    const { container } = await renderPage()
    expect(screen.getByRole("heading", { level: 1, name: "“三体”" })).toBeTruthy()
    expect(screen.getByText("浏览器预览 · 示例数据")).toBeTruthy()
    expect(screen.getByRole("region", { name: "本地媒体库" })).toBeTruthy()
    expect(screen.getByRole("region", { name: "来源结果" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "查看作品：三体" })).toBeTruthy()
    expect(screen.queryByText("热门排行")).toBeNull()
    expect(container.querySelector(".search-results-page")).toBeTruthy()
    expect(mocks.searchLocalLibrary).toHaveBeenCalledWith("三体", "all", expect.any(AbortSignal))
  })

  it("keeps native results distinct from browser example data", async () => {
    mocks.mode = "tauri"
    await renderPage()
    expect(screen.getByRole("button", { name: "查看作品：三体" })).toBeTruthy()
    expect(screen.queryByText("浏览器预览 · 示例数据")).toBeNull()
  })

  it("does not enable either gateway in an unavailable production browser", async () => {
    mocks.mode = "unavailable"
    await renderPage()
    expect(screen.getByText("搜索服务未启用")).toBeTruthy()
    expect(mocks.searchLocalLibrary).not.toHaveBeenCalled()
    expect(mocks.startSourceSearch).not.toHaveBeenCalled()
    expect(screen.queryByRole("region", { name: "本地媒体库" })).toBeNull()
  })

  it("preserves the empty-query search entry and hot boards", async () => {
    const { container } = await renderPage("/search")
    expect(screen.getByRole("heading", { name: "热门排行" })).toBeTruthy()
    expect(container.querySelector(".search-results-page")).toBeNull()
    expect(mocks.searchLocalLibrary).not.toHaveBeenCalled()
    expect(mocks.startSourceSearch).not.toHaveBeenCalled()
  })

  it("passes category selection to both searches and preserves the URL query", async () => {
    mocks.searchLocalLibrary.mockImplementation(async (_query, category) => localItems.filter((item) => category === "all" || item.category === category))
    await renderPage()
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "图书" })) })
    expect(screen.getByRole("button", { name: "图书" }).getAttribute("aria-pressed")).toBe("true")
    expect(mocks.searchLocalLibrary).toHaveBeenLastCalledWith("三体", "book", expect.any(AbortSignal))
    expect(mocks.startSourceSearch).toHaveBeenLastCalledWith("三体", expect.any(Function), "book")
    expect(screen.getByTestId("location").textContent).toContain("category=book")
    expect(screen.queryByRole("button", { name: "查看作品：三体剧集" })).toBeNull()
  })

  it("cycles only local ordering without restarting source searches", async () => {
    await renderPage()
    const buttons = () => within(screen.getByRole("region", { name: "本地媒体库" })).getAllByRole("button")
    expect(buttons()[0].getAttribute("aria-label")).toBe("查看作品：三体")
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "本地排序：相关度，点击切换" })) })
    expect(buttons()[0].getAttribute("aria-label")).toBe("查看作品：三体剧集")
    expect(screen.getByTestId("location").textContent).toContain("sort=year")
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "本地排序：年份最新，点击切换" })) })
    expect(buttons()[0].getAttribute("aria-label")).toBe("查看作品：三体")
    expect(mocks.startSourceSearch).toHaveBeenCalledOnce()
  })

  it("opens a local work using the returned work ID", async () => {
    await renderPage()
    fireEvent.click(screen.getByRole("button", { name: "查看作品：三体" }))
    expect(screen.getByTestId("location").textContent).toBe("/work/work-book")
  })

  it("keeps local loading and empty states within their section", async () => {
    let resolve!: (value: LocalSearchResult[]) => void
    mocks.searchLocalLibrary.mockImplementation(() => new Promise<LocalSearchResult[]>((done) => { resolve = done }))
    await renderPage()
    expect(screen.getByRole("region", { name: "本地媒体库" }).getAttribute("aria-busy")).toBe("true")
    expect(screen.getByRole("status", { name: "正在加载搜索结果" })).toBeTruthy()
    await act(async () => { resolve([]) })
    expect(screen.getByText("没有找到相关作品")).toBeTruthy()
    expect(screen.getByRole("region", { name: "本地媒体库" }).getAttribute("aria-busy")).toBe("false")
  })

  it("preserves retry after a local search error", async () => {
    mocks.searchLocalLibrary.mockRejectedValueOnce(new Error("offline"))
    await renderPage()
    expect(screen.getByText("无法加载搜索结果")).toBeTruthy()
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "重试" })) })
    expect(screen.getByRole("button", { name: "查看作品：三体" })).toBeTruthy()
    expect(mocks.searchLocalLibrary).toHaveBeenCalledTimes(2)
  })

  it("keeps completed source results completed even if events arrive before the start promise resolves", async () => {
    await renderPage()
    expect(screen.getByText("来源中暂无匹配结果")).toBeTruthy()
    expect(screen.queryByText("正在查询已启用来源…")).toBeNull()
    expect(screen.getByRole("region", { name: "来源结果" }).getAttribute("aria-busy")).toBe("false")
  })

  it("renders a candidate's actual source and imports through the existing operation/index", async () => {
    mocks.startSourceSearch.mockImplementation(async (_query, onEvent) => {
      onEvent(event("source_result", [candidate()], "opds_gutenberg"))
      onEvent(event("completed"))
      return { operationId: "op-test", taskId: "task-test", alreadyRunning: false }
    })
    await renderPage()
    expect(screen.getByRole("heading", { level: 3, name: "来源里的三体" })).toBeTruthy()
    expect(screen.getByText("古腾堡计划（OPDS）")).toBeTruthy()
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "导入媒体库：来源里的三体" })) })
    expect(mocks.importSourceWork).toHaveBeenCalledWith({ operationId: "op-test", index: 0 })
    expect(screen.getByTestId("location").textContent).toBe("/work/imported-work")
  })

  it("does not add an import action for search-only candidates", async () => {
    mocks.startSourceSearch.mockImplementation(async (_query, onEvent) => {
      onEvent(event("source_result", [candidate("opds-candidate-custom-example")], "custom-example"))
      onEvent(event("completed"))
      return { operationId: "op-test", taskId: "task-test", alreadyRunning: false }
    })
    await renderPage()
    expect(screen.getByText("仅搜索")).toBeTruthy()
    expect(screen.queryByRole("button", { name: /^导入媒体库/ })).toBeNull()
  })

  it("keeps warning details collapsed and independent of usable local results", async () => {
    mocks.startSourceSearch.mockImplementation(async (_query, onEvent) => {
      onEvent({ ...event("warning"), data: { sourceId: "tvmaze", works: [], code: "SOURCE_UNAVAILABLE", message: "目录服务暂时不可用" } })
      onEvent(event("completed"))
      return { operationId: "op-test", taskId: "task-test", alreadyRunning: false }
    })
    await renderPage()
    const sourceRegion = within(screen.getByRole("region", { name: "来源结果" }))
    expect(sourceRegion.queryByRole("list")).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: /查看明细/ }))
    expect(sourceRegion.getByRole("listitem").textContent).toContain("目录服务暂时不可用")
    expect(sourceRegion.getByRole("listitem").textContent).toContain("TVMaze")
    expect(sourceRegion.getByRole("listitem").textContent).toContain("SOURCE_UNAVAILABLE")
    expect(screen.getByRole("button", { name: /收起明细/ }).getAttribute("aria-expanded")).toBe("true")
    expect(screen.getByRole("button", { name: "查看作品：三体" })).toBeTruthy()
  })

  it("retains local results when a source request fails", async () => {
    mocks.startSourceSearch.mockRejectedValueOnce(new Error("offline"))
    await renderPage()
    expect(within(screen.getByRole("region", { name: "来源结果" })).getByRole("alert")).toBeTruthy()
    expect(screen.getByRole("button", { name: "查看作品：三体" })).toBeTruthy()
  })

  it("dismisses a source failure notice without hiding candidates already returned", async () => {
    mocks.startSourceSearch.mockImplementation(async (_query, onEvent) => {
      onEvent(event("source_result", [candidate()], "opds_gutenberg"))
      onEvent(event("failed"))
      return { operationId: "op-test", taskId: "task-test", alreadyRunning: false }
    })
    await renderPage()
    const sourceRegion = within(screen.getByRole("region", { name: "来源结果" }))
    expect(sourceRegion.getByRole("alert")).toBeTruthy()
    fireEvent.click(sourceRegion.getByRole("button", { name: "知道了" }))
    expect(sourceRegion.queryByRole("alert")).toBeNull()
    expect(sourceRegion.getByRole("heading", { name: "来源里的三体" })).toBeTruthy()
    expect(sourceRegion.getByRole("button", { name: "导入媒体库：来源里的三体" })).toBeTruthy()
  })

  it("clears the submitted query and returns to the existing search entry", async () => {
    await renderPage()
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "清空搜索" })) })
    expect(screen.getByTestId("location").textContent).toBe("/search")
    expect(screen.queryByRole("region", { name: "本地媒体库" })).toBeNull()
    expect(mocks.cancelSourceSearch).toHaveBeenCalledWith("op-test")
  })
})
