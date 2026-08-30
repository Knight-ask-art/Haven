import { useEffect, useState } from "react"
import type { ReadingSettingsValue } from "@/lib/ipc/settings-wire"
import { DEFAULT_READING_SETTINGS, getReadingSettings, getReadingSettingsForResource } from "../ipc/reading-settings-gateway"

export type ReadingSettingsLoadStatus = "loading" | "ready" | "degraded"

export interface ReadingSettingsState {
  settings: ReadingSettingsValue
  status: ReadingSettingsLoadStatus
  /** Scope key prevents a previous resource response from being applied to a new Session. */
  scopeKey: string
}

function resourceScopeKey(mediaItemId?: string, editionId?: string): string {
  const media = mediaItemId?.trim()
  const edition = editionId?.trim()
  return media && edition ? `resource:${media}:${edition}` : "global"
}

/**
 * 读取文本阅读偏好。传入 MediaItem/Edition 后由后端返回 effective 值；未传入
 * 资源身份时保持原有 global 查询。设置读取故障不会阻塞正文 Session；调用者
 * 继续使用与现有阅读器一致的安全默认值，并可按 status 展示降级状态。
 */
export function useReadingSettings(mediaItemId?: string, editionId?: string): ReadingSettingsState {
  const scopeKey = resourceScopeKey(mediaItemId, editionId)
  const [state, setState] = useState<ReadingSettingsState>({
    settings: DEFAULT_READING_SETTINGS,
    status: "loading",
    scopeKey: "global",
  })

  useEffect(() => {
    let active = true
    setState({ settings: DEFAULT_READING_SETTINGS, status: "loading", scopeKey })
    let load: Promise<ReadingSettingsValue>
    if (scopeKey === "global") {
      load = getReadingSettings()
    } else if (mediaItemId && editionId) {
      load = getReadingSettingsForResource(mediaItemId, editionId)
    } else {
      load = Promise.resolve(DEFAULT_READING_SETTINGS)
    }
    void load
      .then((settings) => {
        if (active) setState({ settings, status: "ready", scopeKey })
      })
      .catch(() => {
        if (active) setState({ settings: DEFAULT_READING_SETTINGS, status: "degraded", scopeKey })
      })
    return () => {
      active = false
    }
  }, [editionId, mediaItemId, scopeKey])

  return state.scopeKey === scopeKey
    ? state
    : { settings: DEFAULT_READING_SETTINGS, status: "loading", scopeKey }
}
