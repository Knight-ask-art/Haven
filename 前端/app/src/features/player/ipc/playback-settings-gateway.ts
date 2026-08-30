// 播放设置 Feature API：只暴露已经具备播放器消费者的窄范围字段。
// Page/Component 不直接访问 Tauri；所有持久化读取经 Typed HavenClient。

import type { PlaybackRateWire, PlaybackSettingsValue } from "@/lib/ipc/settings-wire"
import { getHavenClient } from "@/lib/ipc/runtime"

export const DEFAULT_PLAYBACK_SETTINGS: PlaybackSettingsValue = {
  section: "playback",
  defaultPlaybackRate: "one",
  autoResume: true,
  autoNext: true,
}

export async function getPlaybackSettings(): Promise<PlaybackSettingsValue> {
  const snapshot = await getHavenClient().settingsGet("playback")
  return snapshot.value.section === "playback" ? snapshot.value : DEFAULT_PLAYBACK_SETTINGS
}

export function playbackRateToNumber(rate: PlaybackRateWire): number {
  switch (rate) {
    case "point_seven_five": return 0.75
    case "one_point_two_five": return 1.25
    case "one_point_five": return 1.5
    case "two": return 2
    case "one":
    default: return 1
  }
}
