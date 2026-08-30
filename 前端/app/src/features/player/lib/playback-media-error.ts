import type { PlaybackFormatDecision } from "./playback-format-matrix"

export type PlaybackState = "idle" | "opening" | "buffering" | "playing" | "paused" | "ended" | "failed"

export type PlaybackMediaErrorCode =
  | "FORMAT_UNSUPPORTED"
  | "DECODER_FAILED"
  | "RESOURCE_OPEN_FAILED"
  | "RESOURCE_UNAVAILABLE"
  | "INTERNAL_ERROR"

export interface PlaybackMediaErrorView {
  code: PlaybackMediaErrorCode
  state: "failed"
  title: string
  message: string
  retryable: boolean
}

const MEDIA_ERR_ABORTED = 1
const MEDIA_ERR_NETWORK = 2
const MEDIA_ERR_DECODE = 3
const MEDIA_ERR_SRC_NOT_SUPPORTED = 4

export function playbackMediaErrorForActiveSource(
  code: number | null | undefined,
  eventSource: string,
  activeSource: string | null,
): PlaybackMediaErrorView | null {
  if (activeSource === null || eventSource !== activeSource || code === MEDIA_ERR_ABORTED) {
    return null
  }

  if (code === MEDIA_ERR_NETWORK) {
    return {
      code: "RESOURCE_OPEN_FAILED",
      state: "failed",
      title: "视频读取失败",
      message: "无法读取本地视频资源，请重试。",
      retryable: true,
    }
  }

  if (code === MEDIA_ERR_DECODE) {
    return {
      code: "DECODER_FAILED",
      state: "failed",
      title: "无法解码此视频",
      message: "此视频的编码可能不受当前版本支持，或文件内容无法解码。",
      retryable: false,
    }
  }

  if (code === MEDIA_ERR_SRC_NOT_SUPPORTED) {
    return {
      code: "FORMAT_UNSUPPORTED",
      state: "failed",
      title: "不支持此视频",
      message: "当前版本不支持此视频的容器或编码。",
      retryable: false,
    }
  }

  return {
    code: "INTERNAL_ERROR",
    state: "failed",
    title: "无法播放此视频",
    message: "播放器遇到未知错误，请重新打开后再试。",
    retryable: false,
  }
}

/** Convert a preflight matrix refusal into the same safe UI error shape. */
export function playbackMediaErrorForFormatDecision(
  decision: PlaybackFormatDecision,
): PlaybackMediaErrorView | null {
  if (decision.status === "supported") return null
  return {
    code: decision.code,
    state: "failed",
    title: "不支持此视频",
    message: decision.userMessage,
    retryable: false,
  }
}
