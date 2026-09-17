import { describe, expect, it } from "vitest"
import type { PeriodicalTreeDto } from "./generated/wire"
import { isPeriodicalTreeDto } from "./periodical-tree"

const WORK_ID = "11111111-1111-4111-8111-111111111111"
const PERIODICAL_ID = "22222222-2222-4222-8222-222222222201"
const VOLUME_A = "33333333-3333-4333-8333-333333333301"
const VOLUME_B = "33333333-3333-4333-8333-333333333302"
const ISSUE_A1 = "44444444-4444-4444-8444-444444444401"
const ISSUE_A2 = "44444444-4444-4444-8444-444444444402"
const ISSUE_B1 = "44444444-4444-4444-8444-444444444403"
const ARTICLE_A1A = "55555555-5555-4555-8555-555555555501"
const ARTICLE_A1B = "55555555-5555-4555-8555-555555555502"
const ARTICLE_A2A = "55555555-5555-4555-8555-555555555503"
const ARTICLE_B1A = "55555555-5555-4555-8555-555555555504"
const MEDIA_A1A = "66666666-6666-4666-8666-666666666601"
const MEDIA_A1B = "66666666-6666-4666-8666-666666666602"
const MEDIA_A2A = "66666666-6666-4666-8666-666666666603"
const MEDIA_B1A = "66666666-6666-4666-8666-666666666604"

/**
 * 完整合法的期刊树：一个期刊 → 两个卷 → 三个期 → 四篇文章，
 * 覆盖可空字段（ISSN、DOI、页码区间、期号原文）与派生序号。
 */
function normal(): PeriodicalTreeDto {
  return {
    schemaVersion: 1,
    workId: WORK_ID,
    periodical: {
      id: PERIODICAL_ID,
      workId: WORK_ID,
      title: "Nature Communications",
      issnPrint: null,
      issnElectronic: "2041-1723",
      publisher: "Nature Portfolio",
    },
    volumes: [
      {
        id: VOLUME_A,
        periodicalId: PERIODICAL_ID,
        label: null,
        number: 12,
        year: 2024,
        ordinal: 0,
        issues: [
          {
            id: ISSUE_A1,
            volumeId: VOLUME_A,
            label: "3-4",
            number: null,
            publicationDate: "2024-03",
            ordinal: 0,
            articles: [
              {
                id: ARTICLE_A1A,
                issueId: ISSUE_A1,
                mediaItemId: MEDIA_A1A,
                ordinal: 1,
                title: "一篇期刊文章",
                doi: "10.1038/s41467-024-00001-2",
                pageRange: { start: "e12345", end: null },
                sourceKey: "europepmc",
                remoteArticleId: "PMC1",
                availability: "full_text",
              },
              {
                id: ARTICLE_A1B,
                issueId: ISSUE_A1,
                mediaItemId: MEDIA_A1B,
                ordinal: null,
                title: "没有来源序号的第二篇",
                doi: null,
                pageRange: null,
                sourceKey: "europepmc",
                remoteArticleId: "PMC2",
                availability: "metadata_only",
              },
            ],
          },
          {
            id: ISSUE_A2,
            volumeId: VOLUME_A,
            label: null,
            number: 4,
            publicationDate: "2024-04-18",
            ordinal: 1,
            articles: [
              {
                id: ARTICLE_A2A,
                issueId: ISSUE_A2,
                mediaItemId: MEDIA_A2A,
                ordinal: 1,
                title: "不规则页码",
                doi: null,
                pageRange: { start: "S1", end: "S5" },
                sourceKey: "europepmc",
                remoteArticleId: "PMC3",
                availability: "unknown",
              },
            ],
          },
        ],
      },
      {
        id: VOLUME_B,
        periodicalId: PERIODICAL_ID,
        label: "Suppl 1",
        number: null,
        year: 2023,
        ordinal: 1,
        issues: [
          {
            id: ISSUE_B1,
            volumeId: VOLUME_B,
            label: "Spring",
            number: null,
            publicationDate: "2023",
            ordinal: 0,
            articles: [
              {
                id: ARTICLE_B1A,
                issueId: ISSUE_B1,
                mediaItemId: MEDIA_B1A,
                ordinal: 1,
                title: "增刊文章",
                doi: null,
                pageRange: null,
                sourceKey: "crossref",
                remoteArticleId: "10.1000/xyz",
                availability: "full_text",
              },
            ],
          },
        ],
      },
    ],
  }
}

type Json = Record<string, unknown>

function clone(): Json {
  return structuredClone(normal()) as unknown as Json
}

/** 取首个卷 / 期 / 文章的可变引用，便于逐层构造变体。 */
function firstVolume(tree: Json): Json {
  return (tree.volumes as Json[])[0]
}

function firstIssue(tree: Json): Json {
  return (firstVolume(tree).issues as Json[])[0]
}

function firstArticle(tree: Json): Json {
  return (firstIssue(tree).articles as Json[])[0]
}

function periodicalOf(tree: Json): Json {
  return tree.periodical as Json
}

/** 先深拷贝再应用替换器，构造只在一个位置偏离合法的变体。 */
function mutate(apply: (tree: Json) => void): Json {
  const tree = clone()
  apply(tree)
  return tree
}

function withVolumeField(key: string, value: unknown): Json {
  return mutate((tree) => {
    firstVolume(tree)[key] = value
  })
}

function withIssueField(key: string, value: unknown): Json {
  return mutate((tree) => {
    firstIssue(tree)[key] = value
  })
}

function withArticleField(key: string, value: unknown): Json {
  return mutate((tree) => {
    firstArticle(tree)[key] = value
  })
}

describe("isPeriodicalTreeDto", () => {
  it("accepts the complete multi-volume tree", () => {
    expect(isPeriodicalTreeDto(normal())).toBe(true)
  })

  it("stays idempotent for a repeated check of the same payload", () => {
    const tree = normal()
    expect(isPeriodicalTreeDto(tree)).toBe(true)
    expect(isPeriodicalTreeDto(tree)).toBe(true)
  })

  it("accepts the response only when it echoes the requested work", () => {
    expect(isPeriodicalTreeDto(normal(), { workId: WORK_ID })).toBe(true)
    expect(isPeriodicalTreeDto(normal(), { workId: PERIODICAL_ID })).toBe(false)
  })

  it("rejects a tree that belongs to another work", () => {
    const other = "99999999-9999-4999-8999-999999999999"
    expect(isPeriodicalTreeDto(mutate((tree) => { tree.workId = other }))).toBe(false)
    expect(isPeriodicalTreeDto(normal(), { workId: other })).toBe(false)
  })

  it("rejects a non-1 schema version", () => {
    for (const schemaVersion of [0, 2, "1", null, undefined, 1.5]) {
      expect(isPeriodicalTreeDto(mutate((tree) => { tree.schemaVersion = schemaVersion }))).toBe(false)
    }
  })

  it("rejects non-object payloads", () => {
    for (const value of [null, undefined, [], "tree", 1, true]) {
      expect(isPeriodicalTreeDto(value)).toBe(false)
    }
  })

  it("rejects unknown fields at every level", () => {
    expect(isPeriodicalTreeDto(mutate((tree) => { tree.providerUrl = "https://example.invalid" }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => {
      periodicalOf(tree).signedUri = "https://example.invalid/a.pdf"
    }))).toBe(false)
    expect(isPeriodicalTreeDto(withVolumeField("issueCount", 2))).toBe(false)
    expect(isPeriodicalTreeDto(withIssueField("articlesUrl", "https://example.invalid"))).toBe(false)
    expect(isPeriodicalTreeDto(withArticleField("fullTextUri", "https://example.invalid"))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => {
      (firstArticle(tree).pageRange as Json).label = "x"
    }))).toBe(false)
  })

  it("rejects missing fields at every level", () => {
    expect(isPeriodicalTreeDto(mutate((tree) => { delete tree.volumes }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => { delete periodicalOf(tree).publisher }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => { delete firstVolume(tree).issues }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => { delete firstIssue(tree).articles }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => { delete firstArticle(tree).pageRange }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => { delete firstArticle(tree).remoteArticleId }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => { delete firstVolume(tree).year }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => { delete firstIssue(tree).publicationDate }))).toBe(false)
  })

  it("keeps every relation closed", () => {
    const other = "99999999-9999-4999-8999-999999999999"
    // 期刊必须属于树声明的 Work。
    expect(isPeriodicalTreeDto(mutate((tree) => {
      periodicalOf(tree).workId = other
    }))).toBe(false)
    // 卷必须属于本树的期刊。
    expect(isPeriodicalTreeDto(withVolumeField("periodicalId", PERIODICAL_ID.replace("2222", "9999")))).toBe(false)
    // 期必须属于本卷。
    expect(isPeriodicalTreeDto(withIssueField("volumeId", VOLUME_B))).toBe(false)
    // 文章必须属于本期。
    expect(isPeriodicalTreeDto(withArticleField("issueId", ISSUE_A2))).toBe(false)
    expect(isPeriodicalTreeDto(withArticleField("issueId", other))).toBe(false)
  })

  it("rejects non-canonical local identities", () => {
    // 判别数据必须含十六进制字母，否则 toUpperCase() 是恒等变换、断言必然通过。
    expect(isPeriodicalTreeDto(withArticleField("id", "55555555-5555-4555-8555-55555555550A"))).toBe(false)
    expect(isPeriodicalTreeDto(withArticleField("id", "not-a-uuid"))).toBe(false)
    expect(isPeriodicalTreeDto(withArticleField("id", "https://example.invalid/article"))).toBe(false)
    expect(isPeriodicalTreeDto(withArticleField("mediaItemId", "66666666-6666-4666-8666-66666666660A"))).toBe(false)
    expect(isPeriodicalTreeDto(withVolumeField("id", "33333333-3333-4333-8333-33333333330A"))).toBe(false)
    expect(isPeriodicalTreeDto(withVolumeField("id", VOLUME_A.slice(0, -1)))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => {
      periodicalOf(tree).id = "22222222-2222-4222-8222-22222222220A"
    }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => { tree.workId = "11111111-1111-4111-8111-11111111111A" }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => { tree.workId = "https://example.invalid/work" }))).toBe(false)
  })

  it("accepts canonical lowercase identities that carry hex letters", () => {
    const lowercase = "55555555-5555-4555-8555-5555555555a1"
    expect(isPeriodicalTreeDto(withArticleField("id", lowercase))).toBe(true)
    expect(isPeriodicalTreeDto(withArticleField("id", lowercase.toUpperCase()))).toBe(false)
  })

  it("rejects duplicate sibling identities", () => {
    expect(isPeriodicalTreeDto(mutate((tree) => { firstVolume(tree).id = VOLUME_B }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => {
      const issues = firstVolume(tree).issues as Json[]
      issues[1].id = ISSUE_A1
    }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => {
      const articles = firstIssue(tree).articles as Json[]
      articles[1].id = ARTICLE_A1A
    }))).toBe(false)
  })

  it("rejects transferred resource facts such as URLs, paths and credentials", () => {
    expect(isPeriodicalTreeDto(withArticleField("remoteArticleId", "https://example.invalid/a.pdf"))).toBe(false)
    expect(isPeriodicalTreeDto(withArticleField("sourceKey", "C:\\secrets\\key.pem"))).toBe(false)
    expect(isPeriodicalTreeDto(withArticleField("doi", "haven-resource://stream/abc"))).toBe(false)
    expect(isPeriodicalTreeDto(withArticleField("pageRange", { start: "/etc/passwd", end: null }))).toBe(false)
    expect(isPeriodicalTreeDto(withArticleField("pageRange", { start: "1", end: "..\\..\\win.ini" }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => {
      periodicalOf(tree).publisher = "https://example.invalid/?cookie=abc"
    }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => {
      tree.volumes = "https://example.invalid/volumes"
    }))).toBe(false)
  })

  it("rejects empty remote identities", () => {
    for (const value of ["", "   ", "\t\n"]) {
      expect(isPeriodicalTreeDto(withArticleField("remoteArticleId", value))).toBe(false)
      expect(isPeriodicalTreeDto(withArticleField("sourceKey", value))).toBe(false)
    }
  })

  it("accepts exactly the three closed provider availability literals", () => {
    for (const value of ["full_text", "metadata_only", "unknown"]) {
      expect(isPeriodicalTreeDto(withArticleField("availability", value))).toBe(true)
    }
  })

  it("rejects any availability value outside the closed enum", () => {
    // 未定义的取值（包括把状态名写错、或把别的 DTO 的枚举拿过来）一律判为不可信：
    // 页面据此区分的正是「仅有元数据」与「没有观察到」，不能用近似词顶替。
    for (const value of [
      "readable",
      "unavailable",
      "full",
      "metadata",
      "FULL_TEXT",
      "MetadataOnly",
      "",
      " ",
      null,
      undefined,
      0,
      1,
      true,
      ["full_text"],
      { value: "full_text" },
    ]) {
      expect(isPeriodicalTreeDto(withArticleField("availability", value))).toBe(false)
    }
  })

  it("requires the availability field to be present on every article", () => {
    expect(isPeriodicalTreeDto(mutate((tree) => {
      delete firstArticle(tree).availability
    }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => {
      const articles = firstIssue(tree).articles as Json[]
      delete articles[1].availability
    }))).toBe(false)
  })

  it("rejects non-finite or out-of-range volume and issue numbers", () => {
    for (const value of [Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY, "1", -0.5, 1_000_001]) {
      expect(isPeriodicalTreeDto(withVolumeField("number", value))).toBe(false)
      expect(isPeriodicalTreeDto(withIssueField("number", value))).toBe(false)
    }
    // null 表示来源未给出；0 与小数在来源里都可能出现，仍然是有限的非负数值。
    for (const value of [null, 0, 12, 1.5]) {
      expect(isPeriodicalTreeDto(withVolumeField("number", value))).toBe(true)
    }
  })

  it("rejects non-finite or out-of-range years", () => {
    // 后端 `PeriodicalVolume::validate` 只接受 1000..=2999。
    for (const value of [Number.NaN, Number.POSITIVE_INFINITY, -1, 1.5, 999, 3000, "2024"]) {
      expect(isPeriodicalTreeDto(withVolumeField("year", value))).toBe(false)
    }
    for (const value of [null, 1000, 2024, 2999]) {
      expect(isPeriodicalTreeDto(withVolumeField("year", value))).toBe(true)
    }
  })

  it("rejects non-integer or out-of-range ordinals", () => {
    for (const value of [Number.NaN, Number.POSITIVE_INFINITY, -1, 1.5, "0", null]) {
      expect(isPeriodicalTreeDto(withVolumeField("ordinal", value))).toBe(false)
      expect(isPeriodicalTreeDto(withIssueField("ordinal", value))).toBe(false)
    }
    for (const value of [Number.NaN, Number.POSITIVE_INFINITY, -1, 1.5, "1"]) {
      expect(isPeriodicalTreeDto(withArticleField("ordinal", value))).toBe(false)
    }
    // 文章序号可空（来源未给出），非空时仍须是非负整数。
    expect(isPeriodicalTreeDto(withArticleField("ordinal", null))).toBe(true)
    expect(isPeriodicalTreeDto(withArticleField("ordinal", 0))).toBe(true)
    // 序号在 wire 上是 u32：上界必须封住，u32 以内的值仍然通过。
    for (const value of [4_294_967_296, 2 ** 53]) {
      expect(isPeriodicalTreeDto(withVolumeField("ordinal", value))).toBe(false)
      expect(isPeriodicalTreeDto(withArticleField("ordinal", value))).toBe(false)
    }
    expect(isPeriodicalTreeDto(withVolumeField("ordinal", 4_294_967_295))).toBe(true)
  })

  it("rejects malformed ISSN, DOIs and publication dates", () => {
    for (const value of ["2041/1723", "https://example.invalid", "20411723", "2041-172", 20411723]) {
      expect(isPeriodicalTreeDto(mutate((tree) => {
        periodicalOf(tree).issnElectronic = value
      }))).toBe(false)
    }
    for (const value of ["2024-3-4x", "https://example.invalid", "3-4", "2024/03"]) {
      expect(isPeriodicalTreeDto(withIssueField("publicationDate", value))).toBe(false)
    }
    // 后端 `publication_date` 只接受 YYYY / YYYY-MM / YYYY-MM-DD，年份同样封在 1000..=2999。
    for (const value of ["2024-13", "2024-00", "2024-02-31x", "0999", "3000-01", "2024-01-32"]) {
      expect(isPeriodicalTreeDto(withIssueField("publicationDate", value))).toBe(false)
    }
    for (const value of ["2024", "2024-03", "2024-3-4", "2024-04-18", "2999"]) {
      expect(isPeriodicalTreeDto(withIssueField("publicationDate", value))).toBe(true)
    }
    // 后端只把 `10.<registrant>/<suffix>` 形态的规范化文本写进 doi 字段。
    for (const value of ["not-a-doi", "10.1038", "10./xyz", "https://doi.org/10.1/x", "10.1/x y"]) {
      expect(isPeriodicalTreeDto(withArticleField("doi", value))).toBe(false)
    }
    expect(isPeriodicalTreeDto(withArticleField("doi", "10.1038/s41467-024-00001-2"))).toBe(true)
    // 规范形式 `NNNN-NNNX` 的校验位可以确实是 X——此时末位字母是身份的一部分。
    expect(isPeriodicalTreeDto(mutate((tree) => {
      periodicalOf(tree).issnPrint = "1420-682X"
    }))).toBe(true)
  })

  it("rejects ISSN whose mod-11 check digit does not match the payload", () => {
    // 与后端 `Issn::parse` 一致：8 位数字按权重 8..1 加权（X 记 10）之和必须被 11 整除。
    // 下面每个值都只错在最后一位，形态仍然规范，只有校验位不成立。
    for (const value of ["2041-172X", "2041-1724", "1420-6820", "0378-5954", "0028-0833"]) {
      expect(isPeriodicalTreeDto(mutate((tree) => {
        periodicalOf(tree).issnElectronic = value
      }))).toBe(false)
      expect(isPeriodicalTreeDto(mutate((tree) => {
        periodicalOf(tree).issnPrint = value
        periodicalOf(tree).issnElectronic = null
      }))).toBe(false)
    }
    // 加权和能被 11 整除的真实 ISSN 仍然通过，X 校验位不是特例。
    for (const value of ["2041-1723", "1420-682X", "0378-5955", "0028-0836"]) {
      expect(isPeriodicalTreeDto(mutate((tree) => {
        periodicalOf(tree).issnElectronic = value
      }))).toBe(true)
    }
  })

  it("accepts only publication dates that exist in the calendar", () => {
    // 与后端 `publication_date_year` 一致：月份 1..12，日必须是该年该月的真实天数（含闰年）。
    for (const value of [
      "2024-02-31",
      "2023-02-29",
      "1900-02-29",
      "2999-02-29",
      "2024-04-31",
      "2024-06-31",
      "2024-09-31",
      "2024-11-31",
      "2024-02-30",
      "2024-01-32",
    ]) {
      expect(isPeriodicalTreeDto(withIssueField("publicationDate", value))).toBe(false)
    }
    // 有效边界：闰年 2 月 29 日、400 年闰规则、月末、未补零写法。
    for (const value of [
      "2024-02-29",
      "2024-2-9",
      "2000-02-29",
      "2023-02-28",
      "2024-01-31",
      "2024-04-30",
      "1000-01-01",
      "2999-12-31",
    ]) {
      expect(isPeriodicalTreeDto(withIssueField("publicationDate", value))).toBe(true)
    }
  })

  it("requires one distinct ISSN identity for the periodical", () => {
    // 后端 `Periodical::validate`（040 迁移不变量）：期刊至少需要一个 ISSN 身份，
    // 且 print 与 electronic 不得是同一个身份。
    expect(isPeriodicalTreeDto(mutate((tree) => {
      periodicalOf(tree).issnPrint = null
      periodicalOf(tree).issnElectronic = null
    }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => {
      periodicalOf(tree).issnPrint = "2041-1723"
      periodicalOf(tree).issnElectronic = "2041-1723"
    }))).toBe(false)
    // 只声明一个身份，或声明两个互不相同的身份，仍然是合法的期刊。
    expect(isPeriodicalTreeDto(mutate((tree) => {
      periodicalOf(tree).issnPrint = null
    }))).toBe(true)
    expect(isPeriodicalTreeDto(mutate((tree) => {
      periodicalOf(tree).issnPrint = "0378-5955"
    }))).toBe(true)
    expect(isPeriodicalTreeDto(mutate((tree) => {
      periodicalOf(tree).issnPrint = "0378-5955"
      periodicalOf(tree).issnElectronic = "2041-1723"
    }))).toBe(true)
  })

  it("closes exact keys against runtime symbol and non-enumerable extras", () => {
    // Symbol 键不是「看不见就不算」：`Object.keys` 会漏掉它，未知 Symbol 仍是越界字段。
    expect(isPeriodicalTreeDto(mutate((tree) => {
      Object.defineProperty(tree, Symbol("providerUrl"), { value: "https://example.invalid", enumerable: true })
    }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => {
      Object.defineProperty(periodicalOf(tree), Symbol.for("signedUri"), { value: "https://example.invalid", enumerable: true })
    }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => {
      Object.defineProperty(firstVolume(tree), Symbol("issuesUrl"), { value: "https://example.invalid", enumerable: true })
    }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => {
      Object.defineProperty(firstIssue(tree), Symbol("articlesUrl"), { value: "https://example.invalid", enumerable: true })
    }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => {
      Object.defineProperty(firstArticle(tree), Symbol("fullTextUri"), { value: "https://example.invalid", enumerable: true })
    }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => {
      Object.defineProperty(firstArticle(tree).pageRange as Json, Symbol("label"), { value: "x", enumerable: true })
    }))).toBe(false)
    // 不可枚举的自有字符串键同样不在 DTO 的键集合里。
    expect(isPeriodicalTreeDto(mutate((tree) => {
      Object.defineProperty(tree, "providerUrl", { value: "https://example.invalid", enumerable: false })
    }))).toBe(false)
  })

  it("rejects unbounded text, control characters and oversized collections", () => {
    expect(isPeriodicalTreeDto(withArticleField("title", "x".repeat(513)))).toBe(false)
    expect(isPeriodicalTreeDto(withArticleField("title", ""))).toBe(false)
    expect(isPeriodicalTreeDto(withArticleField("remoteArticleId", "x".repeat(513)))).toBe(false)
    // 512 是后端 `identity_text` 自己的上限，长但合法的卷标不应被前端误杀。
    expect(isPeriodicalTreeDto(withVolumeField("label", "x".repeat(512)))).toBe(true)
    expect(isPeriodicalTreeDto(withVolumeField("label", "x".repeat(513)))).toBe(false)
    // 页码文本上限 64 且不含空白（后端 `page_text` 语义）。
    expect(isPeriodicalTreeDto(withArticleField("pageRange", { start: "x".repeat(65), end: null }))).toBe(false)
    expect(isPeriodicalTreeDto(withArticleField("pageRange", { start: "S1 S5", end: null }))).toBe(false)
    expect(isPeriodicalTreeDto(withArticleField("title", "正文\u0000注入"))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => {
      tree.volumes = Array.from({ length: 513 }, (_unused, index) => ({
        id: `33333333-3333-4333-8333-${String(index).padStart(12, "0")}`,
        periodicalId: PERIODICAL_ID,
        label: null,
        number: null,
        year: null,
        ordinal: index,
        issues: [],
      }))
    }))).toBe(false)
    expect(isPeriodicalTreeDto(mutate((tree) => {
      firstIssue(tree).articles = Array.from({ length: 1001 }, (_unused, index) => {
        const article = structuredClone(firstArticle(clone()))
        article.id = `55555555-5555-4555-8555-${String(index).padStart(12, "0")}`
        article.ordinal = index
        return article
      })
    }))).toBe(false)
  })

  it("accepts a periodical that has no volumes yet", () => {
    expect(isPeriodicalTreeDto(mutate((tree) => { tree.volumes = [] }))).toBe(true)
    expect(isPeriodicalTreeDto(mutate((tree) => { firstVolume(tree).issues = [] }))).toBe(true)
    expect(isPeriodicalTreeDto(mutate((tree) => { firstIssue(tree).articles = [] }))).toBe(true)
  })
})
