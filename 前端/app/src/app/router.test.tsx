// @vitest-environment jsdom

import { describe, expect, it, vi } from "vitest"
import type { RouteObject } from "react-router"
import { PeriodicalTreePage } from "@/features/periodical/pages/PeriodicalTreePage"
import { router } from "./router"

// 这里只断言路由表本身。阅读器页面会拉入 pdfjs / hls.js 这类只在浏览器里成立的
// 重依赖，替换成空组件可以让路由断言与它们的实现细节解耦；断言的是路径与元素类型，
// 不是这些页面渲染出了什么。
vi.mock("@/features/reader/pages/ArticleReaderPage", () => ({ ArticleReaderPage: () => null }))
vi.mock("@/features/reader/pages/BookReaderPage", () => ({ BookReaderPage: () => null }))
vi.mock("@/features/comic/pages/ComicReaderPage", () => ({ ComicReaderPage: () => null }))
vi.mock("@/features/player/pages/PlayerPage", () => ({ PlayerPage: () => null }))

/**
 * 路由表从已经导出的 `router` 实例读取（`router.routes`），断言的是同一份配置。
 * 这样 router.tsx 不必为了测试多导出一个非组件常量——那会给它多加一条
 * react-refresh/only-export-components 警告。
 */
function appRoutes(): RouteObject[] {
  return router.routes
}

function shellChildren(): RouteObject[] {
  const shell = appRoutes().find((route) => route.path === "/")
  expect(shell).toBeTruthy()
  return shell?.children ?? []
}

function elementType(route: RouteObject | undefined): unknown {
  return (route?.element as { type?: unknown } | undefined)?.type
}

describe("app router", () => {
  it("registers the periodical tree browser as a real shell child", () => {
    const route = shellChildren().find((child) => child.path === "periodical/:workId")
    expect(route).toBeTruthy()
    expect(elementType(route)).toBe(PeriodicalTreePage)
  })

  it("keeps the existing work, edition and reader routes intact", () => {
    const childPaths = shellChildren().map((child) => child.path)
    for (const path of [
      "library",
      "library/browse/:category",
      "work/:workId",
      "edition/:editionId",
      "search",
      "downloads",
      "settings/:section?",
    ]) {
      expect(childPaths).toContain(path)
    }

    const topLevelPaths = appRoutes().map((route) => route.path)
    for (const path of ["player/:mediaItemId", "reader/:mediaItemId", "comic/:mediaItemId", "article/:mediaItemId"]) {
      expect(topLevelPaths).toContain(path)
    }
  })

  it("does not shadow the article reader route with the periodical browser", () => {
    // 期刊浏览入口导航到 /article/:mediaItemId，因此文章路由必须仍在顶层且独立于 AppShell。
    const article = appRoutes().find((route) => route.path === "article/:mediaItemId")
    expect(article).toBeTruthy()
    expect(shellChildren().some((child) => child.path === "article/:mediaItemId")).toBe(false)
  })
})
