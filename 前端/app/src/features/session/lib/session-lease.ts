import type { HavenClient } from "@/lib/ipc/client"
import type { SessionEngineDto, SessionOpenResultDto } from "@/lib/ipc/generated/wire"
import { closeSession, openSession } from "../ipc/session-gateway.js"

export interface SessionLeaseRequest {
  mediaItemId: string
  engine: SessionEngineDto
}
export interface SessionLease {
  readonly promise: Promise<SessionOpenResultDto>
  release(): void
  releaseAfter(barrier: PromiseLike<unknown>): void
}

interface Entry {
  readonly key: string
  readonly client: HavenClient
  readonly promise: Promise<SessionOpenResultDto>
  consumers: number
  session: SessionOpenResultDto | undefined
  closeQueued: boolean
  closing: boolean
}

// Entries are shared only by the client instance and the caller's request key.
// Keeping this outside React is what makes StrictMode effect replay safe.
const entriesByClient = new WeakMap<object, Map<string, Entry>>()

function scheduleClose(entry: Entry, entries: Map<string, Entry>): void {
  if (entry.consumers !== 0 || entry.closeQueued || entry.closing) return
  entry.closeQueued = true
  const closeIfUnused = () => {
    entry.closeQueued = false
    if (entry.consumers !== 0 || entry.closing) return
    if (!entry.session) return
    entry.closing = true
    if (entries.get(entry.key) === entry) entries.delete(entry.key)
    // Best effort by design: lifecycle cleanup must never become UI error state.
    void closeSession(entry.session.sessionId, entry.client).catch(() => undefined)
  }

  // A microtask lets StrictMode's immediate cleanup/re-acquire cancel the close.
  queueMicrotask(closeIfUnused)
}

function createEntry(
  client: HavenClient,
  key: string,
  request: SessionLeaseRequest,
  entries: Map<string, Entry>,
): Entry {
  const promise = openSession(request, client)
  const entry: Entry = {
    key,
    client,
    promise,
    consumers: 0,
    session: undefined,
    closeQueued: false,
    closing: false,
  }
  entries.set(key, entry)
  promise.then(
    (session) => {
      entry.session = session
      scheduleClose(entry, entries)
      return session
    },
    () => {
      if (entries.get(key) === entry) entries.delete(key)
    },
  )
  return entry
}

/**
 * Acquire a shared session open. `requestKey` must include retry generation when
 * a retry should be isolated from the prior session (the hook does this).
 */
export function acquireSessionLease(
  client: HavenClient,
  requestKey: string,
  request: SessionLeaseRequest,
): SessionLease {
  let entries = entriesByClient.get(client)
  if (!entries) {
    entries = new Map()
    entriesByClient.set(client, entries)
  }
  let entry = entries.get(requestKey)
  if (!entry || entry.closing) entry = createEntry(client, requestKey, request, entries)
  entry.consumers += 1
  entry.closeQueued = false

  let released = false
  let releaseRequested = false
  const release = () => {
    if (released || releaseRequested) return
    releaseRequested = true
    released = true
    entry!.consumers -= 1
    scheduleClose(entry!, entries!)
  }
  return {
    promise: entry.promise,
    release,
    releaseAfter: (barrier) => {
      if (released || releaseRequested) return
      releaseRequested = true
      Promise.resolve(barrier).then(() => {
        released = true
        entry!.consumers -= 1
        scheduleClose(entry!, entries!)
      }, () => {
        released = true
        entry!.consumers -= 1
        scheduleClose(entry!, entries!)
      })
    },
  }
}
