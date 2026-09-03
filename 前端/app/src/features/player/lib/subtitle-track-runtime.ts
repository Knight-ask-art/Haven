import {
  normalizeSubtitle,
  type SubtitleFormat,
  type SubtitleNormalizationOptions,
  type SubtitleDiagnostic,
} from "./subtitle-converter"

export interface SubtitleTrackDescriptor {
  trackId: string
  label: string
  language?: string | null
  format: SubtitleFormat
  contentUri: string
}

export interface SubtitleResourcePayload {
  bytes: ArrayBuffer | Uint8Array
  contentType?: string
}

export type SubtitleResourceLoader = (
  contentUri: string,
  signal: AbortSignal,
) => Promise<SubtitleResourcePayload>

export interface SubtitleObjectUrlFactory {
  createObjectURL: (blob: Blob) => string
  revokeObjectURL: (url: string) => void
}

export interface SubtitleTrackAsset {
  trackId: string
  label: string
  language: string | null
  objectUrl: string
  webVtt: string
  cueCount: number
}

export type SubtitleTrackLoadResult =
  | { status: "ready"; asset: SubtitleTrackAsset; warnings: SubtitleDiagnostic[] }
  | { status: "rejected"; warnings: SubtitleDiagnostic[] }
  | { status: "failed"; warnings: SubtitleDiagnostic[] }
  | { status: "stale" }

const browserObjectUrls: SubtitleObjectUrlFactory = {
  createObjectURL: (blob) => URL.createObjectURL(blob),
  revokeObjectURL: (url) => URL.revokeObjectURL(url),
}

/**
 * Owns exactly one subtitle object URL and one in-flight request. Starting a
 * new load or calling clear invalidates the previous generation first, so a
 * slow old episode cannot publish a track over the current episode.
 */
export class SubtitleTrackRuntime {
  private generation = 0
  private activeRequest: AbortController | null = null
  private currentObjectUrl: string | null = null
  private readonly objectUrls: SubtitleObjectUrlFactory

  constructor(objectUrls: SubtitleObjectUrlFactory = browserObjectUrls) {
    this.objectUrls = objectUrls
  }

  async load(
    track: SubtitleTrackDescriptor,
    loadResource: SubtitleResourceLoader,
    options?: SubtitleNormalizationOptions,
  ): Promise<SubtitleTrackLoadResult> {
    const generation = this.beginGeneration()
    if (!track.contentUri) {
      this.activeRequest = null
      return {
        status: "failed",
        warnings: [{ code: "invalid_resource", message: "字幕资源地址无效。" }],
      }
    }

    const controller = this.activeRequest
    if (controller === null) return { status: "stale" }

    let payload: SubtitleResourcePayload
    try {
      payload = await loadResource(track.contentUri, controller.signal)
    } catch {
      if (generation !== this.generation || controller.signal.aborted) return { status: "stale" }
      this.activeRequest = null
      return {
        status: "failed",
        warnings: [{ code: "resource_unavailable", message: "字幕资源暂时无法读取。" }],
      }
    }
    if (generation !== this.generation || controller.signal.aborted) return { status: "stale" }

    const normalized = normalizeSubtitle(payload.bytes, track.format, options)
    if (normalized.status === "rejected") {
      this.activeRequest = null
      return normalized
    }
    if (generation !== this.generation) return { status: "stale" }

    const objectUrl = this.objectUrls.createObjectURL(new Blob([normalized.webVtt], { type: "text/vtt" }))
    if (generation !== this.generation) {
      this.objectUrls.revokeObjectURL(objectUrl)
      return { status: "stale" }
    }
    this.currentObjectUrl = objectUrl
    this.activeRequest = null
    return {
      status: "ready",
      asset: {
        trackId: track.trackId,
        label: track.label,
        language: track.language ?? null,
        objectUrl,
        webVtt: normalized.webVtt,
        cueCount: normalized.cueCount,
      },
      warnings: normalized.warnings,
    }
  }

  clear(): void {
    this.generation += 1
    this.activeRequest?.abort()
    this.activeRequest = null
    this.revokeCurrentObjectUrl()
  }

  private beginGeneration(): number {
    this.generation += 1
    this.activeRequest?.abort()
    this.activeRequest = new AbortController()
    this.revokeCurrentObjectUrl()
    return this.generation
  }

  private revokeCurrentObjectUrl(): void {
    if (this.currentObjectUrl === null) return
    this.objectUrls.revokeObjectURL(this.currentObjectUrl)
    this.currentObjectUrl = null
  }
}
