// Reader 设置 Feature API。
// 文本阅读器只依赖统一 HavenClient，浏览器 Mock 与 Tauri 共用同一份
// reading Section 契约；组件不直接访问 invoke、SQLite 或 localStorage。

import type { HavenClient } from "@/lib/ipc/client"
import type { PreferenceGetResult, ReadingSettingsValue } from "@/lib/ipc/settings-wire"
import { getHavenClient } from "@/lib/ipc/runtime"

export const DEFAULT_READING_SETTINGS: ReadingSettingsValue = {
  section: "reading",
  fontFamily: "serif",
  customFontFamily: null,
  fontSize: "medium",
  lineHeight: "comfortable",
  contentWidth: "medium",
  theme: "warm",
  customBackground: null,
  customText: null,
  fontWeight: "regular",
  letterSpacing: "normal",
  systemAuto: true,
  pagination: "scroll",
}

/** Normalize legacy snapshots so every Reader consumer receives an explicit mode. */
function normalizeReadingSettings(value: ReadingSettingsValue): ReadingSettingsValue {
  return value.pagination === undefined ? { ...value, pagination: "scroll" } : value
}

export async function getReadingSettings(client: Pick<HavenClient, "settingsGet"> = getHavenClient()): Promise<ReadingSettingsValue> {
  const snapshot = await client.settingsGet("reading")
  return snapshot.value.section === "reading" ? normalizeReadingSettings(snapshot.value) : DEFAULT_READING_SETTINGS
}

/**
 * 读取某个 MediaItem 的 effective Reading 设置。
 *
 * 资源级设置必须经过 `preference_get`，由 Rust 合并 global → edition →
 * media item；Reader 不在前端复制回退规则，也不直接接触持久化细节。
 */
export async function getReadingSettingsForResource(
  mediaItemId: string,
  editionId: string,
  client: Pick<HavenClient, "preferenceGet"> = getHavenClient(),
): Promise<ReadingSettingsValue> {
  const result: PreferenceGetResult = await client.preferenceGet({ mediaItemId, editionId })
  return result.effectiveReading.section === "reading" ? normalizeReadingSettings(result.effectiveReading) : DEFAULT_READING_SETTINGS
}
