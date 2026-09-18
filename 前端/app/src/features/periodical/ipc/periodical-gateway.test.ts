import { describe, expect, it, vi } from "vitest"
import type { HavenClient } from "@/lib/ipc/client"
import { HavenError } from "@/lib/ipc/errors"
import type { PeriodicalTreeDto } from "@/lib/ipc/generated/wire"
import { getPeriodicalTree } from "./periodical-gateway"

// Gateway 只通过注入的 client 访问 IPC。这里切断 runtime → TauriHavenClient 的默认
// 依赖链：既让测试不需要真实 Tauri 运行时，也保证用例不会悄悄走环境默认客户端。
vi.mock("@/lib/ipc/runtime.js", () => ({
  getHavenClient: () => {
    throw new Error("gateway 测试必须显式注入 client")
  },
}))

const WORK_ID = "11111111-1111-4111-8111-111111111111"
const OTHER_WORK_ID = "99999999-9999-4999-8999-999999999999"
const PERIODICAL_ID = "22222222-2222-4222-8222-222222222201"
const VOLUME_ID = "33333333-3333-4333-8333-333333333301"
const ISSUE_ID = "44444444-4444-4444-8444-444444444401"
const ARTICLE_ID = "55555555-5555-4555-8555-555555555501"
const MEDIA_ITEM_ID = "66666666-6666-4666-8666-666666666601"

/** 合法期刊树：一个期刊 → 一卷 → 一期 → 一篇文章，关系全部闭合。 */
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
        id: VOLUME_ID,
        periodicalId: PERIODICAL_ID,
        label: null,
        number: 12,
        year: 2024,
        ordinal: 0,
        issues: [
          {
            id: ISSUE_ID,
            volumeId: VOLUME_ID,
            label: "3-4",
            number: null,
            publicationDate: "2024-03",
            ordinal: 0,
            articles: [
              {
                id: ARTICLE_ID,
                issueId: ISSUE_ID,
                mediaItemId: MEDIA_ITEM_ID,
                ordinal: 1,
                title: "一篇期刊文章",
                doi: null,
                pageRange: { start: "e12345", end: null },
                sourceKey: "europepmc",
                remoteArticleId: "PMC1",
                availability: "metadata_only",
              },
            ],
          },
        ],
      },
    ],
  }
}

type PeriodicalClient = Pick<HavenClient, "periodicalTreeGet">

function clientReturning(value: unknown): PeriodicalClient {
  return { periodicalTreeGet: vi.fn().mockResolvedValue(value) }
}

describe("getPeriodicalTree", () => {
  it("sends only the local work identity and returns the validated tree", async () => {
    const tree = normal()
    const periodicalTreeGet = vi.fn().mockResolvedValue(tree)
    const client: PeriodicalClient = { periodicalTreeGet }

    await expect(getPeriodicalTree(WORK_ID, client)).resolves.toBe(tree)
    // 请求体必须精确等于 { workId }：不夹带来源、Provider 或任何传输参数。
    expect(periodicalTreeGet).toHaveBeenCalledTimes(1)
    expect(periodicalTreeGet).toHaveBeenCalledWith({ workId: WORK_ID })
  })

  it("accepts a canonical lowercase identity that carries hex letters", async () => {
    const lowercase = "a2222222-2222-4222-8222-2222222222a1"
    const echoed = { ...normal(), workId: lowercase }
    const client: PeriodicalClient = {
      periodicalTreeGet: vi.fn().mockResolvedValue({
        ...echoed,
        periodical: { ...echoed.periodical, workId: lowercase },
      }),
    }

    await expect(getPeriodicalTree(lowercase, client)).resolves.toMatchObject({ workId: lowercase })
  })

  it("rejects a non-canonical work identity before invoking IPC", async () => {
    const periodicalTreeGet = vi.fn()
    const client: PeriodicalClient = { periodicalTreeGet }

    // 判别数据必须含十六进制字母，否则 toUpperCase() 是恒等变换、断言必然通过。
    const invalid = [
      "",
      "not-a-uuid",
      "https://example.invalid/work",
      "11111111-1111-4111-8111-11111111111A",
      "11111111-1111-4111-8111-111111111111 ",
      null,
      undefined,
      1111,
    ]

    for (const workId of invalid) {
      await expect(getPeriodicalTree(workId as string, client))
        .rejects.toMatchObject({ code: "INVALID_ARGUMENT", retryable: false })
    }
    expect(periodicalTreeGet).not.toHaveBeenCalled()
  })

  it("rejects a response that does not echo the requested work", async () => {
    const foreign = { ...normal(), workId: OTHER_WORK_ID }
    await expect(getPeriodicalTree(WORK_ID, clientReturning(foreign)))
      .rejects.toMatchObject({ code: "PERIODICAL_INVALID_RESPONSE", retryable: false })
  })

  it("rejects untrusted response shapes without falling back or retrying", async () => {
    const periodicalTreeGet = vi.fn().mockResolvedValue({ ...normal(), providerUrl: "https://example.invalid" })
    const client: PeriodicalClient = { periodicalTreeGet }

    await expect(getPeriodicalTree(WORK_ID, client))
      .rejects.toMatchObject({ code: "PERIODICAL_INVALID_RESPONSE", retryable: false })
    expect(periodicalTreeGet).toHaveBeenCalledTimes(1)
  })

  it("rejects a response whose relations are not closed", async () => {
    const tree = normal()
    const broken = {
      ...tree,
      volumes: [{ ...tree.volumes[0], periodicalId: OTHER_WORK_ID }],
    }
    await expect(getPeriodicalTree(WORK_ID, clientReturning(broken)))
      .rejects.toMatchObject({ code: "PERIODICAL_INVALID_RESPONSE", retryable: false })
  })

  it("rejects a response that leaks a resource address", async () => {
    const tree = normal()
    const issue = tree.volumes[0].issues[0]
    const leaked = {
      ...tree,
      volumes: [{
        ...tree.volumes[0],
        issues: [{
          ...issue,
          articles: [{ ...issue.articles[0], remoteArticleId: "https://example.invalid/a.pdf" }],
        }],
      }],
    }
    await expect(getPeriodicalTree(WORK_ID, clientReturning(leaked)))
      .rejects.toMatchObject({ code: "PERIODICAL_INVALID_RESPONSE", retryable: false })
  })

  it("rejects a response whose article availability is missing or outside the closed enum", async () => {
    const tree = normal()
    const issue = tree.volumes[0].issues[0]
    const withFirstArticle = (article: unknown) => ({
      ...tree,
      volumes: [{
        ...tree.volumes[0],
        issues: [{ ...issue, articles: [article] }],
      }],
    })

    // 缺失字段不能被当成默认的 unknown——那正是页面用来区分「来源只有元数据」的字段。
    const { availability: _omitted, ...withoutAvailability } = issue.articles[0]
    for (const article of [
      withoutAvailability,
      { ...issue.articles[0], availability: "readable" },
      { ...issue.articles[0], availability: "MetadataOnly" },
      { ...issue.articles[0], availability: null },
    ]) {
      await expect(getPeriodicalTree(WORK_ID, clientReturning(withFirstArticle(article))))
        .rejects.toMatchObject({ code: "PERIODICAL_INVALID_RESPONSE", retryable: false })
    }

    // 反过来：闭合枚举内的取值必须原样通过，不做任何归一化。
    for (const availability of ["full_text", "metadata_only", "unknown"]) {
      const accepted = withFirstArticle({ ...issue.articles[0], availability })
      await expect(getPeriodicalTree(WORK_ID, clientReturning(accepted)))
        .resolves.toMatchObject({ workId: WORK_ID })
    }
  })

  it("rejects empty and non-object responses", async () => {
    for (const value of [null, undefined, {}, [], "tree"]) {
      await expect(getPeriodicalTree(WORK_ID, clientReturning(value)))
        .rejects.toMatchObject({ code: "PERIODICAL_INVALID_RESPONSE", retryable: false })
    }
  })

  it("preserves a typed client error", async () => {
    const expected = new HavenError({
      code: "PERIODICAL_NOT_FOUND",
      userMessage: "该作品没有期刊层级",
      retryable: false,
    })
    const client: PeriodicalClient = { periodicalTreeGet: vi.fn().mockRejectedValue(expected) }

    await expect(getPeriodicalTree(WORK_ID, client)).rejects.toBe(expected)
  })

  it("normalizes a non-contract rejection into a HavenError", async () => {
    const client: PeriodicalClient = {
      periodicalTreeGet: vi.fn().mockRejectedValue(new Error("ipc exploded")),
    }

    await expect(getPeriodicalTree(WORK_ID, client))
      .rejects.toMatchObject({ code: "INTERNAL_ERROR", retryable: false })
  })
})
