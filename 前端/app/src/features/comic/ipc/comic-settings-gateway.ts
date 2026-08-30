// Comic Settings Gateway（V02-SETTINGS-READER-COMIC-005B）。
// 漫画 Feature 只依赖统一 HavenClient；不直接访问 Tauri、SQLite 或任意路径。

import type { HavenClient } from "@/lib/ipc/client";
import type { PreferenceGetResult, SettingsSnapshot } from "@/lib/ipc/settings-wire";
import { getHavenClient } from "@/lib/ipc/runtime";

export interface ComicSettingsGateway {
  settingsGet(): Promise<SettingsSnapshot>;
  preferenceGet(mediaItemId: string, editionId: string): Promise<PreferenceGetResult>;
}

export const comicSettingsGateway: ComicSettingsGateway = {
  settingsGet: () => getHavenClient().settingsGet("comic"),
  preferenceGet: (mediaItemId, editionId) => getHavenClient().preferenceGet({ mediaItemId, editionId }),
};

/**
 * 读取某个 MediaItem 的 effective Comic 设置；合并顺序由 Rust
 * ResourcePreferenceService 保持，避免漫画页与设置页出现两套规则。
 */
export async function getComicSettingsForResource(
  mediaItemId: string,
  editionId: string,
  client: Pick<HavenClient, "preferenceGet"> = getHavenClient(),
): Promise<PreferenceGetResult> {
  return client.preferenceGet({ mediaItemId, editionId });
}
