// @vitest-environment jsdom
//
// 设置页总览布局的编辑模型与 CAS 操作。
//
// 与首页布局的用例同形，但断言的是**另一份**布局：三列网格、五个闭合模块、自己的
// revision。这里同时钉住两件容易漂移的事：
//   1. 默认清单必须与领域默认布局一致（否则「未做过任何自定义」的用户点一次保存就会
//      把默认版式改成另一种排法）；
//   2. 总览与首页是两份独立事实——任一方向的保存都不得在另一侧造出状态。

import { act, cleanup, renderHook, waitFor } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"
import { MockHavenClient } from "@/lib/ipc/mock-client"
import { HavenError } from "@/lib/ipc/errors"
import {
  defaultHomeLayout,
  defaultOverviewLayout,
  guardOverviewLayout,
} from "@/lib/ipc/settings-wire"
import type { AppearanceGateway } from "../ipc/appearance-gateway"
import {
  OVERVIEW_MODULES,
  defaultOverviewModuleSettings,
  layoutToOverviewModulePlacements,
  layoutToOverviewModuleSettings,
  moveOverviewModule,
  reorderOverviewModule,
  overviewModuleLabel,
  overviewModuleSettingsEqual,
  overviewModuleSettingsToLayout,
  setOverviewModuleSize,
  setOverviewModuleVisible,
} from "./overview-layout"
import {
  overviewLayoutEdit,
  overviewLayoutFromSnapshot,
  overviewLayoutIsDirty,
  resetOverviewLayout,
  saveOverviewLayout,
  useOverviewLayout,
} from "./useOverviewLayout"

/** 用真实的 MockHavenClient 充当 gateway：CAS、幂等与默认布局都走后端那套语义。 */
function mockGateway(client: MockHavenClient = new MockHavenClient()): AppearanceGateway {
  return {
    appearanceAssetsList: (request) => client.appearanceAssetsList(request),
    appearanceAssetImport: (request) => client.appearanceAssetImport(request),
    appearanceAssetDelete: (request) => client.appearanceAssetDelete(request),
    homeLayoutGet: () => client.homeLayoutGet(),
    homeLayoutSave: (request) => client.homeLayoutSave(request),
    homeLayoutReset: (request) => client.homeLayoutReset(request),
    overviewLayoutGet: () => client.overviewLayoutGet(),
    overviewLayoutSave: (request) => client.overviewLayoutSave(request),
    overviewLayoutReset: (request) => client.overviewLayoutReset(request),
  }
}

afterEach(cleanup)

describe("overview layout editor model", () => {
  it("allowlists exactly the five modules the overview really renders", () => {
    expect(OVERVIEW_MODULES.map((module) => module.id)).toEqual([
      "preferences",
      "metrics",
      "reading-minutes",
      "type-share",
      "reading-heatmap",
    ])
    expect(overviewModuleLabel("reading-minutes")).toBe("每日阅读时长")
    // 页头与统计错误横幅不是可管理的模块：它们不在白名单里，因此不可能被隐藏。
    expect(OVERVIEW_MODULES.some((module) => module.id === ("header" as never))).toBe(false)
  })

  it("starts from the domain default layout instead of inventing an arrangement", () => {
    // 默认清单必须与后端返回的默认布局逐项一致——包括两张图的 2:1 档位差。
    const settings = defaultOverviewModuleSettings()
    expect(settings).toEqual([
      { module: "preferences", visible: true, size: "large" },
      { module: "metrics", visible: true, size: "large" },
      { module: "reading-minutes", visible: true, size: "medium" },
      { module: "type-share", visible: true, size: "small" },
      { module: "reading-heatmap", visible: true, size: "large" },
    ])
    // 未改动就保存不会产生变化：清单 → 布局 → 清单是同一个不动点。
    expect(overviewModuleSettingsEqual(settings, layoutToOverviewModuleSettings(defaultOverviewLayout()))).toBe(true)
    expect(overviewModuleSettingsToLayout(settings)).toEqual(defaultOverviewLayout())
  })

  it("marks modules missing from a saved layout as hidden instead of dropping them", () => {
    const settings = layoutToOverviewModuleSettings({
      schemaVersion: 1,
      modules: [{ module: "reading-heatmap", size: "large", row: 0, column: 0, order: 0 }],
    })
    expect(settings).toEqual([
      { module: "reading-heatmap", visible: true, size: "large" },
      { module: "preferences", visible: false, size: "medium" },
      { module: "metrics", visible: false, size: "medium" },
      { module: "reading-minutes", visible: false, size: "medium" },
      { module: "type-share", visible: false, size: "medium" },
    ])
  })

  it("packs every size into non-overlapping positions that match the three-column grid", () => {
    const expectedCoordinates = {
      // 三列网格：small 1 列、medium 2 列、large 整行。
      // medium 占 2/3 宽，放不下「两个并排」，所以每个 medium 自成一行。
      small: [[0, 0], [0, 1], [0, 2], [1, 0], [1, 1]],
      medium: [[0, 0], [1, 0], [2, 0], [3, 0], [4, 0]],
      large: [[0, 0], [1, 0], [2, 0], [3, 0], [4, 0]],
    } as const
    for (const size of ["small", "medium", "large"] as const) {
      const settings = defaultOverviewModuleSettings().map((entry) => ({ ...entry, size }))
      const layout = overviewModuleSettingsToLayout(settings)
      expect(guardOverviewLayout(layout), `${size} 布局必须合法`).toBe(true)
      expect(layout.modules.map((placement) => placement.order)).toEqual([0, 1, 2, 3, 4])
      expect(layout.modules.map((placement) => [placement.row, placement.column])).toEqual(
        expectedCoordinates[size],
      )
    }
  })

  it("keeps the two charts on one row at 2:1 in the default arrangement", () => {
    const placements = layoutToOverviewModulePlacements(defaultOverviewLayout())
    const minutes = placements.find((placement) => placement.module === "reading-minutes")
    const shares = placements.find((placement) => placement.module === "type-share")
    expect(minutes?.row).toBe(shares?.row)
    expect(minutes?.column).toBe(0)
    expect(shares?.column).toBe(2)
    // 这就是升级前 `1.92fr:1fr` 的那一档：3 列网格里唯一能表达 2:1 的摆法。
    expect([minutes?.size, shares?.size]).toEqual(["medium", "small"])
  })

  it("uses saved positions for the renderer while preserving the user's order", () => {
    const saved = {
      schemaVersion: 1 as const,
      modules: [
        { module: "type-share" as const, size: "small" as const, row: 1, column: 2, order: 1 },
        { module: "metrics" as const, size: "medium" as const, row: 0, column: 0, order: 0 },
      ],
    }
    expect(layoutToOverviewModulePlacements(saved).map(({ module, row, column, order }) => [
      module,
      row,
      column,
      order,
    ])).toEqual([
      ["metrics", 0, 0, 0],
      ["type-share", 1, 2, 1],
    ])
  })

  it("treats hiding every module as an explicit empty layout", () => {
    const hidden = defaultOverviewModuleSettings().map((entry) => ({ ...entry, visible: false }))
    const layout = overviewModuleSettingsToLayout(hidden)
    expect(layout).toEqual({ schemaVersion: 1, modules: [] })
    expect(guardOverviewLayout(layout)).toBe(true)
    expect(layoutToOverviewModuleSettings(layout).every((entry) => !entry.visible)).toBe(true)
  })

  it("refuses arbitrary module ids even if they reach the builder", () => {
    const layout = overviewModuleSettingsToLayout([
      { module: "metrics", visible: true, size: "medium" },
      { module: "evil-module" as never, visible: true, size: "large" },
      // 首页的模块 ID 同样不属于总览。
      { module: "shelf-favorites" as never, visible: true, size: "large" },
    ])
    expect(layout.modules.map((placement) => placement.module)).toEqual(["metrics"])
  })

  it("reorders within bounds and no-ops at the edges", () => {
    const settings = defaultOverviewModuleSettings()
    expect(moveOverviewModule(settings, "metrics", -1).map((entry) => entry.module)).toEqual([
      "metrics",
      "preferences",
      "reading-minutes",
      "type-share",
      "reading-heatmap",
    ])
    expect(moveOverviewModule(settings, "preferences", -1).map((entry) => entry.module)).toEqual(
      settings.map((entry) => entry.module),
    )
    expect(moveOverviewModule(settings, "reading-heatmap", 1).map((entry) => entry.module)).toEqual(
      settings.map((entry) => entry.module),
    )
    expect(moveOverviewModule(settings, "not-a-module" as never, 1)).toEqual(settings)
  })

  it("reorders only visible cards and keeps hidden cards out of the saved preview order", () => {
    const settings = defaultOverviewModuleSettings().map((entry) =>
      entry.module === "metrics" ? { ...entry, visible: false } : entry,
    )
    const reordered = reorderOverviewModule(settings, "reading-heatmap", 0)
    expect(reordered.filter((entry) => entry.visible).map((entry) => entry.module)).toEqual([
      "reading-heatmap",
      "preferences",
      "reading-minutes",
      "type-share",
    ])
    expect(reordered.filter((entry) => !entry.visible).map((entry) => entry.module)).toEqual([
      "metrics",
    ])
    expect(overviewModuleSettingsToLayout(reordered).modules.map((entry) => entry.module)).toEqual([
      "reading-heatmap",
      "preferences",
      "reading-minutes",
      "type-share",
    ])
    expect(reorderOverviewModule(settings, "metrics", 0).filter((entry) => entry.visible)).toEqual(
      settings.filter((entry) => entry.visible),
    )
  })

  it("changes visibility and size without touching other entries", () => {
    const settings = setOverviewModuleSize(defaultOverviewModuleSettings(), "metrics", "small")
    expect(settings[1]).toEqual({ module: "metrics", visible: true, size: "small" })
    const hidden = setOverviewModuleVisible(settings, "metrics", false)
    expect(hidden[1]?.visible).toBe(false)
    expect(overviewModuleSettingsEqual(settings, hidden)).toBe(false)
    expect(setOverviewModuleSize(settings, "metrics", "huge" as never)[1]?.size).toBe("small")
  })

  it("drives order from the editor, not from the stored coordinates", () => {
    const reordered = moveOverviewModule(defaultOverviewModuleSettings(), "type-share", -1)
    const layout = overviewModuleSettingsToLayout(reordered)
    expect(layout.modules.map((placement) => [placement.module, placement.row, placement.column, placement.order])).toEqual([
      ["preferences", 0, 0, 0],
      ["metrics", 1, 0, 1],
      ["type-share", 2, 0, 2],
      ["reading-minutes", 2, 1, 3],
      ["reading-heatmap", 3, 0, 4],
    ])
    expect(guardOverviewLayout(layout)).toBe(true)
  })

  it("ignores edits until the persisted revision is known", () => {
    const loading = {
      status: "loading" as const,
      modules: defaultOverviewModuleSettings(),
      savedModules: defaultOverviewModuleSettings(),
      savedLayout: defaultOverviewLayout(),
      revision: null,
      message: null,
      retryable: false,
    }
    const edited = moveOverviewModule(loading.modules, "metrics", -1)
    expect(overviewLayoutEdit(loading, edited)).toBe(loading)

    const conflict = { ...loading, status: "conflict" as const }
    expect(overviewLayoutEdit(conflict, edited)).toBe(conflict)
  })
})

describe("overview layout CAS operations", () => {
  it("loads the default layout with a null revision and no dirty state", async () => {
    const gateway = mockGateway()
    const snapshot = await gateway.overviewLayoutGet()
    expect(snapshot.revision).toBeNull()
    expect(snapshot.layout).toEqual(defaultOverviewLayout())
    const state = overviewLayoutFromSnapshot(snapshot)
    expect(state.status).toBe("ready")
    expect(overviewLayoutIsDirty(state)).toBe(false)
  })

  it("saves with the read revision and adopts the revision the backend returns", async () => {
    const gateway = mockGateway()
    const state = overviewLayoutFromSnapshot(await gateway.overviewLayoutGet())
    const edited = { ...state, modules: setOverviewModuleSize(state.modules, "type-share", "large") }
    expect(overviewLayoutIsDirty(edited)).toBe(true)
    const outcome = await saveOverviewLayout(gateway, edited)
    expect(outcome.state.status).toBe("ready")
    expect(outcome.state.revision).not.toBeNull()
    expect(outcome.notice).toBe("总览布局已保存")
    expect(overviewLayoutIsDirty(outcome.state)).toBe(false)
    expect(outcome.state.modules.find((entry) => entry.module === "type-share")?.size).toBe("large")
    // 保存后的形态是后端规范化的结果，可再次提交而不产生变化（changed=false → 无提示）。
    const again = await saveOverviewLayout(gateway, outcome.state)
    expect(again.notice).toBeNull()
  })

  it("does not send a save when nothing changed", async () => {
    const client = new MockHavenClient()
    const spy = vi.spyOn(client, "overviewLayoutSave")
    const gateway = mockGateway(client)
    const state = overviewLayoutFromSnapshot(await gateway.overviewLayoutGet())
    const outcome = await saveOverviewLayout(gateway, state)
    expect(spy).not.toHaveBeenCalled()
    expect(outcome.state).toBe(state)
    expect(outcome.notice).toBeNull()
  })

  it("reports a stale revision as a conflict and writes nothing", async () => {
    const client = new MockHavenClient()
    const gateway = mockGateway(client)
    const first = overviewLayoutFromSnapshot(await gateway.overviewLayoutGet())
    const saved = await saveOverviewLayout(gateway, {
      ...first,
      modules: setOverviewModuleSize(first.modules, "metrics", "small"),
    })
    expect(saved.state.revision).not.toBeNull()
    // 另一个窗口又保存了一次：本地 revision 变成过期值。
    await gateway.overviewLayoutSave({
      expectedRevision: saved.state.revision,
      layout: overviewModuleSettingsToLayout(
        setOverviewModuleVisible(saved.state.modules, "reading-heatmap", false),
      ),
    })
    const stale = await saveOverviewLayout(gateway, {
      ...saved.state,
      modules: setOverviewModuleSize(saved.state.modules, "metrics", "large"),
    })
    expect(stale.state.status).toBe("conflict")
    // 冲突文案必须说清是**哪一份**布局：同一页面上还有首页布局编辑器。
    expect(stale.state.message).toContain("其他窗口已保存过总览布局")
    // 零写入：磁盘上仍是上一步保存的那份布局。
    const current = await gateway.overviewLayoutGet()
    expect(
      layoutToOverviewModuleSettings(current.layout).find((entry) => entry.module === "reading-heatmap")?.visible,
    ).toBe(false)
  })

  it("resets back to the domain default and clears the saved revision", async () => {
    const gateway = mockGateway()
    const state = overviewLayoutFromSnapshot(await gateway.overviewLayoutGet())
    const saved = await saveOverviewLayout(gateway, {
      ...state,
      modules: setOverviewModuleVisible(state.modules, "metrics", false),
    })
    const reset = await resetOverviewLayout(gateway, saved.state)
    expect(reset.state.status).toBe("ready")
    expect(reset.state.revision).toBeNull()
    expect(reset.state.modules).toEqual(defaultOverviewModuleSettings())
    expect(reset.notice).toBe("总览布局已恢复默认")
  })

  it("keeps the two layouts independent across a full editor round trip", async () => {
    const gateway = mockGateway()
    const overview = overviewLayoutFromSnapshot(await gateway.overviewLayoutGet())
    await saveOverviewLayout(gateway, {
      ...overview,
      modules: setOverviewModuleVisible(overview.modules, "metrics", false),
    })

    // 保存总览没有在首页那一侧造出任何保存状态。
    expect(await gateway.homeLayoutGet()).toEqual({ layout: defaultHomeLayout(), revision: null })

    // 首页保存也不会覆盖总览的排列。
    await gateway.homeLayoutSave({ expectedRevision: null, layout: defaultHomeLayout() })
    const afterHomeSave = await gateway.overviewLayoutGet()
    expect(
      layoutToOverviewModuleSettings(afterHomeSave.layout).find((entry) => entry.module === "metrics")?.visible,
    ).toBe(false)
  })
})

describe("useOverviewLayout", () => {
  it("loads, edits, saves and reports the notice through the hook", async () => {
    const notices: string[] = []
    // gateway 与回调都必须是稳定引用：Hook 的加载 effect 以 gateway 为依赖。
    const gateway = mockGateway()
    const pushNotice = (message: string) => { notices.push(message) }
    const { result } = renderHook(() => useOverviewLayout(gateway, pushNotice))
    await waitFor(() => expect(result.current.state.status).toBe("ready"))
    expect(result.current.isDirty).toBe(false)
    act(() => result.current.setVisible("metrics", false))
    expect(result.current.isDirty).toBe(true)
    await act(async () => { result.current.save() })
    await waitFor(() => expect(result.current.isSaving).toBe(false))
    expect(result.current.state.status).toBe("ready")
    expect(result.current.isDirty).toBe(false)
    expect(notices).toEqual(["总览布局已保存"])
  })

  it("surfaces a failure without losing the draft", async () => {
    const client = new MockHavenClient()
    const gateway = mockGateway(client)
    vi.spyOn(client, "overviewLayoutSave").mockImplementation(async () => {
      throw new HavenError({ code: "DATABASE_ERROR", userMessage: "数据库错误", retryable: true })
    })
    const { result } = renderHook(() => useOverviewLayout(gateway))
    await waitFor(() => expect(result.current.state.status).toBe("ready"))
    act(() => result.current.setSize("metrics", "small"))
    await act(async () => { result.current.save() })
    await waitFor(() => expect(result.current.state.status).toBe("save-error"))
    expect(result.current.isDirty).toBe(true)
    expect(result.current.state.modules[1]?.size).toBe("small")
  })

  it("rebases local changes over the newest saved layout after a conflict", async () => {
    const gateway = mockGateway()
    const { result } = renderHook(() => useOverviewLayout(gateway))
    await waitFor(() => expect(result.current.state.status).toBe("ready"))

    const base = result.current.state
    act(() => result.current.setSize("metrics", "small"))
    const remote = await gateway.overviewLayoutSave({
      expectedRevision: base.revision,
      layout: overviewModuleSettingsToLayout(
        setOverviewModuleVisible(base.savedModules, "reading-heatmap", false),
      ),
    })

    await act(async () => { result.current.save() })
    await waitFor(() => expect(result.current.state.status).toBe("conflict"))
    act(() => result.current.reload())
    await waitFor(() => {
      expect(result.current.state.status).toBe("ready")
      expect(result.current.state.revision).toBe(remote.revision)
    })

    expect(result.current.state.modules.find((entry) => entry.module === "metrics")?.size).toBe("small")
    expect(result.current.state.modules.find((entry) => entry.module === "reading-heatmap")?.visible).toBe(false)
    expect(result.current.isDirty).toBe(true)

    await act(async () => { result.current.save() })
    await waitFor(() => expect(result.current.isDirty).toBe(false))
    const persisted = await gateway.overviewLayoutGet()
    const modules = layoutToOverviewModuleSettings(persisted.layout)
    expect(modules.find((entry) => entry.module === "metrics")?.size).toBe("small")
    expect(modules.find((entry) => entry.module === "reading-heatmap")?.visible).toBe(false)
  })

  it("keeps the local choice and reports overlapping edits after a conflict", async () => {
    const gateway = mockGateway()
    const { result } = renderHook(() => useOverviewLayout(gateway))
    await waitFor(() => expect(result.current.state.status).toBe("ready"))

    const base = result.current.state
    act(() => result.current.setSize("metrics", "small"))
    await gateway.overviewLayoutSave({
      expectedRevision: base.revision,
      layout: overviewModuleSettingsToLayout(
        setOverviewModuleSize(base.savedModules, "metrics", "medium"),
      ),
    })
    await act(async () => { result.current.save() })
    await waitFor(() => expect(result.current.state.status).toBe("conflict"))

    act(() => result.current.reload())
    await waitFor(() => {
      expect(result.current.state.status).toBe("ready")
      expect(result.current.state.message).toContain("保留本机草稿")
    })
    expect(result.current.state.modules.find((entry) => entry.module === "metrics")?.size).toBe("small")
    expect(result.current.state.savedModules.find((entry) => entry.module === "metrics")?.size).toBe("medium")
    expect(result.current.isDirty).toBe(true)
  })

  it("keeps the local draft when reloading after a conflict fails", async () => {
    const gateway = mockGateway()
    const { result } = renderHook(() => useOverviewLayout(gateway))
    await waitFor(() => expect(result.current.state.status).toBe("ready"))

    const base = result.current.state
    act(() => result.current.setSize("metrics", "small"))
    await gateway.overviewLayoutSave({
      expectedRevision: base.revision,
      layout: overviewModuleSettingsToLayout(
        setOverviewModuleVisible(base.savedModules, "reading-heatmap", false),
      ),
    })
    await act(async () => { result.current.save() })
    await waitFor(() => expect(result.current.state.status).toBe("conflict"))

    vi.spyOn(gateway, "overviewLayoutGet").mockRejectedValue(
      new HavenError({ code: "DATABASE_ERROR", userMessage: "读不到最新布局", retryable: true }),
    )
    act(() => result.current.reload())
    await waitFor(() => {
      expect(result.current.state.status).toBe("conflict")
      expect(result.current.state.message).toContain("重新加载失败")
    })

    expect(result.current.state.modules.find((entry) => entry.module === "metrics")?.size).toBe("small")
    expect(result.current.state.revision).toBe(base.revision)
    expect(result.current.isDirty).toBe(true)
  })

  it("keeps the last known revision when the read fails", async () => {
    const client = new MockHavenClient()
    const gateway = mockGateway(client)
    vi.spyOn(client, "overviewLayoutGet").mockImplementation(async () => {
      throw new HavenError({ code: "DATABASE_ERROR", userMessage: "读不到布局", retryable: true })
    })
    const { result } = renderHook(() => useOverviewLayout(gateway))
    await waitFor(() => expect(result.current.state.status).toBe("error"))
    // 读不到事实时 revision 必须是 null（= 从未保存过），而不是一个编出来的 token：
    // 这时点保存只会撞上 REVISION_CONFLICT，所以编辑器据此禁用保存/重置。
    expect(result.current.state.revision).toBeNull()
    expect(result.current.state.message).toBe("读不到布局")
    expect(result.current.isDirty).toBe(false)
  })

  it("coalesces a second save issued before the first one resolves", async () => {
    const client = new MockHavenClient()
    const gateway = mockGateway(client)
    const real = client.overviewLayoutSave.bind(client)
    let release!: () => void
    let writes = 0
    const gate = new Promise<void>((resolve) => { release = resolve })
    vi.spyOn(client, "overviewLayoutSave").mockImplementation(async (request) => {
      writes += 1
      await gate
      return real(request)
    })

    const { result } = renderHook(() => useOverviewLayout(gateway))
    await waitFor(() => expect(result.current.state.status).toBe("ready"))
    act(() => result.current.setVisible("metrics", false))

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
    const { result } = renderHook(() => useOverviewLayout(gateway))
    await waitFor(() => expect(result.current.state.status).toBe("ready"))
    // 先真的存一份自定义布局：重置只有在存在保存状态时才有东西可重置。
    act(() => result.current.setSize("type-share", "large"))
    await act(async () => { result.current.save() })
    await waitFor(() => expect(result.current.state.revision).not.toBeNull())

    const real = client.overviewLayoutReset.bind(client)
    let release!: () => void
    let resets = 0
    const gate = new Promise<void>((resolve) => { release = resolve })
    vi.spyOn(client, "overviewLayoutReset").mockImplementation(async (request) => {
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
