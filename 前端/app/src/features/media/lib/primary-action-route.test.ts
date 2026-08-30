import { describe, expect, it } from "vitest"
import type { PrimaryActionDto } from "@/lib/ipc/generated/wire"
import { primaryActionRoute } from "./primary-action-route"

function action(kind: PrimaryActionDto["kind"], mediaItemId: string | null, editionId = "edition-1"): PrimaryActionDto {
  return { kind, labelHint: "open", editionId, mediaItemId, locator: null }
}

describe("primaryActionRoute", () => {
  it("routes playback and comic actions by backend kind", () => {
    expect(primaryActionRoute(action("playback", "video-1"))).toBe("/player/video-1")
    expect(primaryActionRoute(action("comic", "comic-1"))).toBe("/comic/comic-1")
  })

  it("routes an edition-open action by the authoritative edition id", () => {
    expect(primaryActionRoute(action("open_edition", null))).toBe("/edition/edition-1")
  })

  it("rejects an action missing its required media item", () => {
    expect(primaryActionRoute(action("reader", null))).toBeNull()
  })
})
