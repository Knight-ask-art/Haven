// @vitest-environment jsdom

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import { MemoryRouter } from "react-router"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { SearchSourceEvent, WorkCardDto } from "@/lib/ipc/generated/wire"
import type { startSourceSearch as StartSourceSearch } from "../ipc/source-search-gateway"
import { SourceCandidates } from "./SourceCandidates"

const mocks = vi.hoisted(() => ({
  startSourceSearch: vi.fn<typeof StartSourceSearch>(),
  cancelSourceSearch: vi.fn(),
  importSourceWork: vi.fn(),
}))

vi.mock("@/lib/ipc/runtime", () => ({ isTauriRuntime: () => false }))
vi.mock("../ipc/source-search-gateway", () => ({
  startSourceSearch: mocks.startSourceSearch,
  cancelSourceSearch: mocks.cancelSourceSearch,
  importSourceWork: mocks.importSourceWork,
}))

function card(posterUri: string | null): WorkCardDto {
  return {
    workId: "content-candidate-mangadex-demo",
    title: "有真实封面的漫画",
    originalTitle: null,
    description: "来源返回的作品说明",
    categories: ["comic"],
    availableMediaTypes: ["comic"],
    posterUri,
    backdropUri: null,
    releaseYear: 2026,
    ratingValue: null,
    ratingScale: null,
    favorite: false,
    progress: null,
    primaryAction: null,
    externalIds: [],
  }
}

function event(kind: SearchSourceEvent["kind"], works: WorkCardDto[] = []): SearchSourceEvent {
  return {
    operationId: "artwork-search",
    sequence: kind === "source_result" ? 1 : 2,
    at: "2026-10-08T00:00:00Z",
    kind,
    data: { sourceId: "mangadex", works, code: null, message: null },
  }
}

async function renderCandidate(work: WorkCardDto) {
  mocks.startSourceSearch.mockImplementation(async (_query, onEvent) => {
    onEvent(event("source_result", [work]))
    onEvent(event("completed"))
    return { operationId: "artwork-search", taskId: "artwork-task", alreadyRunning: false }
  })
  await act(async () => {
    render(<MemoryRouter><SourceCandidates query="漫画" category="comic" /></MemoryRouter>)
  })
  return screen.getByRole("img", { name: work.title })
}

beforeEach(() => {
  mocks.startSourceSearch.mockReset()
  mocks.cancelSourceSearch.mockReset().mockResolvedValue(undefined)
})
afterEach(cleanup)

describe("来源搜索的真实封面", () => {
  it("实际渲染后端登记的受控封面，不停留在来源图标或默认图", async () => {
    const image = await renderCandidate(card("haven://artwork/registered-cover"))
    expect(image.getAttribute("src")).toBe("haven-resource://artwork/registered-cover")
    expect(image.parentElement?.getAttribute("data-artwork-source")).toBe("controlled")
    expect(screen.getByText("来源返回的作品说明")).toBeTruthy()
  })

  it("来源没有图片时才使用对应分类的本地默认封面", async () => {
    const image = await renderCandidate(card(null))
    expect(image.getAttribute("src")).toMatch(/^\/默认图片\/漫画\//)
    expect(image.parentElement?.getAttribute("data-artwork-source")).toBe("default")
  })

  it("受控图片加载失败后回退，结果文字和导入操作仍保留", async () => {
    const image = await renderCandidate(card("haven://artwork/failed-cover"))
    fireEvent.error(image)
    expect(image.getAttribute("src")).toMatch(/^\/默认图片\/漫画\//)
    expect(screen.getByRole("button", { name: "导入媒体库：有真实封面的漫画" })).toBeTruthy()
  })

  it("不让来源绕过受控图片系统在 WebView 直接加载远程地址", async () => {
    const image = await renderCandidate(card("https://images.example.org/cover.jpg?signature=fixture-secret"))
    expect(image.getAttribute("src")).toMatch(/^\/默认图片\/漫画\//)
    expect(document.body.innerHTML).not.toContain("fixture-secret")
    expect(document.body.innerHTML).not.toContain("images.example.org")
  })

  it("没有海报但提供受控背景图时可以正常显示", async () => {
    const work = { ...card(null), backdropUri: "haven://artwork/registered-backdrop" }
    const image = await renderCandidate(work)
    expect(image.getAttribute("src")).toBe("haven-resource://artwork/registered-backdrop")
  })
})
