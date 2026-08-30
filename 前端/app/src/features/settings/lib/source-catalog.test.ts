import { describe, expect, it } from "vitest"
import type { SourceDescriptorDto } from "@/lib/ipc/generated/wire"
import {
  SOURCE_CATEGORY_ORDER,
  SOURCE_MODE_LABELS,
  groupSourcesByMode,
  sourceCapabilityLabels,
  sourceMatchesCategory,
} from "./source-catalog"

const source = (sourceId: string, mode: SourceDescriptorDto["mode"], categories: SourceDescriptorDto["categories"]): SourceDescriptorDto => ({
  sourceId,
  displayName: sourceId,
  kinds: ["metadata"],
  categories,
  mode,
  notes: "测试来源",
  enabled: false,
  health: "unknown",
  endpointConfigured: false,
  lastChecked: null,
  latencyMs: null,
  successRate: null,
})
describe("source catalog display mapping", () => {
  it("keeps the four product categories in stable order", () => {
    expect(SOURCE_CATEGORY_ORDER).toEqual(["video", "book", "comic", "periodical"])
  })

  it("groups collection and single sources without changing source order", () => {
    const grouped = groupSourcesByMode([
      source("single-a", "single", ["book"]),
      source("collection-a", "collection", ["video"]),
      source("single-b", "single", ["comic"]),
    ])
    expect(grouped.collection.map((item) => item.sourceId)).toEqual(["collection-a"])
    expect(grouped.single.map((item) => item.sourceId)).toEqual(["single-a", "single-b"])
    expect(SOURCE_MODE_LABELS.collection).toBe("聚合来源")
  })

  it("maps capabilities and category filters to user-facing semantics", () => {
    expect(sourceCapabilityLabels(["metadata", "stream", "download"])).toEqual([
      "作品信息",
      "在线播放",
      "下载到本机",
    ])
    const book = source("book", "single", ["book"])
    expect(sourceMatchesCategory(book, "book")).toBe(true)
    expect(sourceMatchesCategory(book, "comic")).toBe(false)
    expect(sourceMatchesCategory(book, "all")).toBe(true)
  })
})
