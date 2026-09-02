import { describe, expect, it } from "vitest"
import type { ComicPageModel } from "./comic-reader-model"
import { ComicPageResourcePool } from "./comic-page-resource-pool"

function pageUri(index: number): string {
  const hex = index.toString(16).padStart(8, "0")
  return `haven-resource://comic-page/${hex}-0000-0000-0000-000000000000`
}

function pages(count: number): ComicPageModel[] {
  return Array.from({ length: count }, (_, index) => ({
    pageId: `page-${index}`,
    pageIndex: index,
    pageNumber: index + 1,
    availability: index === 2 ? "unavailable" as const : "ready" as const,
    contentUri: index === 2 ? null : pageUri(index + 1),
  }))
}

describe("comic-page-resource-pool", () => {
  it("grants at most four DOM permits and advances the queue after release", async () => {
    const pool = new ComicPageResourcePool(pages(10), { maxConcurrent: 4 })
    const loads = [1, 2, 3, 4, 5, 6].map((page) => pool.load(page))

    expect(pool.activeCount).toBe(4)
    await expect(loads[2]).resolves.toEqual({ status: "unavailable", resource: null })
    expect(pool.isLoading(6)).toBe(true)
    await expect(loads[4]).resolves.toMatchObject({ status: "loaded", resource: { src: expect.stringContaining("haven-resource://comic-page/") } })

    pool.release(1)
    expect(pool.activeCount).toBe(4)
    await expect(loads[5]).resolves.toMatchObject({ status: "loaded" })
    for (const page of [2, 4, 5, 6]) pool.release(page)

    await expect(loads[0]).resolves.toMatchObject({ status: "loaded" })
    await expect(loads[1]).resolves.toMatchObject({ status: "loaded" })
    await expect(loads[3]).resolves.toMatchObject({ status: "loaded" })
    await expect(loads[4]).resolves.toMatchObject({ status: "loaded" })
    await expect(loads[5]).resolves.toMatchObject({ status: "loaded" })
    expect(pool.activeCount).toBe(0)
  })

  it("cancels queued and granted pages outside the retained window", async () => {
    const pool = new ComicPageResourcePool(pages(12), { maxConcurrent: 4 })
    const loads = [1, 2, 4, 5, 6].map((page) => pool.load(page))
    expect(pool.activeCount).toBe(4)

    pool.retain([2, 4, 5])
    await expect(loads[0]).resolves.toMatchObject({ status: "loaded" })
    await expect(loads[4]).resolves.toEqual({ status: "cancelled", resource: null })
    expect(pool.activeCount).toBe(3)

    pool.release(2)
    pool.release(4)
    pool.release(5)
    expect(pool.activeCount).toBe(0)
  })

  it("reacquires a fresh permit for every remounted DOM image", async () => {
    const pool = new ComicPageResourcePool(pages(12), { maxConcurrent: 4 })
    const window = [1, 2, 4, 5, 6, 7]
    const firstLoads = window.map((page) => pool.load(page))
    for (const page of window) {
      await firstLoads[window.indexOf(page)]
      pool.release(page)
    }
    expect(pool.activeCount).toBe(0)

    const secondLoads = window.map((page) => pool.load(page))
    expect(pool.activeCount).toBe(4)
    expect(pool.isLoading(6)).toBe(true)
    expect(pool.isLoading(7)).toBe(true)
    for (const page of [1, 2, 4, 5]) pool.release(page)
    await secondLoads[4]
    await secondLoads[5]
    pool.release(6)
    pool.release(7)
    expect(pool.activeCount).toBe(0)
  })

  it("resolves a second DOM consumer after the page permit is already granted", async () => {
    const pool = new ComicPageResourcePool(pages(4), { maxConcurrent: 1 })
    const first = await pool.load(1)
    expect(first.status).toBe("loaded")

    const second = pool.load(1)
    await expect(second).resolves.toMatchObject({
      status: "loaded",
      resource: { src: expect.stringContaining("haven-resource://comic-page/") },
    })
    pool.release(1)
    pool.release(1)
  })

  it("keeps a shared page permit until every DOM consumer releases it", async () => {
    const pool = new ComicPageResourcePool(pages(4), { maxConcurrent: 1 })
    const first = await pool.load(1)
    const second = await pool.load(1)
    expect(first.status).toBe("loaded")
    expect(second.status).toBe("loaded")
    if (first.status !== "loaded" || second.status !== "loaded") throw new Error("expected loaded resources")

    first.resource.release()
    expect(pool.activeCount).toBe(1)
    second.resource.release()
    expect(pool.activeCount).toBe(0)
  })

  it("cancels only the unmounted request when consumers share a queued page", async () => {
    const pool = new ComicPageResourcePool(pages(4), { maxConcurrent: 1 })
    await pool.load(1)
    const first = pool.load(2)
    const second = pool.load(2)

    // A single unmount must not cancel the other mounted image request.
    first.cancel()
    expect(pool.isLoading(2)).toBe(true)
    pool.release(1)
    await expect(second).resolves.toMatchObject({ status: "loaded" })
    expect(pool.activeCount).toBe(1)
    await expect(first).resolves.toMatchObject({ status: "cancelled" })
    pool.release(2)
    expect(pool.activeCount).toBe(0)
  })

  it("keeps release and cancellation idempotent for one DOM lease", async () => {
    const pool = new ComicPageResourcePool(pages(2), { maxConcurrent: 1 })
    const request = pool.load(1)
    const result = await request
    expect(result.status).toBe("loaded")
    if (result.status !== "loaded") throw new Error("expected loaded resource")

    result.resource.release()
    result.resource.release()
    request.cancel()
    expect(pool.activeCount).toBe(0)
    expect(pool.isLoading(1)).toBe(false)
  })

  it("keeps permits bounded while a long comic window moves", async () => {
    const pool = new ComicPageResourcePool(pages(1_000), { maxConcurrent: 4 })
    for (const center of [1, 250, 500, 750, 1_000]) {
      const retained = Array.from({ length: 9 }, (_, index) => Math.max(1, Math.min(1_000, center - 4 + index)))
      pool.retain(retained)
      const requests = retained.map((page) => pool.load(page))
      expect(pool.activeCount).toBeLessThanOrEqual(4)
      // Simulate each mounted image finishing and releasing its lease. Waiting
      // for every promise before releasing would intentionally deadlock a
      // bounded pool because only four DOM reads may be active at once.
      await Promise.all(requests.map(async (request) => {
        const result = await request
        if (result.status === "loaded") result.resource.release()
      }))
      expect(pool.activeCount).toBe(0)
    }
  })

  it("does not accept malformed page URIs and preserves unavailable state", async () => {
    const malformed = [{
      pageId: "bad",
      pageIndex: 0,
      pageNumber: 1,
      availability: "ready" as const,
      contentUri: "https://example.com/page.gif",
    }]
    const pool = new ComicPageResourcePool(malformed)
    await expect(pool.load(1)).resolves.toEqual({ status: "error", resource: null })
    await expect(pool.load(99)).resolves.toEqual({ status: "unavailable", resource: null })
  })

  it("disposes permits and ignores late releases from an old pool", async () => {
    const oldPool = new ComicPageResourcePool(pages(4))
    await oldPool.load(1)
    oldPool.dispose()
    expect(oldPool.activeCount).toBe(0)
    oldPool.release(1)

    const newPool = new ComicPageResourcePool(pages(4))
    await newPool.load(1)
    expect(newPool.activeCount).toBe(1)
    oldPool.release(1)
    expect(newPool.activeCount).toBe(1)
    newPool.release(1)
    expect(newPool.activeCount).toBe(0)
  })
})
