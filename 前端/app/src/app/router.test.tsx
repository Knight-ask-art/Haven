// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, render, waitFor } from "@testing-library/react"
import { createMemoryRouter, RouterProvider } from "react-router"
import type { RouteObject } from "react-router"
import type {
  AppearanceAssetsWire,
  AppearanceSettingsValue,
  GeneralSettingsValue,
} from "@/lib/ipc/settings-wire"
import type { InterfaceFontAsset } from "@/lib/ipc/interface-font-wire"
import { CUSTOM_FONT_VARIABLE } from "@/features/settings/lib/appearance-runtime"
import { defaultAppTheme } from "@/features/settings/lib/appearance-palette"
import { PeriodicalTreePage } from "@/features/periodical/pages/PeriodicalTreePage"
import { AppRoot } from "./layouts/AppRoot"
import { AppShell } from "./layouts/AppShell"
import { router } from "./router"

// 这里只断言路由表与根布局的投影行为。阅读器页面会拉入 pdfjs / hls.js 这类只在浏览器
// 里成立的重依赖，替换成空组件可以让断言与它们的实现细节解耦；断言的是路径、元素类型
// 与 documentElement 上的投影，不是这些页面渲染出了什么。
vi.mock("@/features/reader/pages/ArticleReaderPage", () => ({ ArticleReaderPage: () => null }))
vi.mock("@/features/reader/pages/BookReaderPage", () => ({ BookReaderPage: () => null }))
vi.mock("@/features/comic/pages/ComicReaderPage", () => ({ ComicReaderPage: () => null }))
vi.mock("@/features/player/pages/PlayerPage", () => ({ PlayerPage: () => null }))
// 媒体库页会自己去读 IPC 数据；这里只借它验证「非首页的壳层路由仍渲染浮动 Dock」。
vi.mock("@/features/library/pages/LibraryPage", () => ({ LibraryPage: () => null }))
// 首页挂载后会自己起 rAF 循环（见 AppShell.wallpaper.test.tsx 的说明），这里只关心
// 「根布局的投影有没有被导航带走」，用空组件替掉。
vi.mock("@/features/home/pages/HomePage", () => ({ HomePage: () => null }))

const FONT_ASSET_ID = "0196f0d2-0000-7000-8000-00000000f001"
const ACCENT_COLOR = "#ff2d55"

/** 一份已通过后端校验的字体资产；投影只认 `validated` 的资产。 */
const FONT_ASSETS: AppearanceAssetsWire = {
  schemaVersion: 1,
  assets: [
    {
      assetId: FONT_ASSET_ID,
      kind: "font",
      state: "validated",
      byteSize: 2048,
      displayName: null,
    },
  ],
}

const INTERFACE_FONT_ASSET: InterfaceFontAsset = {
  id: FONT_ASSET_ID,
  familyName: "HavenUiCustom",
  fileName: "HavenUiCustom.woff2",
  extension: "woff2",
  mimeType: "font/woff2",
  byteSize: 2048,
  createdAt: 1735689600000,
}

/** 设置运行时读到的外观：自定义调色板 + 强调色 + 一个已校验的界面字体资产。 */
function appearanceValue(): AppearanceSettingsValue {
  return {
    section: "appearance",
    theme: "custom",
    density: "comfortable",
    sidebar: "auto",
    reduceMotion: false,
    interfaceFontMode: "custom",
    interfaceFontAssetId: FONT_ASSET_ID,
    customTheme: { ...defaultAppTheme(), accentColor: ACCENT_COLOR },
    wallpaper: { kind: "none" },
    customFontAssetId: FONT_ASSET_ID,
  }
}

function generalValue(): GeneralSettingsValue {
  // 启动页停在首页且不恢复会话，首屏跳转因此不会介入这个用例。
  return {
    section: "general",
    launchPage: "home",
    restoreSession: false,
    language: "zh_cn",
    notifications: true,
  }
}

// 运行时状态与两个 gateway 都走真实形状的替身，不碰任何 IPC：本文件断言的是
// 「投影挂在哪个布局上」，不是数据通道本身（那由各自的单测钉住）。
vi.mock("@/lib/ipc/runtime", () => ({
  getHavenClientMode: () => "mock",
  getHavenClient: () => {
    throw new Error("根布局投影用例不读取 IPC 数据")
  },
  isTauriRuntime: () => false,
}))

vi.mock("@/features/settings/ipc/gateway", async (importOriginal) => {
  const original = await importOriginal<typeof import("@/features/settings/ipc/gateway")>()
  return {
    ...original,
    settingsGateway: {
      ...original.settingsGateway,
      settingsGet: async (section: string) =>
        section === "appearance"
          ? { value: appearanceValue(), revision: "appearance-1" }
          : { value: generalValue(), revision: "general-1" },
    },
  }
})

vi.mock("@/features/settings/ipc/appearance-gateway", async (importOriginal) => {
  const original = await importOriginal<typeof import("@/features/settings/ipc/appearance-gateway")>()
  return {
    ...original,
    // 受控协议地址的浏览器等价物：只由不透明资产 ID 生成，不引入任何路径或远端 URL。
    appearanceAssetResourceUri: (assetId: string) => `haven-resource://appearance/${assetId}`,
    appearanceGateway: {
      ...original.appearanceGateway,
      appearanceAssetsList: async () => FONT_ASSETS,
    },
  }
})

vi.mock("@/features/settings/ipc/interface-font-gateway", () => ({
  interfaceFontGateway: {
    assetList: async () => [INTERFACE_FONT_ASSET],
  },
}))

/**
 * jsdom 没有 FontFace / document.fonts，字体投影会整条安全跳过——那样这条用例就只能
 * 证明 token 还在，证明不了 `--haven-ui-font-family` 也在。这两个替身让字体投影真的
 * 走完一次（加载 + 写入变量），不改变投影本身的判定。
 */
function stubFontEnvironment(): void {
  class StubFontFace {
    family: string
    source: string
    constructor(family: string, source: string) {
      this.family = family
      this.source = source
    }
    async load(): Promise<unknown> {
      return this
    }
  }
  Object.defineProperty(globalThis, "FontFace", {
    configurable: true,
    writable: true,
    value: StubFontFace,
  })
  Object.defineProperty(document, "fonts", {
    configurable: true,
    value: {
      add: () => undefined,
      delete: () => true,
    },
  })
}

function removeFontEnvironment(): void {
  Reflect.deleteProperty(globalThis, "FontFace")
  Reflect.deleteProperty(document, "fonts")
}

afterEach(() => {
  cleanup()
  removeFontEnvironment()
})

/**
 * 路由表从已经导出的 `router` 实例读取（`router.routes`），断言的是同一份配置。
 * 这样 router.tsx 不必为了测试多导出一个非组件常量——那会给它多加一条
 * react-refresh/only-export-components 警告。
 *
 * 顶层是一个无路径的根布局（AppRoot），所有主要路由都是它的子节点：这个结构本身就是
 * 「外观运行时覆盖全部主要路由」的事实来源，因此这里的断言先钉住它，再钉住投影行为。
 */
function appRoutes(): RouteObject[] {
  return router.routes
}

function rootLayout(): RouteObject {
  const root = appRoutes()[0]
  expect(root).toBeTruthy()
  return root
}

/** 根布局的直接子路由（沉浸式路由与壳层都在这一层）。 */
function rootChildren(): RouteObject[] {
  return rootLayout().children ?? []
}

function shellRoute(): RouteObject | undefined {
  return rootChildren().find((route) => route.path === "/")
}

function shellChildren(): RouteObject[] {
  return shellRoute()?.children ?? []
}

function elementType(route: RouteObject | undefined): unknown {
  return (route?.element as { type?: unknown } | undefined)?.type
}

describe("app router", () => {
  it("registers the AI workbench design preview inside the app shell", () => {
    const route = shellChildren().find((child) => child.path === "dev/ai-workbench-preview")
    expect(route).toBeTruthy()
    expect(route?.element).toBeTruthy()
  })

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

    const topLevelPaths = rootChildren().map((route) => route.path)
    for (const path of ["player/:mediaItemId", "reader/:mediaItemId", "comic/:mediaItemId", "article/:mediaItemId"]) {
      expect(topLevelPaths).toContain(path)
    }
  })

  it("does not shadow the article reader route with the periodical browser", () => {
    // 期刊浏览入口导航到 /article/:mediaItemId，因此文章路由必须仍在顶层且独立于 AppShell。
    const article = rootChildren().find((route) => route.path === "article/:mediaItemId")
    expect(article).toBeTruthy()
    expect(shellChildren().some((child) => child.path === "article/:mediaItemId")).toBe(false)
  })

  it("mounts the appearance runtime on a pathless root layout above every main route", () => {
    const root = rootLayout()
    // 无路径布局：URL 不变，沉浸式路由也照旧不套 AppShell。
    expect(root.path).toBeUndefined()
    expect(elementType(root)).toBe(AppRoot)
    const paths = rootChildren().map((route) => route.path)
    expect(paths).toContain("/")
    for (const path of ["player/:mediaItemId", "reader/:mediaItemId", "comic/:mediaItemId", "article/:mediaItemId"]) {
      expect(paths, `${path} 必须挂在根布局之下，否则进入它就会清掉外观投影`).toContain(path)
    }
    // 壳层本身没有搬家：它仍是 "/" 上的 AppShell，导航、壁纸与壳层行为都留在原处。
    expect(elementType(shellRoute())).toBe(AppShell)
  })
})

describe("appearance projection across immersive routes", () => {
  /** 当前 documentElement 上的两处投影事实：强调色 token 与界面字体变量。 */
  function projection(): { accent: string; font: string } {
    const style = document.documentElement.style
    return {
      accent: style.getPropertyValue("--primary"),
      font: style.getPropertyValue(CUSTOM_FONT_VARIABLE),
    }
  }

  /** 壳层根元素的探针：AppShell 的 data 属性只在壳层里出现。 */
  function shellRoot(container: HTMLElement): Element | null {
    return container.querySelector("[data-haven-wallpaper]")
  }

  it("keeps the custom tokens and UI font after navigating into reader / player / comic / article", async () => {
    stubFontEnvironment()
    const memoryRouter = createMemoryRouter(appRoutes(), { initialEntries: ["/"] })
    const { container } = render(<RouterProvider router={memoryRouter} />)

    // 首屏：设置读盘落地后，自定义主题与字体被投影到 documentElement。
    await waitFor(() => expect(projection().accent).toBe(ACCENT_COLOR))
    expect(projection().font).toContain("HavenImportedFont")
    expect(document.documentElement.dataset.havenTheme).toBe("custom")
    // 首页仍然正常渲染：壳层在 "/" 上照常挂载，投影没有把页面顶掉。
    expect(shellRoot(container)).not.toBeNull()

    // 四个沉浸式路由都是 AppShell 的兄弟：它们不套壳层，但投影必须还在。
    for (const path of ["/reader/book-1", "/player/video-1", "/comic/comic-1", "/article/article-1"]) {
      await act(async () => {
        await memoryRouter.navigate(path)
      })
      expect(shellRoot(container), `${path} 不该套壳层（沉浸式路由是壳层的兄弟）`).toBeNull()
      expect(projection().accent, `${path} 丢掉了自定义强调色 token`).toBe(ACCENT_COLOR)
      expect(projection().font, `${path} 丢掉了自定义界面字体变量`).toContain("HavenImportedFont")
      expect(document.documentElement.dataset.havenTheme, `${path} 丢掉了明暗状态`).toBe("custom")
    }

    // AI 工作台保持全屏画布，内部会话面板负责避开 Dock；Dock 本身仍只由 AppShell
    // 全局渲染。普通页面则继续在 main 上统一预留空间。
    await act(async () => {
      await memoryRouter.navigate("/dev/ai-workbench-preview")
    })
    await waitFor(() => {
      expect(container.querySelectorAll('nav[aria-label="全局浮动导航栏"]')).toHaveLength(1)
    })
    const workbenchDock = container.querySelector('nav[aria-label="全局浮动导航栏"]')
    expect(workbenchDock?.className).toContain("fixed")
    expect(container.querySelector("main.haven-scroll")?.className).not.toContain("pb-[96px]")

    // 回到壳层内：投影从头到尾只装过一次，往返不会把它清掉或装上第二份。
    // 普通非首页壳层路由仍使用视口级 Dock。
    await act(async () => {
      await memoryRouter.navigate("/library")
    })
    expect(shellRoot(container)).not.toBeNull()
    expect(container.querySelector('nav[aria-label="全局浮动导航栏"]')).not.toBeNull()
    expect(container.querySelector('nav[aria-label="全局浮动导航栏"]')?.className).toContain("fixed")
    expect(container.querySelector("main.haven-scroll")?.className).toContain("pb-[96px]")
    expect(projection().accent).toBe(ACCENT_COLOR)
    expect(projection().font).toContain("HavenImportedFont")

    // 两端接上：Dock 的活跃态用的是 `text-primary`，而 `--primary` 正是上面那份自定义
    // 强调色，所以自定义配色真的会反映到底部导航的活跃项上。
    const activeDockItem = container.querySelector('nav[aria-label="全局浮动导航栏"] a[aria-current="page"]')
    expect(activeDockItem).not.toBeNull()
    expect(activeDockItem?.className).toContain("text-primary")
  })

  it("unmounting the whole app does restore the default theme", async () => {
    // 投影的清理契约没有变：只有根布局自己卸载（整个应用退出）时才回退默认主题，
    // 导航到别的路由不是卸载。
    stubFontEnvironment()
    const memoryRouter = createMemoryRouter(appRoutes(), { initialEntries: ["/"] })
    const view = render(<RouterProvider router={memoryRouter} />)
    await waitFor(() => expect(projection().accent).toBe(ACCENT_COLOR))

    view.unmount()
    expect(projection().accent).toBe("")
    expect(projection().font).toBe("")
  })
})
