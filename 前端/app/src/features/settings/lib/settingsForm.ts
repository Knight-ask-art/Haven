// Settings 表单状态机（纯函数，无 React 依赖；FE-SETTINGS-001）。
// 状态：loading / ready / dirty / saving / load-error / save-error / validation-error / conflict。
// 不变量（本轮验收）：
// - 相同值或空 patch 不进入 dirty（不显示"已修改"），不制造假保存状态；
// - REVISION_CONFLICT → conflict（retry = 重新加载快照并保留本地修改）；
// - INVALID_ARGUMENT → validation-error；其余失败 → save-error（retry = 重试同一 patch）；
// - changed=false（幂等）不产生"已保存"提示（savedHint 只由 changed=true 触发）。

import type {
  SettingsPatch,
  SettingsSectionWire,
  SettingsSnapshot,
  SettingsUpdateResult,
  SettingsValue,
} from "../../../lib/ipc/settings-wire";
import {
  applySettingsPatch,
  buildSettingsPatch,
  defaultSettingsValue,
  settingsValuesEqual,
} from "../../../lib/ipc/settings-wire.js";

export type SettingsFormState =
  | { status: "loading" }
  | { status: "ready"; saved: SettingsValue; revision: string | null }
  | { status: "dirty"; saved: SettingsValue; revision: string | null; draft: SettingsValue }
  | { status: "saving"; saved: SettingsValue; revision: string | null; draft: SettingsValue; patch: SettingsPatch }
  | { status: "load-error"; message: string; retryable: boolean }
  | { status: "save-error"; saved: SettingsValue; revision: string | null; draft: SettingsValue; patch: SettingsPatch; message: string; retryable: boolean }
  | { status: "validation-error"; saved: SettingsValue; revision: string | null; draft: SettingsValue; message: string }
  | { status: "conflict"; saved: SettingsValue; revision: string | null; draft: SettingsValue; message: string };

/** 后端错误归一化形状（供状态机消费）。 */
export interface FormErrorInfo {
  code: string;
  message: string;
  retryable: boolean;
}

interface FormBase {
  saved: SettingsValue;
  revision: string | null;
  draft: SettingsValue;
}

function baseOf(state: SettingsFormState): FormBase | null {
  switch (state.status) {
    case "loading":
    case "load-error":
      return null;
    case "ready":
      return { saved: state.saved, revision: state.revision, draft: state.saved };
    case "dirty":
    case "saving":
    case "save-error":
    case "validation-error":
    case "conflict":
      return { saved: state.saved, revision: state.revision, draft: state.draft };
  }
}

export function initialFormState(_section: SettingsSectionWire): SettingsFormState {
  return { status: "loading" };
}

export function formLoaded(snapshot: SettingsSnapshot): SettingsFormState {
  return { status: "ready", saved: snapshot.value, revision: snapshot.revision };
}

export function formLoadFailed(message: string, retryable: boolean): SettingsFormState {
  return { status: "load-error", message, retryable };
}

/** 字段修改：与已保存值相同 → 回到 ready（不制造 dirty）；真实差异 → dirty。 */
export function formChange(state: SettingsFormState, patch: SettingsPatch): SettingsFormState {
  if (state.status === "loading" || state.status === "load-error" || state.status === "saving") return state;
  const base = baseOf(state);
  if (!base) return state;
  const draft = applySettingsPatch(base.draft, patch);
  return settingsValuesEqual(draft, base.saved)
    ? { status: "ready", saved: base.saved, revision: base.revision }
    : { status: "dirty", saved: base.saved, revision: base.revision, draft };
}

/**
 * 提交：只允许 dirty → saving。
 * 空 patch（理论上不可达，防御）→ 直接回 ready，不制造假保存状态。
 */
export function formSave(state: SettingsFormState): { state: SettingsFormState; patch: SettingsPatch | null } {
  if (state.status !== "dirty") return { state, patch: null };
  const patch = buildSettingsPatch(state.saved, state.draft);
  if (!patch) {
    return { state: { status: "ready", saved: state.saved, revision: state.revision }, patch: null };
  }
  return { state: { status: "saving", saved: state.saved, revision: state.revision, draft: state.draft, patch }, patch };
}

/** 保存成功：changed=true → 提示（savedHint）；changed=false → 静默收敛，同样不显示"已修改"。 */
export function formSaveSuccess(state: SettingsFormState, result: SettingsUpdateResult): { state: SettingsFormState; changed: boolean } {
  if (state.status !== "saving") return { state, changed: false };
  return {
    state: { status: "ready", saved: result.value, revision: result.revision },
    changed: result.changed,
  };
}

export function formSaveFailure(state: SettingsFormState, error: FormErrorInfo): SettingsFormState {
  if (state.status !== "saving") return state;
  const base: FormBase = { saved: state.saved, revision: state.revision, draft: state.draft };
  if (error.code === "REVISION_CONFLICT") {
    return { status: "conflict", ...base, message: error.message };
  }
  if (error.code === "INVALID_ARGUMENT") {
    return { status: "validation-error", ...base, message: error.message };
  }
  return { status: "save-error", ...base, patch: state.patch, message: error.message, retryable: error.retryable };
}

/** save-error 的 retry：携带同一 patch 重新进入 saving（瞬时失败可安全重试）。 */
export function formRetrySave(state: SettingsFormState): SettingsFormState {
  if (state.status !== "save-error") return state;
  return { status: "saving", saved: state.saved, revision: state.revision, draft: state.draft, patch: state.patch };
}

/**
 * 重新加载快照（conflict / validation-error 的 retry）：保留本地 draft，以最新 revision 重新比对。
 * draft 与新 saved 相同（数据已被其他窗口保存）→ ready；否则保持 dirty。
 */
export function formRebase(state: SettingsFormState, snapshot: SettingsSnapshot): SettingsFormState {
  const base = baseOf(state);
  if (!base) return { status: "ready", saved: snapshot.value, revision: snapshot.revision };
  return settingsValuesEqual(base.draft, snapshot.value)
    ? { status: "ready", saved: snapshot.value, revision: snapshot.revision }
    : { status: "dirty", saved: snapshot.value, revision: snapshot.revision, draft: base.draft };
}

/** Rebase 拉取失败：保留 conflict/validation 的本地草稿与 revision；其他状态不覆盖。 */
export function formRebaseFailed(state: SettingsFormState, message: string): SettingsFormState {
  if (state.status === "conflict" || state.status === "validation-error") {
    return { ...state, message };
  }
  return state;
}

/** 恢复默认：draft = 契约默认值；与已保存值相同 → ready，否则 dirty。 */
export function formResetToDefaults(state: SettingsFormState, section: SettingsSectionWire): SettingsFormState {
  if (state.status === "saving") return state;
  const base = baseOf(state);
  if (!base) return state;
  const defaults = defaultSettingsValue(section);
  return settingsValuesEqual(defaults, base.saved)
    ? { status: "ready", saved: base.saved, revision: base.revision }
    : { status: "dirty", saved: base.saved, revision: base.revision, draft: defaults };
}

/** 当前应展示的值（loading/load-error → 契约默认值兜底）。 */
export function formDisplayValue(state: SettingsFormState, section: SettingsSectionWire): SettingsValue {
  const base = baseOf(state);
  if (!base) return defaultSettingsValue(section);
  return base.draft;
}

export function formIsDirty(state: SettingsFormState): boolean {
  return state.status === "dirty" || state.status === "saving" || state.status === "save-error"
    || state.status === "validation-error" || state.status === "conflict";
}
