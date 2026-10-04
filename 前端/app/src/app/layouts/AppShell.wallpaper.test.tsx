// @vitest-environment jsdom

import { afterEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react"
import { renderToStaticMarkup } from "react-dom/server"
import { MemoryRouter } from "react-router"
import type {
  AppearanceAssetsWire,
  AppearanceSettingsValue,
  GeneralSettingsValue,
} from "@/lib/ipc/settings-wire"
import {
  currentHomeWallpaperFailure,
  HOME_WALLPAPER_SURFACE_CLASS,
  requestWallpaperRetry,
} from "@/features/settings/lib/appearance-runtime"
import {
  captureSettingsRuntimeEpochs,
  getSettingsRuntimeSnapshot,
  publishLoadedSettingsRuntime,
  publishSettingsRuntimeSnapshot,
  publishSettingsRuntimeValue,
  type SettingsRuntimeSnapshot,
} from "@/features/settings/lib/settings-runtime-state"
import { HomePage } from "@/features/home/pages/HomePage"
import { AppShell, AppearanceWallpaperLayer } from "./AppShell"

const FIRST_ASSET_ID = "0196f0d2-0000-7000-8000-00000000a001"
const SECOND_ASSET_ID = "0196f0d2-0000-7000-8000-00000000a002"
const CONTROLLED_URI = `haven-resource://appearance/${FIRST_ASSET_ID}`
const NEXT_CONTROLLED_URI = `haven-resource://appearance/${SECOND_ASSET_ID}`

/** 两份都已通过后端校验的静帧壁纸：换资产就是换受控 URI。 */
const WALLPAPER_ASSETS: AppearanceAssetsWire = {
  schemaVersion: 1,
  assets: [FIRST_ASSET_ID, SECOND_ASSET_ID].map((assetId) => ({
    assetId,
    kind: "static_wallpaper" as const,
    state: "validated" as const,
    byteSize: 4096,
    displayName: null,
  })),
}

function appearanceValue(assetId: string): AppearanceSettingsValue {
  return {
    section: "appearance",
    theme: "system",
    density: "comfortable",
    sidebar: "auto",
    reduceMotion: false,
    interfaceFontMode: "system",
    customTheme: null,
    wallpaper: { kind: "static", assetId },
    customFontAssetId: null,
  }
}

/**
 * settings-runtime-state 的真实投影在模块级，改动它的用例跑完要放回原样；
 * 这里在模块求值时取一份原始快照，之后无论用例怎么改都能还原。
 */
const INITIAL_RUNTIME_SNAPSHOT = getSettingsRuntimeSnapshot()

/** AppShell 读到的设置投影；换壁纸就是换这里的那份值。 */
let runtimeSnapshot: SettingsRuntimeSnapshot = {
  general: INITIAL_RUNTIME_SNAPSHOT.general,
  appearance: appearanceValue(FIRST_ASSET_ID),
  interfaceFontAssets: INITIAL_RUNTIME_SNAPSHOT.interfaceFontAssets,
}

function selectWallpaper(assetId: string): void {
  runtimeSnapshot = { ...runtimeSnapshot, appearance: appearanceValue(assetId) }
}

/** 明确选择「无壁纸」：首页的默认外观，也是 fail-closed 的落点。 */
function selectNoWallpaper(): void {
  runtimeSnapshot = {
    ...runtimeSnapshot,
    appearance: { ...appearanceValue(FIRST_ASSET_ID), wallpaper: { kind: "none" as const } },
  }
}

// 只在测试运行时读取上面的夹具（模块体在 mock 工厂之后执行），因此不需要 vi.hoisted。
// 壳层现在只订阅运行时投影（读盘与投影都在根布局），所以这里替换的是
// useSettingsRuntimeSnapshot；AppRoot 的启动路径不在本文件的覆盖范围内。
vi.mock("@/features/settings/lib/settings-runtime-state", async (importOriginal) => {
  const original = await importOriginal<typeof import("@/features/settings/lib/settings-runtime-state")>()
  return {
    ...original,
    useSettingsRuntimeSnapshot: () => runtimeSnapshot,
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
      appearanceAssetsList: async () => WALLPAPER_ASSETS,
    },
  }
})

afterEach(() => {
  cleanup()
  requestWallpaperRetry()
})

function renderShell() {
  return render(
    <MemoryRouter initialEntries={["/"]}>
      <AppShell />
    </MemoryRouter>,
  )
}

function renderAt(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <AppShell />
    </MemoryRouter>,
  )
}

/** 重渲染成同一个路由：模拟「设置没变，只是又渲染了一次」。 */
function rerenderShell(rerender: ReturnType<typeof render>["rerender"], path = "/"): void {
  rerender(
    <MemoryRouter initialEntries={[path]}>
      <AppShell />
    </MemoryRouter>,
  )
}

/** shell 根元素：所有 data-haven-* 事实与图层类名都在它身上。 */
function shellRoot(container: HTMLElement): HTMLElement {
  const shell = container.firstElementChild
  if (!(shell instanceof HTMLElement)) throw new Error("AppShell 必须渲染出一个根元素")
  return shell
}

/** 直接子元素里的下标；用来断言「谁排在谁前面」，不依赖 compareDocumentPosition。 */
function childIndex(shell: HTMLElement, element: Element | null): number {
  if (!element) throw new Error("元素不存在，无法比较层级")
  return Array.from(shell.children).indexOf(element)
}

/** jsdom 默认没有 matchMedia：只回答减少动效查询，其余一律 false。 */
function stubReducedMotionPreference(matches: boolean): void {
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    writable: true,
    value: (query: string) => ({
      matches: query.includes("prefers-reduced-motion") ? matches : false,
      media: query,
      onchange: null,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
      addListener: () => undefined,
      removeListener: () => undefined,
      dispatchEvent: () => false,
    }),
  })
}

/** React 可能把布尔媒体属性写成 attribute 或 DOM property：两种形态都算「开着」。 */
function hasMediaFlag(element: Element, ...names: readonly string[]): boolean {
  return names.some(
    (name) =>
      element.hasAttribute(name) ||
      (element as unknown as Record<string, unknown>)[name] === true,
  )
}

describe("appearance wallpaper layer", () => {
  it("renders a static wallpaper as a decorative, non-interactive image", () => {
    const { container } = render(
      <AppearanceWallpaperLayer
        layer={{ kind: "static", uri: CONTROLLED_URI }}
        onUnavailable={() => undefined}
      />,
    )

    const image = container.querySelector("img")
    expect(image).not.toBeNull()
    expect(image?.getAttribute("src")).toBe(CONTROLLED_URI)
    expect(image?.getAttribute("aria-hidden")).toBe("true")
    expect(image?.getAttribute("alt")).toBe("")
    expect(image?.className).toContain("pointer-events-none")
    expect(image?.className).toContain("object-cover")
    expect(container.querySelector("video")).toBeNull()
  })

  it("renders a dynamic wallpaper as a muted, looping, non-interactive video", () => {
    const { container } = render(
      <AppearanceWallpaperLayer
        layer={{ kind: "dynamic", uri: CONTROLLED_URI }}
        onUnavailable={() => undefined}
      />,
    )

    const video = container.querySelector("video")
    if (!video) throw new Error("动态壁纸必须渲染成 video")
    expect(video.getAttribute("src")).toBe(CONTROLLED_URI)
    expect(video.getAttribute("aria-hidden")).toBe("true")
    // 静音 + 循环 + 内联播放是自动播放能被 WebView 接受的前提。
    expect(hasMediaFlag(video, "muted")).toBe(true)
    expect(hasMediaFlag(video, "loop")).toBe(true)
    expect(hasMediaFlag(video, "autoplay")).toBe(true)
    expect(hasMediaFlag(video, "playsinline", "playsInline")).toBe(true)
    // 壁纸层不吃指针事件，底部 Dock 与页面交互都不受影响。
    expect(video.className).toContain("pointer-events-none")
    expect(container.querySelector("img")).toBeNull()
  })

  it("reports unreadable bytes instead of keeping a dead background element", () => {
    const onUnavailable = vi.fn()
    const { container } = render(
      <AppearanceWallpaperLayer
        layer={{ kind: "static", uri: CONTROLLED_URI }}
        onUnavailable={onUnavailable}
      />,
    )

    const image = container.querySelector("img")
    if (!image) throw new Error("静帧壁纸必须渲染成 img")
    fireEvent.error(image)
    expect(onUnavailable).toHaveBeenCalledTimes(1)
  })
})

describe("app shell wallpaper fail-closed", () => {
  afterEach(() => selectWallpaper(FIRST_ASSET_ID))

  it("keeps the failure bound to the failed URI and brings the layer back for a new one", async () => {
    const { container, rerender } = renderShell()

    // 首页：受控 URI + 后端校验通过 → 真的渲染出背景层。
    await waitFor(() => expect(container.querySelector("img")).not.toBeNull())
    const failed = container.querySelector("img")
    if (!failed) throw new Error("首页必须渲染壁纸层")
    expect(failed.getAttribute("src")).toBe(CONTROLLED_URI)

    // 受控 URI 解析成功不等于字节读得到：AppShell 把这次投影收敛回「无壁纸」，
    // 而不是留一个永远加载不出来的背景元素。
    fireEvent.error(failed)
    await waitFor(() => expect(container.querySelector("img")).toBeNull())
    expect(currentHomeWallpaperFailure()?.uri).toBe(CONTROLLED_URI)
    expect(container.querySelector("video")).toBeNull()

    // 失败记在「哪一份受控 URI」上，而不是一个开关：设置没变、只是又渲染一次，
    // 读不出来的还是同一份字节，图层不该被带回来。
    rerenderShell(rerender)
    expect(container.querySelector("img")).toBeNull()

    // 绕着「无壁纸」转一圈再回到同一份 URI：仍然是一次新的选择、也仍然查得通过，
    // 但字节还是那份读不出来的字节，所以照样收敛。
    selectNoWallpaper()
    rerenderShell(rerender)
    expect(container.querySelector("img")).toBeNull()
    selectWallpaper(FIRST_ASSET_ID)
    rerenderShell(rerender)
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    expect(container.querySelector("img"), "同一份读不出来的 URI 不得当成新投影").toBeNull()

    // 换一份受控 URI 才是新投影：上一次的失败不跟着走。
    selectWallpaper(SECOND_ASSET_ID)
    rerenderShell(rerender)
    await waitFor(() =>
      expect(container.querySelector("img")?.getAttribute("src")).toBe(NEXT_CONTROLLED_URI),
    )
  })

  it("ignores a late error event from the previous retry generation", async () => {
    const { container } = renderShell()
    await waitFor(() => expect(container.querySelector("img")).not.toBeNull())

    const oldImage = container.querySelector("img")
    if (!oldImage) throw new Error("首页必须渲染壁纸层")
    fireEvent.error(oldImage)
    await waitFor(() => expect(container.querySelector("img")).toBeNull())
    const recordedFailure = currentHomeWallpaperFailure()

    act(() => requestWallpaperRetry())
    await waitFor(() => expect(container.querySelector("img")).not.toBeNull())
    fireEvent.error(oldImage)

    expect(container.querySelector("img")?.getAttribute("src")).toBe(CONTROLLED_URI)
    expect(currentHomeWallpaperFailure()).toBe(recordedFailure)
  })

  it("brings the same selection back on a real retry, without touching the saved setting", async () => {
    const { container } = renderShell()
    await waitFor(() => expect(container.querySelector("img")).not.toBeNull())

    const failed = container.querySelector("img")
    if (!failed) throw new Error("首页必须渲染壁纸层")
    fireEvent.error(failed)
    await waitFor(() => expect(container.querySelector("img")).toBeNull())

    // 已保存的外观值就是这一份对象：重试只是重新投影，不是一次设置写入。
    const savedAppearance = runtimeSnapshot.appearance

    // 设置页的「重试预览」按下的就是这个信号：同一份选择重新挂载、重新请求字节。
    act(() => {
      requestWallpaperRetry()
    })

    await waitFor(() => expect(container.querySelector("img")).not.toBeNull())
    const restored = container.querySelector("img")
    // 重试不接受地址：重新请求的还是那份受控 URI，没有 query、没有路径、没有第二份地址。
    expect(restored?.getAttribute("src")).toBe(CONTROLLED_URI)
    expect(restored?.getAttribute("src")).not.toContain("?")
    expect(restored?.getAttribute("src")).not.toContain("\\")
    expect(runtimeSnapshot.appearance, "重试不得改写已保存的外观设置").toBe(savedAppearance)
    // 图层真的回来了：首页面板重新让开背景，而不是停在「无壁纸」的收敛态。
    expect(shellRoot(container).getAttribute("data-haven-wallpaper")).toBe("true")
    expect(container.querySelector("video")).toBeNull()
  })

  it("stays converged when the retried bytes fail again, and reports no success", async () => {
    const { container } = renderShell()
    await waitFor(() => expect(container.querySelector("img")).not.toBeNull())

    const firstFailure = container.querySelector("img")
    if (!firstFailure) throw new Error("首页必须渲染壁纸层")
    fireEvent.error(firstFailure)

    act(() => {
      requestWallpaperRetry()
    })
    await waitFor(() => expect(container.querySelector("img")).not.toBeNull())

    // 第二次同样读不出来：重试没有制造成功，图层照旧收敛回「无壁纸」，
    // 不会留一个永远加载不出来的背景元素。
    const secondFailure = container.querySelector("img")
    if (!secondFailure) throw new Error("重试之后必须重新挂载壁纸层")
    fireEvent.error(secondFailure)
    await waitFor(() => expect(container.querySelector("img")).toBeNull())
    expect(container.querySelector("video")).toBeNull()
    expect(shellRoot(container).getAttribute("data-haven-wallpaper")).toBe("false")
  })

  it("keeps the floating transparent dock and no wallpaper off the home route", async () => {
    const { container } = render(
      <MemoryRouter initialEntries={["/library"]}>
        <AppShell />
      </MemoryRouter>,
    )

    // 原样的半透明浮动 Dock：壁纸层没把它换成不透明底，也没抢走指针事件。
    const nav = container.querySelector('nav[aria-label="全局浮动导航栏"]')
    expect(nav).not.toBeNull()
    expect(nav?.className).toContain("z-50")
    expect(nav?.className).toContain("fixed")
    expect(container.querySelector("nav .backdrop-blur-2xl")).not.toBeNull()

    // 壁纸是首页壁纸：其它路由不渲染背景层，即使设置里确实选了一张、并且它已经
    // 通过后端校验——等异步校验落地后再断言，避免「只是还没查完」造成的假通过。
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    expect(container.querySelector("img")).toBeNull()
    expect(container.querySelector("video")).toBeNull()

    // 没有渲染壁纸层，shell 也就不进层叠上下文：Dock 的 z-50 与半透明底都保持原样。
    const shell = shellRoot(container)
    expect(shell.getAttribute("data-haven-wallpaper")).toBe("false")
    expect(shell.className).not.toContain("isolate")
    expect(container.querySelector("main")?.className).toContain("pb-[96px]")
  })

  it("lets the settings canvas extend behind the fixed dock without an outer bottom inset", () => {
    const { container } = renderAt("/settings/appearance")
    const main = container.querySelector("main")
    const dock = container.querySelector('nav[aria-label="全局浮动导航栏"]')

    expect(main).not.toBeNull()
    expect(main?.className).not.toContain("pb-[96px]")
    expect(main?.className).not.toContain("pb-[80px]")
    expect(dock?.className).toContain("fixed")
    expect(dock?.className).toContain("z-50")
    expect(container.querySelector("nav .backdrop-blur-2xl")).not.toBeNull()
  })
})

describe("homepage wallpaper layer order", () => {
  afterEach(() => selectWallpaper(FIRST_ASSET_ID))

  it("puts the wallpaper under the home content, inside the shell's own stacking context", async () => {
    const { container } = renderShell()
    await waitFor(() => expect(container.querySelector("img")).not.toBeNull())

    const shell = shellRoot(container)
    const image = container.querySelector("img")
    const main = container.querySelector("main")

    // 显式的图层顺序：shell 只在渲染壁纸时成为层叠上下文（isolate），壁纸是其中唯一的
    // 负 z-index 层，因此永远排在内容之后；DOM 上它也排在 main 之前。
    expect(shell.className).toContain("isolate")
    expect(image?.className).toContain("-z-10")
    expect(shell.firstElementChild).toBe(image)
    expect(childIndex(shell, image)).toBeLessThan(childIndex(shell, main))

    // 媒体层不吃指针事件、不进无障碍树：内容与浮层 Dock 的交互不会被它截走。
    expect(image?.className).toContain("pointer-events-none")
    expect(image?.getAttribute("aria-hidden")).toBe("true")
  })

  it("leaves the shell stacking untouched when no wallpaper is selected", async () => {
    selectNoWallpaper()
    const { container } = renderAt("/")
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0))
    })

    const shell = shellRoot(container)
    expect(container.querySelector("img")).toBeNull()
    expect(container.querySelector("video")).toBeNull()
    // 无壁纸的默认外观：不进层叠上下文，也不给首页面板任何「让开背景」的信号。
    expect(shell.className).not.toContain("isolate")
    expect(shell.getAttribute("data-haven-wallpaper")).toBe("false")
  })

  it("hands the home surface over only while a wallpaper layer renders", async () => {
    // 契约的另一半在 index.css（[data-haven-wallpaper="true"] 时让开背景色）。
    // 这里钉住两端：AppShell 写出的属性取值，以及 HomePage 根元素确实带着那个类名。
    // HomePage 用静态渲染取标记：它的云朵吉祥物挂载后会自己起 rAF 循环，
    // 没必要为了这一个类名把整棵子树挂进 jsdom。
    const homeHtml = renderToStaticMarkup(
      <MemoryRouter initialEntries={["/"]}>
        <HomePage />
      </MemoryRouter>,
    )
    expect(homeHtml).toContain(HOME_WALLPAPER_SURFACE_CLASS)

    const withWallpaper = renderShell()
    await waitFor(() => expect(withWallpaper.container.querySelector("img")).not.toBeNull())
    expect(shellRoot(withWallpaper.container).getAttribute("data-haven-wallpaper")).toBe("true")

    // 选中的壁纸读不出来时同样是 "false"：首页回到不透明外观，不会露出空背景。
    const unreadable = withWallpaper.container.querySelector("img")
    if (!unreadable) throw new Error("首页必须渲染壁纸层")
    fireEvent.error(unreadable)
    await waitFor(() => expect(withWallpaper.container.querySelector("img")).toBeNull())
    expect(shellRoot(withWallpaper.container).getAttribute("data-haven-wallpaper")).toBe("false")

    selectNoWallpaper()
    const withoutWallpaper = renderAt("/")
    expect(shellRoot(withoutWallpaper.container).getAttribute("data-haven-wallpaper")).toBe("false")
  })
})

describe("app shell reduce-motion projection", () => {
  afterEach(() => {
    selectWallpaper(FIRST_ASSET_ID)
    Reflect.deleteProperty(window, "matchMedia")
  })

  it("takes the system preference into the shell motion state, not only the wallpaper", async () => {
    stubReducedMotionPreference(true)
    const { container } = renderShell()
    await waitFor(() => expect(container.querySelector("img")).not.toBeNull())

    // 设置项是 false，减少动效完全来自系统偏好。图层的抑制本来就走系统偏好，
    // shell 的类名与 data 状态也必须跟上，否则页面自带的过渡照常动。
    expect(runtimeSnapshot.appearance.reduceMotion).toBe(false)
    const shell = shellRoot(container)
    expect(shell.getAttribute("data-haven-reduce-motion")).toBe("true")
    expect(shell.className).toContain("[&_*]:transition-none")
    expect(shell.className).toContain("[&_*]:duration-0")
    expect(shell.className).toContain("[&_*]:animate-none")
  })

  it("keeps the shell motion state off when the system asks for nothing", () => {
    stubReducedMotionPreference(false)
    const { container } = renderShell()
    const shell = shellRoot(container)
    expect(shell.getAttribute("data-haven-reduce-motion")).toBe("false")
    expect(shell.className).not.toContain("transition-none")
  })
})

/**
 * 首屏设置加载与「用户在加载途中改动」的竞态，用的是 settings-runtime-state 里那份
 * 内存发布代数；这个夹具文件本来就在渲染真正的 AppShell，回归用例放在这里不用新开文件。
 */
describe("settings runtime publish epoch", () => {
  const loadedGeneral: GeneralSettingsValue = {
    section: "general",
    launchPage: "library",
    restoreSession: false,
    language: "zh_cn",
    notifications: true,
  }

  afterEach(() => publishSettingsRuntimeSnapshot(INITIAL_RUNTIME_SNAPSHOT))

  it("does not let the initial load roll back a publication made while it was in flight", () => {
    const userChoice = { ...appearanceValue(SECOND_ASSET_ID), theme: "dark" as const }

    // 加载出发：先记下代数，请求才开始飞。
    const epochsAtRequestStart = captureSettingsRuntimeEpochs()
    // 用户在加载途中改了外观（首页的主题按钮就是这么发布的）。
    publishSettingsRuntimeValue(userChoice)
    // 加载结果落地：外观分区已经有人改过，不得被磁盘上的旧值覆盖；
    // 启动分区没人动过，照常采用加载结果。
    publishLoadedSettingsRuntime(
      { general: loadedGeneral, appearance: appearanceValue(FIRST_ASSET_ID), interfaceFontAssets: [] },
      epochsAtRequestStart,
    )

    const snapshot = getSettingsRuntimeSnapshot()
    expect(snapshot.appearance, "加载不得顶掉用户在飞的那次改动").toBe(userChoice)
    expect(snapshot.general).toBe(loadedGeneral)
  })

  it("still takes the loaded value for a section nobody published to", () => {
    const loadedAppearance = appearanceValue(SECOND_ASSET_ID)
    publishLoadedSettingsRuntime(
      { general: loadedGeneral, appearance: loadedAppearance, interfaceFontAssets: [] },
      captureSettingsRuntimeEpochs(),
    )

    const snapshot = getSettingsRuntimeSnapshot()
    expect(snapshot.appearance).toBe(loadedAppearance)
    expect(snapshot.general).toBe(loadedGeneral)
  })
})
