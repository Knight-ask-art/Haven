/**
 * v0.1.0's WebView2 <video> format contract.
 *
 * This is deliberately a conservative, inspectable matrix. It describes the
 * formats the Player is prepared to consume; it is not evidence that every
 * Windows/WebView2/driver combination can decode them. The real-device and
 * long-running stability gate remains a release blocker.
 */

export type PlaybackMatrixFormat =
  | "mp4-h264-aac"
  | "mp4-h264-no-audio"
  | "webm-vp8-opus"
  | "webm-vp8-vorbis"
  | "webm-vp8-no-audio"
  | "webm-vp9-opus"
  | "webm-vp9-vorbis"
  | "webm-vp9-no-audio"

export type PlaybackFormatRejection =
  | "empty_descriptor"
  | "container_not_supported"
  | "video_codec_not_supported"
  | "audio_codec_not_supported"
  | "external_subtitles_not_supported"
  | "audio_track_switching_not_supported"

export interface PlaybackFormatDescriptor {
  /** Resource MIME type, not a filename extension. */
  mimeType: string | null | undefined
  /** Codec token for the video stream (for example `avc1.4d401f`). */
  videoCodec: string | null | undefined
  /** Codec token for the selected/default audio stream; null means no audio. */
  audioCodec: string | null | undefined
  /** v0.1.0 does not load an external subtitle resource. */
  hasExternalSubtitles?: boolean
  /** More than one track would require an unimplemented track chooser. */
  audioTrackCount?: number | null
}

export interface SupportedPlaybackFormat {
  readonly format: PlaybackMatrixFormat
  readonly mimeType: "video/mp4" | "video/webm"
  readonly videoCodec: "avc1" | "vp8" | "vp09"
  readonly audioCodec: "mp4a.40.2" | "opus" | "vorbis" | null
}

export type PlaybackFormatDecision =
  | { status: "supported"; format: SupportedPlaybackFormat }
  | {
      status: "unsupported"
      reason: PlaybackFormatRejection
      code: "FORMAT_UNSUPPORTED"
      userMessage: string
    }

/**
 * The exact rows intentionally use H.264/AVC + AAC-LC for MP4 and VP8/VP9 +
 * Opus/Vorbis for WebM. HEVC/H.265, Matroska, external subtitles and audio
 * track switching are outside this first-release contract.
 */
export const WEBVIEW2_PLAYBACK_MATRIX: readonly SupportedPlaybackFormat[] = [
  { format: "mp4-h264-aac", mimeType: "video/mp4", videoCodec: "avc1", audioCodec: "mp4a.40.2" },
  { format: "mp4-h264-no-audio", mimeType: "video/mp4", videoCodec: "avc1", audioCodec: null },
  { format: "webm-vp8-opus", mimeType: "video/webm", videoCodec: "vp8", audioCodec: "opus" },
  { format: "webm-vp8-vorbis", mimeType: "video/webm", videoCodec: "vp8", audioCodec: "vorbis" },
  { format: "webm-vp8-no-audio", mimeType: "video/webm", videoCodec: "vp8", audioCodec: null },
  { format: "webm-vp9-opus", mimeType: "video/webm", videoCodec: "vp09", audioCodec: "opus" },
  { format: "webm-vp9-vorbis", mimeType: "video/webm", videoCodec: "vp09", audioCodec: "vorbis" },
  { format: "webm-vp9-no-audio", mimeType: "video/webm", videoCodec: "vp09", audioCodec: null },
]

function normalizeToken(value: string | null | undefined): string {
  return value?.trim().toLowerCase() ?? ""
}

function codecFamily(value: string | null | undefined): string {
  return normalizeToken(value).split(".", 1)[0] ?? ""
}

function unsupported(
  reason: PlaybackFormatRejection,
  userMessage: string,
): PlaybackFormatDecision {
  return { status: "unsupported", reason, code: "FORMAT_UNSUPPORTED", userMessage }
}

/**
 * Check a metadata descriptor before the element is asked to play it.
 * Unknown/empty metadata fails closed; it must not be mistaken for a known
 * supported format merely because the extension looks familiar.
 */
export function checkWebView2PlaybackFormat(
  descriptor: PlaybackFormatDescriptor,
): PlaybackFormatDecision {
  const mimeType = normalizeToken(descriptor.mimeType)
  const videoCodec = codecFamily(descriptor.videoCodec)
  const audioCodec = normalizeToken(descriptor.audioCodec)

  if (
    !mimeType
    || !videoCodec
    || descriptor.audioCodec === undefined
    || (typeof descriptor.audioCodec === "string" && !audioCodec)
  ) {
    return unsupported("empty_descriptor", "视频格式信息为空，当前版本无法安全判断是否支持。")
  }

  if (descriptor.hasExternalSubtitles) {
    return unsupported("external_subtitles_not_supported", "当前版本不支持外挂字幕。")
  }

  if (descriptor.audioTrackCount !== null && descriptor.audioTrackCount !== undefined) {
    if (!Number.isInteger(descriptor.audioTrackCount) || descriptor.audioTrackCount < 0) {
      return unsupported("empty_descriptor", "视频音轨信息无效，当前版本无法安全判断是否支持。")
    }
    if (descriptor.audioTrackCount > 1) {
      return unsupported("audio_track_switching_not_supported", "当前版本不支持音轨切换。")
    }
    if (descriptor.audioTrackCount === 0 && descriptor.audioCodec !== null) {
      return unsupported("empty_descriptor", "视频音轨信息不完整，当前版本无法安全判断是否支持。")
    }
    if (descriptor.audioTrackCount === 1 && descriptor.audioCodec === null) {
      return unsupported("empty_descriptor", "视频音轨信息不完整，当前版本无法安全判断是否支持。")
    }
  }

  if (mimeType !== "video/mp4" && mimeType !== "video/webm") {
    return unsupported("container_not_supported", "当前版本仅支持 MP4 和 WebM 视频容器。")
  }

  const matchingRows = WEBVIEW2_PLAYBACK_MATRIX.filter(
    (row) => row.mimeType === mimeType && row.videoCodec === videoCodec,
  )
  if (matchingRows.length === 0) {
    return unsupported("video_codec_not_supported", "当前版本不支持此视频编码。首发不支持 HEVC/H.265。")
  }

  const row = matchingRows.find((candidate) => candidate.audioCodec === (audioCodec || null))
  if (row === undefined) {
    return unsupported("audio_codec_not_supported", "当前版本不支持此视频音频编码。")
  }

  return { status: "supported", format: row }
}
