import { useEffect, useState, useRef } from "react"
import { useNavigate, useSearchParams } from "react-router"
import { ChevronRight, Star, Check, RefreshCw, Search } from "lucide-react"
import { SearchBar } from "../components/SearchBar"
import { SearchHistory } from "../components/SearchHistory"
import { TrendingBoard } from "../components/TrendingBoard"
import { getHavenClientMode } from "@/lib/ipc/runtime"
import { HavenError } from "@/lib/ipc/errors"
import { cn } from "@/lib/utils"
import { ArtworkImage } from "@/components/ui/haven/ArtworkImage"
import { defaultCoverCategoryForMediaType, type DefaultCoverCategory } from "@/lib/default-cover"
import { resolveSearchRuntimeState } from "../lib/search-runtime-state"
import { SourceCandidates } from "../components/SourceCandidates"
import {
  searchLocalLibrary,
  type LocalSearchResult,
  type SearchCategory,
} from "../ipc/search-gateway"
import { useTrendingBoards } from "../hooks/use-trending-boards"
import {
  clearSearchHistory,
  getSearchHistorySetting,
  listSearchHistory,
  recordSearchHistory,
  removeSearchHistory,
} from "../ipc/search-history-gateway"

type ResultFilter = SearchCategory
type SortMode = "relevance" | "year" | "rating"

const FILTER_OPTIONS: Array<{ id: ResultFilter; label: string }> = [
  { id: "all", label: "全部" },
  { id: "video", label: "影视" },
  { id: "book", label: "图书" },
  { id: "comic", label: "漫画" },
  { id: "periodical", label: "报刊资料" },
]

type LocalSearchStatus = "idle" | "loading" | "success" | "error"

// ==========================================
// COMPONENT
// ==========================================

function useDebouncedValue<T>(value: T, delay: number): T {
  const [debounced, setDebounced] = useState(value)
  useEffect(() => {
    const id = window.setTimeout(() => setDebounced(value), delay)
    return () => window.clearTimeout(id)
  }, [value, delay])
  return debounced
}

export function SearchPage() {
  const navigate = useNavigate()
  const [searchParams, setSearchParams] = useSearchParams()
  const clientMode = getHavenClientMode()
  const queryParam = searchParams.get("q") ?? ""
  const categoryParam = searchParams.get("category")
  const sortParam = searchParams.get("sort")
  const submittedQuery = queryParam.trim()
  const [searchValue, setSearchValue] = useState(queryParam)
  const debouncedSearchValue = useDebouncedValue(searchValue, 300)
  const searchRuntimeState = resolveSearchRuntimeState(clientMode, submittedQuery)
  const localMode = searchRuntimeState === "ready_empty" || searchRuntimeState === "ready_query"
  const [history, setHistory] = useState<string[]>([])
  const [historyEnabled, setHistoryEnabled] = useState(true)
  const [historyLoaded, setHistoryLoaded] = useState(false)
  const [filterType, setFilterType] = useState<ResultFilter>(() => parseFilter(categoryParam))
  const [sortMode, setSortMode] = useState<SortMode>(() => parseSort(sortParam))
  const [localStatus, setLocalStatus] = useState<LocalSearchStatus>("idle")
  const [localResults, setLocalResults] = useState<LocalSearchResult[]>([])
  const [localErrorMessage, setLocalErrorMessage] = useState<string | null>(null)
  const [retryRevision, setRetryRevision] = useState(0)

  useEffect(() => {
    if (clientMode === "unavailable") {
      setHistory([])
      setHistoryLoaded(true)
      return
    }
    let active = true
    setHistoryLoaded(false)
    void Promise.all([listSearchHistory(), getSearchHistorySetting()]).then(([terms, enabled]) => {
      if (!active) return
      setHistory(terms)
      setHistoryEnabled(enabled)
      setHistoryLoaded(true)
    }).catch(() => {
      if (!active) return
      setHistory([])
      setHistoryLoaded(true)
    })
    return () => {
      active = false
    }
  }, [clientMode])

  useEffect(() => {
    setSearchValue(queryParam)
  }, [queryParam])

  useEffect(() => {
    setFilterType(parseFilter(categoryParam))
  }, [categoryParam])

  useEffect(() => {
    setSortMode(parseSort(sortParam))
  }, [sortParam])

  useEffect(() => {
    if (!localMode || !submittedQuery) {
      setLocalStatus("idle")
      setLocalResults([])
      setLocalErrorMessage(null)
      return
    }

    const controller = new AbortController()
    setLocalStatus("loading")
    searchLocalLibrary(submittedQuery, filterType, controller.signal)
      .then((items) => {
        if (controller.signal.aborted) return
        setLocalResults(items)
        setLocalErrorMessage(null)
        setLocalStatus("success")
      })
      .catch((error: unknown) => {
        if (controller.signal.aborted) return
        setLocalResults([])
        setLocalErrorMessage(error instanceof HavenError ? error.dto.userMessage : "本地媒体库暂时不可用，请稍后重试。")
        setLocalStatus("error")
      })

    return () => controller.abort()
  }, [filterType, localMode, retryRevision, submittedQuery])

  // 防抖：输入 300ms 后自动提交（AbortController 已在 localMode effect 中处理取消）
  const debouncedCommittedRef = useRef<string>(submittedQuery)
  useEffect(() => {
    const trimmed = debouncedSearchValue.trim()
    if (trimmed && trimmed !== debouncedCommittedRef.current && trimmed !== submittedQuery) {
      debouncedCommittedRef.current = trimmed
      commitSearch(trimmed)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [debouncedSearchValue])

  useEffect(() => {
    debouncedCommittedRef.current = submittedQuery
  }, [submittedQuery])

  const commitSearch = (value: string) => {
    if (clientMode === "unavailable") return
    const trimmed = value.trim()
    if (!trimmed) return
    if (trimmed.length > 200) return
    // 设置读取完成前不产生隐式写入，避免用户在冷启动窗口中绕过已关闭的搜索历史开关。
    if (historyLoaded && historyEnabled) {
      void recordSearchHistory(trimmed).then(setHistory).catch(() => undefined)
    }
    setSearchValue(trimmed)
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev)
        next.set("q", trimmed)
        return next
      },
      { replace: true }
    )
  }

  const handleClearHistory = () => {
    void clearSearchHistory().then(setHistory).catch(() => undefined)
  }

  const handleRemoveItem = (itemToRemove: string) => {
    void removeSearchHistory(itemToRemove).then(setHistory).catch(() => undefined)
  }

  const handleItemClick = (item: string) => {
    setSearchValue(item)
    commitSearch(item)
  }

  const openBoardItem = (title: string) => {
    setSearchValue(title)
    commitSearch(title)
  }

  const cycleSortMode = () => {
    const next = sortMode === "relevance" ? "year" : sortMode === "year" ? "rating" : "relevance"
    updateSearchParam(searchParams, setSearchParams, "sort", next, "relevance")
  }

  const changeFilterType = (next: ResultFilter) => {
    updateSearchParam(searchParams, setSearchParams, "category", next, "all")
  }

  const clearSearch = () => {
    setSearchValue("")
    const next = new URLSearchParams(searchParams)
    next.delete("q")
    setSearchParams(next, { replace: true })
  }

  const suggestItems = searchValue.trim()
    ? history.filter((h) => h.toLowerCase().includes(searchValue.trim().toLowerCase())).slice(0, 5)
    : []
  const [showSuggest, setShowSuggest] = useState(false)

  return (
    <div className="w-full flex flex-col min-h-full bg-background/50 pt-[32px] pb-[120px] px-[24px] md:px-[48px] lg:px-[64px] gap-10 max-w-[1600px] mx-auto">
      {/* 巨型搜索框 + 联想下拉 */}
      <div className="w-full max-w-5xl mx-auto relative">
        <SearchBar
          value={searchValue}
          onChange={(e) => setSearchValue(e.target.value)}
          onSearch={() => commitSearch(searchValue)}
          onClear={clearSearch}
          onFocus={() => setShowSuggest(true)}
          onBlur={() => setTimeout(() => setShowSuggest(false), 150)}
          disabled={clientMode === "unavailable"}
          placeholder={clientMode === "unavailable" ? "搜索服务未启用" : "搜索本地媒体库"}
        />
        {showSuggest && suggestItems.length > 0 && (
          <div className="absolute left-0 right-0 top-full z-20 mt-2 rounded-2xl border border-black/5 bg-white p-1.5 shadow-xl dark:border-white/10 dark:bg-zinc-900">
            {suggestItems.map((item) => (
              <button
                key={item}
                type="button"
                onMouseDown={(e) => {
                  e.preventDefault()
                  setSearchValue(item)
                  commitSearch(item)
                  setShowSuggest(false)
                }}
                className="flex w-full items-center gap-2 rounded-xl px-3 py-2 text-left text-sm hover:bg-black/5 dark:hover:bg-white/10"
              >
                <Search className="h-3.5 w-3.5 text-muted-foreground" />
                <span>{item}</span>
              </button>
            ))}
          </div>
        )}
      </div>

      {(searchRuntimeState === "unavailable_empty" || searchRuntimeState === "unavailable_query") && (
        <SearchUnavailable state={searchRuntimeState} />
      )}

      {/* 历史搜索：空输入时展示，位于热门排行之前 */}
      {searchValue.trim() === "" && history.length > 0 && (
        <div className="w-full max-w-[1600px] mx-auto">
          <SearchHistory 
            history={history} 
            onClear={handleClearHistory}
            onRemoveItem={handleRemoveItem}
            onItemClick={handleItemClick}
          />
        </div>
      )}
      {searchRuntimeState === "ready_empty" && historyLoaded && history.length === 0 && (
        <LocalSearchEmpty />
      )}
      {searchValue.trim() === "" && (
        <TrendingBoardsFromBackend onOpenItem={openBoardItem} />
      )}

      {searchRuntimeState === "ready_query" && (
        <LocalSearchResults
          query={submittedQuery}
          items={localResults}
          status={localStatus}
          errorMessage={localErrorMessage}
          filterType={filterType}
          sortMode={sortMode}
          onFilterTypeChange={changeFilterType}
          onCycleSort={cycleSortMode}
          onOpen={(id) => navigate(`/work/${id}`)}
          onRetry={() => setRetryRevision((current) => current + 1)}
        />
      )}

      {searchRuntimeState === "ready_query" && submittedQuery && (
        <SourceCandidates query={submittedQuery} category={filterType} />
      )}
    </div>
  )
}

function TrendingBoardsFromBackend({ onOpenItem }: { onOpenItem: (title: string) => void }) {
  const { boards, status, error, retry, retryAvailableAt } = useTrendingBoards(true)
  const loading = status === "loading" && !boards?.boards.length
  const refreshing = status === "refreshing"

  if (loading) {
    return (
      <section className="w-full flex flex-col gap-6 max-w-[1600px] mx-auto">
        <h2 className="text-xl font-bold tracking-tight">热门排行</h2>
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-6">
          {Array.from({ length: 4 }).map((_, i) => (
            <div key={i} className="flex flex-col bg-white dark:bg-zinc-900 rounded-3xl p-6 shadow-sm border border-black/5 dark:border-white/5 animate-pulse">
              <div className="h-6 w-24 bg-muted rounded mb-6" />
              <div className="flex flex-col gap-[16px]">
                <div className="h-20 bg-muted rounded" />
                <div className="h-20 bg-muted rounded" />
              </div>
            </div>
          ))}
        </div>
      </section>
    )
  }

  if (!boards || boards.boards.length === 0) {
    return (
      <section className="w-full flex flex-col gap-6 max-w-[1600px] mx-auto">
        <h2 className="text-xl font-bold tracking-tight">热门排行</h2>
        <p className="text-xs text-muted-foreground">{error?.dto.userMessage ?? "暂无热榜数据"}</p>
        <button
          type="button"
          onClick={retry}
          disabled={Boolean(retryAvailableAt && retryAvailableAt > Date.now())}
          className="self-start rounded-full border border-border/60 px-3 py-1.5 text-xs font-semibold text-muted-foreground disabled:cursor-not-allowed disabled:opacity-50"
        >
          {retryAvailableAt && retryAvailableAt > Date.now() ? "稍后重试" : "重试"}
        </button>
      </section>
    )
  }

  return (
    <section className="w-full flex flex-col gap-6 max-w-[1600px] mx-auto">
      <div className="flex items-center gap-2">
        <h2 className="text-xl font-bold tracking-tight">热门排行</h2>
        {refreshing && <span className="text-xs font-normal text-muted-foreground">正在更新</span>}
      </div>
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-6">
        {boards.boards.map((board) => (
          <TrendingBoard
            key={board.boardId}
            title={board.title}
            subtitle={board.subtitle}
            items={board.items.map((item, index) => ({
              title: item.title,
              subtitle: item.subtitle,
              description: item.description,
              imageUrl: item.posterUri,
              statusBadge: item.statusBadge ?? undefined,
              fallbackCategory: trendingCoverCategory(board.boardId),
              fallbackSeed: `${board.boardId}:${index}`,
              onClick: () => onOpenItem(item.title),
            }))}
          />
        ))}
      </div>
    </section>
  )
}

function SearchUnavailable({ state }: { state: "unavailable_empty" | "unavailable_query" }) {
  return (
    <section className="mx-auto flex min-h-[256px] w-full max-w-[1200px] flex-col items-center justify-center text-center">
      <p className="text-sm font-semibold text-foreground">搜索服务未启用</p>
      <p className="mt-[8px] text-xs text-muted-foreground">
        {state === "unavailable_query" ? "当前运行环境不能返回搜索结果。" : "当前运行环境没有可用的搜索数据。"}
      </p>
    </section>
  )
}

function LocalSearchEmpty() {
  return (
    <section className="mx-auto flex min-h-[256px] w-full max-w-[1200px] flex-col items-center justify-center text-center">
      <Search className="h-8 w-8 text-muted-foreground" aria-hidden="true" />
      <p className="mt-[16px] text-sm font-semibold text-foreground">搜索本地媒体库</p>
      <p className="mt-[8px] text-xs text-muted-foreground">输入作品标题或原名，按 Enter 查看结果。</p>
    </section>
  )
}

function LocalSearchResults({
  query,
  items,
  status,
  errorMessage,
  filterType,
  sortMode,
  onFilterTypeChange,
  onCycleSort,
  onOpen,
  onRetry,
}: {
  query: string
  items: LocalSearchResult[]
  status: LocalSearchStatus
  errorMessage: string | null
  filterType: ResultFilter
  sortMode: SortMode
  onFilterTypeChange: (filter: ResultFilter) => void
  onCycleSort: () => void
  onOpen: (id: string) => void
  onRetry: () => void
}) {
  const sortedItems = [...items]
  if (sortMode === "relevance") {
    const needle = query.trim().toLocaleLowerCase()
    sortedItems.sort((a, b) => relevanceScore(b, needle) - relevanceScore(a, needle))
  } else if (sortMode === "year") {
    sortedItems.sort((a, b) => (b.year ?? 0) - (a.year ?? 0))
  } else if (sortMode === "rating") {
    sortedItems.sort((a, b) => (b.rating ?? 0) - (a.rating ?? 0))
  }

  return (
    <section className="mx-auto w-full max-w-[1200px]">
      <SearchResultHeader
        query={query}
        count={status === "success" ? sortedItems.length : null}
        filterType={filterType}
        sortMode={sortMode}
        onFilterTypeChange={onFilterTypeChange}
        onCycleSort={onCycleSort}
      />
      {status === "loading" && <SearchResultsSkeleton />}
      {status === "error" && (
        <div className="flex min-h-[256px] flex-col items-center justify-center text-center">
          <p className="text-sm font-semibold text-foreground">无法加载搜索结果</p>
          <p className="mt-[8px] text-xs text-muted-foreground">{errorMessage ?? "本地媒体库暂时不可用，请稍后重试。"}</p>
          <button
            type="button"
            onClick={onRetry}
            className="mt-[16px] inline-flex items-center gap-[8px] rounded-md bg-primary px-[14px] py-[8px] text-xs font-semibold text-primary-foreground"
          >
            <RefreshCw className="h-3.5 w-3.5" aria-hidden="true" />
            重试
          </button>
        </div>
      )}
      {status === "success" && sortedItems.length === 0 && (
        <div className="flex min-h-[256px] flex-col items-center justify-center text-center text-muted-foreground">
          <p className="text-sm font-semibold text-foreground">没有找到相关作品</p>
          <p className="mt-[8px] text-xs">可以尝试作品原名、更短的关键词或其他分类。</p>
        </div>
      )}
      {status === "success" && sortedItems.length > 0 && (
        <div className="mt-5 grid gap-3 md:grid-cols-2">
          {sortedItems.map((item) => (
            <LocalSearchResultCard key={item.id} item={item} onOpen={onOpen} />
          ))}
        </div>
      )}
    </section>
  )
}

function parseFilter(value: string | null): ResultFilter {
  return FILTER_OPTIONS.some((option) => option.id === value) ? value as ResultFilter : "all"
}

function parseSort(value: string | null): SortMode {
  return value === "year" || value === "rating" ? value : "relevance"
}

function updateSearchParam(
  current: URLSearchParams,
  setSearchParams: ReturnType<typeof useSearchParams>[1],
  key: string,
  value: string,
  defaultValue: string,
) {
  const next = new URLSearchParams(current)
  if (value === defaultValue) next.delete(key)
  else next.set(key, value)
  setSearchParams(next, { replace: true })
}

function relevanceScore(item: LocalSearchResult, query: string): number {
  const title = item.title.toLocaleLowerCase()
  const originalTitle = item.originalTitle?.toLocaleLowerCase() ?? ""
  if (title === query || originalTitle === query) return 4
  if (title.startsWith(query) || originalTitle.startsWith(query)) return 3
  if (title.includes(query) || originalTitle.includes(query)) return 2
  return item.description?.toLocaleLowerCase().includes(query) ? 1 : 0
}

function SearchResultHeader({
  query,
  count,
  filterType,
  sortMode,
  onFilterTypeChange,
  onCycleSort,
}: {
  query: string
  count: number | null
  filterType: ResultFilter
  sortMode: SortMode
  onFilterTypeChange: (filter: ResultFilter) => void
  onCycleSort: () => void
}) {
  const sortLabel = sortMode === "year" ? "年份最新" : sortMode === "rating" ? "评分最高" : "相关度"

  return (
    <>
      <div className="flex flex-col gap-[16px] border-b border-border/60 pb-5 sm:flex-row sm:items-end sm:justify-between">
        <div>
          <p className="text-xs font-semibold uppercase tracking-[0.18em] text-muted-foreground">Search Results</p>
          <h2 className="mt-1 text-2xl font-bold">“{query}”</h2>
          <p className="mt-1 text-xs text-muted-foreground">{count === null ? "正在搜索..." : `找到 ${count} 个作品结果`}</p>
        </div>
        <div className="flex items-center gap-[8px]">
          <button
            type="button"
            onClick={onCycleSort}
            className={cn(
              "flex items-center gap-1.5 rounded-full px-3 py-[8px] text-xs font-semibold transition-colors",
              sortMode !== "relevance" ? "bg-primary/10 text-primary" : "bg-muted/60 text-muted-foreground hover:text-foreground",
            )}
          >
            {sortLabel}
            {sortMode !== "relevance" && <Check className="h-3 w-3" aria-hidden="true" />}
          </button>
        </div>
      </div>
      <div className="flex flex-wrap gap-[8px] pt-[16px]" role="tablist" aria-label="搜索结果分类">
        {FILTER_OPTIONS.map((option) => (
          <button
            key={option.id}
            type="button"
            role="tab"
            aria-selected={filterType === option.id}
            onClick={() => onFilterTypeChange(option.id)}
            className={cn(
              "rounded-full px-3.5 py-1.5 text-xs font-semibold transition-colors",
              filterType === option.id ? "bg-foreground text-background" : "bg-muted/60 text-muted-foreground hover:text-foreground",
            )}
          >
            {option.label}
          </button>
        ))}
      </div>
    </>
  )
}

function SearchResultsSkeleton() {
  return (
    <div className="mt-5 grid gap-3 md:grid-cols-2" aria-label="正在加载搜索结果">
      {Array.from({ length: 4 }).map((_, index) => (
        <div key={index} className="flex gap-[16px] rounded-2xl border border-border/60 p-3">
          <div className="h-[112px] w-[80px] shrink-0 animate-pulse rounded-xl bg-muted" />
          <div className="flex min-w-0 flex-1 flex-col gap-3 py-2">
            <div className="h-4 w-3/4 animate-pulse rounded bg-muted" />
            <div className="h-3 w-1/2 animate-pulse rounded bg-muted/80" />
            <div className="h-3 w-full animate-pulse rounded bg-muted/60" />
          </div>
        </div>
      ))}
    </div>
  )
}

function LocalSearchResultCard({ item, onOpen }: { item: LocalSearchResult; onOpen: (id: string) => void }) {
  const categoryLabel = item.category === "video"
    ? "影视"
    : item.category === "book"
      ? "图书"
      : item.category === "comic"
        ? "漫画"
        : "报刊资料"

  return (
    <div
      role="button"
      tabIndex={0}
      onClick={() => onOpen(item.id)}
      onKeyDown={(event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault()
          onOpen(item.id)
        }
      }}
      className="group flex cursor-pointer gap-[16px] rounded-2xl border border-border/60 bg-background/70 p-3 text-left outline-none transition-all hover:-translate-y-0.5 hover:bg-background hover:shadow-md focus-visible:ring-2 focus-visible:ring-primary"
    >
      <div className="h-[112px] w-[80px] shrink-0 overflow-hidden rounded-xl bg-muted">
        <ArtworkImage
          src={item.imageUrl}
          alt={item.title}
          allowExternal={getHavenClientMode() !== "tauri"}
          fallbackCategory={defaultCoverCategoryForMediaType(item.category)}
          fallbackSeed={item.id}
          className="h-full w-full object-cover"
          loading="lazy"
        />
      </div>
      <div className="min-w-0 flex-1 py-1">
        <div className="flex items-start justify-between gap-[8px]">
          <h3 className="line-clamp-2 text-sm font-bold leading-snug text-foreground">{item.title}</h3>
          <ChevronRight className="h-[16px] w-[16px] shrink-0 text-muted-foreground transition-transform group-hover:translate-x-1" aria-hidden="true" />
        </div>
        {item.originalTitle && <span className="mt-1 block truncate text-xs text-muted-foreground">{item.originalTitle}</span>}
        {item.description && <p className="mt-[10px] line-clamp-2 text-xs leading-relaxed text-muted-foreground">{item.description}</p>}
        <div className="mt-3 flex items-center gap-3 text-xs font-semibold text-muted-foreground">
          {item.year ? <span>{item.year}</span> : null}
          <span>{categoryLabel}</span>
          {item.rating !== undefined ? <span className="flex items-center gap-1 text-amber-500"><Star className="h-3.5 w-3.5 fill-current" aria-hidden="true" />{item.rating}</span> : null}
        </div>
      </div>
    </div>
  )
}

function trendingCoverCategory(boardId: string): DefaultCoverCategory {
  const normalized = boardId.toLocaleLowerCase()
  if (normalized.includes("comic") || normalized.includes("manga")) return "comic"
  if (normalized.includes("book") || normalized.includes("novel")) return "book"
  if (normalized.includes("article") || normalized.includes("periodical") || normalized.includes("magazine")) return "article"
  return "video"
}
