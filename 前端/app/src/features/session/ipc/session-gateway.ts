import { getHavenClient, isTauriRuntime } from "@/lib/ipc/runtime.js"
import { HavenError, toHavenError } from "@/lib/ipc/errors.js"
import type { HavenClient } from "@/lib/ipc/client"
import type {
  SessionCloseRequest,
  SessionCloseResultDto,
  SessionEngineDto,
  SessionOpenRequest,
  SessionOpenResultDto,
  SubtitleFormatDto,
  SubtitleTrackDto,
} from "@/lib/ipc/generated/wire"

const SESSION_URI_PATTERN = /^haven-resource:\/\/session\/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
const SUBTITLE_URI_PATTERN = /^haven-resource:\/\/session\/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\/subtitle\/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
const STREAM_URI_PATTERN = /^haven-resource:\/\/stream\/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
const SESSION_ID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
const BROWSER_URI_PATTERN = /^https:\/\/[^\s]+$/i
const SESSION_ENGINES: readonly SessionEngineDto[] = ["playback", "reader", "comic", "article"]
const SUBTITLE_FORMATS: readonly SubtitleFormatDto[] = [
  "srt",
  "vtt",
  "sbv",
  "ass",
  "ssa",
  "ttml",
  "dfxp",
  "sub",
  "lrc",
]
const MAX_SUBTITLE_TRACKS = 16
const SESSION_ERROR = {
  code: "SESSION_INVALID_RESPONSE",
  userMessage: "播放会话不可用，请稍后重试",
  retryable: false,
} as const

function invalidSessionResponse(): HavenError {
  return new HavenError(SESSION_ERROR)
}

function isSessionCloseResult(value: unknown): value is SessionCloseResultDto {
  if (typeof value !== "object" || value === null) return false
  const result = value as Record<string, unknown>
  return result.schemaVersion === 1 && result.closed === true
}

function isCanonicalSessionId(value: unknown): value is string {
  return typeof value === "string" && SESSION_ID_PATTERN.test(value)
}

function isSubtitleTrack(value: unknown, sessionId: string): value is SubtitleTrackDto {
  if (typeof value !== "object" || value === null) return false
  const track = value as Record<string, unknown>
  if (
    !isCanonicalSessionId(track.trackId)
    || typeof track.label !== "string"
    || track.label.trim().length === 0
    || track.label.length > 128
    || (track.language !== null
      && (typeof track.language !== "string" || track.language.length === 0 || track.language.length > 32))
    || typeof track.format !== "string"
    || !SUBTITLE_FORMATS.includes(track.format as SubtitleFormatDto)
    || typeof track.contentUri !== "string"
  ) return false
  if (isTauriRuntime()) {
    return track.contentUri === "haven-resource://session/" + sessionId + "/subtitle/" + track.trackId
      && SUBTITLE_URI_PATTERN.test(track.contentUri)
  }
  return BROWSER_URI_PATTERN.test(track.contentUri)
}

function isSubtitleTracks(value: unknown, sessionId: string): value is SubtitleTrackDto[] | undefined {
  if (value === undefined) return true
  if (!Array.isArray(value) || value.length > MAX_SUBTITLE_TRACKS) return false
  const trackIds = new Set<string>()
  for (const track of value) {
    if (!isSubtitleTrack(track, sessionId) || trackIds.has(track.trackId)) return false
    trackIds.add(track.trackId)
  }
  return true
}

function isSessionResult(value: unknown, request: SessionOpenRequest): value is SessionOpenResultDto {
  if (typeof value !== "object" || value === null) return false
  const result = value as Record<string, unknown>
  const sessionId = result.sessionId
  const progress = result.progress
  const streamKind = result.streamKind
  const streamKindIsValid = streamKind === undefined || streamKind === null || streamKind === "hls" || streamKind === "direct"
  const progressIsValid = progress === null || (
    typeof progress === "object" && progress !== null
    && typeof (progress as Record<string, unknown>).revision === "string"
    && ((progress as Record<string, unknown>).revision as string).length > 0
  )
  const subtitleTracksIsValid = request.engine === "playback"
    ? typeof sessionId === "string" && isSubtitleTracks(result.subtitleTracks, sessionId)
    : result.subtitleTracks === undefined
  return (
    result.schemaVersion === 1 &&
    isCanonicalSessionId(sessionId) &&
    ((typeof result.contentUri === "string" && result.contentUri.length > 0) || result.contentUri === null) &&
    typeof result.workId === "string" && result.workId.length > 0 &&
    typeof result.editionId === "string" && result.editionId.length > 0 &&
    result.mediaItemId === request.mediaItemId &&
    result.engine === request.engine &&
    streamKindIsValid &&
    subtitleTracksIsValid &&
    progressIsValid
  )
}

function isRequest(value: unknown): value is SessionOpenRequest {
  if (typeof value !== "object" || value === null) return false
  const request = value as Record<string, unknown>
  return (
    typeof request.mediaItemId === "string" &&
    request.mediaItemId.trim().length > 0 &&
    typeof request.engine === "string" &&
    SESSION_ENGINES.includes(request.engine as SessionEngineDto)
  )
}

/** Open a playback session through the typed client boundary.
 * 本地资源优先；无本地候选（远端流条目，V2-B）时回退受控流会话。 */
export async function openSession(
  request: SessionOpenRequest,
  client: HavenClient = getHavenClient(),
): Promise<SessionOpenResultDto> {
  if (!isRequest(request)) {
    throw new HavenError({ code: "INVALID_ARGUMENT", userMessage: "媒体条目无效", retryable: false })
  }

  let result: SessionOpenResultDto
  try {
    try {
      result = await client.sessionOpen(request)
    } catch (error) {
      const havenError = toHavenError(error)
      if (havenError.code !== "RESOURCE_NOT_FOUND") throw havenError
      // 无本地候选 → 远端流（haven-resource://stream/<grant>，原始 URL 不出 IPC）。
      result = await client.streamOpen(request)
    }
  } catch (error) {
    throw toHavenError(error)
  }

  if (!isSessionResult(result, request)) throw invalidSessionResponse()
  const uriIsAllowed = result.engine === "comic"
    ? result.contentUri === null
    : typeof result.contentUri === "string" && (
        isTauriRuntime()
          ? (SESSION_URI_PATTERN.test(result.contentUri) || STREAM_URI_PATTERN.test(result.contentUri))
          : BROWSER_URI_PATTERN.test(result.contentUri)
      )
  if (!uriIsAllowed) {
    throw invalidSessionResponse()
  }
  return result
}

export const openMediaSession = openSession

/** Close a playback session through the typed client boundary. */
export async function closeSession(
  sessionId: string,
  client: HavenClient = getHavenClient(),
): Promise<SessionCloseResultDto> {
  if (!isCanonicalSessionId(sessionId)) {
    throw new HavenError({ code: "INVALID_ARGUMENT", userMessage: "会话标识无效", retryable: false })
  }

  const request: SessionCloseRequest = { sessionId }
  let result: SessionCloseResultDto
  try {
    result = await client.sessionClose(request)
  } catch (error) {
    throw toHavenError(error)
  }

  if (!isSessionCloseResult(result)) throw invalidSessionResponse()
  return result
}

export {
  BROWSER_URI_PATTERN,
  SESSION_ID_PATTERN,
  SESSION_URI_PATTERN,
  STREAM_URI_PATTERN,
  SUBTITLE_URI_PATTERN,
}
