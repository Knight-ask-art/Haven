import type { SubtitleTrackDto } from "@/lib/ipc/generated/wire"

export interface HlsSubtitleTrackLike {
  id: number
  name?: string
  lang?: string
  forced?: boolean
}

export interface PlayerSubtitleOption {
  id: string
  label: string
  language: string | null
  source: "external" | "hls"
  externalTrack?: SubtitleTrackDto
  hlsIndex?: number
}

function boundedDisplayText(value: string | undefined, maxLength: number): string {
  return Array.from(value ?? "")
    .filter((character) => {
      const codePoint = character.codePointAt(0) ?? 0
      return codePoint >= 0x20 && codePoint !== 0x7f
    })
    .join("")
    .trim()
    .slice(0, maxLength)
}

export function externalSubtitleOption(track: SubtitleTrackDto): PlayerSubtitleOption {
  return {
    id: `external:${track.trackId}`,
    label: track.label,
    language: track.language ?? null,
    source: "external",
    externalTrack: track,
  }
}

export function hlsSubtitleOptions(
  tracks: readonly HlsSubtitleTrackLike[],
): PlayerSubtitleOption[] {
  return tracks.map((track, index) => {
    const language = boundedDisplayText(track.lang, 32) || null
    const baseLabel = boundedDisplayText(track.name, 128) || language || `字幕 ${index + 1}`
    const label = (track.forced && !/强制/.test(baseLabel) ? `${baseLabel}（强制）` : baseLabel).slice(0, 128)
    return {
      id: `hls:${track.id}`,
      label,
      language,
      source: "hls",
      hlsIndex: index,
    }
  })
}
