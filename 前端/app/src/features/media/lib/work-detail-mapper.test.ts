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
      categories: ["book"],
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

  it("keeps the periodical category from being shadowed by the document media type", () => {
    const dto = {
      workId: "work-periodical",
      title: "某刊 2026 年第 3 期",
      originalTitle: null,
      description: null,
      favorite: false,
      categories: ["periodical"],
      availableMediaTypes: ["document"],
      posterUri: null,
      backdropUri: null,
      releaseYear: 2026,
      progress: null,
      primaryAction: null,
    } as unknown as WorkDetailHeaderDto

    expect(mapWorkDetailHeaderToMediaDetail(dto).type).toBe("periodical")
  })

  it("still projects a non-periodical document work as document", () => {
    const dto = {
      workId: "work-document",
      title: "技术手册",
      originalTitle: null,
      description: null,
      favorite: false,
      categories: ["book"],
      availableMediaTypes: ["document"],
      posterUri: null,
      backdropUri: null,
      releaseYear: null,
      progress: null,
      primaryAction: null,
    } as unknown as WorkDetailHeaderDto

    expect(mapWorkDetailHeaderToMediaDetail(dto).type).toBe("document")
  })

  it("keeps an article work from being projected as periodical", () => {
    const dto = {
      workId: "work-article",
      title: "一篇论文",
      originalTitle: null,
      description: null,
      favorite: false,
      categories: ["periodical"],
      availableMediaTypes: ["article"],
      posterUri: null,
      backdropUri: null,
      releaseYear: null,
      progress: null,
      primaryAction: null,
    } as unknown as WorkDetailHeaderDto

    // 报刊分类优先于 document/article 的通用媒介类型兜底。
    expect(mapWorkDetailHeaderToMediaDetail(dto).type).toBe("periodical")
  })
})
