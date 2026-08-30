import { useCallback, useEffect, useRef, useState } from "react"
import type { HavenClient } from "@/lib/ipc/client"
import { getHavenClient } from "@/lib/ipc/runtime.js"
import type { SessionEngineDto, SessionOpenResultDto } from "@/lib/ipc/generated/wire"
import { acquireSessionLease, type SessionLease } from "./session-lease.js"
import {
  errorSessionState,
  idleSessionState,
  openingSessionState,
  readySessionState,
  type MediaSessionState,
} from "./session-state.js"

interface RequestRecord {
  key: string
  generation: number
  lease: SessionLease
}

export function useMediaSession(
  mediaItemId: string | undefined,
  engine: SessionEngineDto,
) {
  const [state, setState] = useState<MediaSessionState>(() => idleSessionState())
  const [retryNonce, setRetryNonce] = useState(0)
  const generationRef = useRef(0)
  const requestRef = useRef<RequestRecord | null>(null)
  const barrierProviderRef = useRef<(() => PromiseLike<unknown>) | null>(null)
  const clientRef = useRef<HavenClient | null>(null)
  if (clientRef.current === null) clientRef.current = getHavenClient()
  const client = clientRef.current

  useEffect(() => {
    let active = true

    if (!mediaItemId || mediaItemId.trim().length === 0) {
      ++generationRef.current
      requestRef.current?.lease.release()
      requestRef.current = null
      setState(idleSessionState())
      return () => {
        active = false
      }
    }

    const key = `${mediaItemId}:${engine}:${retryNonce}`
    const previous = requestRef.current
    const request: RequestRecord = previous?.key === key
      ? {
        ...previous,
        lease: acquireSessionLease(client, key, { mediaItemId, engine }),
      }
      : {
        key,
        generation: ++generationRef.current,
        lease: acquireSessionLease(client, key, { mediaItemId, engine }),
      }
    requestRef.current = request
    if (request.key !== previous?.key) setState(openingSessionState())

    request.lease.promise
      .then((session: SessionOpenResultDto) => {
        if (active && generationRef.current === request.generation) setState(readySessionState(session))
      })
      .catch((error: unknown) => {
        if (active && generationRef.current === request.generation) setState(errorSessionState(error))
      })

    return () => {
      active = false
      const provider = barrierProviderRef.current
      barrierProviderRef.current = null
      request.lease.releaseAfter(provider ? provider() : Promise.resolve())
    }
  }, [client, mediaItemId, engine, retryNonce])

  const retry = useCallback(() => {
    if (state.status === "opening") return
    setRetryNonce((value) => value + 1)
  }, [state.status])

  // Stable registration point for playback cleanup barriers (progress flush, etc.).
  const registerReleaseBarrier = useCallback((provider: (() => PromiseLike<unknown>) | null) => {
    barrierProviderRef.current = provider
  }, [])

  return { ...state, state, retry, registerReleaseBarrier }
}
