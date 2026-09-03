import { describe, expect, it } from "vitest"
import { normalizeSubtitle } from "./subtitle-converter"

function utf8(value: string): ArrayBuffer {
  return new TextEncoder().encode(value).buffer
}

describe("subtitle converter", () => {
  it("normalizes UTF-8 SRT with common formatting into WebVTT", () => {
    const result = normalizeSubtitle(
      utf8("\uFEFF1\r\n00:00:01,250 --> 00:00:03,500\r\n<i>Hello</i><br>world\r\n\r\n2\r\n00:00:04,000 --> 00:00:05,000\r\nSafe <script>alert(1)</script>\r\n"),
      "srt",
    )

    expect(result).toMatchObject({ status: "ok", format: "srt", cueCount: 2 })
    if (result.status !== "ok") return
    expect(result.webVtt).toContain("WEBVTT")
    expect(result.webVtt).toContain("00:00:01.250 --> 00:00:03.500")
    expect(result.webVtt).toContain("<i>Hello</i>\nworld")
    expect(result.webVtt).toContain("&lt;script&gt;alert(1)&lt;/script&gt;")
    expect(result.warnings.map((warning) => warning.code)).toContain("unsafe_markup")
  })

  it("accepts a WebVTT header and preserves cue settings", () => {
    const result = normalizeSubtitle(
      utf8("WEBVTT - fixture\n\ncaption-1\n00:00:00.000 --> 00:00:02.000 line:90% position:50%\nHello\n"),
      "vtt",
    )

    expect(result).toMatchObject({ status: "ok", format: "vtt", cueCount: 1 })
    if (result.status !== "ok") return
    expect(result.webVtt).toContain("00:00:00.000 --> 00:00:02.000 line:90% position:50%")
    expect(result.webVtt).toContain("Hello")
  })

  it("normalizes SBV timestamps and sorts cues by their start time", () => {
    const result = normalizeSubtitle(
      utf8("0:00:05.000,0:00:06.000\nsecond\n\n0:00:01.000,0:00:02.000\nfirst\n"),
      "sbv",
    )

    expect(result).toMatchObject({ status: "ok", format: "sbv", cueCount: 2 })
    if (result.status !== "ok") return
    expect(result.webVtt.indexOf("first")).toBeLessThan(result.webVtt.indexOf("second"))
  })

  it("reports malformed and overlapping cues without blocking valid captions", () => {
    const result = normalizeSubtitle(
      utf8([
        "1",
        "00:00:04,000 --> 00:00:02,000",
        "reversed",
        "",
        "2",
        "00:00:01,000 --> 00:00:03,000",
        "first",
        "",
        "3",
        "00:00:02,000 --> 00:00:04,000",
        "overlap",
      ].join("\n")),
      "srt",
    )

    expect(result).toMatchObject({ status: "partial", cueCount: 2 })
    if (result.status !== "partial") return
    expect(result.warnings.map((warning) => warning.code)).toEqual(
      expect.arrayContaining(["reversed_timestamp", "overlapping_cues"]),
    )
  })

  it("fails closed for unsupported formats, bad encoding, and budget violations", () => {
    expect(normalizeSubtitle(utf8("WEBVTT\n\n00:00:00.000 --> 00:00:01.000\ntext"), "ass")).toMatchObject({
      status: "rejected",
      format: "ass",
      warnings: expect.arrayContaining([expect.objectContaining({ code: "unsupported_format" })]),
    })

    expect(normalizeSubtitle(new Uint8Array([0xff, 0xfe, 0x00]).buffer, "srt")).toMatchObject({
      status: "rejected",
      warnings: expect.arrayContaining([expect.objectContaining({ code: "invalid_encoding" })]),
    })

    expect(normalizeSubtitle(utf8("1\n00:00:00,000 --> 00:00:01,000\n12345"), "srt", {
      maxCueTextLength: 4,
    })).toMatchObject({
      status: "rejected",
      warnings: expect.arrayContaining([expect.objectContaining({ code: "cue_text_too_large" })]),
    })
  })

  it("decodes UTF-16LE input when the BOM identifies it", () => {
    const result = normalizeSubtitle(
      new Uint8Array([
        0xff, 0xfe,
        0x31, 0x00, 0x0a, 0x00,
        0x30, 0x00, 0x30, 0x00, 0x3a, 0x00, 0x30, 0x00, 0x30, 0x00, 0x3a, 0x00, 0x30, 0x00, 0x30, 0x00, 0x2e, 0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x20, 0x00, 0x2d, 0x00, 0x2d, 0x00, 0x3e, 0x00, 0x20, 0x00,
        0x30, 0x00, 0x30, 0x00, 0x3a, 0x00, 0x30, 0x00, 0x30, 0x00, 0x3a, 0x00, 0x30, 0x00, 0x31, 0x00, 0x2e, 0x00, 0x30, 0x00, 0x30, 0x00, 0x30, 0x00, 0x0a, 0x00,
        0x68, 0x00, 0x69, 0x00,
      ]).buffer,
      "srt",
    )

    expect(result).toMatchObject({ status: "ok", cueCount: 1 })
  })
})
