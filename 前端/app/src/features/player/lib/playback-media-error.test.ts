import { describe, expect, it } from "vitest"
import { playbackMediaErrorForActiveSource, playbackMediaErrorForFormatDecision } from "./playback-media-error"
import { checkWebView2PlaybackFormat } from "./playback-format-matrix"

const ACTIVE_SOURCE = "haven-resource://session/current"

describe("playback media error", () => {
  it("reports an unsupported container or codec as terminal", () => {
    expect(playbackMediaErrorForActiveSource(4, ACTIVE_SOURCE, ACTIVE_SOURCE)).toEqual({
      code: "FORMAT_UNSUPPORTED",
      state: "failed",
      title: "不支持此视频",
      message: "当前版本不支持此视频的容器或编码。",
      retryable: false,
    })
  })

  it("describes decode failures without claiming that the file is damaged", () => {
    const error = playbackMediaErrorForActiveSource(3, ACTIVE_SOURCE, ACTIVE_SOURCE)

    expect(error).toEqual({
      code: "DECODER_FAILED",
      state: "failed",
      title: "无法解码此视频",
      message: "此视频的编码可能不受当前版本支持，或文件内容无法解码。",
      retryable: false,
    })
    expect(error?.message).not.toContain("损坏")
  })

  it("allows retrying a safe local resource read failure", () => {
    expect(playbackMediaErrorForActiveSource(2, ACTIVE_SOURCE, ACTIVE_SOURCE)).toEqual({
      code: "RESOURCE_OPEN_FAILED",
      state: "failed",
      title: "视频读取失败",
      message: "无法读取本地视频资源，请重试。",
      retryable: true,
    })
  })

  it("ignores an aborted media load", () => {
    expect(playbackMediaErrorForActiveSource(1, ACTIVE_SOURCE, ACTIVE_SOURCE)).toBeNull()
  })

  it("ignores a late error from a superseded session source", () => {
    expect(playbackMediaErrorForActiveSource(4, "haven-resource://session/old", ACTIVE_SOURCE)).toBeNull()
  })

  it("uses a safe terminal fallback for an unknown media error", () => {
    expect(playbackMediaErrorForActiveSource(null, ACTIVE_SOURCE, ACTIVE_SOURCE)).toEqual({
      code: "INTERNAL_ERROR",
      state: "failed",
      title: "无法播放此视频",
      message: "播放器遇到未知错误，请重新打开后再试。",
      retryable: false,
    })
  })

  it("uses the same stable format code and safe message for a matrix refusal", () => {
    const decision = checkWebView2PlaybackFormat({
      mimeType: "video/mp4",
      videoCodec: "hvc1.1.6.L93.B0",
      audioCodec: "mp4a.40.2",
    })

    expect(playbackMediaErrorForFormatDecision(decision)).toEqual({
      code: "FORMAT_UNSUPPORTED",
      state: "failed",
      title: "不支持此视频",
      message: "当前版本不支持此视频编码。首发不支持 HEVC/H.265。",
      retryable: false,
    })
  })
})
