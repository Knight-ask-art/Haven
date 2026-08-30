import { describe, expect, it } from "vitest"
import type { SessionOpenResultDto } from "@/lib/ipc/generated/wire"
import { HavenError } from "@/lib/ipc/errors"
import {
  canConsumeSession,
  idleSessionState,
  sessionStateTransition,
} from "./session-state"

const session = {
  schemaVersion: 1,
  sessionId: "session-1",
  contentUri: "https://example.test/content.mp4",
  workId: "work-1",
  editionId: "edition-1",
  mediaItemId: "media-1",
  engine: "playback",
  progress: null,
} as SessionOpenResultDto

describe("session state", () => {
  it("transitions opening to ready", () => {
    const opening = sessionStateTransition(idleSessionState(), { type: "open_started" })
    expect(opening.status).toBe("opening")
    const ready = sessionStateTransition(opening, { type: "open_succeeded", session })
    expect(ready).toEqual({ status: "ready", session })
    expect(canConsumeSession(opening)).toBe(false)
    expect(canConsumeSession(ready)).toBe(true)
  })

  it("separates retryable and terminal errors", () => {
    const retryable = sessionStateTransition(idleSessionState(), {
      type: "open_failed",
      error: new HavenError({ code: "TEMPORARY", userMessage: "retry", retryable: true }),
    })
    const terminal = sessionStateTransition(idleSessionState(), {
      type: "open_failed",
      error: new HavenError({ code: "INVALID", userMessage: "invalid", retryable: false }),
    })
    expect(retryable.status).toBe("retryable_error")
    expect(terminal.status).toBe("terminal_error")
    expect(canConsumeSession(retryable)).toBe(false)
    expect(canConsumeSession(terminal)).toBe(false)
  })
})
