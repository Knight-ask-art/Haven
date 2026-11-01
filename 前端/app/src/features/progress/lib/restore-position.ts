import type { LocatorDto, SessionOpenResultDto } from "@/lib/ipc/generated/wire"

/** 显式打开动作携带的一次性位置；不是持久化 Progress 的替代值。 */
export interface InitialPosition {
  key: string
  mediaItemId: string
  locator: LocatorDto
  /** 首次成功恢复后绑定该会话；重试会话改用其最新持久化进度。 */
  restoredSessionId?: string
}

export function initialPositionForSession(session: SessionOpenResultDto, initial?: InitialPosition) {
  return initial?.mediaItemId === session.mediaItemId
    && (!initial.restoredSessionId || initial.restoredSessionId === session.sessionId)
    ? initial : undefined
}

export function resolveRestorePosition(session: SessionOpenResultDto, initial?: InitialPosition) {
  const explicit = initialPositionForSession(session, initial)
  return {
    locator: explicit?.locator ?? session.progress?.locator,
    reset: !explicit && session.progress?.completion === "not_started",
    identity: `${session.sessionId}:${session.mediaItemId}:${session.contentUri}${explicit ? `:${explicit.key}` : ""}`,
    markRestored: () => {
      if (explicit) explicit.restoredSessionId = session.sessionId
    },
  }
}
