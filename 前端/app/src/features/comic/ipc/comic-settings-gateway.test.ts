import { describe, expect, it } from "vitest"
import type { HavenClient } from "@/lib/ipc/client"
import type { PreferenceGetResult } from "@/lib/ipc/settings-wire"
import { getComicSettingsForResource } from "./comic-settings-gateway"

describe("comic settings gateway", () => {
  it("keeps resource identity in the typed preference query", async () => {
    const result: PreferenceGetResult = {
      schemaVersion: 1,
      mediaItemId: "comic-media-1",
      editionId: "comic-edition-1",
      readingPatch: null,
      comicPatch: { viewMode: "double", direction: "ltr" },
      editionReadingPatch: null,
      editionComicPatch: null,
      mediaItemReadingPatch: null,
      mediaItemComicPatch: { viewMode: "double", direction: "ltr" },
      effectiveReading: {
        section: "reading",
        fontFamily: "serif",
        customFontFamily: null,
        fontSize: "medium",
        lineHeight: "comfortable",
        contentWidth: "medium",
        theme: "warm",
        customBackground: null,
        customText: null,
        fontWeight: "regular",
        letterSpacing: "normal",
        systemAuto: true,
        pagination: "scroll",
      },
      effectiveComic: { section: "comic", viewMode: "double", direction: "ltr", pageGap: "twelve", preloadPages: "three" },
      mediaItemRevision: "media-rev-1",
      editionRevision: null,
    }
    const client = {
      preferenceGet: async (request: { mediaItemId: string; editionId: string }) => {
        expect(request).toEqual({ mediaItemId: "comic-media-1", editionId: "comic-edition-1" })
        return result
      },
    } as Pick<HavenClient, "preferenceGet">

    await expect(getComicSettingsForResource("comic-media-1", "comic-edition-1", client)).resolves.toEqual(result)
  })
})
