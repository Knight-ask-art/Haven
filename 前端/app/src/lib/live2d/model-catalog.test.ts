import { describe, expect, it } from "vitest"
import { CHARACTER_REGISTRY } from "@/lib/mascotState"
import { bundledLive2dModelCatalog } from "./model-catalog"

describe("bundledLive2dModelCatalog", () => {
  it("resolves every configured character inside the dedicated model directory", () => {
    for (const character of CHARACTER_REGISTRY) {
      const resolved = bundledLive2dModelCatalog.resolve(character.id)

      expect(resolved?.character).toBe(character)
      expect(resolved?.model.entryUrl).toMatch(
        new RegExp(`^/live2d/models/${character.id}/`),
      )
      expect(resolved?.model.source).toBe("bundled")
    }
  })

  it("lists the same resolved entries used by model loading", () => {
    expect(bundledLive2dModelCatalog.list()).toHaveLength(CHARACTER_REGISTRY.length)
    expect(bundledLive2dModelCatalog.list()[0]).toBe(
      bundledLive2dModelCatalog.resolve(CHARACTER_REGISTRY[0].id),
    )
  })

  it("does not turn an unknown user selection into an arbitrary asset path", () => {
    expect(bundledLive2dModelCatalog.resolve("../../remote-model")).toBeNull()
  })
})
