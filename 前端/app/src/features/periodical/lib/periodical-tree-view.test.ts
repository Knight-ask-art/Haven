import { describe, expect, it } from "vitest"
import type {
  PeriodicalArticleDto,
  PeriodicalIssueDto,
  PeriodicalTreeDto,
  PeriodicalVolumeDto,
} from "@/lib/ipc/generated/wire"
import {
  articleMetaText,
  articleOrdinalLabel,
  collectArticleMediaItemIds,
  formatPageRange,
  formatPublicationDate,
  issnEntries,
  issueHeading,
  issueMeta,
  volumeHeading,
  volumeMeta,
} from "./periodical-tree-view"

function volume(overrides: Partial<PeriodicalVolumeDto> = {}): PeriodicalVolumeDto {
  return {
    id: "v1",
    periodicalId: "p1",
    label: null,
    number: null,
    year: null,
    ordinal: 0,
    issues: [],
    ...overrides,
  }
}

function issue(overrides: Partial<PeriodicalIssueDto> = {}): PeriodicalIssueDto {
  return {
    id: "i1",
    volumeId: "v1",
    label: null,
    number: null,
    publicationDate: null,
    ordinal: 0,
    articles: [],
    ...overrides,
  }
}

function article(overrides: Partial<PeriodicalArticleDto> = {}): PeriodicalArticleDto {
  return {
    id: "a1",
    issueId: "i1",
    mediaItemId: "m1",
    ordinal: null,
    title: "一篇文章",
    doi: null,
    pageRange: null,
    sourceKey: "europepmc",
    remoteArticleId: "r1",
    availability: "unknown",
    ...overrides,
  }
}

describe("formatPublicationDate", () => {
  it("renders each precision the guard accepts without inventing missing parts", () => {
    expect(formatPublicationDate("2024")).toBe("2024 年")
    expect(formatPublicationDate("2024-03")).toBe("2024 年 3 月")
    expect(formatPublicationDate("2024-3")).toBe("2024 年 3 月")
    expect(formatPublicationDate("2024-12-31")).toBe("2024 年 12 月 31 日")
    expect(formatPublicationDate("2024-3-5")).toBe("2024 年 3 月 5 日")
  })

  it("returns null instead of a placeholder when the source has no date", () => {
    expect(formatPublicationDate(null)).toBeNull()
    expect(formatPublicationDate(undefined)).toBeNull()
    expect(formatPublicationDate("")).toBeNull()
    expect(formatPublicationDate("不详")).toBeNull()
    expect(formatPublicationDate("2024-13")).toBeNull()
    expect(formatPublicationDate("2024-00")).toBeNull()
    expect(formatPublicationDate("2024-03-05-06")).toBeNull()
    expect(formatPublicationDate("2024-3-")).toBeNull()
  })

  it("rejects dates that do not exist in the calendar, exactly like the Wire guard", () => {
    // 与 `isPeriodicalTreeDto` / 后端 `publication_date_year` 的真实日历规则一致：
    // 形态合法但不存在的日期不能渲染成一个看起来可信的日期。
    for (const value of [
      "2024-02-31",
      "2024-02-30",
      "2023-02-29",
      "1900-02-29",
      "2999-02-29",
      "2024-04-31",
      "2024-06-31",
      "2024-09-31",
      "2024-11-31",
      "2024-01-32",
    ]) {
      expect(formatPublicationDate(value)).toBeNull()
    }
    // 有效边界：闰年 2 月 29 日、400 年闰规则、大小月月末。
    expect(formatPublicationDate("2024-02-29")).toBe("2024 年 2 月 29 日")
    expect(formatPublicationDate("2024-2-29")).toBe("2024 年 2 月 29 日")
    expect(formatPublicationDate("2000-02-29")).toBe("2000 年 2 月 29 日")
    expect(formatPublicationDate("2024-04-30")).toBe("2024 年 4 月 30 日")
    expect(formatPublicationDate("2024-01-31")).toBe("2024 年 1 月 31 日")
  })

  it("rejects years outside the range the backend and the Wire guard accept", () => {
    for (const value of ["0999", "3000-01", "999-12-31"]) {
      expect(formatPublicationDate(value)).toBeNull()
    }
    expect(formatPublicationDate("1000-01-01")).toBe("1000 年 1 月 1 日")
    expect(formatPublicationDate("2999-12-31")).toBe("2999 年 12 月 31 日")
  })
})

describe("volumeHeading", () => {
  it("prefers the source label, then the volume number, then the backend position", () => {
    expect(volumeHeading(volume({ label: "Suppl 1", number: 12, ordinal: 4 }))).toBe("Suppl 1")
    expect(volumeHeading(volume({ number: 12, ordinal: 4 }))).toBe("第 12 卷")
    // 没有标签也没有卷号时只能说它在本树里的位置，不能说成来源卷号。
    expect(volumeHeading(volume({ ordinal: 2 }))).toBe("卷（第 3 项）")
    expect(volumeHeading(volume({ ordinal: 0 }))).toBe("卷（第 1 项）")
  })

  it("never prints a source-shaped volume number when the source gave none", () => {
    for (const ordinal of [0, 1, 9]) {
      const heading = volumeHeading(volume({ ordinal }))
      expect(heading).not.toMatch(/第\s*\d+\s*卷/)
      expect(heading).toBe(`卷（第 ${ordinal + 1} 项）`)
    }
  })

  it("keeps the year out of the heading so it stays a separate fact", () => {
    expect(volumeMeta(volume({ year: 2024 }))).toBe("2024 年")
    expect(volumeMeta(volume())).toBeNull()
  })
})

describe("issueHeading", () => {
  it("prefers the source label, then the issue number, then the backend position", () => {
    expect(issueHeading(issue({ label: "3-4", number: 4, ordinal: 9 }))).toBe("3-4")
    expect(issueHeading(issue({ number: 4, ordinal: 9 }))).toBe("第 4 期")
    expect(issueHeading(issue({ ordinal: 1 }))).toBe("期（第 2 项）")
    expect(issueHeading(issue({ ordinal: 0 }))).toBe("期（第 1 项）")
  })

  it("never prints a source-shaped issue number when the source gave none", () => {
    for (const ordinal of [0, 3, 11]) {
      const heading = issueHeading(issue({ ordinal }))
      expect(heading).not.toMatch(/第\s*\d+\s*期/)
      expect(heading).toBe(`期（第 ${ordinal + 1} 项）`)
    }
  })

  it("renders the publication date as issue metadata and nothing when absent", () => {
    expect(issueMeta(issue({ publicationDate: "2024-04-18" }))).toBe("2024 年 4 月 18 日")
    expect(issueMeta(issue({ publicationDate: "2023-02-29" }))).toBeNull()
    expect(issueMeta(issue())).toBeNull()
  })
})

describe("page ranges and article metadata", () => {
  it("renders a page range only from the facts the source gave", () => {
    expect(formatPageRange({ start: "S1", end: "S5" })).toBe("S1–S5")
    expect(formatPageRange({ start: "e12345", end: null })).toBe("e12345")
    expect(formatPageRange(null)).toBeNull()
  })

  it("does not invent an ordinal when the source omitted it", () => {
    expect(articleOrdinalLabel(1)).toBe("第 1 篇")
    expect(articleOrdinalLabel(0)).toBe("第 0 篇")
    expect(articleOrdinalLabel(null)).toBeNull()
  })

  it("joins ordinal, page range and DOI in a stable order", () => {
    expect(articleMetaText(article({
      ordinal: 1,
      pageRange: { start: "S1", end: "S5" },
      doi: "10.1038/s41586-024-00001-2",
    }))).toBe("第 1 篇 · 页码 S1–S5 · DOI 10.1038/s41586-024-00001-2")

    expect(articleMetaText(article({ ordinal: null, pageRange: null, doi: null }))).toBe("")
    expect(articleMetaText(article({ pageRange: { start: "e12345", end: null } }))).toBe("页码 e12345")
  })

  it("never puts the source key or the remote article identity into display text", () => {
    const meta = articleMetaText(article({
      ordinal: 2,
      sourceKey: "europepmc",
      remoteArticleId: "PMC7654321",
      pageRange: { start: "1", end: "2" },
    }))
    expect(meta).not.toContain("europepmc")
    expect(meta).not.toContain("PMC7654321")
  })
})

describe("issnEntries", () => {
  it("lists only the ISSN identities the backend actually returned", () => {
    expect(issnEntries("1234-5679", null)).toEqual([
      { key: "print", label: "ISSN（印刷版）", value: "1234-5679" },
    ])
    expect(issnEntries(null, "2041-1723")).toEqual([
      { key: "electronic", label: "ISSN（电子版）", value: "2041-1723" },
    ])
    expect(issnEntries("1234-5679", "2041-1723")).toEqual([
      { key: "print", label: "ISSN（印刷版）", value: "1234-5679" },
      { key: "electronic", label: "ISSN（电子版）", value: "2041-1723" },
    ])
  })
})

describe("collectArticleMediaItemIds", () => {
  const tree: PeriodicalTreeDto = {
    schemaVersion: 1,
    workId: "11111111-1111-4111-8111-111111111111",
    periodical: {
      id: "p1",
      workId: "11111111-1111-4111-8111-111111111111",
      title: "示范期刊",
      issnPrint: null,
      issnElectronic: "1234-5679",
      publisher: null,
    },
    volumes: [
      volume({
        id: "v1",
        ordinal: 0,
        issues: [
          issue({ id: "i1", ordinal: 0, articles: [article({ id: "a1", mediaItemId: "m1" }), article({ id: "a2", mediaItemId: "m2" })] }),
          issue({ id: "i2", ordinal: 1, articles: [article({ id: "a3", mediaItemId: "m2" })] }),
        ],
      }),
      volume({ id: "v2", ordinal: 1, issues: [issue({ id: "i3", articles: [article({ id: "a4", mediaItemId: "m3" })] })] }),
    ],
  }

  it("collects media item ids in backend order without duplicates", () => {
    expect(collectArticleMediaItemIds(tree)).toEqual(["m1", "m2", "m3"])
  })

  it("returns nothing for a tree the backend confirmed as empty", () => {
    expect(collectArticleMediaItemIds({ ...tree, volumes: [] })).toEqual([])
  })
})
