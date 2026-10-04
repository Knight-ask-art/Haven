// 首页布局编辑器（Appearance Stage 1B）。
//
// 与 SettingsFormController 同一模式：把异步适配拆成可独立断言的纯函数
// （`saveHomeLayout` / `resetHomeLayout`），Hook 只负责请求身份与状态收口。
//
// CAS 语义完全交给后端：每次保存都带上「上一次读取到的 revision」，对不上就是
// `REVISION_CONFLICT` 且零写入。前端不重试、不覆盖——冲突时重新读取最新布局，
// 让用户看着最新事实再决定。

import { useCallback, useEffect, useRef, useState } from "react"
import { toHavenError } from "@/lib/ipc/errors"
import type { HomeLayoutSnapshotWire } from "@/lib/ipc/settings-wire"
import type { AppearanceGateway } from "../ipc/appearance-gateway"
import { appearanceGateway } from "../ipc/appearance-gateway"
import { rebaseLayoutModules } from "./layout-rebase"
import type { HomeModuleId, HomeModuleSetting, HomeModuleSize } from "./home-layout"
import {
  defaultHomeModuleSettings,
  homeModuleSettingsEqual,
  homeModuleSettingsToLayout,
  layoutToHomeModuleSettings,
  moveHomeModule,
  reorderHomeModule,
  setHomeModuleSize,
  setHomeModuleVisible,
} from "./home-layout"

const CONFLICT_CODE = "REVISION_CONFLICT"
const REBASE_OVERLAP_MESSAGE = "两个窗口修改了相同的布局属性；已保留本机草稿，请检查后再保存。"

export type HomeLayoutEditorStatus =
  | "loading"
  | "ready"
  | "saving"
  | "error"
  | "conflict"
  | "save-error"

export interface HomeLayoutEditorState {
  status: HomeLayoutEditorStatus
  /** 编辑器里的模块清单（含隐藏项）。 */
  modules: HomeModuleSetting[]
  /** 已保存的模块清单；dirty 就是与它比对得出。 */
  savedModules: HomeModuleSetting[]
  /** 已保存布局的 revision；null = 从未保存过自定义布局。 */
  revision: string | null
  message: string | null
  retryable: boolean
}

export function homeLayoutIsDirty(state: HomeLayoutEditorState): boolean {
  return !homeModuleSettingsEqual(state.modules, state.savedModules)
}

export async function loadHomeLayout(
  gateway: AppearanceGateway,
): Promise<HomeLayoutEditorState> {
  try {
    return homeLayoutFromSnapshot(await gateway.homeLayoutGet())
  } catch (error) {
    const haven = toHavenError(error)
    return {
      status: "error",
      modules: defaultHomeModuleSettings(),
      savedModules: defaultHomeModuleSettings(),
      revision: null,
      message: haven.message,
      retryable: haven.retryable,
    }
  }
}

export function homeLayoutFromSnapshot(snapshot: HomeLayoutSnapshotWire): HomeLayoutEditorState {
  const modules = layoutToHomeModuleSettings(snapshot.layout)
  return {
    status: "ready",
    modules,
    savedModules: modules.map((entry) => ({ ...entry })),
    revision: snapshot.revision,
    message: null,
    retryable: false,
  }
}

/** 编辑动作：进入 ready + 未保存状态；冲突/错误提示随之清空（用户已经开始处理它）。 */
export function homeLayoutEdit(
  state: HomeLayoutEditorState,
  modules: HomeModuleSetting[],
): HomeLayoutEditorState {
  // 未确认当前 revision 时不能先改默认草稿：初始加载完成后必须采用数据库真值，
  // 而不是把用户刚点的值静默覆盖。冲突/读取错误同样要求先重新读取。
  if (state.status !== "ready" && state.status !== "save-error") return state
  return { ...state, status: "ready", modules, message: null, retryable: false }
}

export interface HomeLayoutSaveOutcome {
  state: HomeLayoutEditorState
  /** 需要提示给用户的成功文案；无变化时为 null（不制造假保存提示）。 */
  notice: string | null
}

export async function saveHomeLayout(
  gateway: AppearanceGateway,
  state: HomeLayoutEditorState,
): Promise<HomeLayoutSaveOutcome> {
  if (state.status === "saving") return { state, notice: null }
  if (!homeLayoutIsDirty(state)) return { state, notice: null }
  const saving: HomeLayoutEditorState = { ...state, status: "saving", message: null, retryable: false }
  try {
    const result = await gateway.homeLayoutSave({
      expectedRevision: state.revision,
      layout: homeModuleSettingsToLayout(state.modules),
    })
    const saved = layoutToHomeModuleSettings(result.layout)
    return {
      state: {
        status: "ready",
        // 以**后端返回的规范形态**为准回填：`order` 由领域归一，编辑器不该保留
        // 一份自己以为的坐标。
        modules: saved,
        savedModules: saved.map((entry) => ({ ...entry })),
        revision: result.revision,
        message: null,
        retryable: false,
      },
      notice: result.changed ? "首页布局已保存" : null,
    }
  } catch (error) {
    const haven = toHavenError(error)
    if (haven.code === CONFLICT_CODE) {
      return {
        state: {
          ...saving,
          status: "conflict",
          message: `${haven.message}（其他窗口已保存过首页布局，请重新加载后再改）`,
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

export async function resetHomeLayout(
  gateway: AppearanceGateway,
  state: HomeLayoutEditorState,
): Promise<HomeLayoutSaveOutcome> {
  if (state.status === "saving") return { state, notice: null }
  const saving: HomeLayoutEditorState = { ...state, status: "saving", message: null, retryable: false }
  try {
    const result = await gateway.homeLayoutReset({ expectedRevision: state.revision })
    const saved = layoutToHomeModuleSettings(result.layout)
    return {
      state: {
        status: "ready",
        modules: saved,
        savedModules: saved.map((entry) => ({ ...entry })),
        revision: result.revision,
        message: null,
        retryable: false,
      },
      notice: result.changed ? "首页布局已恢复默认" : "首页布局已经是默认形态",
    }
  } catch (error) {
    const haven = toHavenError(error)
    if (haven.code === CONFLICT_CODE) {
      return {
        state: {
          ...saving,
          status: "conflict",
          message: `${haven.message}（其他窗口已保存过首页布局，请重新加载后再改）`,
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

export interface HomeLayoutController {
  state: HomeLayoutEditorState
  isDirty: boolean
  isSaving: boolean
  move: (module: HomeModuleId, delta: number) => void
  reorder: (module: HomeModuleId, targetVisibleIndex: number) => void
  setSize: (module: HomeModuleId, size: HomeModuleSize) => void
  setVisible: (module: HomeModuleId, visible: boolean) => void
  save: () => void
  reset: () => void
  reload: () => void
}

export function useHomeLayout(
  gateway: AppearanceGateway = appearanceGateway,
  onNotice?: (message: string) => void,
): HomeLayoutController {
  const [state, setState] = useState<HomeLayoutEditorState>(() => ({
    status: "loading",
    modules: defaultHomeModuleSettings(),
    savedModules: defaultHomeModuleSettings(),
    revision: null,
    message: null,
    retryable: false,
  }))
  const stateRef = useRef(state)
  stateRef.current = state
  const requestId = useRef(0)
  const conflictDraftRef = useRef<HomeLayoutEditorState | null>(null)

  /**
   * 发布一份新状态：**同步**写进 `stateRef`，再交给 React 渲染。
   *
   * 顺序不能反。React 的渲染是异步的，只调用 `setState` 时 `stateRef.current` 要等到
   * 下一次渲染才更新；在那之前发生的第二次点击读到的仍是旧状态，「已经在飞」这个判断
   * 就拦不住它——同一个 `expectedRevision` 会被提交两次。
   */
  const publish = useCallback((next: HomeLayoutEditorState): void => {
    stateRef.current = next
    setState(next)
  }, [])

  const load = useCallback(async (): Promise<void> => {
    const current = stateRef.current
    const conflictDraft = current.status === "conflict"
      ? current
      : current.status === "loading" ? conflictDraftRef.current : null
    const shouldRebase = conflictDraft !== null && homeLayoutIsDirty(conflictDraft)
    if (current.status === "conflict") conflictDraftRef.current = current
    const id = ++requestId.current
    publish({ ...current, status: "loading", message: null })
    const loaded = await loadHomeLayout(gateway)
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
   *    一次：保存撞 `REVISION_CONFLICT`（用户看到一条自己没做错什么的冲突），重置把同一
   *    次重置提交两遍。
   * 2. 「正在保存」在第一个 await **之前**发布，并走 `publish` 同步落地，所以同一个 tick
   *    里的第二次点击读到的就是 `saving`，被上面那条判断挡掉。
   *
   * `willWrite` 用来避免一帧假状态：没有任何改动的保存不会写盘，也就不该先闪一下
   * 「正在保存…」。
   */
  const run = useCallback(
    (
      operation: (gateway: AppearanceGateway, current: HomeLayoutEditorState) => Promise<HomeLayoutSaveOutcome>,
      willWrite: (current: HomeLayoutEditorState) => boolean,
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

  const edit = useCallback((next: HomeModuleSetting[]) => {
    publish(homeLayoutEdit(stateRef.current, next))
  }, [publish])

  return {
    state,
    isDirty: homeLayoutIsDirty(state),
    isSaving: state.status === "saving",
    move: (module, delta) => edit(moveHomeModule(stateRef.current.modules, module, delta)),
    reorder: (module, targetVisibleIndex) => edit(reorderHomeModule(stateRef.current.modules, module, targetVisibleIndex)),
    setSize: (module, size) => edit(setHomeModuleSize(stateRef.current.modules, module, size)),
    setVisible: (module, visible) => edit(setHomeModuleVisible(stateRef.current.modules, module, visible)),
    save: () => run(saveHomeLayout, homeLayoutIsDirty),
    reset: () => run(resetHomeLayout, () => true),
    reload: () => void load(),
  }
}
