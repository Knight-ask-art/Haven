import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useNavigate, useParams, useSearchParams } from "react-router"
import {
  ArrowLeft,
  ChevronDown,
  ChevronUp,
  RotateCcw,
  Search,
  SlidersHorizontal,
  X,
} from "lucide-react"
import { cn } from "@/lib/utils"
import { canLoadLibraryNextPage } from "../lib/library-runtime-state"
import { acceptLibraryCursor, getLibraryBrowsePage } from "../ipc/gateway"
import { isTauriRuntime } from "@/lib/ipc/runtime"
import { onFavoriteChanged, onLibraryChanged } from "@/lib/ipc/events"
import { toHavenError, type HavenError } from "@/lib/ipc/errors"
import { deriveLibrarySliceState } from "@/lib/slice-state"
import { MediaItem } from "../components/MediaItem"
import type { LibraryMediaItemData } from "../components/MediaItem"

type BrowseCategory = "video" | "comic" | "periodical" | "book" | "document"

interface FilterGroupConfig {
  key: string
  label: string
  options: string[]
}

const CATEGORY_FILTERS: Record<BrowseCategory, FilterGroupConfig[]> = {
  video: [
    { key: "region", label: "地区", options: ["全部", "中国大陆", "港澳台", "日本", "韩国", "欧美"] },
    { key: "format", label: "形态", options: ["全部", "电影", "剧集", "动画", "纪录片", "综艺"] },
    { key: "genre", label: "类型", options: ["全部", "剧情", "喜剧", "动作", "科幻", "悬疑", "推理", "惊悚", "动画", "冒险", "历史", "纪录"] },
    { key: "year", label: "年份", options: ["全部", "2026", "2025", "2024", "2023", "2022", "2021", "2020", "2019", "2018", "2015以前"] },
    { key: "season", label: "季度", options: ["全部", "1月", "4月", "7月", "10月"] },
    { key: "status", label: "观看状态", options: ["全部", "未看", "观看中", "已看完"] },
    { key: "letter", label: "首字母", options: ["全部", "A", "B", "C", "D", "E", "F", "G", "H", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "W", "X", "Y", "Z"] },
    { key: "source", label: "内容位置", options: ["全部", "本地媒体库", "个人云盘", "合法网络"] },
  ],
  comic: [
    { key: "region", label: "产地", options: ["全部", "国漫", "日漫", "美漫", "韩漫"] },
    { key: "format", label: "形态", options: ["全部", "单行本", "连载", "短篇", "条漫"] },
    { key: "genre", label: "题材", options: ["全部", "热血", "冒险", "奇幻", "校园", "日常", "科幻", "悬疑", "搞笑", "恋爱", "竞技"] },
    { key: "status", label: "阅读进度", options: ["全部", "未读", "阅读中", "已完结", "收藏中"] },
    { key: "fileType", label: "格式", options: ["全部", "CBZ", "CBR", "PDF", "EPUB"] },
    { key: "letter", label: "首字母", options: ["全部", "A", "B", "C", "D", "E", "F", "G", "H", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "W", "X", "Y", "Z"] },
    { key: "source", label: "内容位置", options: ["全部", "本地媒体库", "个人云盘"] },
  ],
  book: [
    { key: "genre", label: "图书分类", options: ["全部", "文学小说", "历史传记", "科学技术", "商业理财", "哲学心理", "艺术设计", "社会科学", "教材工具"] },
    { key: "format", label: "形态", options: ["全部", "小说", "非虚构", "教材", "工具书"] },
    { key: "language", label: "语言", options: ["全部", "中文", "英文", "日文", "其他"] },
    { key: "status", label: "阅读状态", options: ["全部", "未读", "阅读中", "已读完", "已标记"] },
    { key: "year", label: "出版年份", options: ["全部", "2026", "2025", "2024", "2023", "2022", "2020以前"] },
    { key: "fileType", label: "格式", options: ["全部", "EPUB", "PDF", "MOBI", "AZW3", "TXT"] },
    { key: "letter", label: "首字母", options: ["全部", "A", "B", "C", "D", "E", "F", "G", "H", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "W", "X", "Y", "Z"] },
    { key: "source", label: "内容位置", options: ["全部", "本地书库", "个人云盘"] },
  ],
  periodical: [
    { key: "genre", label: "刊物分类", options: ["全部", "商业财经", "科技前沿", "摄影人文", "新闻周刊", "学术论文", "官方规范"] },
    { key: "format", label: "发行周期", options: ["全部", "日刊", "周刊", "半月刊", "月刊", "季刊", "特刊/合订本"] },
    { key: "language", label: "语言", options: ["全部", "中文", "英文", "双语"] },
    { key: "year", label: "出版年份", options: ["全部", "2026", "2025", "2024", "2023", "2022", "2020以前"] },
    { key: "status", label: "状态", options: ["全部", "最新期", "已阅读", "已归档"] },
    { key: "fileType", label: "格式", options: ["全部", "PDF", "HTML", "EPUB"] },
    { key: "source", label: "内容位置", options: ["全部", "本地智库", "网络接入"] },
  ],
  document: [
    { key: "genre", label: "资料分类", options: ["全部", "官方规范", "学术论文", "科技前沿", "白皮书", "技术手册"] },
    { key: "format", label: "形态", options: ["全部", "PDF", "HTML", "EPUB", "Markdown"] },
    { key: "language", label: "语言", options: ["全部", "中文", "英文", "双语"] },
    { key: "year", label: "发布年份", options: ["全部", "2026", "2025", "2024", "2023", "2022", "2020以前"] },
    { key: "status", label: "状态", options: ["全部", "最新", "已阅读", "已归档"] },
    { key: "fileType", label: "格式", options: ["全部", "PDF", "HTML", "EPUB"] },
    { key: "source", label: "内容位置", options: ["全部", "本地智库", "网络接入"] },
  ],
}

const CATEGORY_CONFIG: Record<BrowseCategory, {
  eyebrow: string
  title: string
  description: string
  searchPlaceholder: string
}> = {
  video: {
    eyebrow: "影视空间",
    title: "影视库",
    description: "按内容类型、年份和观看状态整理你的影视内容。栖阅会自动选择可播放资源，用户只需要挑选想看的作品。",
    searchPlaceholder: "搜索片名、原名...",
  },
  comic: {
    eyebrow: "漫画空间",
    title: "漫画库",
    description: "把本地漫画、个人云盘和已收藏的连载集中在一起，按作品和阅读状态继续阅读。",
    searchPlaceholder: "搜索漫画名、原名...",
  },
  periodical: {
    eyebrow: "报刊空间",
    title: "报刊库",
    description: "按刊物、年份和阅读状态整理杂志与深度期刊，保留属于你的长期阅读线索。",
    searchPlaceholder: "搜索刊物名、原名...",
  },
  document: {
    eyebrow: "资料空间",
    title: "资料库",
    description: "按类型、年份和状态整理官方规范、学术论文与技术白皮书。",
    searchPlaceholder: "搜索资料名、原名...",
  },
  book: {
    eyebrow: "图书空间",
    title: "图书库",
    description: "按作品、年份和阅读状态整理你的电子书与长篇内容。",
    searchPlaceholder: "搜索书名、原名...",
  },
}

const DEFAULT_CATEGORY: BrowseCategory = "video"

interface ItemMeta {
  region?: string
  genre?: string
  season?: string
  status?: string
  source?: string
  language?: string
  format?: string
  order?: number
}

// 每条作品的筛选元数据（集中维护，不修改共享卡片数据）
const ITEM_META: Record<string, ItemMeta> = {
  "2": { region: "欧美", genre: "科幻,冒险", season: "10月", status: "观看中", source: "本地媒体库", format: "电影", order: 18 },
  "10": { region: "欧美", genre: "科幻,冒险", season: "7月", status: "观看中", source: "本地媒体库", format: "电影", order: 17 },
  "6": { region: "欧美", genre: "历史", season: "1月", status: "已看完", source: "本地媒体库", format: "电影", order: 16 },
  "4": { region: "欧美", genre: "悬疑,科幻", season: "10月", status: "观看中", source: "本地媒体库", format: "剧集", order: 15 },
  "1": { genre: "艺术设计", format: "工具书", language: "英文", status: "阅读中", source: "本地书库", order: 14 },
  "3": { genre: "历史传记", format: "非虚构", language: "中文", status: "已读完", source: "本地书库", order: 13 },
  "5": { genre: "科学技术", format: "教材", language: "中文", status: "阅读中", source: "本地书库", order: 12 },
  "comic-aot": { region: "日漫", genre: "热血,冒险", format: "单行本", status: "阅读中", source: "本地媒体库", order: 11 },
  "c2": { region: "日漫", genre: "冒险,搞笑", format: "单行本", status: "阅读中", source: "本地媒体库", order: 10 },
  "c3": { region: "日漫", genre: "奇幻,冒险", format: "单行本", status: "未读", source: "本地媒体库", order: 9 },
  "p1": { genre: "摄影人文", format: "月刊", language: "英文", status: "最新期", source: "本地智库", order: 8 },
  "p2": { genre: "商业财经", format: "周刊", language: "英文", status: "最新期", source: "网络接入", order: 7 },
  "p3": { genre: "科技前沿", format: "特刊/合订本", language: "英文", status: "最新期", source: "网络接入", order: 6 },
  "p4": { genre: "商业财经", format: "月刊", language: "英文", status: "已阅读", source: "本地智库", order: 5 },
  "p5": { genre: "学术论文", format: "周刊", language: "英文", status: "最新期", source: "网络接入", order: 4 },
  "p6": { genre: "新闻周刊", format: "周刊", language: "英文", status: "已归档", source: "网络接入", order: 3 },
  "d1": { genre: "官方规范", format: "特刊/合订本", language: "英文", status: "已阅读", source: "本地智库", order: 2 },
  "d2": { genre: "学术论文", format: "特刊/合订本", language: "英文", status: "已阅读", source: "网络接入", order: 1 },
  "d3": { genre: "官方规范", format: "特刊/合订本", language: "英文", status: "最新期", source: "网络接入", order: 0 },
  "d4": { genre: "科技前沿", format: "特刊/合订本", language: "中文", status: "已阅读", source: "本地智库", order: 0 },
  "d5": { genre: "科技前沿", format: "特刊/合订本", language: "中文", status: "阅读中", source: "本地智库", order: 0 },
  "d6": { genre: "科技前沿", format: "特刊/合订本", language: "中文", status: "最新期", source: "本地智库", order: 0 },
}

function parseBeforeYear(value: string): number | null {
  const match = value.match(/^(\d{4})以前$/)
  return match ? Number(match[1]) : null
}

function matchesMetaFilter(item: LibraryMediaItemData, key: string, value: string): boolean {
  if (value === "全部" || !value) return true

  if (key === "letter") {
    const firstChar = item.originalTitle?.[0]?.toUpperCase()
    return Boolean(firstChar) && firstChar === value
  }

  if (key === "year") {
    const beforeYear = parseBeforeYear(value)
    if (beforeYear !== null) return item.year > 0 && item.year <= beforeYear
    const exactYear = Number(value)
    return Number.isInteger(exactYear) && item.year === exactYear
  }

  const meta = ITEM_META[item.id]
  // Production WorkCardDto currently exposes no genre/source/status metadata.
  // Unsupported groups are hidden below; fail closed if stale UI state still
  // sends one instead of silently treating it as a match-all filter.
  if (!meta) return !isTauriRuntime()

  if (key === "genre") {
    return meta.genre?.split(",").includes(value) ?? false
  }

  return meta[key as keyof ItemMeta] === value
}

function getCategoryKey(value?: string): BrowseCategory {
  if (value === "movie") return "video"
  return value && value in CATEGORY_CONFIG ? value as BrowseCategory : DEFAULT_CATEGORY
}

export function LibraryBrowsePage() {
  const navigate = useNavigate()
  const { category } = useParams<{ category?: string }>()
  const [searchParams, setSearchParams] = useSearchParams()
  const categoryKey = getCategoryKey(category)
  const categoryConfig = CATEGORY_CONFIG[categoryKey]
  const filterGroups = CATEGORY_FILTERS[categoryKey]
  // 数据层接线（SLICE-LIBRARY-001）：经 gateway 拉取 library_list 投影，
  // 分类过滤与筛选仍在本页客户端进行（与既有交互一致）。
  const [browseItems, setBrowseItems] = useState<LibraryMediaItemData[]>([])
  const [isLoading, setIsLoading] = useState(true)
  const [isLoadingMore, setIsLoadingMore] = useState(false)
  const [isFirstPagePending, setIsFirstPagePending] = useState(true)
  const [loadError, setLoadError] = useState<HavenError | null>(null)
  const [partialError, setPartialError] = useState<HavenError | null>(null)
  const [nextCursor, setNextCursor] = useState<string | null>(null)
  const [total, setTotal] = useState<number | null>(null)
  const [reloadToken, setReloadToken] = useState(0)
  const seenCursorsRef = useRef(new Set<string>())
  const browseItemsRef = useRef<LibraryMediaItemData[]>([])
  const queryGenerationRef = useRef(0)
  const activeQueryKeyRef = useRef("")
  const cursorQueryKeyRef = useRef<string | null>(null)
  const browseRequestKey = searchParams.toString()
  const listQuery = useMemo(() => ({
    category: categoryKey === "document" ? "periodical" : categoryKey,
    query: searchParams.get("q") ?? "",
    sort: searchParams.get("sort") === "年份最新" ? "release_date" : searchParams.get("sort") === "评分最高" ? "rating" : "recently_added",
  } as const), [categoryKey, searchParams])
  const queryKey = `${categoryKey}\u0000${browseRequestKey}\u0000${reloadToken}`

  useEffect(() => {
    const generation = ++queryGenerationRef.current
    activeQueryKeyRef.current = queryKey
    cursorQueryKeyRef.current = null
    seenCursorsRef.current = new Set<string>()
    let cancelled = false
    // A cursor belongs to the complete query key. Clear the previous result
    // set before the new first page starts so a manual load-more can never
    // borrow the old cursor while the new request is in flight.
    browseItemsRef.current = []
    setBrowseItems([])
    const hadData = false
    setIsLoading(true)
    setIsLoadingMore(false)
    setIsFirstPagePending(true)
    setLoadError(null)
    setPartialError(null)
    setNextCursor(null)
    getLibraryBrowsePage(null, listQuery)
      .then((page) => {
        if (cancelled || queryGenerationRef.current !== generation) return
        browseItemsRef.current = page.items
        setBrowseItems(page.items)
        setNextCursor(acceptLibraryCursor(seenCursorsRef.current, page.nextCursor))
        cursorQueryKeyRef.current = queryKey
        setTotal(page.total)
        setIsLoading(false)
        setIsFirstPagePending(false)
      })
      .catch((error: unknown) => {
        if (cancelled || queryGenerationRef.current !== generation) return
        if (!hadData) {
          browseItemsRef.current = []
          setBrowseItems([])
          setLoadError(toHavenError(error))
        } else {
          setPartialError(toHavenError(error))
        }
        setIsLoading(false)
        setIsFirstPagePending(false)
      })
    return () => {
      cancelled = true
    }
  }, [browseRequestKey, categoryKey, listQuery, queryKey, reloadToken])

  const reloadFirstPage = () => setReloadToken((value) => value + 1)

  // library-changed → 重新拉取（SLICE-SCAN-001：扫描终态后列表刷新，无需重启；
  // 仅 Tauri 环境）。
  useEffect(() => {
    if (!isTauriRuntime()) return
    let unlisten: (() => void) | null = null
    let disposed = false
    onLibraryChanged(() => {
      if (!disposed) reloadFirstPage()
    })
      .then((fn) => {
        if (disposed) fn()
        else unlisten = fn
      })
      .catch(() => {})
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

  // favorite-changed → 就地补丁收藏投影（SLICE-FAVORITE-001：跨入口一致；
  // 仅 Tauri 环境；投影随导航传给详情页）。
  useEffect(() => {
    if (!isTauriRuntime()) return
    let unlisten: (() => void) | null = null
    let disposed = false
    onFavoriteChanged((payload) => {
      setBrowseItems((prev) =>
        prev.map((item) =>
          item.id === payload.workId ? { ...item, favorite: payload.favorite } : item,
        ),
      )
    })
      .then((fn) => {
        if (disposed) fn()
        else unlisten = fn
      })
      .catch(() => {})
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])
  const [filters, setFilters] = useState<Record<string, string>>(() => {
    const result: Record<string, string> = {}
    for (const group of filterGroups) {
      const value = searchParams.get(group.key)
      if (value) result[group.key] = value
    }
    return result
  })
  const [query, setQuery] = useState(() => searchParams.get("q") || "")
  const [sortBy, setSortBy] = useState(() => searchParams.get("sort") || "最近添加")
  const [isFilterExpanded, setIsFilterExpanded] = useState(true)
  const previousCategoryRef = useRef(categoryKey)

  // 切换分类时重置筛选与排序（不同分类的筛选组不同）
  useEffect(() => {
    if (previousCategoryRef.current !== categoryKey) {
      previousCategoryRef.current = categoryKey
      setFilters({})
      setQuery("")
      setSortBy("最近添加")
      setSearchParams((prev) => {
        const next = new URLSearchParams(prev)
        for (const key of Array.from(next.keys())) {
          if (key !== "category") next.delete(key)
        }
        return next
      }, { replace: true })
    }
  }, [categoryKey, setSearchParams])

  const syncParams = (nextFilters: Record<string, string>, nextQuery: string, nextSort: string) => {
    setSearchParams((prev) => {
      const next = new URLSearchParams(prev)
      for (const group of filterGroups) {
        const value = nextFilters[group.key]
        if (value && value !== "全部") next.set(group.key, value)
        else next.delete(group.key)
      }
      if (nextQuery) next.set("q", nextQuery)
      else next.delete("q")
      if (nextSort !== "最近添加") next.set("sort", nextSort)
      else next.delete("sort")
      return next
    }, { replace: true })
  }

  const filteredItems = browseItems
    .filter((item) => (
      categoryKey === "video"
        ? item.type === "movie" || item.type === "tv"
        : item.type === categoryKey
    ))
    .filter((item) => {
      const normalizedQuery = query.trim().toLowerCase()
      const matchesQuery = !normalizedQuery
        || item.title.toLowerCase().includes(normalizedQuery)
        || item.originalTitle?.toLowerCase().includes(normalizedQuery)

      for (const group of filterGroups) {
        const value = filters[group.key]
        if (!value || value === "全部") continue

        if (group.key === "format" && categoryKey === "video") {
          const matchesFormat = (value === "电影" && item.type === "movie")
            || (["剧集", "动画"].includes(value) && item.type === "tv")
          if (!matchesFormat) return false
          continue
        }

        if (group.key === "year") {
          const beforeYear = parseBeforeYear(value)
          const matchesYear = beforeYear !== null ? item.year < beforeYear : String(item.year) === value
          if (!matchesYear) return false
          continue
        }

        if (!matchesMetaFilter(item, group.key, value)) return false
      }

      return matchesQuery
    })
    .sort((a, b) => {
      if (sortBy === "评分最高") return Number(b.rating ?? 0) - Number(a.rating ?? 0)
      if (sortBy === "年份最新") return b.year - a.year
      return (ITEM_META[b.id]?.order ?? 0) - (ITEM_META[a.id]?.order ?? 0)
    })

  const activeFilterCount = Object.values(filters).filter((value) => value && value !== "全部").length

  const sliceState = deriveLibrarySliceState({
    loading: isLoading,
    itemCount: filteredItems.length,
    error: loadError,
    partial: partialError !== null,
  })

  const loadMore = useCallback(async (allowRetryAfterError = false) => {
    if (!canLoadLibraryNextPage({
      nextCursor,
      isLoadingMore,
      isFirstPagePending,
      cursorQueryKey: cursorQueryKeyRef.current,
      activeQueryKey: queryKey,
      partialError: allowRetryAfterError ? false : partialError !== null,
    })) return
    const generation = queryGenerationRef.current
    const requestQueryKey = queryKey
    const requestCursor = nextCursor
    setIsLoadingMore(true)
    setPartialError(null)
    try {
      const page = await getLibraryBrowsePage(requestCursor, listQuery)
      if (
        queryGenerationRef.current !== generation
        || activeQueryKeyRef.current !== requestQueryKey
      ) return
      setBrowseItems((current) => {
        const merged = [...current, ...page.items]
        browseItemsRef.current = merged
        return merged
      })
      try {
        setNextCursor(acceptLibraryCursor(seenCursorsRef.current, page.nextCursor))
      } catch (error) {
        // Keep the valid page just received, but stop a malformed cursor chain.
        setNextCursor(null)
        throw error
      }
      setTotal(page.total)
    } catch (error) {
      if (
        queryGenerationRef.current === generation
        && activeQueryKeyRef.current === requestQueryKey
      ) setPartialError(toHavenError(error))
    } finally {
      if (
        queryGenerationRef.current === generation
        && activeQueryKeyRef.current === requestQueryKey
      ) setIsLoadingMore(false)
    }
  }, [isFirstPagePending, isLoadingMore, listQuery, nextCursor, partialError, queryKey])

  // Year/initial filters are projections over the typed page DTO. Consume the
  // cursor chain while such a filter is active so a match cannot be hidden on
  // a later page or make the empty state suppress the remaining cursor.
  useEffect(() => {
    if (
      activeFilterCount === 0
      || !nextCursor
      || isLoading
      || !canLoadLibraryNextPage({
        nextCursor,
        isLoadingMore,
        isFirstPagePending,
        cursorQueryKey: cursorQueryKeyRef.current,
        activeQueryKey: queryKey,
        partialError: partialError !== null,
      })
    ) return
    void loadMore()
  }, [activeFilterCount, isFirstPagePending, isLoading, isLoadingMore, loadMore, nextCursor, partialError, queryKey])

  const updateFilter = (key: string, value: string) => {
    setFilters((current) => {
      const next = { ...current, [key]: value }
      syncParams(next, query, sortBy)
      return next
    })
  }

  const clearFilters = () => {
    setFilters({})
    setQuery("")
    setSortBy("最近添加")
    syncParams({}, "", "最近添加")
  }

  // 默认展示针对该分类精选的前 4 组主要筛选条件
  // WorkCard exposes releaseYear and titles, but no file extension/genre/source
  // facts. Keep only filters whose predicates are backed by the production DTO.
  const productionSupported = new Set(["year", "letter"])
  const supportedFilterGroups = isTauriRuntime()
    ? filterGroups.filter((group) => productionSupported.has(group.key))
    : filterGroups
  const visibleFilterGroups = isFilterExpanded
    ? supportedFilterGroups
    : supportedFilterGroups.slice(0, 4)

  return (
    <div
      className="min-h-screen bg-background text-foreground selection:bg-primary/20 transition-colors"
      data-slice-state={sliceState.kind}
      aria-busy={sliceState.kind === "loading"}
    >
      <div className="mx-auto w-full max-w-[1600px] px-6 pt-[32px] sm:px-10 lg:px-14">
        {/* 顶部导航 */}
        <header className="flex items-center justify-between gap-[16px]">
          <button
            type="button"
            onClick={() => navigate("/library")}
            className="group flex items-center gap-[8px] text-sm font-semibold text-muted-foreground transition-colors hover:text-foreground"
          >
            <ArrowLeft size={16} className="transition-transform group-hover:-translate-x-1 shrink-0" />
            <span>返回媒体库</span>
          </button>
          <span className="hidden text-xs font-bold uppercase tracking-[0.2em] text-muted-foreground/60 sm:block">
            栖阅 / 媒体筛选
          </span>
        </header>

        {/* 标题 & 搜索区 */}
        <section className="mt-[32px] flex flex-col gap-3 pb-6">
          <p className="text-xs font-bold uppercase tracking-[0.25em] text-primary">{categoryConfig.eyebrow}</p>

          <div className="flex flex-col gap-[16px] sm:flex-row sm:items-center sm:justify-between">
            <h1 className="text-4xl font-extrabold tracking-tight sm:text-5xl">{categoryConfig.title}</h1>

            <div className="flex h-11 w-full sm:w-[320px] md:w-[380px] items-center gap-[8px] rounded-full border border-black/10 dark:border-white/15 bg-black/5 dark:bg-white/5 px-[16px] backdrop-blur-xl shadow-sm transition-all focus-within:ring-2 focus-within:ring-primary/40 focus-within:bg-background">
              <Search size={18} className="shrink-0 text-muted-foreground" />
              <input
                value={query}
                onChange={(event) => {
                  const nextQuery = event.target.value
                  setQuery(nextQuery)
                  syncParams(filters, nextQuery, sortBy)
                }}
                placeholder={categoryConfig.searchPlaceholder}
                aria-label={`搜索${categoryConfig.title}`}
                className="min-w-0 flex-1 bg-transparent text-sm text-foreground outline-none placeholder:text-muted-foreground/70"
              />
              {query && (
                <button
                  type="button"
                  onClick={() => setQuery("")}
                  aria-label="清除搜索"
                  className="text-muted-foreground hover:text-foreground transition-colors"
                >
                  <X size={16} />
                </button>
              )}
            </div>
          </div>

          <p className="mt-1 max-w-2xl text-sm leading-relaxed text-muted-foreground font-medium">{categoryConfig.description}</p>
        </section>

        {/* 悬浮毛玻璃筛选器卡片 */}
        <section 
          className="mt-[8px] rounded-[1.75rem] border border-black/[0.08] dark:border-white/10 bg-black/[0.025] dark:bg-white/[0.035] p-5 backdrop-blur-xl shadow-none transition-colors"
          aria-label={`${categoryConfig.title}筛选`}
        >
          <div className="flex items-center justify-between gap-3 pb-3 border-b border-black/5 dark:border-white/5">
            <div className="flex items-center gap-[10px]">
              <div className="p-[8px] rounded-xl bg-primary/10 text-primary">
                <SlidersHorizontal size={16} />
              </div>
              <h2 className="text-sm font-bold tracking-tight">按条件筛选</h2>
              {activeFilterCount > 0 && (
                <span className="rounded-full bg-primary/15 px-[10px] py-0.5 text-[11px] font-bold text-primary">
                  已选 {activeFilterCount} 项
                </span>
              )}
            </div>
            
            <div className="flex items-center gap-3">
              {(activeFilterCount > 0 || query) && (
                <button
                  type="button"
                  onClick={clearFilters}
                  className="flex items-center gap-1.5 text-xs font-semibold text-muted-foreground hover:text-destructive transition-colors"
                >
                  <RotateCcw size={14} />
                  重置筛选
                </button>
              )}
              
              <button
                type="button"
                onClick={() => setIsFilterExpanded(!isFilterExpanded)}
                className="flex items-center gap-1 text-xs font-semibold text-muted-foreground hover:text-foreground transition-colors px-3 py-1.5 rounded-full bg-black/5 dark:bg-white/5"
              >
                <span>{isFilterExpanded ? "收起筛选" : "展开全部筛选"}</span>
                {isFilterExpanded ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
              </button>
            </div>
          </div>

          <div className="flex flex-col divide-y divide-black/5 dark:divide-white/5 pt-[8px]">
            {visibleFilterGroups.map((group) => (
              <FilterRow
                key={group.key}
                label={group.label}
                options={group.options}
                value={filters[group.key]}
                onChange={(value) => updateFilter(group.key, value)}
              />
            ))}
          </div>
        </section>

        {/* 结果列表与排序选项 */}
        <section className="mt-[48px]">
          <div className="flex flex-col gap-[16px] border-b border-black/5 dark:border-white/10 pb-[16px] sm:flex-row sm:items-center sm:justify-between">
            <div>
              <h2 className="text-xl sm:text-2xl font-bold tracking-tight">{categoryConfig.title.replace("库", "")}结果</h2>
              <p className="mt-1 text-xs font-medium text-muted-foreground">找到 {filteredItems.length} 项符合条件的作品</p>
            </div>

            <div className="flex items-center gap-1.5 bg-black/5 dark:bg-white/5 p-1 rounded-full border border-black/5 dark:border-white/5 self-start sm:self-auto">
              {["最近添加", "年份最新", "评分最高"].map((option) => (
                <button
                  key={option}
                  type="button"
                  onClick={() => {
                    setSortBy(option)
                    syncParams(filters, query, option)
                  }}
                  className={cn(
                    "rounded-full px-3.5 py-1.5 text-xs font-bold transition-all",
                    sortBy === option
                      ? "bg-background text-foreground shadow-sm scale-105"
                      : "text-muted-foreground hover:text-foreground hover:bg-black/5 dark:hover:bg-white/10"
                  )}
                >
                  {option}
                </button>
              ))}
            </div>
          </div>

          {/* 网格展示选中的海报卡片 (Poster Grid) */}
          {sliceState.kind === "loading" ? (
            <p className="mt-[48px] text-center text-sm font-medium text-muted-foreground">
              正在加载{categoryConfig.title}…
            </p>
          ) : sliceState.kind === "retryable_error" || sliceState.kind === "terminal_error" ? (
            <div className="mt-[48px] flex min-h-[300px] flex-col items-center justify-center rounded-[2rem] border border-dashed border-border bg-black/5 dark:bg-white/5 text-center p-[32px]">
              <p className="text-base font-bold">{categoryConfig.title}加载失败</p>
              <p className="mt-[8px] text-xs text-muted-foreground max-w-sm">{sliceState.message}</p>
              {sliceState.canRetry && (
                <button
                  type="button"
                  onClick={reloadFirstPage}
                  className="mt-6 rounded-full bg-foreground px-5 py-[10px] text-xs font-bold text-background"
                >
                  重试加载
                </button>
              )}
            </div>
          ) : sliceState.kind === "empty" ? (
            <div className="mt-[48px] flex min-h-[300px] flex-col items-center justify-center rounded-[2rem] border border-dashed border-border bg-black/5 dark:bg-white/5 text-center p-[32px]">
              <p className="text-base font-bold">没有找到匹配的{categoryConfig.title.replace("库", "")}内容</p>
              <p className="mt-[8px] text-xs text-muted-foreground max-w-sm">试试清除筛选条件，或者切换其他分类进行浏览。</p>
              <button
                type="button"
                onClick={clearFilters}
                className="mt-6 rounded-full bg-foreground px-5 py-[10px] text-xs font-bold text-background transition-transform hover:scale-105 active:scale-95"
              >
                重置所有筛选
              </button>
              {nextCursor && (
                <button
                  type="button"
                  onClick={() => void loadMore(true)}
                  disabled={isLoadingMore}
                  className="mt-3 rounded-full border border-border px-5 py-[10px] text-xs font-bold text-foreground disabled:opacity-50"
                >
                  {isLoadingMore ? "正在查找…" : "继续查找后续内容"}
                </button>
              )}
            </div>
          ) : (
            <>
              {partialError && (
                <div className="mt-6 flex items-center justify-between gap-4 rounded-xl border border-amber-500/30 bg-amber-500/10 px-4 py-3 text-xs">
                  <span>{partialError.message || "部分内容加载失败"}</span>
                  <button type="button" onClick={nextCursor ? () => void loadMore(true) : reloadFirstPage} className="shrink-0 font-bold text-primary">
                    {nextCursor ? "重试继续" : "重新加载"}
                    </button>
                </div>
              )}
              <div className="mt-[32px] grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6 gap-5 md:gap-7">
                {filteredItems.map((item) => (
                  <MediaItem key={item.id} item={item} />
                ))}
              </div>
              {nextCursor && (
                <div className="mt-8 flex flex-col items-center gap-2">
                  <p className="text-xs text-muted-foreground">
                    已加载 {browseItems.length} 项{total !== null ? ` / 媒体库共 ${total} 项` : ""}
                  </p>
                  <button
                    type="button"
                    onClick={() => void loadMore(true)}
                    disabled={isLoadingMore}
                    className="rounded-full border border-border px-5 py-[10px] text-xs font-bold text-foreground disabled:opacity-50"
                  >
                    {isLoadingMore ? "正在加载…" : "加载更多"}
                  </button>
                </div>
              )}
            </>
          )}
        </section>
      </div>
    </div>
  )
}

function FilterRow({
  label,
  options,
  value,
  onChange,
}: {
  label: string
  options: string[]
  value: string
  onChange: (value: string) => void
}) {
  return (
    <div className="flex flex-col gap-[10px] py-3 sm:flex-row sm:items-center">
      <span className="w-28 shrink-0 whitespace-nowrap pr-[16px] text-xs font-bold text-muted-foreground/80">{label}</span>
      <div className="flex flex-1 flex-wrap gap-1.5">
        {options.map((option) => {
          const isSelected = option === value
          return (
            <button
              key={option}
              type="button"
              onClick={() => onChange(option)}
              className={cn(
                "whitespace-nowrap rounded-full px-3.5 py-1 text-xs font-semibold transition-all duration-200 cursor-pointer select-none",
                isSelected
                  ? "bg-foreground text-background shadow-md font-bold scale-105"
                  : "text-muted-foreground hover:text-foreground hover:bg-black/5 dark:hover:bg-white/10"
              )}
            >
              {option}
            </button>
          )
        })}
      </div>
    </div>
  )
}
