import { describe, expect, it, vi } from "vitest"
import { SubtitleTrackRuntime, type SubtitleTrackDescriptor } from "./subtitle-track-runtime"

const firstTrack: SubtitleTrackDescriptor = {
  trackId: "track-1",
  label: "中文",
  language: "zh-CN",
  format: "srt",
  contentUri: "haven-resource://session/0196f0d2-0000-7000-8000-000000000001/subtitle/0196f0d2-0000-7000-8000-000000000002",
}

const secondTrack: SubtitleTrackDescriptor = {
  ...firstTrack,
  trackId: "track-2",
  label: "English",
  language: "en",
  contentUri: "haven-resource://session/0196f0d2-0000-7000-8000-000000000001/subtitle/0196f0d2-0000-7000-8000-000000000003",
}

function payload(text: string): { bytes: ArrayBuffer; contentType: string } {
  return { bytes: new TextEncoder().encode(text).buffer, contentType: "text/plain" }
}

describe("subtitle track runtime", () => {
  it("creates one object URL and revokes it when the track is replaced or cleared", async () => {
    const created: string[] = []
    const revoked: string[] = []
    const runtime = new SubtitleTrackRuntime({
      createObjectURL: () => {
        const url = `blob:subtitle-${created.length + 1}`
        created.push(url)
        return url
      },
      revokeObjectURL: (url) => revoked.push(url),
    })
    const fetchResource = vi.fn(async (uri: string, _signal: AbortSignal) => payload(
      uri === firstTrack.contentUri
        ? "1\n00:00:00,000 --> 00:00:01,000\n中文"
        : "1\n00:00:00,000 --> 00:00:01,000\nEnglish",
    ))

    const first = await runtime.load(firstTrack, fetchResource)
    expect(first).toMatchObject({ status: "ready", asset: { trackId: "track-1", objectUrl: "blob:subtitle-1" } })
    const second = await runtime.load(secondTrack, fetchResource)
    expect(second).toMatchObject({ status: "ready", asset: { trackId: "track-2", objectUrl: "blob:subtitle-2" } })
    expect(revoked).toEqual(["blob:subtitle-1"])

    runtime.clear()
    expect(revoked).toEqual(["blob:subtitle-1", "blob:subtitle-2"])
    expect(created).toEqual(["blob:subtitle-1", "blob:subtitle-2"])
  })

  it("marks an old request stale and never lets it publish over a newer request", async () => {
    let resolveFirst: ((value: { bytes: ArrayBuffer; contentType: string }) => void) | undefined
    const runtime = new SubtitleTrackRuntime({
      createObjectURL: () => "blob:current",
      revokeObjectURL: vi.fn(),
    })
    const fetchResource = vi.fn((uri: string, _signal: AbortSignal) => {
      if (uri === firstTrack.contentUri) {
        return new Promise<{ bytes: ArrayBuffer; contentType: string }>((resolve) => {
          resolveFirst = resolve
        })
      }
      return Promise.resolve(payload("1\n00:00:00,000 --> 00:00:01,000\nnew"))
    })

    const firstPromise = runtime.load(firstTrack, fetchResource)
    const secondPromise = runtime.load(secondTrack, fetchResource)
    const second = await secondPromise
    resolveFirst?.(payload("1\n00:00:00,000 --> 00:00:01,000\nold"))
    const first = await firstPromise

    expect(second).toMatchObject({ status: "ready", asset: { trackId: "track-2" } })
    expect(first).toEqual({ status: "stale" })
  })

  it("does not create an object URL when normalization rejects the resource", async () => {
    const createObjectURL = vi.fn(() => "blob:should-not-exist")
    const runtime = new SubtitleTrackRuntime({
      createObjectURL,
      revokeObjectURL: vi.fn(),
    })
    const unsupportedTrack = { ...firstTrack, format: "ass" as const }

    const result = await runtime.load(
      unsupportedTrack,
      async () => payload("[Script Info]\nTitle: unsafe"),
    )

    expect(result).toMatchObject({ status: "rejected" })
    expect(createObjectURL).not.toHaveBeenCalled()
  })

  it("aborts the active request and makes its completion stale when cleared", async () => {
    let resolveResource: ((value: { bytes: ArrayBuffer; contentType: string }) => void) | undefined
    let requestSignal: AbortSignal | undefined
    const runtime = new SubtitleTrackRuntime({
      createObjectURL: () => "blob:never",
      revokeObjectURL: vi.fn(),
    })
    const loadPromise = runtime.load(firstTrack, (_uri, signal) => {
      requestSignal = signal
      return new Promise<{ bytes: ArrayBuffer; contentType: string }>((resolve) => {
        resolveResource = resolve
      })
    })

    runtime.clear()
    expect(requestSignal?.aborted).toBe(true)
    resolveResource?.(payload("1\n00:00:00,000 --> 00:00:01,000\nlate"))
    await expect(loadPromise).resolves.toEqual({ status: "stale" })
  })
})
