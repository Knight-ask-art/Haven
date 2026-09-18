import { describe, expect, it } from "vitest"
import { MockHavenClient } from "./mock-client"
import { isPeriodicalTreeDto } from "./periodical-tree"

const CANONICAL_UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
/** Mock 只为这个演示 Work 提供期刊层级；其它 Work（含演示书库里的图书）都必须被判为无期刊。 */
const DEMO_PERIODICAL_WORK_ID = "0196f0d2-0000-7000-8000-00000000e001"
const DEMO_LIBRARY_BOOK_WORK_ID = "0196f0d2-0000-7000-8000-000000000001"

/** 递归收集投影中的全部字符串，用于断言没有夹带任何资源地址。 */
function collectStrings(value: unknown, collected: string[] = []): string[] {
  if (typeof value === "string") {
    collected.push(value)
  } else if (Array.isArray(value)) {
    for (const item of value) collectStrings(item, collected)
  } else if (typeof value === "object" && value !== null) {
    for (const item of Object.values(value)) collectStrings(item, collected)
  }
  return collected
}

describe("MockHavenClient periodicalTreeGet", () => {
  it("returns a closed multi-volume tree for the demo periodical work", async () => {
    const client = new MockHavenClient()
    const tree = await client.periodicalTreeGet({ workId: DEMO_PERIODICAL_WORK_ID })

    expect(isPeriodicalTreeDto(tree, { workId: DEMO_PERIODICAL_WORK_ID })).toBe(true)
    // 至少两个卷层级，每卷都有期，每期都有归属到既有 MediaItem 的文章。
    expect(tree.volumes.length).toBeGreaterThanOrEqual(2)
    const issues = tree.volumes.flatMap((volume) => volume.issues)
    expect(issues.length).toBeGreaterThanOrEqual(2)
    expect(tree.volumes.every((volume) => volume.issues.length > 0)).toBe(true)
    const articles = issues.flatMap((issue) => issue.articles)
    expect(articles.length).toBeGreaterThan(0)
    expect(issues.every((issue) => issue.articles.length > 0)).toBe(true)
    expect(articles.every((article) => CANONICAL_UUID_PATTERN.test(article.mediaItemId))).toBe(true)
    expect(new Set(articles.map((article) => article.mediaItemId)).size).toBe(articles.length)
    // Demo 必须带上闭合的 Provider 正文观察，且同时覆盖三态：页面据此区分
    // 「仅有元数据」与「没有观察到」，不能用同一个默认值糊过去。
    const availability = new Set(articles.map((article) => article.availability))
    expect([...availability].sort()).toEqual(["full_text", "metadata_only", "unknown"])
  })

  it("stays deterministic and free of resource addresses", async () => {
    const client = new MockHavenClient()
    const first = await client.periodicalTreeGet({ workId: DEMO_PERIODICAL_WORK_ID })
    const second = await client.periodicalTreeGet({ workId: DEMO_PERIODICAL_WORK_ID })

    expect(second).toEqual(first)
    // Demo 不伪造正文 URL、signed URL 或本地路径。
    const strings = collectStrings(first)
    expect(strings.some((value) => value.includes("://"))).toBe(false)
    expect(strings.some((value) => value.startsWith("/"))).toBe(false)
  })

  it("rejects every other work with a stable PERIODICAL_NOT_FOUND", async () => {
    const client = new MockHavenClient()

    for (const workId of [DEMO_LIBRARY_BOOK_WORK_ID, "0196f0d2-0000-7000-8000-0000000000ff"]) {
      await expect(client.periodicalTreeGet({ workId }))
        .rejects.toMatchObject({ code: "PERIODICAL_NOT_FOUND", retryable: false })
    }
    // 同一未知 Work 重复查询必须给出同一个稳定错误，而不是先成功后失败。
    await expect(client.periodicalTreeGet({ workId: DEMO_LIBRARY_BOOK_WORK_ID }))
      .rejects.toMatchObject({ code: "PERIODICAL_NOT_FOUND", retryable: false })
  })
})
