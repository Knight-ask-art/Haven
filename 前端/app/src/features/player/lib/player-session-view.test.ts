import { describe, expect, it } from "vitest"
import { HavenError } from "@/lib/ipc/errors"
import { errorSessionState, idleSessionState, openingSessionState, readySessionState } from "@/features/session/lib/session-state"
import type { SessionOpenResultDto } from "@/lib/ipc/generated/wire"
import { selectPlayerSessionView } from "./player-session-view"

const session = {
  schemaVersion: 1,
  sessionId: "session-1",
  contentUri: "https://session.example.test/content.mp4",
  workId: "work-1",
  editionId: "edition-1",
  mediaItemId: "media-1",
  engine: "playback",
  progress: null,
} as SessionOpenResultDto

describe("player session view", () => {
  it("only exposes the exact gateway-approved URI when ready", () => {
    expect(selectPlayerSessionView(readySessionState(session), session.mediaItemId)).toMatchObject({
      status: "ready",
      contentUri: session.contentUri,
      retryable: false,
      message: null,
    })
  })

  it("never exposes a fallback URI while opening or in an error state", () => {
    expect(selectPlayerSessionView(idleSessionState(), session.mediaItemId).contentUri).toBeNull()
    expect(selectPlayerSessionView(openingSessionState(), session.mediaItemId).contentUri).toBeNull()
    expect(selectPlayerSessionView(errorSessionState(new HavenError({
      code: "TEMPORARY",
      userMessage: "请重试",
      retryable: true,
    })), session.mediaItemId)).toMatchObject({ contentUri: null, retryable: true, message: "请重试" })
    expect(selectPlayerSessionView(errorSessionState(new HavenError({
      code: "INVALID",
      userMessage: "媒体条目无效",
      retryable: false,
    })), session.mediaItemId)).toMatchObject({ contentUri: null, retryable: false, message: "媒体条目无效" })
  })

  it("hides a ready session from a stale route", () => {
    expect(selectPlayerSessionView(readySessionState(session), "other-media")).toMatchObject({
      status: "opening",
      contentUri: null,
    })
  })
})
