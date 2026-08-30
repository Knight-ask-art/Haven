import { useEffect, useState } from "react"
import { getHavenClientMode } from "@/lib/ipc/runtime"
import {
  DEFAULT_PLAYBACK_SETTINGS,
  getPlaybackSettings,
  playbackRateToNumber,
} from "../ipc/playback-settings-gateway"

export type PlaybackSettingsRuntime = {
  defaultPlaybackRate: number
  autoResume: boolean
  autoNext: boolean
  status: "loading" | "ready" | "degraded"
}

export function usePlaybackSettings(): PlaybackSettingsRuntime {
  const mode = getHavenClientMode()
  const [state, setState] = useState<PlaybackSettingsRuntime>(() => ({
    defaultPlaybackRate: playbackRateToNumber(DEFAULT_PLAYBACK_SETTINGS.defaultPlaybackRate),
    autoResume: DEFAULT_PLAYBACK_SETTINGS.autoResume,
    autoNext: DEFAULT_PLAYBACK_SETTINGS.autoNext,
    status: "loading",
  }))

  useEffect(() => {
    if (mode !== "tauri" && mode !== "mock") {
      setState((current) => ({ ...current, status: "degraded" }))
      return undefined
    }
    let active = true
    setState((current) => ({ ...current, status: "loading" }))
    void getPlaybackSettings()
      .then((settings) => {
        if (!active) return
        setState({
          defaultPlaybackRate: playbackRateToNumber(settings.defaultPlaybackRate),
          autoResume: settings.autoResume,
          autoNext: settings.autoNext,
          status: "ready",
        })
      })
      .catch(() => {
        if (!active) return
        setState({
          defaultPlaybackRate: playbackRateToNumber(DEFAULT_PLAYBACK_SETTINGS.defaultPlaybackRate),
          autoResume: DEFAULT_PLAYBACK_SETTINGS.autoResume,
          autoNext: DEFAULT_PLAYBACK_SETTINGS.autoNext,
          status: "degraded",
        })
      })
    return () => {
      active = false
    }
  }, [mode])

  return state
}
