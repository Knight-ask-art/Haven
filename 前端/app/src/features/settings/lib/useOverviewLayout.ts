// 设置页总览布局编辑器。
//
// 与 useHomeLayout 同一模式：把异步适配拆成可独立断言的纯函数
// （`saveOverviewLayout` / `resetOverviewLayout`），Hook 只负责请求身份与状态收口。
//
// CAS 语义完全交给后端：每次保存都带上「上一次读取到的 revision」，对不上就是
// `REVISION_CONFLICT` 且零写入。前端不重试、不覆盖——冲突时重新读取最新布局，
// 让用户看着最新事实再决定。
//
// 这份状态与首页布局的编辑器**完全无关**：两个 hook 各读各的快照、各写各的 revision，
// 因此在外观分区里同时挂载也不会互相覆盖。

import { useCallback, useEffect, useRef, useState } from "react"
import { toHavenError } from "@/lib/ipc/errors"
import type { OverviewLayoutSnapshotWire, OverviewLayoutWire } from "@/lib/ipc/settings-wire"
import { defaultOverviewLayout } from "@/lib/ipc/settings-wire"
import type { AppearanceGateway } from "../ipc/appearance-gateway"
import { appearanceGateway } from "../ipc/appearance-gateway"
import { rebaseLayoutModules } from "./layout-rebase"
import type { OverviewModuleId, OverviewModuleSetting, OverviewModuleSize } from "./overview-layout"
import {
  defaultOverviewModuleSettings,
  moveOverviewModule,
  reorderOverviewModule,
  overviewModuleSettingsEqual,
  overviewModuleSettingsToLayout,
  layoutToOverviewModuleSettings,
  setOverviewModuleSize,
  setOverviewModuleVisible,
} from "./overview-layout"

const CONFLICT_CODE = "REVISION_CONFLICT"
/** 冲突提示里的对象名必须说清是**哪一份**布局：同一页面上还有首页布局编辑器。 */
const CONFLICT_HINT = "（其他窗口已保存过总览布局，请重新加载后再改）"
const REBASE_OVERLAP_MESSAGE = "两个窗口修改了相同的布局属性；已保留本机草稿，请检查后再保存。"

export type OverviewLayoutEditorStatus =
  | "loading"
  | "ready"
  | "saving"
  | "error"
  | "conflict"
  | "save-error"

export interface OverviewLayoutEditorState {
  status: OverviewLayoutEditorStatus
  /** 编辑器里的模块清单（含隐藏项）。 */
  modules: OverviewModuleSetting[]
  /** 已保存的模块清单；dirty 就是与它比对得出。 */
  savedModules: OverviewModuleSetting[]
  /**
   * 已保存布局的**原始 wire 形态**（含后端保存的坐标）。
   *
   * 渲染器必须按它排列，而不是把 `savedModules` 重新打包一遍：模块清单是有损投影
   * （顺序 + 可见性 + 档位），坐标是**存储里的既成事实**。前端打包出来的布局当然与它
   * 一致（打包是同一个不动点），但别的写入方给出的合法坐标不必与本前端的打包结果相同，
   * 那时「按清单重算」就会显示一份磁盘上不存在的排列。
   *
   * 非空：与 `modules` / `savedModules` 同一约定——「从未保存过」由 `revision === null`
   * 表达，那时它就是领域默认布局（后端在同一个情形下返回的也正是它）。
   */
  savedLayout: OverviewLayoutWire
  /** 已保存布局的 revision；null = 从未保存过自定义布局。 */
  revision: string | null
  message: string | null
  retryable: boolean
}

export function overviewLayoutIsDirty(state: OverviewLayoutEditorState): boolean {
  return !overviewModuleSettingsEqual(state.modules, state.savedModules)
}

export async function loadOverviewLayout(
  gateway: AppearanceGateway,
): Promise<OverviewLayoutEditorState> {
  try {
    return overviewLayoutFromSnapshot(await gateway.overviewLayoutGet())
  } catch (error) {
    const haven = toHavenError(error)
    return {
      status: "error",
      modules: defaultOverviewModuleSettings(),
      savedModules: defaultOverviewModuleSettings(),
      // 读不到布局时手上的坐标是**未知**的：这里放的是领域默认布局，页面会同时用横幅
      // 说明「以下按默认布局显示」，不会把它冒充成用户保存过的排列。
      savedLayout: defaultOverviewLayout(),
      revision: null,
      message: haven.message,
      retryable: haven.retryable,
    }
  }
}

export function overviewLayoutFromSnapshot(
  snapshot: OverviewLayoutSnapshotWire,
): OverviewLayoutEditorState {
  const modules = layoutToOverviewModuleSettings(snapshot.layout)
  return {
    status: "ready",
    modules,
    savedModules: modules.map((entry) => ({ ...entry })),
    savedLayout: snapshot.layout,
    revision: snapshot.revision,
    message: null,
    retryable: false,
  }
}

/** 编辑动作：进入 ready + 未保存状态；冲突/错误提示随之清空（用户已经开始处理它）。 */
export function overviewLayoutEdit(
  state: OverviewLayoutEditorState,
  modules: OverviewModuleSetting[],
): OverviewLayoutEditorState {
  // 未确认当前 revision 时不能先改默认草稿：初始加载完成后必须采用数据库真值，
  // 而不是把用户刚点的值静默覆盖。冲突/读取错误同样要求先重新读取。
  if (state.status !== "ready" && state.status !== "save-error") return state
  return { ...state, status: "ready", modules, message: null, retryable: false }
}

export interface OverviewLayoutSaveOutcome {
  state: OverviewLayoutEditorState
  /** 需要提示给用户的成功文案；无变化时为 null（不制造假保存提示）。 */
  notice: string | null
}

export async function saveOverviewLayout(
  gateway: AppearanceGateway,
  state: OverviewLayoutEditorState,
): Promise<OverviewLayoutSaveOutcome> {
  if (state.status === "saving") return { state, notice: null }
  if (!overviewLayoutIsDirty(state)) return { state, notice: null }
  const saving: OverviewLayoutEditorState = { ...state, status: "saving", message: null, retryable: false }
  try {
    const result = await gateway.overviewLayoutSave({
      expectedRevision: state.revision,
      layout: overviewModuleSettingsToLayout(state.modules),
    })
    const saved = layoutToOverviewModuleSettings(result.layout)
    return {
      state: {
        status: "ready",
        // 以**后端返回的规范形态**为准回填：清单与坐标都取自它，编辑器不该保留一份
        // 自己以为的排列。
        modules: saved,
        savedModules: saved.map((entry) => ({ ...entry })),
        savedLayout: result.layout,
        revision: result.revision,
        message: null,
        retryable: false,
      },
      notice: result.changed ? "总览布局已保存" : null,
    }
  } catch (error) {
    const haven = toHavenError(error)
    if (haven.code === CONFLICT_CODE) {
      return {
        state: {
          ...saving,
          status: "conflict",
          message: `${haven.message}${CONFLICT_HINT}`,
          retryable: false,
        },
        notice: null,
      }
    }
    return {
      state: {
        ...saving,
        status: "save-error",
        message: haven.message,
        retryable: haven.retryable,
      },
      notice: null,
    }
  }
}

export async function resetOverviewLayout(
  gateway: AppearanceGateway,
  state: OverviewLayoutEditorState,
): Promise<OverviewLayoutSaveOutcome> {
  if (state.status === "saving") return { state, notice: null }
  const saving: OverviewLayoutEditorState = { ...state, status: "saving", message: null, retryable: false }
  try {
    const result = await gateway.overviewLayoutReset({ expectedRevision: state.revision })
    const saved = layoutToOverviewModuleSettings(result.layout)
    return {
      state: {
        status: "ready",
        modules: saved,
        savedModules: saved.map((entry) => ({ ...entry })),
        savedLayout: result.layout,
        revision: result.revision,
        message: null,
        retryable: false,
      },
      notice: result.changed ? "总览布局已恢复默认" : "总览布局已经是默认形态",
    }
  } catch (error) {
    const haven = toHavenError(error)
    if (haven.code === CONFLICT_CODE) {
      return {
        state: {
          ...saving,
          status: "conflict",
          message: `${haven.message}${CONFLICT_HINT}`,
          retryable: false,
        },
        notice: null,
      }
    }
    return {
      state: { ...saving, status: "save-error", message: haven.message, retryable: haven.retryable },
      notice: null,
    }
  }
}

export interface OverviewLayoutController {
  state: OverviewLayoutEditorState
  isDirty: boolean
  isSaving: boolean
  move: (module: OverviewModuleId, delta: number) => void
  reorder: (module: OverviewModuleId, targetVisibleIndex: number) => void
  setSize: (module: OverviewModuleId, size: OverviewModuleSize) => void
  setVisible: (module: OverviewModuleId, visible: boolean) => void
  save: () => void
  reset: () => void
  reload: () => void
}

export function useOverviewLayout(
  gateway: AppearanceGateway = appearanceGateway,
  onNotice?: (message: string) => void,
): OverviewLayoutController {
  const [state, setState] = useState<OverviewLayoutEditorState>(() => ({
    status: "loading",
    modules: defaultOverviewModuleSettings(),
    savedModules: defaultOverviewModuleSettings(),
    savedLayout: defaultOverviewLayout(),
    revision: null,
    message: null,
    retryable: false,
  }))
  const stateRef = useRef(state)
  stateRef.current = state
  const requestId = useRef(0)
  const conflictDraftRef = useRef<OverviewLayoutEditorState | null>(null)

  /**
   * 发布一份新状态：**同步**写进 `stateRef`，再交给 React 渲染。
   *
   * 顺序不能反。React 的渲染是异步的，只调用 `setState` 时 `stateRef.current` 要等到
   * 下一次渲染才更新；在那之前发生的第二次点击读到的仍是旧状态，「已经在飞」这个判断
   * 就拦不住它——同一个 `expectedRevision` 会被提交两次。
   */
  const publish = useCallback((next: OverviewLayoutEditorState): void => {
    stateRef.current = next
    setState(next)
  }, [])

  const load = useCallback(async (): Promise<void> => {
    const current = stateRef.current
    const conflictDraft = current.status === "conflict"
      ? current
      : current.status === "loading" ? conflictDraftRef.current : null
    const shouldRebase = conflictDraft !== null && overviewLayoutIsDirty(conflictDraft)
    if (current.status === "conflict") conflictDraftRef.current = current
    const id = ++requestId.current
    publish({ ...current, status: "loading", message: null })
    const loaded = await loadOverviewLayout(gateway)
    if (id !== requestId.current) return
    conflictDraftRef.current = null
    if (shouldRebase && conflictDraft) {
      if (loaded.status === "error") {
        publish({
          ...conflictDraft,
          status: "conflict",
          message: `重新加载失败：${loaded.message}。本机草稿仍保留，可以重试。`,
          retryable: loaded.retryable,
        })
        return
      }
      const merged = rebaseLayoutModules(
        conflictDraft.savedModules,
        conflictDraft.modules,
        loaded.savedModules,
      )
      publish({
        ...loaded,
        modules: merged.modules,
        message: merged.hasOverlappingChanges ? REBASE_OVERLAP_MESSAGE : null,
      })
      return
    }
    publish(loaded)
  }, [gateway, publish])

  useEffect(() => {
    void load()
  }, [load])

  /**
   * 保存 / 重置的唯一入口。
   *
   * 两条不变量都在这里，而不是在异步操作内部（那里已经太晚）：
   * 1. 已经在飞时重复点击是空操作。否则第二次会带着**同一个** `expectedRevision` 再写
   *    一次：保存撞 `REVISION_CONFLICT`，重置把同一次重置提交两遍。
   * 2. 「正在保存」在第一个 await **之前**发布，并走 `publish` 同步落地，所以同一个 tick
   *    里的第二次点击读到的就是 `saving`，被上面那条判断挡掉。
   *
   * `willWrite` 用来避免一帧假状态：没有任何改动的保存不会写盘，也就不该先闪一下
   * 「正在保存…」。
   */
  const run = useCallback(
    (
      operation: (gateway: AppearanceGateway, current: OverviewLayoutEditorState) => Promise<OverviewLayoutSaveOutcome>,
      willWrite: (current: OverviewLayoutEditorState) => boolean,
    ) => {
      const current = stateRef.current
      if (current.status === "saving") return
      const id = ++requestId.current
      if (willWrite(current)) {
        publish({ ...current, status: "saving", message: null, retryable: false })
      }
      void operation(gateway, current).then((outcome) => {
        if (id !== requestId.current) return
        publish(outcome.state)
        if (outcome.notice) onNotice?.(outcome.notice)
      })
    },
    [gateway, onNotice, publish],
  )

  const edit = useCallback((next: OverviewModuleSetting[]) => {
    publish(overviewLayoutEdit(stateRef.current, next))
  }, [publish])

  return {
    state,
    isDirty: overviewLayoutIsDirty(state),
    isSaving: state.status === "saving",
    move: (module, delta) => edit(moveOverviewModule(stateRef.current.modules, module, delta)),
    reorder: (module, targetVisibleIndex) => edit(reorderOverviewModule(stateRef.current.modules, module, targetVisibleIndex)),
    setSize: (module, size) => edit(setOverviewModuleSize(stateRef.current.modules, module, size)),
    setVisible: (module, visible) => edit(setOverviewModuleVisible(stateRef.current.modules, module, visible)),
    save: () => run(saveOverviewLayout, overviewLayoutIsDirty),
    reset: () => run(resetOverviewLayout, () => true),
    reload: () => void load(),
  }
}
