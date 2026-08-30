import { describe, expect, it } from "vitest"
import { checkWebView2PlaybackFormat, WEBVIEW2_PLAYBACK_MATRIX } from "./playback-format-matrix"

describe("v0.1.0 WebView2 playback matrix", () => {
  it("freezes the conservative MP4/WebM rows", () => {
    expect(WEBVIEW2_PLAYBACK_MATRIX).toEqual(expect.arrayContaining([
      { format: "mp4-h264-aac", mimeType: "video/mp4", videoCodec: "avc1", audioCodec: "mp4a.40.2" },
      { format: "webm-vp8-opus", mimeType: "video/webm", videoCodec: "vp8", audioCodec: "opus" },
      { format: "webm-vp9-vorbis", mimeType: "video/webm", videoCodec: "vp09", audioCodec: "vorbis" },
    ]))
  })

  it("accepts supported codec profiles and video without an audio stream", () => {
    expect(checkWebView2PlaybackFormat({
      mimeType: "video/mp4",
      videoCodec: "avc1.4d401f",
      audioCodec: "mp4a.40.2",
    })).toMatchObject({ status: "supported", format: { format: "mp4-h264-aac" } })
    expect(checkWebView2PlaybackFormat({
      mimeType: "video/webm",
      videoCodec: "vp09.00.10.08",
      audioCodec: null,
    })).toMatchObject({ status: "supported", format: { format: "webm-vp9-no-audio" } })
  })

  it("rejects HEVC and unsupported containers without calling them damaged", () => {
    const hevc = checkWebView2PlaybackFormat({
      mimeType: "video/mp4",
      videoCodec: "hev1.1.6.L93.B0",
      audioCodec: "mp4a.40.2",
    })
    const matroska = checkWebView2PlaybackFormat({
      mimeType: "video/x-matroska",
      videoCodec: "avc1.4d401f",
      audioCodec: "mp4a.40.2",
    })

    expect(hevc).toMatchObject({ status: "unsupported", reason: "video_codec_not_supported", code: "FORMAT_UNSUPPORTED" })
    expect(matroska).toMatchObject({ status: "unsupported", reason: "container_not_supported", code: "FORMAT_UNSUPPORTED" })
    expect(JSON.stringify(hevc)).not.toContain("损坏")
  })

  it("rejects external subtitles and multi-track audio explicitly", () => {
    expect(checkWebView2PlaybackFormat({
      mimeType: "video/mp4",
      videoCodec: "avc1.4d401f",
      audioCodec: "mp4a.40.2",
      hasExternalSubtitles: true,
    })).toMatchObject({ status: "unsupported", reason: "external_subtitles_not_supported" })
    expect(checkWebView2PlaybackFormat({
      mimeType: "video/mp4",
      videoCodec: "avc1.4d401f",
      audioCodec: "mp4a.40.2",
      audioTrackCount: 2,
    })).toMatchObject({ status: "unsupported", reason: "audio_track_switching_not_supported" })
  })

  it("fails closed for empty or incomplete metadata", () => {
    expect(checkWebView2PlaybackFormat({ mimeType: "", videoCodec: "", audioCodec: undefined })).toMatchObject({
      status: "unsupported",
      reason: "empty_descriptor",
      code: "FORMAT_UNSUPPORTED",
    })
    expect(checkWebView2PlaybackFormat({ mimeType: "video/mp4", videoCodec: "avc1.4d401f", audioCodec: undefined })).toMatchObject({
      status: "unsupported",
      reason: "empty_descriptor",
    })
  })
})
