import type { SessionOpenResultDto } from "@/lib/ipc/generated/wire"
import { HavenError, toHavenError } from "@/lib/ipc/errors.js"

export type SessionStatus = "idle" | "opening" | "ready" | "retryable_error" | "terminal_error"

export type MediaSessionState =
  | { status: "idle" }
  | { status: "opening" }
  | { status: "ready"; session: SessionOpenResultDto }
  | { status: "retryable_error"; error: HavenError }
  | { status: "terminal_error"; error: HavenError }

export type SessionAction =
  | { type: "open_started" }
  | { type: "open_succeeded"; session: SessionOpenResultDto }
  | { type: "open_failed"; error: unknown }
  | { type: "reset" }

export const idleSessionState = (): MediaSessionState => ({ status: "idle" })
export const openingSessionState = (): MediaSessionState => ({ status: "opening" })
export const readySessionState = (session: SessionOpenResultDto): MediaSessionState => ({ status: "ready", session })

export const errorSessionState = (error: unknown): MediaSessionState => {
  const normalized = toHavenError(error)
  return normalized.retryable
    ? { status: "retryable_error", error: normalized }
    : { status: "terminal_error", error: normalized }
}

/** Pure state transition used by the hook and by focused state tests. */
export function sessionStateTransition(state: MediaSessionState, action: SessionAction): MediaSessionState {
  switch (action.type) {
    case "open_started":
      return openingSessionState()
    case "open_succeeded":
      return readySessionState(action.session)
    case "open_failed":
      return errorSessionState(action.error)
    case "reset":
      return idleSessionState()
    default:
      return state
  }
}

export const sessionReducer = sessionStateTransition

export function isSessionReady(state: MediaSessionState): state is Extract<MediaSessionState, { status: "ready" }> {
  return state.status === "ready"
}

/** Content is intentionally consumable only after gateway validation reaches ready. */
export function canConsumeSession(state: MediaSessionState): boolean {
  return isSessionReady(state)
}
