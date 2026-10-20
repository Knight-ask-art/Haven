import type { LocatorDto, SessionOpenResultDto } from "@/lib/ipc/generated/wire"

/** 显式打开动作携带的一次性位置；不是持久化 Progress 的替代值。 */
export interface InitialPosition {
  key: string
  mediaItemId: string
  locator: LocatorDto
}

export function resolveRestorePosition(session: SessionOpenResultDto, initial?: InitialPosition) {
  const explicit = initial?.mediaItemId === session.mediaItemId ? initial : undefined
  return {
    locator: explicit?.locator ?? session.progress?.locator,
    reset: !explicit && session.progress?.completion === "not_started",
    identity: `${session.sessionId}:${session.mediaItemId}:${session.contentUri}${explicit ? `:${explicit.key}` : ""}`,
  }
}
