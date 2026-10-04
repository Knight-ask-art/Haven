// @vitest-environment jsdom

import { act, cleanup, renderHook, waitFor } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"
import { MockHavenClient } from "@/lib/ipc/mock-client"
import { HavenError } from "@/lib/ipc/errors"
import { defaultHomeLayout, guardHomeLayout } from "@/lib/ipc/settings-wire"
import type { AppearanceGateway } from "../ipc/appearance-gateway"
import {
  HOME_MODULES,
  defaultHomeModuleSettings,
  homeModuleLabel,
  homeModuleSettingsEqual,
  homeModuleSettingsToLayout,
  layoutToHomeModulePlacements,
  layoutToHomeModuleSettings,
  moveHomeModule,
  reorderHomeModule,
  setHomeModuleSize,
  setHomeModuleVisible,
} from "./home-layout"
import {
  homeLayoutEdit,
  homeLayoutFromSnapshot,
  homeLayoutIsDirty,
  resetHomeLayout,
  saveHomeLayout,
  useHomeLayout,
} from "./useHomeLayout"

/** 用真实的 MockHavenClient 充当 gateway：CAS、幂等与默认布局都走后端那套语义。 */
function mockGateway(client: MockHavenClient = new MockHavenClient()): AppearanceGateway {
  return {
    appearanceAssetsList: (request) => client.appearanceAssetsList(request),
    appearanceAssetImport: (request) => client.appearanceAssetImport(request),
    appearanceAssetDelete: (request) => client.appearanceAssetDelete(request),
    homeLayoutGet: () => client.homeLayoutGet(),
    homeLayoutSave: (request) => client.homeLayoutSave(request),
    homeLayoutReset: (request) => client.homeLayoutReset(request),
    // 首页布局的用例不碰总览通道：这里保留真实的 Mock 转发，但没有任何断言依赖它。
    overviewLayoutGet: () => client.overviewLayoutGet(),
    overviewLayoutSave: (request) => client.overviewLayoutSave(request),
    overviewLayoutReset: (request) => client.overviewLayoutReset(request),
  }
}

afterEach(cleanup)

describe("home layout editor model", () => {
  it("allowlists exactly the three real modules", () => {
    expect(HOME_MODULES.map((module) => module.id)).toEqual([
      "continue",
      "recently_added",
      "shelf-favorites",
    ])
    expect(homeModuleLabel("shelf-favorites")).toBe("收藏")
  })

  it("reads the domain default layout as three visible modules", () => {
    const settings = layoutToHomeModuleSettings(defaultHomeLayout())
    expect(settings).toEqual([
      { module: "continue", visible: true, size: "medium" },
      { module: "recently_added", visible: true, size: "medium" },
      { module: "shelf-favorites", visible: true, size: "medium" },
    ])
  })

  it("ignores edits until the persisted revision is known", () => {
    const loading = {
      status: "loading" as const,
      modules: defaultHomeModuleSettings(),
      savedModules: defaultHomeModuleSettings(),
      revision: null,
      message: null,
      retryable: false,
    }
    const edited = moveHomeModule(loading.modules, "recently_added", -1)
    expect(homeLayoutEdit(loading, edited)).toBe(loading)

    const conflict = { ...loading, status: "conflict" as const }
    expect(homeLayoutEdit(conflict, edited)).toBe(conflict)
  })

  it("marks modules missing from a saved layout as hidden instead of dropping them", () => {
    const settings = layoutToHomeModuleSettings({
      schemaVersion: 1,
      modules: [{ module: "shelf-favorites", size: "large", row: 0, column: 0, order: 0 }],
    })
    expect(settings).toEqual([
      { module: "shelf-favorites", visible: true, size: "large" },
      { module: "continue", visible: false, size: "medium" },
      { module: "recently_added", visible: false, size: "medium" },
    ])
  })

  it("packs every size into non-overlapping positions that match the four-column grid", () => {
    const expectedCoordinates = {
      small: [[0, 0], [0, 1], [0, 2]],
      medium: [[0, 0], [0, 2], [1, 0]],
      large: [[0, 0], [1, 0], [2, 0]],
    } as const
    for (const size of ["small", "medium", "large"] as const) {
      const settings = defaultHomeModuleSettings().map((entry) => ({ ...entry, size }))
      const layout = homeModuleSettingsToLayout(settings)
      expect(guardHomeLayout(layout), `${size} 布局必须合法`).toBe(true)
      expect(layout.modules.map((placement) => placement.order)).toEqual([0, 1, 2])
      expect(layout.modules.map((placement) => [placement.row, placement.column])).toEqual(
        expectedCoordinates[size],
      )
    }
  })

  it("uses saved positions for the homepage while preserving the user's order", () => {
    const saved = {
      schemaVersion: 1 as const,
      modules: [
        { module: "shelf-favorites" as const, size: "small" as const, row: 1, column: 2, order: 1 },
        { module: "recently_added" as const, size: "medium" as const, row: 0, column: 0, order: 0 },
      ],
    }
    expect(layoutToHomeModulePlacements(saved).map(({ module, row, column, order }) => [
      module,
      row,
      column,
      order,
    ])).toEqual([
      ["recently_added", 0, 0, 0],
      ["shelf-favorites", 1, 2, 1],
    ])
  })

  it("treats hiding every module as an explicit empty layout", () => {
    const hidden = defaultHomeModuleSettings().map((entry) => ({ ...entry, visible: false }))
    const layout = homeModuleSettingsToLayout(hidden)
    expect(layout).toEqual({ schemaVersion: 1, modules: [] })
    expect(guardHomeLayout(layout)).toBe(true)
    // 再读回来仍然是三个隐藏模块，而不是一份「从未保存过」的默认布局。
    expect(layoutToHomeModuleSettings(layout).every((entry) => !entry.visible)).toBe(true)
  })

  it("refuses arbitrary module ids even if they reach the builder", () => {
    const layout = homeModuleSettingsToLayout([
      { module: "continue", visible: true, size: "medium" },
      { module: "evil-module" as never, visible: true, size: "large" },
    ])
    expect(layout.modules.map((placement) => placement.module)).toEqual(["continue"])
  })

  it("reorders within bounds and no-ops at the edges", () => {
    const settings = defaultHomeModuleSettings()
    expect(moveHomeModule(settings, "recently_added", -1).map((entry) => entry.module)).toEqual([
      "recently_added",
      "continue",
      "shelf-favorites",
    ])
    expect(moveHomeModule(settings, "continue", -1).map((entry) => entry.module)).toEqual(
      settings.map((entry) => entry.module),
    )
    expect(moveHomeModule(settings, "shelf-favorites", 1).map((entry) => entry.module)).toEqual(
      settings.map((entry) => entry.module),
    )
    expect(moveHomeModule(settings, "not-a-module" as never, 1)).toEqual(settings)
  })

  it("reorders visible modules by preview order and keeps hidden entries out of drag order", () => {
    const settings = defaultHomeModuleSettings().map((entry) =>
      entry.module === "recently_added" ? { ...entry, visible: false } : entry,
    )
    const reordered = reorderHomeModule(settings, "shelf-favorites", 0)
    expect(reordered.filter((entry) => entry.visible).map((entry) => entry.module)).toEqual([
      "shelf-favorites",
      "continue",
    ])
    expect(reordered.filter((entry) => !entry.visible).map((entry) => entry.module)).toEqual([
      "recently_added",
    ])
    expect(homeModuleSettingsToLayout(reordered).modules.map((entry) => entry.module)).toEqual([
      "shelf-favorites",
      "continue",
    ])
    expect(reorderHomeModule(settings, "recently_added", 0).filter((entry) => entry.visible)).toEqual(
      settings.filter((entry) => entry.visible),
    )
  })

  it("changes visibility and size without touching other entries", () => {
    const settings = setHomeModuleSize(defaultHomeModuleSettings(), "continue", "large")
    expect(settings[0]).toEqual({ module: "continue", visible: true, size: "large" })
    const hidden = setHomeModuleVisible(settings, "continue", false)
    expect(hidden[0]?.visible).toBe(false)
    expect(homeModuleSettingsEqual(settings, hidden)).toBe(false)
    expect(setHomeModuleSize(settings, "continue", "huge" as never)[0]?.size).toBe("large")
  })

  it("drives order from the editor, not from the stored coordinates", () => {
    const reordered = moveHomeModule(defaultHomeModuleSettings(), "shelf-favorites", -1)
    const layout = homeModuleSettingsToLayout(reordered)
    expect(layout.modules.map((placement) => [placement.module, placement.row, placement.order])).toEqual([
      ["continue", 0, 0],
      ["shelf-favorites", 0, 1],
      ["recently_added", 1, 2],
    ])
    expect(guardHomeLayout(layout)).toBe(true)
  })
})

describe("home layout CAS operations", () => {
  it("loads the default layout with a null revision and no dirty state", async () => {
    const gateway = mockGateway()
    const snapshot = await gateway.homeLayoutGet()
    expect(snapshot.revision).toBeNull()
    const state = homeLayoutFromSnapshot(snapshot)
    expect(state.status).toBe("ready")
    expect(homeLayoutIsDirty(state)).toBe(false)
  })

  it("saves with the read revision and adopts the revision the backend returns", async () => {
    const gateway = mockGateway()
    const state = homeLayoutFromSnapshot(await gateway.homeLayoutGet())
    const edited = { ...state, modules: setHomeModuleSize(state.modules, "continue", "large") }
    expect(homeLayoutIsDirty(edited)).toBe(true)
    const outcome = await saveHomeLayout(gateway, edited)
    expect(outcome.state.status).toBe("ready")
    expect(outcome.state.revision).not.toBeNull()
    expect(outcome.notice).toBe("首页布局已保存")
    expect(homeLayoutIsDirty(outcome.state)).toBe(false)
    expect(outcome.state.modules[0]).toEqual({ module: "continue", visible: true, size: "large" })
    // 保存后的形态是后端规范化的结果，可再次提交而不产生变化。
    const again = await saveHomeLayout(gateway, outcome.state)
    expect(again.notice).toBeNull()
  })

  it("does not send a save when nothing changed", async () => {
    const client = new MockHavenClient()
    const spy = vi.spyOn(client, "homeLayoutSave")
    const gateway = mockGateway(client)
    const state = homeLayoutFromSnapshot(await gateway.homeLayoutGet())
    const outcome = await saveHomeLayout(gateway, state)
    expect(spy).not.toHaveBeenCalled()
    expect(outcome.state).toBe(state)
    expect(outcome.notice).toBeNull()
  })

  it("reports a stale revision as a conflict and writes nothing", async () => {
    const client = new MockHavenClient()
    const gateway = mockGateway(client)
    const first = homeLayoutFromSnapshot(await gateway.homeLayoutGet())
    const saved = await saveHomeLayout(gateway, {
      ...first,
      modules: setHomeModuleSize(first.modules, "continue", "large"),
    })
    expect(saved.state.revision).not.toBeNull()
    // 另一个窗口又保存了一次：本地 revision 变成过期值。
    await gateway.homeLayoutSave({
      expectedRevision: saved.state.revision,
      layout: homeModuleSettingsToLayout(
        setHomeModuleVisible(saved.state.modules, "recently_added", false),
      ),
    })
    const stale = await saveHomeLayout(gateway, {
      ...saved.state,
      modules: setHomeModuleSize(saved.state.modules, "recently_added", "small"),
    })
    expect(stale.state.status).toBe("conflict")
    expect(stale.state.message).toContain("其他窗口已保存过首页布局")
    // 零写入：磁盘上仍是上一步保存的那份布局。
    const current = await gateway.homeLayoutGet()
    expect(layoutToHomeModuleSettings(current.layout).find((entry) => entry.module === "recently_added")?.visible).toBe(false)
  })

  it("resets back to the domain default and clears the saved revision", async () => {
    const gateway = mockGateway()
    const state = homeLayoutFromSnapshot(await gateway.homeLayoutGet())
    const saved = await saveHomeLayout(gateway, {
      ...state,
      modules: setHomeModuleVisible(state.modules, "continue", false),
    })
    const reset = await resetHomeLayout(gateway, saved.state)
    expect(reset.state.status).toBe("ready")
    expect(reset.state.revision).toBeNull()
    expect(reset.state.modules).toEqual(defaultHomeModuleSettings())
    expect(reset.notice).toBe("首页布局已恢复默认")
  })
})

describe("useHomeLayout", () => {
  it("loads, edits, saves and reports the notice through the hook", async () => {
    const notices: string[] = []
    // gateway 与回调都必须是稳定引用：Hook 的加载 effect 以 gateway 为依赖，
    // 每次渲染都换一个对象会让它反复重载。
    const gateway = mockGateway()
    const pushNotice = (message: string) => { notices.push(message) }
    const { result } = renderHook(() => useHomeLayout(gateway, pushNotice))
    await waitFor(() => expect(result.current.state.status).toBe("ready"))
    expect(result.current.isDirty).toBe(false)
    act(() => result.current.setVisible("continue", false))
    expect(result.current.isDirty).toBe(true)
    await act(async () => { result.current.save() })
    await waitFor(() => expect(result.current.isSaving).toBe(false))
    expect(result.current.state.status).toBe("ready")
    expect(result.current.isDirty).toBe(false)
    expect(notices).toEqual(["首页布局已保存"])
  })

  it("surfaces a failure without losing the draft", async () => {
    const client = new MockHavenClient()
    const gateway = mockGateway(client)
    vi.spyOn(client, "homeLayoutSave").mockImplementation(async () => {
      throw new HavenError({ code: "DATABASE_ERROR", userMessage: "数据库错误", retryable: true })
    })
    const { result } = renderHook(() => useHomeLayout(gateway))
    await waitFor(() => expect(result.current.state.status).toBe("ready"))
    act(() => result.current.setSize("continue", "large"))
    await act(async () => { result.current.save() })
    await waitFor(() => expect(result.current.state.status).toBe("save-error"))
    expect(result.current.isDirty).toBe(true)
    expect(result.current.state.modules[0]?.size).toBe("large")
  })

  it("rebases local changes over the newest saved layout after a conflict", async () => {
    const gateway = mockGateway()
    const { result } = renderHook(() => useHomeLayout(gateway))
    await waitFor(() => expect(result.current.state.status).toBe("ready"))

    const base = result.current.state
    act(() => result.current.setSize("continue", "large"))
    const remote = await gateway.homeLayoutSave({
      expectedRevision: base.revision,
      layout: homeModuleSettingsToLayout(
        setHomeModuleVisible(base.savedModules, "recently_added", false),
      ),
    })

    await act(async () => { result.current.save() })
    await waitFor(() => expect(result.current.state.status).toBe("conflict"))
    act(() => result.current.reload())
    await waitFor(() => {
      expect(result.current.state.status).toBe("ready")
      expect(result.current.state.revision).toBe(remote.revision)
    })

    expect(result.current.state.modules.find((entry) => entry.module === "continue")?.size).toBe("large")
    expect(result.current.state.modules.find((entry) => entry.module === "recently_added")?.visible).toBe(false)
    expect(result.current.isDirty).toBe(true)

    await act(async () => { result.current.save() })
    await waitFor(() => expect(result.current.isDirty).toBe(false))
    const persisted = await gateway.homeLayoutGet()
    const modules = layoutToHomeModuleSettings(persisted.layout)
    expect(modules.find((entry) => entry.module === "continue")?.size).toBe("large")
    expect(modules.find((entry) => entry.module === "recently_added")?.visible).toBe(false)
  })

  it("keeps the local draft when reloading after a conflict fails", async () => {
    const gateway = mockGateway()
    const { result } = renderHook(() => useHomeLayout(gateway))
    await waitFor(() => expect(result.current.state.status).toBe("ready"))

    const base = result.current.state
    act(() => result.current.setSize("continue", "large"))
    await gateway.homeLayoutSave({
      expectedRevision: base.revision,
      layout: homeModuleSettingsToLayout(
        setHomeModuleVisible(base.savedModules, "recently_added", false),
      ),
    })
    await act(async () => { result.current.save() })
    await waitFor(() => expect(result.current.state.status).toBe("conflict"))

    vi.spyOn(gateway, "homeLayoutGet").mockRejectedValue(
      new HavenError({ code: "DATABASE_ERROR", userMessage: "读不到最新布局", retryable: true }),
    )
    act(() => result.current.reload())
    await waitFor(() => {
      expect(result.current.state.status).toBe("conflict")
      expect(result.current.state.message).toContain("重新加载失败")
    })

    expect(result.current.state.modules.find((entry) => entry.module === "continue")?.size).toBe("large")
    expect(result.current.state.revision).toBe(base.revision)
    expect(result.current.isDirty).toBe(true)
  })

  it("coalesces a second save issued before the first one resolves", async () => {
    const client = new MockHavenClient()
    const gateway = mockGateway(client)
    const real = client.homeLayoutSave.bind(client)
    let release!: () => void
    let writes = 0
    const gate = new Promise<void>((resolve) => { release = resolve })
    vi.spyOn(client, "homeLayoutSave").mockImplementation(async (request) => {
      writes += 1
      await gate
      return real(request)
    })

    const { result } = renderHook(() => useHomeLayout(gateway))
    await waitFor(() => expect(result.current.state.status).toBe("ready"))
    act(() => result.current.setVisible("continue", false))

    // 两次点击都发生在第一次请求落地之前。
    act(() => {
      result.current.save()
      result.current.save()
    })

    // 「正在保存」在第一个 await 之前就发布出去了，所以第二次点击读到的是 saving，
    // 不会带着同一个 expectedRevision 再发一次写（那只会撞 REVISION_CONFLICT）。
    expect(result.current.isSaving).toBe(true)
    expect(writes).toBe(1)

    await act(async () => { release() })
    await waitFor(() => expect(result.current.state.status).toBe("ready"))
    expect(result.current.isDirty).toBe(false)
    expect(writes).toBe(1)
  })

  it("coalesces a second reset issued before the first one resolves", async () => {
    const client = new MockHavenClient()
    const gateway = mockGateway(client)
    const { result } = renderHook(() => useHomeLayout(gateway))
    await waitFor(() => expect(result.current.state.status).toBe("ready"))
    // 先真的存一份自定义布局：重置只有在存在保存状态时才有东西可重置。
    act(() => result.current.setSize("continue", "large"))
    await act(async () => { result.current.save() })
    await waitFor(() => expect(result.current.state.revision).not.toBeNull())

    const real = client.homeLayoutReset.bind(client)
    let release!: () => void
    let resets = 0
    const gate = new Promise<void>((resolve) => { release = resolve })
    vi.spyOn(client, "homeLayoutReset").mockImplementation(async (request) => {
      resets += 1
      await gate
      return real(request)
    })

    act(() => {
      result.current.reset()
      result.current.reset()
    })

    expect(result.current.isSaving).toBe(true)
    expect(resets).toBe(1)

    await act(async () => { release() })
    await waitFor(() => expect(result.current.state.status).toBe("ready"))
    expect(result.current.state.revision).toBeNull()
    expect(resets).toBe(1)
  })
})
