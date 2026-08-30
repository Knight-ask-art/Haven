import { describe, expect, it } from "vitest"
import { groupHistoryByDate, type HistoryItem } from "./group-history-by-date"

function item(id: string, lastActiveAt: string): HistoryItem {
  return { id, title: id, imageUrl: `http://${id}`, lastActiveAt } as HistoryItem
}

describe("groupHistoryByDate", () => {
  it("empty returns empty", () => {
    expect(groupHistoryByDate([])).toEqual([])
  })
  it("groups today / yesterday / earlier month", () => {
    const now = new Date()
    const today = now.toISOString()
    const yesterday = new Date(now)
    yesterday.setDate(yesterday.getDate() - 1)
    const yesterdayIso = yesterday.toISOString()
    const earlier = new Date(now)
    earlier.setMonth(earlier.getMonth() - 1)
    const earlierIso = earlier.toISOString()

    const groups = groupHistoryByDate([
      item("t1", today),
      item("y1", yesterdayIso),
      item("e1", earlierIso),
    ])
    expect(groups[0].title).toBe("今天")
    expect(groups[1].title).toBe("昨天")
    expect(groups[2].title).toMatch(/\d+年\d+月/)
  })
  it("sorts earlier months desc", () => {
    const groups = groupHistoryByDate([
      item("a", "2026-05-10T12:00:00.000Z"),
      item("b", "2026-07-10T12:00:00.000Z"),
      item("c", "2026-06-10T12:00:00.000Z"),
    ])
    // none are today/yesterday (mock today is real now, but 2026 is future vs 2026? Let's force 2026 group ordering)
    // Ensure July before June before May
    const titles = groups.map((g) => g.title)
    const julyIdx = titles.indexOf("2026年7月")
    const juneIdx = titles.indexOf("2026年6月")
    const mayIdx = titles.indexOf("2026年5月")
    expect(julyIdx).toBeLessThan(juneIdx)
    expect(juneIdx).toBeLessThan(mayIdx)
  })
})
