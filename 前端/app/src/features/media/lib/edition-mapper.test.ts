import { describe, expect, it } from "vitest"
import type { EditionListByWorkResultDto } from "../ipc/edition-wire"
import {
  isEditionListByWorkResult,
  mapEditionListToDetailItems,
  partitionEditionItems,
  mediaTypeLabel,
  type EditionListItem,
} from "./edition-mapper"

const result: EditionListByWorkResultDto = {
  schemaVersion: 1,
  items: [{
    editionId: "edition-1",
    title: "正式版本",
    subtitle: null,
    mediaType: "book",
    workId: "work-1",
    releaseDate: "2024-01-01",
    language: "zh-CN",
    region: null,
    mediaItemCount: 1,
    availability: { available: 1, offlineAvailable: 1, unavailable: 0 },
    download: null,
    progress: { progressRatio: 0.25 } as never,
    primaryAction: {
      kind: "reader",
      labelHint: "continue",
      editionId: "edition-1",
      mediaItemId: "item-1",
      locator: null,
    },
  }],
  nextCursor: null,
  total: 1,
  revision: null,
}

describe("edition mapper", () => {
  it("preserves server edition and primary-action facts", () => {
    expect(mapEditionListToDetailItems(result)).toEqual([{
      id: "edition-1",
      number: "book",
      title: "正式版本",
      durationOrPages: "2024-01-01 · zh-CN",
      progress: 25,
      primaryAction: result.items[0].primaryAction,
      mediaType: "book",
    }])
  })

  it("rejects an illegal result shape", () => {
    expect(isEditionListByWorkResult({ schemaVersion: 1, items: [], nextCursor: null })).toBe(false)
  })

  it("partitions editions by media type in fixed display order", () => {
    const items: EditionListItem[] = [
      { id: "b1", number: "book", title: "图书一", durationOrPages: "", progress: undefined, primaryAction: null, mediaType: "book" },
      { id: "c1", number: "comic", title: "漫画一", durationOrPages: "", progress: undefined, primaryAction: null, mediaType: "comic" },
      { id: "b2", number: "book", title: "图书二", durationOrPages: "", progress: undefined, primaryAction: null, mediaType: "book" },
      { id: "m1", number: "movie", title: "电影一", durationOrPages: "", progress: undefined, primaryAction: null, mediaType: "movie" },
    ]
    const groups = partitionEditionItems(items)
    expect(groups.map((g) => g.mediaType)).toEqual(["movie", "book", "comic"])
    expect(groups[0].label).toBe("电影")
    expect(groups[1].items.map((i) => i.id)).toEqual(["b1", "b2"])
    expect(groups[1].label).toBe("图书")
  })

  it("returns one group for a single type and labels unknown types", () => {
    const items: EditionListItem[] = [
      { id: "x1", number: "audio", title: "音频", durationOrPages: "", progress: undefined, primaryAction: null, mediaType: "audio" },
    ]
    const groups = partitionEditionItems(items)
    expect(groups.length).toBe(1)
    expect(groups[0].label).toBe("音频")
    expect(mediaTypeLabel("weird-type")).toBe("weird-type")
  })

  it("keeps empty input empty", () => {
    expect(partitionEditionItems([])).toEqual([])
  })
})
