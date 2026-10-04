// @vitest-environment jsdom
//
// 切分区不该把「外观分区持有的编辑器状态」一起卸载。
//
// 总览布局编辑器、外观资产列表与外观表单草稿都是**页面级**状态：外观分区只是它们的
// 渲染位置。如果它们随分区挂载/卸载，用户「导入资产 → 切到阅读分区 → 切回来」会看到
// 列表重新读一遍、上一次导入/删除的结果消失，总览布局里还没保存的改动也会被丢掉。
//
// 这条用例断言的是**行为**而不是接线形状：切走再切回来之后，gateway 的读取次数必须
// 还是 1（没有重读），而且那份没保存的布局草稿必须还在页面上。

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { MemoryRouter, Route, Routes } from "react-router"
import { afterEach, describe, expect, it, vi } from "vitest"

import { SettingsPage } from "./SettingsPage"

/** gateway 被读了几次：这就是「状态有没有被卸载重来」的观察点。 */
const reads = vi.hoisted(() => ({ assetsList: 0, homeLayoutGet: 0, overviewLayoutGet: 0 }))

vi.mock("@/lib/ipc/runtime", () => ({
  getHavenClient: () => { throw new Error("组件测试不提供 IPC 客户端") },
  getHavenClientMode: () => "mock",
  isTauriRuntime: () => false,
  resolveTauriRuntime: () => false,
  selectHavenClientMode: () => "mock",
}))

vi.mock("@/app/notice-center/notice-context", () => ({
  useNotice: () => ({ push: () => undefined }),
}))

// 只替换数据通道：Hook、投影与渲染器全部走真实实现，接错线就会在这里暴露。
// 布局在工厂里按需动态导入，避免依赖测试文件的静态导入顺序。
vi.mock("@/features/settings/ipc/appearance-gateway", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/features/settings/ipc/appearance-gateway")>()
  const { defaultHomeLayout, defaultOverviewLayout } = await import("@/lib/ipc/settings-wire")
  const unexpected = () => Promise.reject(new Error("本用例不写任何布局"))
  return {
    ...actual,
    appearanceGateway: {
      appearanceAssetsList: async () => {
        reads.assetsList += 1
        // 空列表是后端空库的真实形态：不伪造资产。
        return { schemaVersion: 1, assets: [] }
      },
      appearanceAssetImport: () => Promise.reject(new Error("本用例不导入")),
      appearanceAssetDelete: () => Promise.resolve({ deleted: false, fileRemoved: false }),
      homeLayoutGet: async () => {
        reads.homeLayoutGet += 1
        return { layout: defaultHomeLayout(), revision: null }
      },
      homeLayoutSave: unexpected,
      homeLayoutReset: unexpected,
      overviewLayoutGet: async () => {
        reads.overviewLayoutGet += 1
        return { layout: defaultOverviewLayout(), revision: null }
      },
      overviewLayoutSave: unexpected,
      overviewLayoutReset: unexpected,
    },
  }
})

afterEach(() => {
  cleanup()
  reads.assetsList = 0
  reads.homeLayoutGet = 0
  reads.overviewLayoutGet = 0
})

function renderSettings(section: string) {
  return render(
    <MemoryRouter initialEntries={[`/settings/${section}`]}>
      <Routes>
        <Route path="/settings/:section" element={<SettingsPage />} />
      </Routes>
    </MemoryRouter>,
  )
}

/** 侧栏导航里的一项（只在这个 nav 里找，避免撞上分区正文里的同名标题）。 */
function navItem(label: string): HTMLButtonElement {
  const nav = within(screen.getByRole("navigation", { name: "设置分类" }))
  const button = nav.getByText(label).closest("button")
  if (!(button instanceof HTMLButtonElement)) throw new Error(`导航项「${label}」必须是按钮`)
  return button
}

describe("settings section navigation keeps editor state alive", () => {
  it("keeps the asset list and overview draft alive without loading retired home layout", async () => {
    renderSettings("appearance")
    await waitFor(() => expect(reads.overviewLayoutGet).toBe(1))
    await waitFor(() => expect(reads.assetsList).toBe(1))

    expect(screen.queryByText("首页布局")).toBeNull()
    expect(screen.queryByLabelText("显示最近添加")).toBeNull()
    // 总览布局仍有真实消费者；切换分区不能丢掉它的草稿。
    fireEvent.click(screen.getByLabelText("显示每日阅读时长"))
    expect(screen.getByText("有未保存的布局修改")).toBeTruthy()

    // 切到别的分区，再切回来。
    fireEvent.click(navItem("阅读"))
    expect(document.querySelector('[aria-label="Reading"]')).not.toBeNull()
    fireEvent.click(navItem("外观"))
    expect(document.querySelector('[aria-label="Appearance"]')).not.toBeNull()

    // 活跃资源只读一次，已退役的首页布局不再加载。
    expect(reads.homeLayoutGet).toBe(0)
    expect(reads.overviewLayoutGet).toBe(1)
    expect(reads.assetsList).toBe(1)
    // 而且那份没保存的草稿原样还在——这正是「卸载重来」会丢掉的东西。
    expect(screen.getByText("有未保存的布局修改")).toBeTruthy()
  })

  it("keeps the overview layout read at one across the same round trip", async () => {
    renderSettings("overview")
    await waitFor(() => expect(reads.overviewLayoutGet).toBe(1))

    fireEvent.click(navItem("阅读"))
    fireEvent.click(navItem("总览"))

    // 总览布局仍是页面级状态，往返导航不会重建它。
    expect(reads.overviewLayoutGet).toBe(1)
  })
})
