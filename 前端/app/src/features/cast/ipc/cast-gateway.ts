import { getHavenClient } from "@/lib/ipc/runtime"
import { toHavenError } from "@/lib/ipc/errors"
import type {
  CastDeviceDto,
  CastDiscoverRequest,
  CastDiscoverResult,
  CastPlayRequest,
  CastPlayResult,
  CastStatusRequest,
  CastStatusDto,
  CastStopRequest,
  CastStopResult,
} from "@/lib/ipc/generated/wire"

export async function discoverCastDevices(timeoutMs?: number): Promise<CastDeviceDto[]> {
  const req: CastDiscoverRequest = { timeoutMs: timeoutMs ?? 5000 }
  const result: CastDiscoverResult = await getHavenClient().castDiscover(req)
  if (!isCastDiscoverResult(result)) throw toHavenError({ code: "INVALID_RESPONSE", userMessage: "投屏发现结果格式无效", retryable: false })
  return result.devices
}

export async function playCast(mediaItemId: string, deviceId: string): Promise<CastPlayResult> {
  const req: CastPlayRequest = { mediaItemId, deviceId, engine: "playback" }
  const result = await getHavenClient().castPlay(req)
  if (!isCastPlayResult(result)) throw toHavenError({ code: "INVALID_RESPONSE", userMessage: "投屏会话格式无效", retryable: false })
  return result
}

export async function getCastStatus(castSessionId: string): Promise<CastStatusDto> {
  const req: CastStatusRequest = { castSessionId }
  return getHavenClient().castStatus(req)
}

export async function stopCast(castSessionId: string): Promise<CastStopResult> {
  const req: CastStopRequest = { castSessionId }
  return getHavenClient().castStop(req)
}

export function normalizeCastError(error: unknown) {
  return toHavenError(error)
}

function isCastDiscoverResult(v: unknown): v is CastDiscoverResult {
  if (typeof v !== "object" || v === null) return false
  const r = v as Record<string, unknown>
  return r.schemaVersion === 1 && Array.isArray(r.devices)
}

function isCastPlayResult(v: unknown): v is CastPlayResult {
  if (typeof v !== "object" || v === null) return false
  const r = v as Record<string, unknown>
  return r.schemaVersion === 1 && typeof r.castSessionId === "string" && typeof r.lanUrl === "string"
}
