import { describe, expect, it } from "vitest"
import { externalSubtitleOption, hlsSubtitleOptions } from "./subtitle-options"

describe("subtitle options", () => {
  it("keeps external subtitle identity separate from the display label", () => {
    expect(externalSubtitleOption({
      trackId: "0196f0d2-0000-7000-8000-000000000002",
      label: "中文",
      language: "zh-CN",
      format: "srt",
      contentUri: "haven-resource://session/0196f0d2-0000-7000-8000-000000000001/subtitle/0196f0d2-0000-7000-8000-000000000002",
    })).toEqual({
      id: "external:0196f0d2-0000-7000-8000-000000000002",
      label: "中文",
      language: "zh-CN",
      source: "external",
      externalTrack: {
        trackId: "0196f0d2-0000-7000-8000-000000000002",
        label: "中文",
        language: "zh-CN",
        format: "srt",
        contentUri: "haven-resource://session/0196f0d2-0000-7000-8000-000000000001/subtitle/0196f0d2-0000-7000-8000-000000000002",
      },
    })
  })

  it("maps HLS subtitle tracks to index-bound options without exposing their URLs", () => {
    const options = hlsSubtitleOptions([
      { id: 3, name: "English", lang: "en" },
      { id: 7, lang: "zh-CN", forced: true },
      { id: 9 },
    ])
    expect(options).toEqual([
      { id: "hls:3", label: "English", language: "en", source: "hls", hlsIndex: 0 },
      { id: "hls:7", label: "zh-CN（强制）", language: "zh-CN", source: "hls", hlsIndex: 1 },
      { id: "hls:9", label: "字幕 3", language: null, source: "hls", hlsIndex: 2 },
    ])
    expect(hlsSubtitleOptions([
      { id: 7, lang: "zh-CN", forced: true },
      { id: 3, name: "English", lang: "en" },
    ])[1]).toMatchObject({ id: "hls:3", hlsIndex: 1 })
  })

  it("bounds HLS labels and removes control characters before rendering", () => {
    const option = hlsSubtitleOptions([{
      id: 11,
      name: `  A\u0000${"x".repeat(200)}  `,
      lang: `en\u0007${"y".repeat(50)}`,
    }])[0]

    expect(option.label).toHaveLength(128)
    expect(option.label).not.toContain("\u0000")
    expect(option.language).toBe(`en${"y".repeat(30)}`)
  })
})
