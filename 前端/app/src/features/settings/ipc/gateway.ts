// Settings Gateway（FE-SETTINGS-001 / IPC-MOCK-SETTINGS-001）。
// Feature 只依赖统一 HavenClient；Runtime 决定 Tauri、显式 Mock 或不可用状态。

import type {
  PreferenceGetRequest,
  PreferenceGetResult,
  PreferenceUpdateRequest,
  PreferenceUpdateResult,
  SettingsSectionWire,
  SettingsSnapshot,
  SettingsUpdateRequest,
  SettingsUpdateResult,
} from "../../../lib/ipc/settings-wire.js";
import { getHavenClient } from "../../../lib/ipc/runtime.js";

export interface SettingsGateway {
  settingsGet(section: SettingsSectionWire): Promise<SettingsSnapshot>;
  settingsUpdate(request: SettingsUpdateRequest): Promise<SettingsUpdateResult>;
  preferenceGet(request: PreferenceGetRequest): Promise<PreferenceGetResult>;
  preferenceUpdate(request: PreferenceUpdateRequest): Promise<PreferenceUpdateResult>;
}

/** Runtime-selected gateway; unavailable browser production fails at the client boundary. */
export const settingsGateway: SettingsGateway = {
  settingsGet: (section) => getHavenClient().settingsGet(section),
  settingsUpdate: (request) => getHavenClient().settingsUpdate(request),
  preferenceGet: (request) => getHavenClient().preferenceGet(request),
  preferenceUpdate: (request) => getHavenClient().preferenceUpdate(request),
};
