// @vitest-environment jsdom
//
// 外观分区里几件「不能假报成功」的事：
//   1. 壁纸预览读不出字节时必须收敛成一个明确的空态，而不是留一个破损的 img/video；
//   2. 减少动效（设置项与系统 prefers-reduced-motion 合并后）生效时，动态壁纸预览
//      根本不渲染 video——不是「渲染出来但不播放」；
//   3. 「恢复默认」只重置当前分区的表单，不写入 Rust；
//   4. 没有渲染器的分区渲染「暂未注册渲染器」，不静默回落到通用设置。

import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { MemoryRouter, Route, Routes } from "react-router"
import { afterEach, describe, expect, it, vi } from "vitest"

import type { AppearanceAssetWire, WallpaperSelection } from "@/lib/ipc/settings-wire"
import {
  currentWallpaperRetryEpoch,
  reportHomeWallpaperUnavailable,
  requestWallpaperRetry,
} from "@/features/settings/lib/appearance-runtime"
import { defaultOverviewModuleSettings, overviewModuleSettingsToLayout } from "../lib/overview-layout"
import type { OverviewLayoutController, OverviewLayoutEditorStatus } from "../lib/useOverviewLayout"
import type { AppearanceAssetLists, AppearanceAssetActionResult } from "../lib/useAppearanceAssets"
import {
  AppearanceFontPicker,
  AppearanceWallpaperGroup,
  OverviewLayoutEditor,
  SettingsPage,
  SettingsSectionUnavailable,
} from "./SettingsPage"

/** 通知是这次断言的观察点：重置到底对用户说了什么。 */
const notices = vi.hoisted(() => [] as string[])
const resetCalls = vi.hoisted(() => ({ count: 0 }))
/**
 * 页面看到的外观草稿（`vi.mock` 工厂先于模块求值，只能从这里读）。
 *
 * 用例在渲染前把它设成 `appearanceDraftValue(...)`；默认值只有 `section`，因此
 * 「没设过」时的外观分区就是一个空草稿，而不是另一份看起来合法的外观。
 */
const formsState = vi.hoisted(() => ({
  appearance: { section: "appearance" } as Record<string, unknown>,
}))
const appearanceAssetHookState = vi.hoisted(() => ({
  lists: { fonts: [], staticWallpapers: [], dynamicWallpapers: [] } as unknown,
}))
/**
 * 分区里对草稿的每一次写入。
 *
 * 重试预览必须是纯运行时动作：它不产生任何草稿改动（也就更不会落盘）。这个数组就是
 * 「没有假写入」的观察点。
 */
const draftWrites = vi.hoisted(() => [] as unknown[])

// 只挂载外观分区的组件：不触达任何 IPC，也不需要真实的 Tauri 运行时。
vi.mock("@/lib/ipc/runtime", () => ({
  getHavenClient: () => { throw new Error("组件测试不提供 IPC 客户端") },
  getHavenClientMode: () => "mock",
  isTauriRuntime: () => false,
  resolveTauriRuntime: () => false,
  selectHavenClientMode: () => "mock",
}))

// 受控资源地址的浏览器等价物：本文件断言的是「预览读到失败之后怎么收敛」，
// 地址本身由 appearance-runtime 的测试钉住。
vi.mock("@/features/settings/lib/appearance-runtime", async (importOriginal) => {
  const original = await importOriginal<typeof import("@/features/settings/lib/appearance-runtime")>()
  return {
    ...original,
    appearanceAssetRequestUri: (assetId: string) => `haven-resource://appearance/${assetId}`,
  }
})

vi.mock("@/app/notice-center/notice-context", () => ({
  useNotice: () => ({
    push: (notice: { message?: string }) => { notices.push(notice.message ?? "") },
  }),
}))

// 表单控制器与「重置」语义无关：这里只需要它记录 resetToDefaults 被调过几次。
// appearance 那一份返回上面那份可变草稿，好让外观分区按用例的取值渲染。
vi.mock("@/features/settings/lib/useSettingsForm", () => ({
  useSettingsForm: (section: string) => ({
    section,
    state: {
      status: "ready",
      saved: { section: "general", launchPage: "home", restoreSession: false, language: "zh_cn", notifications: true },
      revision: null,
    },
    displayValue: section === "appearance"
      ? formsState.appearance
      : { section: "general", launchPage: "home", restoreSession: false, language: "zh_cn", notifications: true },
    isLoading: false,
    isSaving: false,
    isDirty: false,
    hasError: false,
    errorMessage: null,
    change: (patch: unknown) => { draftWrites.push(patch) },
    save: () => undefined,
    retry: () => undefined,
    reload: () => undefined,
    resetToDefaults: () => { resetCalls.count += 1 },
  }),
}))

vi.mock("@/features/settings/lib/useAppearanceAssets", async (importOriginal) => {
  const original = await importOriginal<typeof import("@/features/settings/lib/useAppearanceAssets")>()
  return {
    ...original,
    useAppearanceAssets: () => ({
      state: {
        status: "ready" as const,
        lists: appearanceAssetHookState.lists as AppearanceAssetLists,
      },
      action: null,
      pending: null,
      reload: () => undefined,
      importAsset: () => undefined,
      deleteAsset: () => undefined,
    }),
  }
})

/** 只回答减少动效查询的 matchMedia 替身（jsdom 默认没有 matchMedia）。 */
function stubSystemReducedMotion(matches: boolean): void {
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

afterEach(() => {
  cleanup()
  Reflect.deleteProperty(window, "queryLocalFonts")
  notices.length = 0
  resetCalls.count = 0
  draftWrites.length = 0
  formsState.appearance = { section: "appearance" }
  appearanceAssetHookState.lists = { fonts: [], staticWallpapers: [], dynamicWallpapers: [] }
  Reflect.deleteProperty(window, "matchMedia")
  requestWallpaperRetry()
})

const STATIC_ID = "0196f0d2-0000-7000-8000-00000000b001"
const SECOND_STATIC_ID = "0196f0d2-0000-7000-8000-00000000b002"
const DYNAMIC_ID = "0196f0d2-0000-7000-8000-00000000b003"
const FONT_ID = "0196f0d2-0000-7000-8000-00000000b004"

/** 页面级用例的外观草稿：一份选了动态壁纸、减少动效按用例给的合法外观值。 */
function appearanceDraftValue(reduceMotion: boolean): Record<string, unknown> {
  return {
    section: "appearance",
    theme: "system",
    density: "comfortable",
    sidebar: "auto",
    reduceMotion,
    wallpaper: { kind: "dynamic", assetId: DYNAMIC_ID },
  }
}

function asset(assetId: string, kind: AppearanceAssetWire["kind"], displayName: string | null = null): AppearanceAssetWire {
  return { assetId, kind, state: "validated", byteSize: 4096, displayName }
}

const LISTS: AppearanceAssetLists = {
  fonts: [asset(FONT_ID, "font", "霞鹜文楷")],
  staticWallpapers: [asset(STATIC_ID, "static_wallpaper", "晨光壁纸"), asset(SECOND_STATIC_ID, "static_wallpaper", "雾林壁纸")],
  dynamicWallpapers: [asset(DYNAMIC_ID, "dynamic_wallpaper", "流光壁纸")],
}

function renderWallpaperGroup(
  selection: WallpaperSelection,
  reducedMotion = false,
  onSelect: (value: WallpaperSelection) => void = () => undefined,
  assets: AppearanceAssetLists = LISTS,
) {
  return render(
    <AppearanceWallpaperGroup
      assets={assets}
      loading={false}
      error={null}
      selection={selection}
      pending={false}
      onImport={() => undefined}
      onSelect={onSelect}
      onDelete={() => undefined}
      blockedReasonFor={() => null}
      reducedMotion={reducedMotion}
      action={null}
    />,
  )
}

const UNAVAILABLE_COPY = "当前壁纸暂时不可用。"

describe("wallpaper preview fail-closed", () => {
  it("drops the broken static preview and says the wallpaper is unavailable", () => {
    const { container } = renderWallpaperGroup({ kind: "static", assetId: STATIC_ID })

    const image = screen.getByAltText("当前静态壁纸预览")
    expect(image.getAttribute("src")).toBe(`haven-resource://appearance/${STATIC_ID}`)
    expect(screen.queryByText(UNAVAILABLE_COPY)).toBeNull()

    fireEvent.error(image)

    // 当前选中的破损预览会退出 DOM；网格中其它壁纸的缩略图仍然保留。
    expect(screen.queryByAltText("当前静态壁纸预览")).toBeNull()
    expect(container.querySelector('img[alt="雾林壁纸预览"]')).not.toBeNull()
    expect(container.querySelector("video")).toBeNull()
    expect(screen.getByText(UNAVAILABLE_COPY)).toBeTruthy()
  })

  it("drops the broken dynamic preview the same way", () => {
    const { container } = renderWallpaperGroup({ kind: "dynamic", assetId: DYNAMIC_ID })

    const video = screen.getByLabelText("当前动态壁纸预览")
    expect(video.tagName).toBe("VIDEO")

    fireEvent.error(video)

    expect(screen.queryByLabelText("当前动态壁纸预览")).toBeNull()
    expect(container.querySelector("video")).toBeNull()
    expect(container.querySelector('img[alt="晨光壁纸预览"]')).not.toBeNull()
    expect(screen.getByText(UNAVAILABLE_COPY)).toBeTruthy()
  })

  it("treats another controlled URI as a fresh preview", () => {
    const { rerender } = renderWallpaperGroup({ kind: "static", assetId: STATIC_ID })
    fireEvent.error(screen.getByAltText("当前静态壁纸预览"))
    expect(screen.getByText(UNAVAILABLE_COPY)).toBeTruthy()

    // 失败记在「哪一份 URI」上：换一份资产就是一次新投影，上一次的失败不跟着走。
    rerender(
      <AppearanceWallpaperGroup
        assets={LISTS}
        loading={false}
        error={null}
        selection={{ kind: "static", assetId: SECOND_STATIC_ID }}
        pending={false}
        onImport={() => undefined}
        onSelect={() => undefined}
        onDelete={() => undefined}
        blockedReasonFor={() => null}
        reducedMotion={false}
        action={null}
      />,
    )

    const image = screen.getByAltText("当前静态壁纸预览")
    expect(image.getAttribute("src")).toBe(`haven-resource://appearance/${SECOND_STATIC_ID}`)
    expect(screen.queryByText(UNAVAILABLE_COPY)).toBeNull()
  })

  it("shows the built-in no-wallpaper option as selected when nothing is chosen", () => {
    renderWallpaperGroup({ kind: "none" })
    const noWallpaper = screen.getByRole("button", { name: "不使用壁纸" })
    expect(noWallpaper.getAttribute("aria-pressed")).toBe("true")
    expect(noWallpaper.querySelector("[data-selection-mark='true']")).not.toBeNull()
    expect(noWallpaper.className).toContain("border-[var(--haven-settings-primary)]")
    expect(screen.queryByText(UNAVAILABLE_COPY)).toBeNull()
  })

  it("shows concise formats and gives both import actions the same visual style", () => {
    const { container } = renderWallpaperGroup({ kind: "none" })
    expect(screen.getByText("支持 JPG、PNG、WebP 格式")).toBeTruthy()
    expect(screen.getByText("支持 MP4、WebM 格式")).toBeTruthy()
    const imageButton = screen.getByRole("button", { name: /导入图片/ })
    const videoButton = screen.getByRole("button", { name: /导入视频/ })
    expect(imageButton.className).toBe(videoButton.className)
    expect(imageButton.className).toContain("flex-col")
    expect(imageButton.className).toContain("text-center")
    expect(imageButton.className).toContain("min-h-[156px]")
    expect(container.textContent).not.toMatch(/后端|App Shell|受控容器|检测窗口|设置草稿|文件头/)
  })

  it("selects a wallpaper directly from its preview card", () => {
    const onSelect = vi.fn()
    renderWallpaperGroup({ kind: "none" }, false, onSelect)
    fireEvent.click(screen.getByRole("button", { name: "选择晨光壁纸" }))
    expect(onSelect).toHaveBeenCalledWith({ kind: "static", assetId: STATIC_ID })
  })

  it("keeps dynamic videos idle until that wallpaper is selected", () => {
    const { container } = renderWallpaperGroup({ kind: "none" })

    expect(container.querySelector("video")).toBeNull()
    expect(screen.getByText("视频壁纸")).toBeTruthy()
  })

  it("keeps empty wallpaper guidance in the group description and shows import entries", () => {
    const { container } = render(
      <AppearanceWallpaperGroup
        assets={{ fonts: [], staticWallpapers: [], dynamicWallpapers: [] }}
        loading={false}
        error={null}
        selection={{ kind: "none" }}
        pending={false}
        onImport={() => undefined}
        onSelect={() => undefined}
        onDelete={() => undefined}
        blockedReasonFor={() => null}
        reducedMotion={false}
        action={null}
      />,
    )
    expect(screen.getByText(/还没有首页壁纸，导入图片或视频后可从下方选择/)).toBeTruthy()
    expect(container.querySelector("[role='status']")).toBeNull()
    expect(screen.getByRole("button", { name: /导入图片/ })).toBeTruthy()
    expect(screen.getByRole("button", { name: /导入视频/ })).toBeTruthy()
  })

  it("offers compact font presets and a Chinese typography specimen instead of large tiles", () => {
    const onSelectPreset = vi.fn()
    render(
      <AppearanceFontPicker
        title="界面字体"
        description="调整界面字形"
        assets={LISTS.fonts}
        loading={false}
        error={null}
        selectedAssetId={null}
        preset="system"
        family={null}
        pending={false}
        onImport={() => undefined}
        onSelectPreset={onSelectPreset}
        onSelectSystemFont={() => undefined}
        onSelectAsset={() => undefined}
        onDelete={() => undefined}
        blockedReasonFor={() => null}
        action={null}
      />,
    )

    expect(screen.getByRole("group", { name: "界面字体预设" })).toBeTruthy()
    expect(screen.getByRole("button", { name: /跟随系统/ }).getAttribute("aria-pressed")).toBe("true")
    expect(screen.getByText("栖阅 · 沉浸阅读体验 0123 Aa")).toBeTruthy()
    expect(screen.getByText("白日依山尽，黄河入海流。")).toBeTruthy()
    expect(screen.queryByRole("button", { name: "默认界面字体" })).toBeNull()
    expect(screen.queryByText("Aa", { selector: "span" })).toBeNull()

    fireEvent.click(screen.getByRole("button", { name: /人文衬线/ }))
    expect(onSelectPreset).toHaveBeenCalledWith("humanist_serif")
  })

  it("searches local fonts by pinyin, previews the font face, and selects with the keyboard", async () => {
    Object.defineProperty(window, "queryLocalFonts", {
      configurable: true,
      value: async () => [
        { family: "Consolas", fullName: "Consolas", postscriptName: "Consolas" },
        { family: "霞鹜文楷", fullName: "霞鹜文楷", postscriptName: "LXGWWenKai-Regular" },
      ],
    })
    const onSelectSystemFont = vi.fn()
    render(
      <AppearanceFontPicker
        title="界面字体"
        description="调整界面字形"
        assets={[]}
        loading={false}
        error={null}
        selectedAssetId={null}
        preset="system"
        family={null}
        pending={false}
        onImport={() => undefined}
        onSelectPreset={() => undefined}
        onSelectSystemFont={onSelectSystemFont}
        onSelectAsset={() => undefined}
        onDelete={() => undefined}
        blockedReasonFor={() => null}
        action={null}
      />,
    )

    fireEvent.click(screen.getByRole("button", { name: /自定义系统字体/ }))
    const search = await screen.findByRole("combobox", { name: "搜索本机字体名称或拼音" })
    await screen.findByRole("option", { name: "Consolas" })
    fireEvent.change(search, { target: { value: "xiawuwen" } })
    const fontOption = await screen.findByRole("option", { name: "霞鹜文楷" })
    expect(fontOption.getAttribute("style")).toContain("霞鹜文楷")

    fireEvent.mouseEnter(fontOption)
    expect(screen.getByText("栖阅 · 沉浸阅读体验 0123 Aa").getAttribute("style")).toContain("霞鹜文楷")
    fireEvent.mouseLeave(screen.getByRole("listbox", { name: "本机字体" }))
    expect(screen.getByText("栖阅 · 沉浸阅读体验 0123 Aa").getAttribute("style")).not.toContain("霞鹜文楷")
    fireEvent.keyDown(search, { key: "ArrowDown" })
    fireEvent.keyDown(search, { key: "Enter" })
    expect(onSelectSystemFont).toHaveBeenCalledWith("霞鹜文楷")
  })

  it("reports when local font search is unavailable and keeps import as a small secondary action", async () => {
    const onImport = vi.fn()
    render(
      <AppearanceFontPicker
        title="界面字体"
        description="调整界面字形"
        assets={[]}
        loading={false}
        error={null}
        selectedAssetId={null}
        preset="system"
        family={null}
        pending={false}
        onImport={onImport}
        onSelectPreset={() => undefined}
        onSelectSystemFont={() => undefined}
        onSelectAsset={() => undefined}
        onDelete={() => undefined}
        blockedReasonFor={() => null}
        action={null}
      />,
    )

    fireEvent.click(screen.getByRole("button", { name: /自定义系统字体/ }))
    expect(await screen.findByText(/当前环境暂不支持搜索本机字体/)).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "导入本地字体" }))
    expect(onImport).toHaveBeenCalledOnce()
  })

  it("keeps imported font assets in compact rows and preserves delete protection", () => {
    const onSelectAsset = vi.fn()
    const onDelete = vi.fn()
    render(
      <AppearanceFontPicker
        title="界面字体"
        description="调整界面字形"
        assets={LISTS.fonts}
        loading={false}
        error={null}
        selectedAssetId={null}
        preset="system"
        family={null}
        pending={false}
        onImport={() => undefined}
        onSelectPreset={() => undefined}
        onSelectSystemFont={() => undefined}
        onSelectAsset={onSelectAsset}
        onDelete={onDelete}
        blockedReasonFor={() => "正在使用的字体不能删除"}
        action={null}
      />,
    )

    const font = screen.getByRole("button", { name: "选择霞鹜文楷" })
    expect(font.getAttribute("aria-pressed")).toBe("false")
    fireEvent.click(font)
    expect(onSelectAsset).toHaveBeenCalledWith(FONT_ID)
    const remove = screen.getByRole("button", { name: "删除霞鹜文楷" })
    expect(remove.hasAttribute("disabled")).toBe(true)
    fireEvent.click(remove)
    expect(onDelete).not.toHaveBeenCalled()
    expect(screen.getByRole("list", { name: "已导入字体列表" })).toBeTruthy()
  })

  it("does not expose an internal asset id when an asset has no display name", () => {
    const unnamedAssets: AppearanceAssetLists = {
      fonts: [],
      staticWallpapers: [asset(STATIC_ID, "static_wallpaper")],
      dynamicWallpapers: [],
    }
    const { container } = renderWallpaperGroup({ kind: "none" }, false, () => undefined, unnamedAssets)
    expect(container.textContent).toContain("未命名图片壁纸")
    expect(container.textContent).not.toContain(STATIC_ID.slice(0, 8))
  })
})

describe("wallpaper preview retry", () => {
  const RETRY_LABEL = "重试预览"

  /** 重试入口必须是真实按钮：键盘可达、可聚焦，且不是隐式提交。 */
  function retryButton(): HTMLButtonElement {
    const button = screen.getByRole("button", { name: RETRY_LABEL })
    if (!(button instanceof HTMLButtonElement)) throw new Error("重试预览必须是一个真实按钮")
    expect(button.getAttribute("type")).toBe("button")
    return button
  }

  it("re-projects the same controlled URI and leaves the saved setting alone", () => {
    renderWallpaperGroup({ kind: "static", assetId: STATIC_ID })
    fireEvent.error(screen.getByAltText("当前静态壁纸预览"))
    expect(screen.getByText(UNAVAILABLE_COPY)).toBeTruthy()

    const epochBefore = currentWallpaperRetryEpoch()
    fireEvent.click(retryButton())

    // 预览回来了，而且是**同一份**受控 URI：重试没有地址入口，也拼不出第二个地址。
    const image = screen.getByAltText("当前静态壁纸预览")
    expect(image.getAttribute("src")).toBe(`haven-resource://appearance/${STATIC_ID}`)
    expect(image.getAttribute("src")).not.toContain("?")
    expect(image.getAttribute("src")).not.toContain("\\")
    expect(screen.queryByText(UNAVAILABLE_COPY)).toBeNull()

    // 驱动的是首页壁纸层订阅的同一个信号，不是设置页自己的一份局部状态。
    expect(currentWallpaperRetryEpoch()).toBe(epochBefore + 1)
    // 重试不改草稿、不落盘：它不是一次设置写入。
    expect(draftWrites, "重试不得改写外观设置草稿").toEqual([])
  })

  it("retries a broken dynamic preview the same way", () => {
    const { container } = renderWallpaperGroup({ kind: "dynamic", assetId: DYNAMIC_ID })
    fireEvent.error(screen.getByLabelText("当前动态壁纸预览"))
    expect(container.querySelector("video")).toBeNull()

    fireEvent.click(retryButton())

    const video = screen.getByLabelText("当前动态壁纸预览")
    expect(video.tagName).toBe("VIDEO")
    expect(video.getAttribute("src")).toBe(`haven-resource://appearance/${DYNAMIC_ID}`)
    expect(draftWrites).toEqual([])
  })

  it("offers a real retry when the homepage background failed but its settings preview works", () => {
    const epoch = currentWallpaperRetryEpoch()
    const uri = "haven-resource://appearance/" + STATIC_ID
    reportHomeWallpaperUnavailable(uri, epoch)

    renderWallpaperGroup({ kind: "static", assetId: STATIC_ID })

    expect(screen.getByAltText("当前静态壁纸预览").getAttribute("src")).toBe(uri)
    expect(screen.getByText("首页背景暂时无法显示。")).toBeTruthy()
    fireEvent.click(retryButton())

    expect(currentWallpaperRetryEpoch()).toBe(epoch + 1)
    expect(screen.queryByText("首页背景暂时无法显示。")).toBeNull()
    expect(screen.getByAltText("当前静态壁纸预览").getAttribute("src")).toBe(uri)
    expect(draftWrites, "重试不得改写外观设置草稿").toEqual([])
  })

  it("reports no success when the retried bytes fail again", () => {
    renderWallpaperGroup({ kind: "static", assetId: STATIC_ID })
    fireEvent.error(screen.getByAltText("当前静态壁纸预览"))
    fireEvent.click(retryButton())

    // 第二次同样读不出来：回到同一个明确的不可用态，而不是宣布「已恢复」。
    fireEvent.error(screen.getByAltText("当前静态壁纸预览"))

    expect(screen.queryByAltText("当前静态壁纸预览")).toBeNull()
    expect(screen.getByText(UNAVAILABLE_COPY)).toBeTruthy()
    expect(retryButton(), "仍然读不出来时重试入口保持可用").toBeTruthy()
    expect(notices, "重试不做假成功，不发任何成功提示").toEqual([])
  })

  it("offers no retry when nothing actually failed", () => {
    renderWallpaperGroup({ kind: "static", assetId: STATIC_ID })
    expect(screen.queryByRole("button", { name: RETRY_LABEL })).toBeNull()
    cleanup()

    // 减少动效下的抑制不是一次读取失败：那里没有可重试的东西，给按钮只会是假动作。
    renderWallpaperGroup({ kind: "dynamic", assetId: DYNAMIC_ID }, true)
    expect(screen.getByText("减少动效已开启")).toBeTruthy()
    expect(screen.queryByRole("button", { name: RETRY_LABEL })).toBeNull()
    cleanup()

    renderWallpaperGroup({ kind: "none" })
    expect(screen.queryByRole("button", { name: RETRY_LABEL })).toBeNull()
  })
})

describe("dynamic wallpaper preview and reduced motion", () => {
  const SUPPRESSED_COPY = "减少动效已开启"

  it("never renders a video while reduced motion is on", () => {
    const { container } = renderWallpaperGroup({ kind: "dynamic", assetId: DYNAMIC_ID }, true)

    // 不是「渲染出来但不播放」：video 元素根本不进 DOM。
    expect(container.querySelector("video")).toBeNull()
    expect(screen.queryByLabelText("当前动态壁纸预览")).toBeNull()
    expect(screen.getByText(SUPPRESSED_COPY)).toBeTruthy()
  })

  it("keeps the static preview when reduced motion is on", () => {
    const { container } = renderWallpaperGroup({ kind: "static", assetId: STATIC_ID }, true)

    expect(container.querySelector("img")).not.toBeNull()
    expect(screen.queryByText(SUPPRESSED_COPY)).toBeNull()
  })

  it("still renders the video when reduced motion is off", () => {
    renderWallpaperGroup({ kind: "dynamic", assetId: DYNAMIC_ID }, false)

    expect(screen.getByLabelText("当前动态壁纸预览").tagName).toBe("VIDEO")
    expect(screen.queryByText(SUPPRESSED_COPY)).toBeNull()
  })
})

describe("appearance section feeds the merged reduced-motion value into the preview", () => {
  function renderAppearanceSection() {
    appearanceAssetHookState.lists = LISTS
    return render(
      <MemoryRouter initialEntries={["/settings/appearance"]}>
        <Routes>
          <Route path="/settings/:section" element={<SettingsPage />} />
        </Routes>
      </MemoryRouter>,
    )
  }

  it("drops the video when the system asks for reduced motion, even with the setting off", () => {
    formsState.appearance = appearanceDraftValue(false)
    stubSystemReducedMotion(true)

    const { container } = renderAppearanceSection()

    expect(container.querySelector("video")).toBeNull()
    expect(screen.getByText("减少动效已开启")).toBeTruthy()
  })

  it("drops the video when the setting itself is on", () => {
    formsState.appearance = appearanceDraftValue(true)
    stubSystemReducedMotion(false)

    const { container } = renderAppearanceSection()

    expect(container.querySelector("video")).toBeNull()
    expect(screen.getByText("减少动效已开启")).toBeTruthy()
  })

  it("keeps the preview when neither the setting nor the system asks for it", () => {
    formsState.appearance = appearanceDraftValue(false)
    stubSystemReducedMotion(false)

    renderAppearanceSection()

    expect(screen.getByLabelText("当前动态壁纸预览").tagName).toBe("VIDEO")
  })
})

describe("section reset behavior", () => {
  function renderSettingsAt(section: string) {
    return render(
      <MemoryRouter initialEntries={[`/settings/${section}`]}>
        <Routes>
          <Route path="/settings/:section" element={<SettingsPage />} />
        </Routes>
      </MemoryRouter>,
    )
  }

  it("resets only the active section and does not claim the change was saved", () => {
    renderSettingsAt("general")

    fireEvent.click(screen.getByText("恢复默认"))

    expect(resetCalls.count).toBe(1)
    expect(notices).toEqual([])
  })
})

describe("appearance theme presets and live preview", () => {
  function renderAppearance() {
    return render(
      <MemoryRouter initialEntries={["/settings/appearance"]}>
        <Routes>
          <Route path="/settings/:section" element={<SettingsPage />} />
        </Routes>
      </MemoryRouter>,
    )
  }

  it("shows four understandable presets and custom color buttons instead of native system dialogs", () => {
    formsState.appearance = appearanceDraftValue(false)
    const { container } = renderAppearance()

    for (const label of ["墨黑", "燕麦", "黛青", "秋栗"]) {
      expect(screen.getByRole("button", { name: new RegExp(label) })).toBeTruthy()
    }
    expect(screen.getByRole("button", { name: "浅色页面底色" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "浅色卡片" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "浅色文字" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "强调色" })).toBeTruthy()
    expect(container.querySelectorAll('input[type="color"]')).toHaveLength(0)
    const backgroundPicker = screen.getByRole("button", { name: "浅色页面底色" }) as HTMLButtonElement
    expect(backgroundPicker.style.backgroundColor).toBe("rgb(245, 245, 241)")
    expect(screen.getByLabelText("浅色主题配色预览")).toBeTruthy()
    expect(screen.queryByText("当前配置")).toBeNull()
    expect(screen.queryByText(/语义色彩|inline token|--primary/)).toBeNull()
    expect(container.querySelector("section.settings-scrollbar-hidden")?.className).toContain("pb-[112px]")
  })

  it("applies a preset to the appearance draft and uses that same draft for the live preview", () => {
    formsState.appearance = appearanceDraftValue(false)
    const rendered = renderAppearance()

    fireEvent.click(screen.getByRole("button", { name: /燕麦/ }))

    const patch = draftWrites.at(-1) as {
      section?: string
      theme?: string
      customTheme?: {
        accentColor?: string
        light?: { background?: string; card?: string; foreground?: string }
        dark?: { background?: string; card?: string; foreground?: string }
      }
    }
    expect(patch.section).toBe("appearance")
    expect(patch.theme).toBe("custom")
    expect(patch.customTheme?.accentColor).toBe("#8a6d4a")

    // 此 mock 不会像 SettingsFormController 一样触发订阅更新，所以把保存草稿回灌页面，
    // 再验证真实预览组件读取的正是新草稿，而非一份仅供展示的本地假值。
    formsState.appearance = { ...formsState.appearance, ...patch }
    rendered.rerender(
      <MemoryRouter initialEntries={["/settings/appearance"]}>
        <Routes>
          <Route path="/settings/:section" element={<SettingsPage />} />
        </Routes>
      </MemoryRouter>,
    )
    const preview = screen.getByLabelText("浅色主题配色预览") as HTMLElement
    expect(preview.style.backgroundColor).toBe("rgb(244, 239, 230)")
    expect(screen.getByRole("button", { name: /燕麦/ }).getAttribute("aria-pressed")).toBe("true")
  })

  it("maps familiar color choices onto the stored theme roles", () => {
    formsState.appearance = appearanceDraftValue(false)
    const rendered = renderAppearance()

    fireEvent.click(screen.getByRole("button", { name: "浅色卡片" }))
    fireEvent.change(screen.getByRole("textbox", { name: "Hex 色值" }), { target: { value: "#123456" } })
    expect(draftWrites).toHaveLength(0)
    const livePreview = screen.getByLabelText("浅色主题配色预览")
    expect(Array.from(livePreview.querySelectorAll("div")).some((node) => node.style.backgroundColor === "rgb(18, 52, 86)")).toBe(true)
    fireEvent.click(screen.getByRole("button", { name: "应用颜色" }))
    const patch = draftWrites.at(-1) as {
      section?: string
      theme?: string
      customTheme?: { light?: { card?: string; popover?: string } }
    }
    expect(patch.section).toBe("appearance")
    expect(patch.theme).toBe("custom")
    expect(patch.customTheme?.light?.card).toBe("#123456")
    expect(patch.customTheme?.light?.popover).toBe("#123456")

    // 表单 mock 只记录 patch；把它回灌后再验证生产组件会显示更新后的色样。
    formsState.appearance = { ...formsState.appearance, ...patch }
    rendered.rerender(
      <MemoryRouter initialEntries={["/settings/appearance"]}>
        <Routes>
          <Route path="/settings/:section" element={<SettingsPage />} />
        </Routes>
      </MemoryRouter>,
    )
    const updatedCardPicker = screen.getByLabelText("浅色卡片") as HTMLButtonElement
    expect(updatedCardPicker.style.backgroundColor).toBe("rgb(18, 52, 86)")
    fireEvent.click(screen.getByRole("button", { name: "浅色卡片" }))
    expect(screen.getByRole("button", { name: "最近使用色 #123456" })).toBeTruthy()
  })

  it("keeps both runtime-controlled colors synchronized when changing the accent", () => {
    formsState.appearance = appearanceDraftValue(false)
    renderAppearance()

    fireEvent.click(screen.getByRole("button", { name: "强调色" }))
    fireEvent.change(screen.getByRole("textbox", { name: "Hex 色值" }), { target: { value: "#8c6651" } })
    fireEvent.click(screen.getByRole("button", { name: "应用颜色" }))

    const patch = draftWrites.at(-1) as {
      section?: string
      customTheme?: {
        accentColor?: string
        light?: { primary?: string; ring?: string }
        dark?: { primary?: string; ring?: string }
      }
    }
    expect(patch.section).toBe("appearance")
    expect(patch.customTheme?.accentColor).toBe("#8c6651")
    for (const palette of [patch.customTheme?.light, patch.customTheme?.dark]) {
      expect(palette?.primary).toBe("#8c6651")
      expect(palette?.ring).toBe("#8c6651")
    }
  })

  it("keeps the settings draft unchanged when the picker is cancelled", () => {
    formsState.appearance = appearanceDraftValue(false)
    renderAppearance()

    fireEvent.click(screen.getByRole("button", { name: "浅色页面底色" }))
    fireEvent.change(screen.getByRole("textbox", { name: "Hex 色值" }), { target: { value: "#2c2e29" } })
    expect(screen.getByText(/对比度偏低/)).toBeTruthy()
    expect(draftWrites).toHaveLength(0)
    fireEvent.click(screen.getByRole("button", { name: "取消" }))

    expect(draftWrites).toHaveLength(0)
    expect(screen.queryByRole("dialog")).toBeNull()
    expect((screen.getByRole("button", { name: "浅色页面底色" }) as HTMLButtonElement).style.backgroundColor)
      .toBe("rgb(245, 245, 241)")
  })
})

describe("asset action messaging", () => {
  function renderAction(action: AppearanceAssetActionResult | null) {
    return render(
      <AppearanceWallpaperGroup
        assets={LISTS}
        loading={false}
        error={null}
        selection={{ kind: "none" }}
        pending={false}
        onImport={() => undefined}
        onSelect={() => undefined}
        onDelete={() => undefined}
        blockedReasonFor={() => null}
        reducedMotion={false}
        action={action}
      />,
    )
  }

  it("reports a cancelled file picker as information, not as a failure", () => {
    renderAction({ kind: "cancelled", message: "已取消选择文件，没有导入任何资产。" })

    // 用户自己取消不是故障：既不该被播报成 alert，也不该穿上失败的红字。
    expect(screen.queryByRole("alert")).toBeNull()
    const status = screen.getByRole("status")
    expect(status.textContent).toContain("已取消选择文件")
    expect(status.className).not.toContain("danger")
  })

  it("shows a blocked delete as a prompt rather than as an error", () => {
    renderAction({ kind: "blocked", message: "该资产正在使用中，请先切换到别的字体或壁纸再删除。" })

    expect(screen.queryByRole("alert")).toBeNull()
    expect(screen.getByRole("status").textContent).toContain("正在使用中")
  })

  it("surfaces successful deletion results, including incomplete file cleanup", () => {
    renderAction({ kind: "success", message: "资产已从列表移除，文件清理未完成" })

    expect(screen.getByRole("status").textContent).toContain("文件清理未完成")
  })

  it("keeps real failures on the alert/danger treatment", () => {
    renderAction({ kind: "failure", message: "资产体积超出上限" })

    const alert = screen.getByRole("alert")
    expect(alert.textContent).toContain("资产体积超出上限")
    expect(alert.className).toContain("danger")
  })
})

describe("settings section renderer fallback", () => {  it("shows an unregistered-renderer state instead of silently rendering general settings", () => {
    // `renderSettingsSection` 的 default 分支渲染的就是这个组件（「登记表里的每个分区
    // 都有自己的 case」由 settings-registry.test.ts 钉住，未知值一律走这里）。这里断言
    // 它自己绝不伪装成通用设置——`sync` 在分区词表里，但不在生产登记表里。
    render(<SettingsSectionUnavailable section="sync" />)

    expect(screen.getByText("该设置分区暂未注册渲染器")).toBeTruthy()
    expect(screen.getByText(/分区「sync」/)).toBeTruthy()
    // 通用设置的标题与第一项都不该出现：那正是「静默落到 General」的样子。
    expect(screen.queryByText("通用")).toBeNull()
    expect(screen.queryByText("默认启动页")).toBeNull()
  })
})

describe("overview layout editor", () => {
  function controllerIn(status: OverviewLayoutEditorStatus): OverviewLayoutController {
    const modules = defaultOverviewModuleSettings()
    return {
      state: {
        status,
        modules,
        savedModules: modules,
        savedLayout: overviewModuleSettingsToLayout(modules),
        revision: null,
        message: null,
        retryable: false,
      },
      isDirty: false,
      isSaving: status === "saving",
      move: () => undefined,
      reorder: () => undefined,
      setSize: () => undefined,
      setVisible: () => undefined,
      save: () => undefined,
      reset: vi.fn(),
      reload: () => undefined,
    }
  }

  function resetButton(): HTMLButtonElement {
    const button = screen.getByText("恢复默认").closest("button")
    if (!(button instanceof HTMLButtonElement)) throw new Error("恢复默认必须是一个按钮")
    return button
  }

  it("edits exactly the five real overview modules", () => {
    const { container } = render(<OverviewLayoutEditor controller={controllerIn("ready")} />)

    const rows = [...container.querySelectorAll("[data-layout-module-row]")]
    expect(rows).toHaveLength(5)
    for (const [index, label] of ["当前偏好", "阅读指标", "每日阅读时长", "作品类型分布", "阅读时段"].entries()) {
      expect(rows[index]?.textContent).toContain(label)
    }
    // 首页的模块不得出现在总览编辑器里（两套闭合集合互不通用）。
    expect(screen.queryByText("继续")).toBeNull()
    expect(screen.queryByText("最近添加")).toBeNull()
    expect(screen.queryByText("收藏")).toBeNull()
    // 页头不是可管理的模块，因此编辑器不提供它的开关。
    expect(screen.queryByLabelText("显示页头")).toBeNull()
  })

  it("explains layout changes without exposing implementation details", () => {
    const { container } = render(<OverviewLayoutEditor controller={controllerIn("ready")} />)

    expect(screen.getByText("拖动卡片调整顺序，选择紧凑、标准或通栏宽度；标题与统计范围始终显示，窄屏下会自动排成单列。")).toBeTruthy()
    expect(container.textContent).not.toMatch(/确定性|任意拖拽|空布局|从未保存过|后端/)
    expect(screen.getByRole("button", { name: "拖动或用方向键调整阅读指标顺序" })).toBeTruthy()
    expect(screen.getByRole("group", { name: "阅读指标显示宽度" })).toBeTruthy()
    expect(screen.getByRole("img", { name: "总览布局实时预览" })).toBeTruthy()
    expect(container.textContent).not.toMatch(/\d\/\d\s*宽/)
  })

  it("draws each width option as its actual horizontal grid proportion", () => {
    render(<OverviewLayoutEditor controller={controllerIn("ready")} />)

    const widthGroup = screen.getByRole("group", { name: "阅读指标显示宽度" })
    const fillPercent = (name: string) => {
      const button = [...widthGroup.querySelectorAll("button")].find((entry) => entry.getAttribute("aria-label") === name)
      const fill = button?.querySelector<HTMLElement>("[data-layout-width-fill]")
      if (!fill) throw new Error("总览宽度选项必须有比例示意")
      return Number.parseFloat(fill.style.width)
    }
    expect(fillPercent("阅读指标：紧凑")).toBeCloseTo(100 / 3, 5)
    expect(fillPercent("阅读指标：标准")).toBeCloseTo(200 / 3, 5)
    expect(fillPercent("阅读指标：通栏")).toBe(100)
  })

  it("omits hidden overview modules from the preview and keeps their switch available", () => {
    const controller = controllerIn("ready")
    controller.state = {
      ...controller.state,
      modules: controller.state.modules.map((entry) => entry.module === "metrics" ? { ...entry, visible: false } : entry),
    }
    controller.setVisible = vi.fn()
    const { container } = render(<OverviewLayoutEditor controller={controller} />)

    const row = container.querySelector('[data-layout-module-row="metrics"]')
    if (!row) throw new Error("隐藏的总览模块行必须存在")
    expect(row.querySelector("[data-layout-static-controls]")?.className).toContain("opacity-40")
    expect(row.querySelector("[data-layout-static-controls]")?.className).toContain("pointer-events-none")
    const widthGroup = screen.getByRole("group", { name: "阅读指标显示宽度" })
    expect([...widthGroup.querySelectorAll("button")].every((button) => (button as HTMLButtonElement).disabled)).toBe(true)

    const visibilitySwitch = screen.getByRole("switch", { name: "显示阅读指标" }) as HTMLButtonElement
    expect(visibilitySwitch.disabled).toBe(false)
    fireEvent.click(visibilitySwitch)
    expect(controller.setVisible).toHaveBeenCalledWith("metrics", true)

    const preview = screen.getByRole("img", { name: "总览布局实时预览" })
    expect(preview.textContent).not.toContain("阅读指标")
  })

  it("uses the drag handle as a keyboard-accessible reorder control", () => {
    const controller = controllerIn("ready")
    controller.reorder = vi.fn()
    render(<OverviewLayoutEditor controller={controller} />)

    fireEvent.keyDown(screen.getByRole("button", { name: "拖动或用方向键调整阅读指标顺序" }), { key: "ArrowUp" })

    expect(controller.reorder).toHaveBeenCalledWith("metrics", 0)
  })

  it("offers the reset only once the current revision is actually known", () => {
    for (const unknown of ["loading", "error"] as const) {
      render(<OverviewLayoutEditor controller={controllerIn(unknown)} />)
      expect(resetButton().disabled, `${unknown} 时不该提供恢复默认`).toBe(true)
      cleanup()
    }

    render(<OverviewLayoutEditor controller={controllerIn("ready")} />)
    expect(resetButton().disabled).toBe(false)
  })

  it("disables saving until the layout is dirty and editable", () => {
    render(<OverviewLayoutEditor controller={controllerIn("ready")} />)
    expect((screen.getByText("保存布局").closest("button") as HTMLButtonElement).disabled).toBe(true)
    cleanup()

    const dirty: OverviewLayoutController = { ...controllerIn("ready"), isDirty: true }
    render(<OverviewLayoutEditor controller={dirty} />)
    expect((screen.getByText("保存布局").closest("button") as HTMLButtonElement).disabled).toBe(false)
    cleanup()

    // 冲突态下不得继续写：必须先重新加载，否则只会再撞一次 REVISION_CONFLICT。
    const conflict: OverviewLayoutController = {
      ...controllerIn("conflict"),
      isDirty: true,
      state: {
        status: "conflict",
        modules: defaultOverviewModuleSettings(),
        savedModules: defaultOverviewModuleSettings(),
        savedLayout: overviewModuleSettingsToLayout(defaultOverviewModuleSettings()),
        revision: "overview-1",
        message: "外观设置已被其他窗口更新",
        retryable: false,
      },
    }
    render(<OverviewLayoutEditor controller={conflict} />)
    expect((screen.getByText("保存布局").closest("button") as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText("重新加载")).toBeTruthy()
  })
})
