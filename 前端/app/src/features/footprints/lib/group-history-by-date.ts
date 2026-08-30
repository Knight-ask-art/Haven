import type { MediaCardProps } from "@/components/ui/haven/MediaCard"

export type HistoryItem = MediaCardProps & { lastActiveAt: string }

export interface HistoryGroup {
  title: string
  items: MediaCardProps[]
}

function startOfDay(date: Date): Date {
  const d = new Date(date)
  d.setHours(0, 0, 0, 0)
  return d
}

function isSameDay(a: Date, b: Date): boolean {
  return (
    a.getFullYear() === b.getFullYear() &&
    a.getMonth() === b.getMonth() &&
    a.getDate() === b.getDate()
  )
}

function formatMonthGroup(date: Date): string {
  return `${date.getFullYear()}年${date.getMonth() + 1}月`
}

/**
 * 按 lastActiveAt 将历史分组为：今天 / 昨天 / 更早（按月归档）。
 * 输入已按 lastActiveAt 倒序；组内保持原序，组间按时间倒序。
 */
export function groupHistoryByDate(items: HistoryItem[]): HistoryGroup[] {
  if (items.length === 0) return []

  const now = new Date()
  const todayStart = startOfDay(now)
  const yesterdayStart = new Date(todayStart)
  yesterdayStart.setDate(yesterdayStart.getDate() - 1)

  const today: MediaCardProps[] = []
  const yesterday: MediaCardProps[] = []
  const earlierByMonth = new Map<string, MediaCardProps[]>()

  for (const item of items) {
    const d = new Date(item.lastActiveAt)
    if (Number.isNaN(d.getTime())) {
      // Invalid date falls into earliest bucket
      const key = "更早"
      const bucket = earlierByMonth.get(key)
      if (bucket) bucket.push(item)
      else earlierByMonth.set(key, [item])
      continue
    }
    if (isSameDay(d, now)) {
      today.push(item)
    } else if (isSameDay(d, yesterdayStart)) {
      yesterday.push(item)
    } else {
      const key = formatMonthGroup(d)
      const bucket = earlierByMonth.get(key)
      if (bucket) bucket.push(item)
      else earlierByMonth.set(key, [item])
    }
  }

  const groups: HistoryGroup[] = []
  if (today.length > 0) groups.push({ title: "今天", items: today })
  if (yesterday.length > 0) groups.push({ title: "昨天", items: yesterday })
  // earlier months sorted desc by parsed year/month
  const monthEntries = [...earlierByMonth.entries()].sort((a, b) => {
    // "2026年7月"  vs "更早"
    if (a[0] === "更早") return 1
    if (b[0] === "更早") return -1
    const parse = (s: string) => {
      const m = s.match(/(\d+)年(\d+)月/)
      if (!m) return 0
      return Number(m[1]) * 12 + Number(m[2])
    }
    return parse(b[0]) - parse(a[0])
  })
  for (const [title, monthItems] of monthEntries) {
    groups.push({ title, items: monthItems })
  }
  return groups
}
