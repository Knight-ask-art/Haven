import type { PrimaryActionDto } from "@/lib/ipc/generated/wire"

/** Resolve a backend-selected action without consulting the Work's broad media type. */
export function primaryActionRoute(action: PrimaryActionDto | null | undefined): string | null {
  if (!action) return null
  if (action.kind === "open_edition") {
    return `/edition/${action.editionId}`
  }
  if (!action.mediaItemId) return null
  switch (action.kind) {
    case "playback": return `/player/${action.mediaItemId}`
    case "reader": return `/reader/${action.mediaItemId}`
    case "comic": return `/comic/${action.mediaItemId}`
    case "article": return `/article/${action.mediaItemId}`
  }
}
