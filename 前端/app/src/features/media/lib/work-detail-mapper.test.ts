import { describe, expect, it } from "vitest"
import type { WorkDetailHeaderDto } from "@/lib/ipc/generated/wire"
import { mapWorkDetailHeaderToMediaDetail } from "./work-detail-mapper"

describe("mapWorkDetailHeaderToMediaDetail", () => {
  it("maps header facts without inventing WorkCard/resource fields", () => {
    const dto = {
      workId: "work-1",
      title: "权威作品",
      originalTitle: "Authoritative Work",
      description: "来自 work_get",
      favorite: true,
      categories: ["book"],
      availableMediaTypes: ["book"],
      posterUri: "haven://poster/1",
      backdropUri: null,
      releaseYear: 2024,
      progress: { progressRatio: 0.42 },
      primaryAction: null,
    } as unknown as WorkDetailHeaderDto

    expect(mapWorkDetailHeaderToMediaDetail(dto)).toEqual({
      id: "work-1",
      title: "权威作品",
      originalTitle: "Authoritative Work",
      type: "book",
      year: 2024,
      backdropUrl: "haven://poster/1",
      posterUrl: "haven://poster/1",
      description: "来自 work_get",
      authorOrDirector: "Authoritative Work",
      publisherOrStudio: undefined,
      favorite: true,
      progress: 42,
      episodesOrChapters: [],
    })
  })
})
