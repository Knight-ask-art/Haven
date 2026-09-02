import type { MediaSessionState } from "@/features/session/lib/session-state"
import type { StreamKindDto } from "@/lib/ipc/generated/wire"

export interface PlayerSessionView {
  status: MediaSessionState["status"]
  contentUri: string | null
  retryable: boolean
  message: string | null
  streamKind: StreamKindDto | null
}

/** Only a validated ready session may provide content to the video element. */
export function selectPlayerSessionView(state: MediaSessionState, mediaItemId: string | undefined): PlayerSessionView {
  switch (state.status) {
    case "ready":
      if (state.session.mediaItemId !== mediaItemId) {
        return {
          status: "opening",
          contentUri: null,
          retryable: false,
          message: null,
          streamKind: null,
        }
      }
      return {
        status: state.status,
        contentUri: state.session.contentUri,
        retryable: false,
        message: null,
        streamKind: state.session.streamKind ?? null,
      }
    case "retryable_error":
      return {
        status: state.status,
        contentUri: null,
        retryable: true,
        message: state.error.dto.userMessage,
        streamKind: null,
      }
    case "terminal_error":
      return {
        status: state.status,
        contentUri: null,
        retryable: false,
        message: state.error.dto.userMessage,
        streamKind: null,
      }
    case "opening":
    case "idle":
      return {
        status: state.status,
        contentUri: null,
        retryable: false,
        message: null,
        streamKind: null,
      }
  }
}
