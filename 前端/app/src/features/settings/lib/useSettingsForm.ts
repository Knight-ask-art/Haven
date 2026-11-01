// Settings 分区表单 Hook（FE-SETTINGS-001）。
// 包装纯状态机（settingsForm.ts）+ SettingsGateway：
// - 首次进入与 section 切换 → 加载快照；
// - loading / dirty / saving / validation-error / REVISION_CONFLICT / retry 全部经状态机；
// - retry 语义：load-error/conflict/validation-error → 重新拉取快照（保留草稿）；
//   save-error（瞬时失败）→ 重试同一 patch。

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import type {
  SettingsPatch,
  SettingsSectionWire,
  SettingsSnapshot,
  SettingsValue,
} from "../../../lib/ipc/settings-wire"
import { toHavenError } from "../../../lib/ipc/errors.js"
import type { FormErrorInfo } from "./settingsForm"
import {
  formChange,
  formDisplayValue,
  formIsDirty,
  formLoadFailed,
  formLoaded,
  formRebase,
  formRebaseFailed,
  formRefresh,
  formResetToDefaults,
  formRetrySave,
  formSave,
  formSaveFailure,
  formSaveSuccess,
  initialFormState,
  type SettingsFormState,
} from "./settingsForm.js"
import type { SettingsGateway } from "../ipc/gateway"

/** `reload` 的可选回源模式；缺省是原有语义（丢弃草稿、重新读取）。 */
export interface SettingsReloadOptions {
  /**
   * 外部写入（例如栖伴批准提案）后的静默回源：不进入 loading，不丢弃本地未保存
   * 改动，只把用户自己的改动重放到最新快照；后台读取失败也不改写页面错误态。
   */
  preserveDraft?: boolean
}

export interface SettingsFormController {
  section: SettingsSectionWire
  state: SettingsFormState
  /** 当前应展示的值（loading → 契约默认值兜底）。 */
  displayValue: SettingsValue
  isLoading: boolean
  isSaving: boolean
  isDirty: boolean
  hasError: boolean
  errorMessage: string | null
  change: (patch: SettingsPatch) => void
  save: () => void
  /** 重新加载快照（load-error / conflict / validation-error 的 retry；保留草稿重新比对）。 */
  retry: () => void
  /**
   * 丢弃本地修改，重新拉取快照。
   *
   * `{ preserveDraft: true }` 用于外部写入后的回源：保留草稿、静默重读；单飞且可重入——
   * 若此刻有保存在途，则等它落定后再读，不用旧快照覆盖保存结果；若此刻已有回源读取在途，
   * 本次失效只登记一次，由在途读取落定后重读（因此连续两次外部写入都会读到）。
   */
  reload: (options?: SettingsReloadOptions) => void
  /** 恢复契约默认值（进入 dirty，等待用户保存）。 */
  resetToDefaults: () => void
}

export function settingsFormErrorInfo(error: unknown): FormErrorInfo {
  const haven = toHavenError(error)
  return { code: haven.code, message: haven.message, retryable: haven.retryable }
}

export interface SettingsSaveOperationResult {
  state: SettingsFormState
  changed: boolean
}

/** Hook 与 runtime fixture check 共用的异步 save 生产路径。 */
export async function runSettingsSaveOperation(
  gateway: SettingsGateway,
  section: SettingsSectionWire,
  savingState: Extract<SettingsFormState, { status: "saving" }>,
): Promise<SettingsSaveOperationResult> {
  try {
    const result = await gateway.settingsUpdate({
      section,
      expectedRevision: savingState.revision,
      patch: savingState.patch,
    })
    const outcome = formSaveSuccess(savingState, result)
    return { state: outcome.state, changed: outcome.changed }
  } catch (error) {
    return { state: formSaveFailure(savingState, settingsFormErrorInfo(error)), changed: false }
  }
}

export type SettingsRebaseFetchResult =
  | { kind: "success"; snapshot: SettingsSnapshot }
  | { kind: "failure"; error: FormErrorInfo }

/** Rebase 拉取的唯一异步适配路径；状态应用由 Hook 的函数式 updater 完成。 */
export async function runSettingsRebaseFetch(
  gateway: SettingsGateway,
  section: SettingsSectionWire,
): Promise<SettingsRebaseFetchResult> {
  try {
    return { kind: "success", snapshot: await gateway.settingsGet(section) }
  } catch (error) {
    return { kind: "failure", error: settingsFormErrorInfo(error) }
  }
}

export function useSettingsForm(
  section: SettingsSectionWire,
  gateway: SettingsGateway,
  onSaved?: (changed: boolean, value: SettingsValue) => void,
): SettingsFormController {
  const [state, setState] = useState<SettingsFormState>(() => initialFormState(section))
  const requestId = useRef(0)
  const stateRef = useRef(state)
  stateRef.current = state
  /** 卸载后到达的读取 / 保存结果一律丢弃（同时作废在途请求的序号）。 */
  const mountedRef = useRef(true)
  /**
   * 外部写入后的回源标记：在**发起读取时消费**，而不是读取完成时清除。读取期间又来一次
   * 失效会重新置位，本轮落定后必然再读一次（两次连续批准都能看到第二次写入）。
   */
  const pendingExternalRefreshRef = useRef(false)
  /** 外部回源单飞循环是否在跑：同一时刻最多一个外部回源读取。 */
  const externalRefreshLoopRef = useRef(false)
  /** 在途 settingsGet 数量（load / rebase / 外部回源共用）；外部回源不与它们并发。 */
  const readInFlightRef = useRef(0)

  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
      requestId.current += 1
      pendingExternalRefreshRef.current = false
    }
  }, [])

  /**
   * 发布一次「正在保存」：**同步**写进 `stateRef`，再交给 React 渲染。
   *
   * React 的渲染是异步的，只调用 `setState` 时 `stateRef.current` 要等到下一次渲染才
   * 更新；在那之前发生的第二次提交读到的仍是 `dirty`，`formSave` 于是把**同一个** patch
   * 再发一次（两个请求带同一个 `expectedRevision`，后一个必然撞 `REVISION_CONFLICT`）。
   * 同步落地之后，第二次提交读到的是 `saving`，`formSave` 返回空 patch，直接成为空操作。
   */
  const publishSaving = useCallback((next: SettingsFormState): void => {
    stateRef.current = next
    setState(next)
  }, [])

  const fetchSnapshot = useCallback(async (mode: "load" | "rebase"): Promise<void> => {
    const id = ++requestId.current
    readInFlightRef.current += 1
    try {
      const result = await runSettingsRebaseFetch(gateway, section)
      if (!mountedRef.current || id !== requestId.current) return
      if (result.kind === "success") {
        setState((current) => (mode === "rebase" ? formRebase(current, result.snapshot) : formLoaded(result.snapshot)))
        return
      }
      setState((current) => (
        mode === "rebase"
          ? formRebaseFailed(current, result.error.message)
          : formLoadFailed(result.error.message, result.error.retryable)
      ))
    } finally {
      readInFlightRef.current -= 1
    }
  }, [gateway, section])

  /**
   * 外部写入后的单飞回源循环（`reload({ preserveDraft: true })` 的唯一执行者）。
   *
   * - 每轮**开始**时消费待回源标记；读取期间到来的新失效会重新置位，本轮落定后必再读，
   *   所以「先发起的读取」不会把后一次写入的失效吞掉；
   * - 保存在途 / 另一次读取在途时暂停，由落定的一方（保存、rebase、load）再次排空；
   * - 读取失败保持静默（不把稳定表单改写成错误态），也不会自行重试——下一次保存仍由
   *   revision 的 CAS 保护；
   * - 被保存 / rebase 取代的读取不发布（序号已变），待回源由发起方重新登记。
   */
  const drainExternalRefresh = useCallback(async (): Promise<void> => {
    if (externalRefreshLoopRef.current) return
    externalRefreshLoopRef.current = true
    try {
      while (mountedRef.current && pendingExternalRefreshRef.current) {
        if (stateRef.current.status === "saving" || readInFlightRef.current > 0) return
        pendingExternalRefreshRef.current = false
        const id = ++requestId.current
        readInFlightRef.current += 1
        try {
          const result = await runSettingsRebaseFetch(gateway, section)
          if (!mountedRef.current || id !== requestId.current) continue
          if (result.kind === "success") setState((current) => formRefresh(current, result.snapshot))
        } finally {
          readInFlightRef.current -= 1
        }
      }
    } finally {
      externalRefreshLoopRef.current = false
    }
  }, [gateway, section])

  const load = useCallback(async () => {
    // 全量读取本身就是权威回源：清掉此前登记的失效；读取期间的新失效仍会在落定后重读。
    pendingExternalRefreshRef.current = false
    setState(initialFormState(section))
    await fetchSnapshot("load")
    void drainExternalRefresh()
  }, [section, fetchSnapshot, drainExternalRefresh])

  useEffect(() => {
    void load()
  }, [load])

  const change = useCallback((patch: SettingsPatch) => {
    setState((current) => formChange(current, patch))
  }, [])

  /**
   * 外部写入后的回源。总是先登记一次失效再尝试排空：保存在途 / 读取在途 / 已有回源
   * 循环时只登记，由当前循环或落定的一方接着读；绝不用旧快照覆盖稍后才落定的保存。
   */
  const refreshAfterExternalChange = useCallback(() => {
    pendingExternalRefreshRef.current = true
    void drainExternalRefresh()
  }, [drainExternalRefresh])

  /**
   * 保存 / rebase 读取要作废一切在途请求。被作废的外部回源不会自己发布（它读到的
   * 可能正是保存前的快照），必须重新登记一次，由落定后的排空再读。
   */
  const supersedeExternalRefresh = useCallback(() => {
    if (externalRefreshLoopRef.current) pendingExternalRefreshRef.current = true
  }, [])

  const save = useCallback(async () => {
    const current = stateRef.current
    const prepared = formSave(current)
    if (!prepared.patch) return
    if (prepared.state.status !== "saving") return
    // 保存会作废在途读取（含外部回源）；作废进来的回源要在保存落定后重新读。
    supersedeExternalRefresh()
    const id = ++requestId.current
    const savingState = prepared.state
    publishSaving(savingState)
    const outcome = await runSettingsSaveOperation(gateway, section, savingState)
    if (!mountedRef.current) return
    if (id !== requestId.current) {
      // 这次保存已被一次更新的读取取代；它的结果不再落地，但外部回源仍要等它结束
      // 再去读，否则读到的可能是一次「保存之前」的快照。
      void drainExternalRefresh()
      return
    }
    stateRef.current = outcome.state
    setState(outcome.state)
    if (outcome.state.status === "ready") onSaved?.(outcome.changed, outcome.state.saved)
    // 保存落定后做权威回源：无论成功还是 CAS 冲突，都要读到保存之后的快照。
    void drainExternalRefresh()
  }, [gateway, section, onSaved, publishSaving, drainExternalRefresh, supersedeExternalRefresh])

  const retry = useCallback(async () => {
    const current = stateRef.current
    if (current.status === "load-error") {
      await load()
      return
    }
    if (current.status === "conflict" || current.status === "validation-error") {
      // rebase 读取同样作废在途回源；被作废的回源要在这个读取落定后重新执行，
      // 否则读取失败时仍停在旧 conflict/dirty 上，下一次保存会写回陈旧字段。
      supersedeExternalRefresh()
      await fetchSnapshot("rebase")
      void drainExternalRefresh()
      return
    }
    if (current.status === "save-error") {
      supersedeExternalRefresh()
      const id = ++requestId.current
      const retryState = formRetrySave(current)
      if (retryState.status !== "saving") return
      publishSaving(retryState)
      const outcome = await runSettingsSaveOperation(gateway, section, retryState)
      // 这次重试被更新的读取取代：它的结果不落地，但待回源要继续。
      if (!mountedRef.current || id !== requestId.current) {
        void drainExternalRefresh()
        return
      }
      stateRef.current = outcome.state
      setState(outcome.state)
      if (outcome.state.status === "ready") onSaved?.(outcome.changed, outcome.state.saved)
      void drainExternalRefresh()
    }
  }, [gateway, section, load, fetchSnapshot, onSaved, publishSaving, drainExternalRefresh, supersedeExternalRefresh])

  const reload = useCallback((options?: SettingsReloadOptions) => {
    if (options?.preserveDraft) {
      refreshAfterExternalChange()
      return
    }
    void load()
  }, [load, refreshAfterExternalChange])

  const resetToDefaults = useCallback(() => {
    setState((current) => formResetToDefaults(current, section))
  }, [section])

  return useMemo(() => ({
    section,
    state,
    displayValue: formDisplayValue(state, section),
    isLoading: state.status === "loading",
    isSaving: state.status === "saving",
    isDirty: formIsDirty(state),
    hasError: state.status === "load-error" || state.status === "save-error"
      || state.status === "validation-error" || state.status === "conflict",
    errorMessage: state.status === "load-error" || state.status === "save-error"
      || state.status === "validation-error" || state.status === "conflict"
      ? state.message
      : null,
    change,
    save,
    retry,
    reload,
    resetToDefaults,
  }), [section, state, change, save, retry, reload, resetToDefaults])
}
