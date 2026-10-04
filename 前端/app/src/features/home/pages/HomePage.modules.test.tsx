// @vitest-environment jsdom

import { cleanup, render, screen, waitFor } from "@testing-library/react"
import { MemoryRouter } from "react-router"
import { afterEach, describe, expect, it, vi } from "vitest"
import type { HomeDto, HomeLayoutDto, WorkCardDto } from "@/lib/ipc/generated/wire"
import { defaultHomeLayout, guardHomeLayout } from "@/lib/ipc/settings-wire"
import { HavenError } from "@/lib/ipc/errors"

// mascot 与首页模块区无关：它在 jsdom 里会拉起一整套动画运行时，这里只关心模块区。
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

function layout(modules: HomeLayoutDto["modules"]): HomeLayoutDto {
  const value: HomeLayoutDto = { schemaVersion: 1, modules }
  if (!guardHomeLayout(value)) throw new Error("测试布局必须合法")
  return value
}

function renderHome() {
  return render(
    <MemoryRouter initialEntries={["/"]}>
      <HomePage />
    </MemoryRouter>,
  )
}

function renderedModules(container: HTMLElement): Array<[string | null, string | null]> {
  return [...container.querySelectorAll("[data-module]")].map((element) => [
    element.getAttribute("data-module"),
    element.getAttribute("data-module-size"),
  ])
}

function renderedModulePositions(container: HTMLElement): Array<[string | null, string | null, string | null]> {
  return [...container.querySelectorAll("[data-module]")].map((element) => [
    element.getAttribute("data-module"),
    element.getAttribute("data-module-row"),
    element.getAttribute("data-module-column"),
  ])
}

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe("首页模块区（真实布局 + 投影）", () => {
  it("renders the saved modules in saved order and size", async () => {
    homeGetProjection.mockResolvedValue(homeProjection())
    homeLayoutGet.mockResolvedValue({
      layout: layout([
        { module: "shelf-favorites", size: "small", row: 0, column: 2, order: 1 },
        { module: "recently_added", size: "medium", row: 0, column: 0, order: 0 },
      ]),
      revision: "appearance-1",
    })

    const { container } = renderHome()
    // 加载态先出现，不是一片空白。
    expect(screen.getByText("正在读取首页内容…")).toBeTruthy()

    await waitFor(() => expect(renderedModules(container)).toHaveLength(2))
    expect(renderedModules(container)).toEqual([
      ["recently_added", "medium"],
      ["shelf-favorites", "small"],
    ])
    expect(renderedModulePositions(container)).toEqual([
      ["recently_added", "0", "0"],
      ["shelf-favorites", "0", "2"],
    ])
    expect(screen.getByText("海伯利安")).toBeTruthy()
  })

  it("keeps a hidden module hidden and honours an explicit empty layout", async () => {
    homeGetProjection.mockResolvedValue(homeProjection())
    homeLayoutGet.mockResolvedValue({
      layout: layout([{ module: "recently_added", size: "medium", row: 0, column: 0, order: 0 }]),
      revision: "appearance-2",
    })

    const { container } = renderHome()
    await waitFor(() => expect(renderedModules(container)).toHaveLength(1))
    expect(renderedModules(container)).toEqual([["recently_added", "medium"]])
    // 用户没有选择的模块不会因为「投影里有内容」而被塞回来。
    expect(screen.queryByText("海伯利安")).toBeNull()
    expect(screen.queryByText("暂无内容")).toBeNull()
  })

  it("renders nothing at all for a saved empty layout", async () => {
    homeGetProjection.mockResolvedValue(homeProjection())
    homeLayoutGet.mockResolvedValue({ layout: layout([]), revision: "appearance-3" })

    const { container } = renderHome()
    await waitFor(() => expect(screen.queryByText("正在读取首页内容…")).toBeNull())
    expect(renderedModules(container)).toEqual([])
    // 显式空布局是用户的选择，不是「没有数据」。
    expect(screen.queryByText("暂无内容")).toBeNull()
  })

  it("shows an explicit empty state when the projection has nothing to show", async () => {
    homeGetProjection.mockResolvedValue({ schemaVersion: 1, continueItems: [], recentlyAdded: [], shelves: [] })
    homeLayoutGet.mockResolvedValue({ layout: defaultHomeLayout(), revision: null })

    const { container } = renderHome()
    await waitFor(() => expect(screen.getByText("暂无内容")).toBeTruthy())
    expect(renderedModules(container)).toEqual([])
  })

  it("surfaces a load failure with a retry that really re-reads", async () => {
    homeGetProjection.mockRejectedValue(
      new HavenError({ code: "DATABASE_ERROR", userMessage: "首页读取失败", retryable: true }),
    )
    homeLayoutGet.mockResolvedValue({ layout: defaultHomeLayout(), revision: null })

    const { container } = renderHome()
    await waitFor(() => expect(screen.getByRole("alert")).toBeTruthy())
    expect(screen.getByRole("alert").textContent).toContain("首页读取失败")

    homeGetProjection.mockResolvedValue(homeProjection())
    homeLayoutGet.mockResolvedValue({
      layout: layout([{ module: "recently_added", size: "medium", row: 0, column: 0, order: 0 }]),
      revision: "appearance-4",
    })
    screen.getByRole("button", { name: "重试" }).click()
    await waitFor(() => expect(renderedModules(container)).toHaveLength(1))
    expect(renderedModules(container)).toEqual([["recently_added", "medium"]])
  })
})
