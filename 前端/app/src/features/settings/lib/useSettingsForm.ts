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
  formResetToDefaults,
  formRetrySave,
  formSave,
  formSaveFailure,
  formSaveSuccess,
  initialFormState,
  type SettingsFormState,
} from "./settingsForm.js"
import type { SettingsGateway } from "../ipc/gateway"

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
  /** 丢弃本地修改，重新拉取快照。 */
  reload: () => void
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

  const fetchSnapshot = useCallback(async (rebase: boolean): Promise<void> => {
    const id = ++requestId.current
    const result = await runSettingsRebaseFetch(gateway, section)
    if (id !== requestId.current) return
    if (result.kind === "success") {
      setState((current) => (rebase ? formRebase(current, result.snapshot) : formLoaded(result.snapshot)))
    } else {
      setState((current) => (
        rebase
          ? formRebaseFailed(current, result.error.message)
          : formLoadFailed(result.error.message, result.error.retryable)
      ))
    }
  }, [gateway, section])

  const load = useCallback(async () => {
    setState(initialFormState(section))
    await fetchSnapshot(false)
  }, [section, fetchSnapshot])

  useEffect(() => {
    void load()
  }, [load])

  const change = useCallback((patch: SettingsPatch) => {
    setState((current) => formChange(current, patch))
  }, [])

  const save = useCallback(async () => {
    const current = stateRef.current
    const prepared = formSave(current)
    if (!prepared.patch) return
    if (prepared.state.status !== "saving") return
    const id = ++requestId.current
    const savingState = prepared.state
    setState(savingState)
    const outcome = await runSettingsSaveOperation(gateway, section, savingState)
    if (id !== requestId.current) return
    setState(outcome.state)
    if (outcome.state.status === "ready") onSaved?.(outcome.changed, outcome.state.saved)
  }, [gateway, section, onSaved])

  const retry = useCallback(async () => {
    const current = stateRef.current
    if (current.status === "load-error") {
      await load()
      return
    }
    if (current.status === "conflict" || current.status === "validation-error") {
      await fetchSnapshot(true)
      return
    }
    if (current.status === "save-error") {
      const id = ++requestId.current
      const retryState = formRetrySave(current)
      if (retryState.status !== "saving") return
      setState(retryState)
      const outcome = await runSettingsSaveOperation(gateway, section, retryState)
      if (id !== requestId.current) return
      setState(outcome.state)
      if (outcome.state.status === "ready") onSaved?.(outcome.changed, outcome.state.saved)
    }
  }, [gateway, section, load, fetchSnapshot, onSaved])

  const reload = useCallback(() => {
    void load()
  }, [load])

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
