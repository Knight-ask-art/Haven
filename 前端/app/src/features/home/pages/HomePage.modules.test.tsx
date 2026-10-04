// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react"
import { MemoryRouter } from "react-router"
import { afterEach, describe, expect, it, vi } from "vitest"
import type { HomeDto, WorkCardDto } from "@/lib/ipc/generated/wire"
import { defaultHomeLayout } from "@/lib/ipc/settings-wire"

// mascot 在 jsdom 里会拉起动画运行时；这里验证欢迎区下方不再渲染或请求内容模块。
vi.mock("../components/mascot", () => ({ HavenMascot: () => null }))

vi.mock("../ipc/home-gateway", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc/home-gateway")>()
  return { ...actual, getHomeProjection: vi.fn() }
})

vi.mock("@/features/settings/ipc/appearance-gateway", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/features/settings/ipc/appearance-gateway")>()
  return { ...actual, appearanceGateway: { ...actual.appearanceGateway, homeLayoutGet: vi.fn() } }
})

const { getHomeProjection } = await import("../ipc/home-gateway")
const { appearanceGateway } = await import("@/features/settings/ipc/appearance-gateway")
const { HomePage } = await import("./HomePage")

const homeGetProjection = vi.mocked(getHomeProjection)
const homeLayoutGet = vi.mocked(appearanceGateway.homeLayoutGet)

function workCard(workId: string, title: string): WorkCardDto {
  return {
    workId,
    title,
    originalTitle: null,
    description: null,
    categories: ["video"],
    availableMediaTypes: ["movie"],
    posterUri: null,
    backdropUri: null,
    releaseYear: 2024,
    ratingValue: null,
    ratingScale: null,
    favorite: false,
    progress: null,
    primaryAction: null,
    externalIds: [],
  }
}

function homeProjection(): HomeDto {
  return {
    schemaVersion: 1,
    continueItems: [],
    recentlyAdded: [workCard("0196f0d2-0000-7000-8000-000000000001", "沙丘2")],
    shelves: [
      {
        shelfId: "shelf-favorites",
        titleKey: "shelf.favorites",
        preview: [workCard("0196f0d2-0000-7000-8000-000000000002", "海伯利安")],
        viewMore: null,
      },
    ],
  }
}

function renderHome() {
  return render(
    <MemoryRouter initialEntries={["/"]}>
      <HomePage />
    </MemoryRouter>,
  )
}

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe("首页欢迎区与留白", () => {
  it.each(["empty", "populated"] as const)("does not render or fetch modules for a %s library", (library) => {
    homeGetProjection.mockResolvedValue(library === "populated"
      ? homeProjection()
      : { schemaVersion: 1, continueItems: [], recentlyAdded: [], shelves: [] })
    homeLayoutGet.mockResolvedValue({ layout: defaultHomeLayout(), revision: "saved-layout" })

    const { container } = renderHome()
    expect(container.querySelector("[data-module]")).toBeNull()
    expect(screen.queryByRole("region", { name: "首页模块" })).toBeNull()
    expect(screen.queryByText("暂无内容")).toBeNull()
    expect(screen.queryByText("正在读取首页内容…")).toBeNull()
    expect(screen.queryByText("沙丘2")).toBeNull()
    expect(screen.queryByText("海伯利安")).toBeNull()
    expect(homeGetProjection).not.toHaveBeenCalled()
    expect(homeLayoutGet).not.toHaveBeenCalled()
    expect(screen.getByRole("link", { name: "进入媒体库" })).toBeTruthy()
    expect(screen.getByRole("link", { name: "搜索一部作品" })).toBeTruthy()
  })

  it("keeps the complete motto in one separately labelled block", () => {
    renderHome()
    const note = screen.getByRole("complementary", { name: "首页寄语" })
    expect(note.textContent).toContain("KEEP WHAT MATTERS.")
    expect(note.textContent).toContain("RETURN WHENEVER YOU WANT.")
    expect(note.textContent).toContain("栖阅 / 2026")
    expect(screen.getAllByText("RETURN WHENEVER YOU WANT.")).toHaveLength(1)
  })
})
