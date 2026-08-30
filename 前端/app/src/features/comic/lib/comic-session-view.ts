import type { MediaSessionState } from "@/features/session/lib/session-state"

export interface ComicSessionView {
  status: MediaSessionState["status"]
  contentUri: string | null
  retryable: boolean
  message: string | null
}

/** Only a validated ready session may provide content to the comic reader. */
export function selectComicSessionView(state: MediaSessionState, mediaItemId: string | undefined): ComicSessionView {
  switch (state.status) {
    case "ready":
      if (state.session.mediaItemId !== mediaItemId) {
        return { status: "opening", contentUri: null, retryable: false, message: null }
      }
      return { status: state.status, contentUri: state.session.contentUri, retryable: false, message: null }
    case "retryable_error":
      return { status: state.status, contentUri: null, retryable: true, message: state.error.dto.userMessage }
    case "terminal_error":
      return { status: state.status, contentUri: null, retryable: false, message: state.error.dto.userMessage }
    case "opening":
    case "idle":
      return { status: state.status, contentUri: null, retryable: false, message: null }
  }
}
