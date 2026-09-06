import { useState, useMemo, useEffect, useRef, useCallback } from "react"
import { useNavigate, useSearchParams } from "react-router"
import { LibraryTVExpandedSidebar } from "../components/LibraryTVExpandedSidebar"
import { LibraryTVHeroInfo } from "../components/LibraryTVHeroInfo"
import { LibraryTVRowShelf } from "../components/LibraryTVRowShelf"
import { LibraryGrid } from "../components/LibraryGrid"
import type { LibraryMediaItemData } from "../components/MediaItem"
import { getLibraryBrowseItems } from "../ipc/gateway"
import { isTauriRuntime } from "@/lib/ipc/runtime"
import { onFavoriteChanged, onLibraryChanged } from "@/lib/ipc/events"
import { toHavenError, type HavenError } from "@/lib/ipc/errors"
import { deriveLibrarySliceState } from "@/lib/slice-state"

export function LibraryPage() {
  const [searchParams, setSearchParams] = useSearchParams()
  const navigate = useNavigate()
  const requestedCategory = searchParams.get("category") || "all"
  const activeCategory = requestedCategory === "movie"
    ? "video"
    : requestedCategory

  // 数据层接线（IPC-MOCK-001）：落地页经 gateway 消费 library_list，
  // Hero 焦点与陈列栏在数据到达后填充（视觉结构不变）。
  const [libraryItems, setLibraryItems] = useState<LibraryMediaItemData[]>([])
  const [isLoading, setIsLoading] = useState(true)
  const [loadError, setLoadError] = useState<HavenError | null>(null)
  const loadRequestRef = useRef(0)
  // 10-foot TV UI 核心焦点：当前选中的媒体原信息展示（数据到达前为 null，隐藏 Hero 与背景图）
  const [focusedItem, setFocusedItem] = useState<LibraryMediaItemData | null>(null)

  const loadLibrary = useCallback(async () => {
    const requestId = ++loadRequestRef.current
    setIsLoading(true)
    setLoadError(null)
    try {
      const items = await getLibraryBrowseItems()
      if (loadRequestRef.current === requestId) setLibraryItems(items)
    } catch (error) {
      if (loadRequestRef.current === requestId) setLoadError(toHavenError(error))
    } finally {
      if (loadRequestRef.current === requestId) setIsLoading(false)
    }
  }, [])

  useEffect(() => {
    void loadLibrary()
    return () => {
      loadRequestRef.current += 1
    }
  }, [loadLibrary])

  // library-changed → 重新拉取（SLICE-SCAN-001：扫描终态后库内容刷新，无需重启；
  // 仅 Tauri 环境；浏览器演示目录不经此路径）。
  useEffect(() => {
    if (!isTauriRuntime()) return
    let unlisten: (() => void) | null = null
    let disposed = false
    onLibraryChanged(() => {
      if (!disposed) void loadLibrary()
    })
      .then((fn) => {
        if (disposed) fn()
        else unlisten = fn
      })
      .catch(() => {
        // 事件订阅失败不阻塞页面
      })
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [loadLibrary])

  // favorite-changed → 就地补丁收藏投影（SLICE-FAVORITE-001：跨入口一致；
  // 仅 Tauri 环境）。
  useEffect(() => {
    if (!isTauriRuntime()) return
    let unlisten: (() => void) | null = null
    let disposed = false
    onFavoriteChanged((payload) => {
      setLibraryItems((prev) =>
        prev.map((item) =>
          item.id === payload.workId ? { ...item, favorite: payload.favorite } : item,
        ),
      )
    })
      .then((fn) => {
        if (disposed) fn()
        else unlisten = fn
      })
      .catch(() => {
        // 事件订阅失败不阻塞页面
      })
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

  const getCategoryItems = (cat: string) => filterByCategory(libraryItems, cat)

  const handleSelectCategory = (categoryId: string) => {
    setSearchParams(
      (prev) => {
        prev.set("category", categoryId)
        return prev
      },
      { replace: true }
    )
    const categoryItems = getCategoryItems(categoryId)
    if (categoryItems.length > 0) {
      setFocusedItem(categoryItems[0])
    }
  }

  // 背景图预载：焦点切换时背景已是缓存命中，消除网络等待造成的闪烁/卡顿
  useEffect(() => {
    if (libraryItems.length === 0) return
    for (const item of libraryItems) {
      const src = item.backdropUrl || item.imageUrl
      if (!src) continue
      const img = new Image()
      img.decoding = "async"
      img.src = src
    }
  }, [libraryItems])

  // 悬停意图防抖：快速划过货架不触发焦点切换（每次切换 = 整页重渲染 +
  // 背景 <img key> 卸载重挂 + 大图解码，是悬停卡顿主因）
  const hoverTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const handleHoverSpotlight = useCallback((item: LibraryMediaItemData) => {
    if (hoverTimerRef.current) clearTimeout(hoverTimerRef.current)
    hoverTimerRef.current = setTimeout(() => setFocusedItem(item), 220)
  }, [])
  useEffect(() => () => {
    if (hoverTimerRef.current) clearTimeout(hoverTimerRef.current)
  }, [])

  // 直接带分类参数进入页面或数据到达时，同步 Hero 焦点（首项优先）
  useEffect(() => {
    const categoryItems = filterByCategory(libraryItems, activeCategory)
    if (categoryItems.length > 0) {
      setFocusedItem(categoryItems[0])
    }
  }, [activeCategory, libraryItems])

  // 根据分类预筛选不同的陈列栏数据 (Shelves)
  const movies = useMemo(() => libraryItems.filter((i) => i.type === "movie" || i.type === "tv"), [libraryItems])
  const books = useMemo(() => libraryItems.filter((i) => i.type === "book"), [libraryItems])
  const comics = useMemo(() => libraryItems.filter((i) => i.type === "comic"), [libraryItems])
  const periodicals = useMemo(() => libraryItems.filter((i) => i.type === "periodical"), [libraryItems])
  const documents = useMemo(() => libraryItems.filter((i) => i.type === "document"), [libraryItems])
  const sliceState = deriveLibrarySliceState({
    loading: isLoading,
    itemCount: libraryItems.length,
    error: loadError,
  })

  return (
    <div
      className="w-full h-full min-h-0 bg-black text-foreground flex select-none relative"
      data-slice-state={sliceState.kind}
      aria-busy={sliceState.kind === "loading"}
    >
      {/*
        ====================================================
        GLOBAL BACKGROUND (Netflix TV Style)
        ====================================================
      */}
      <div className="fixed inset-0 z-0">
        {focusedItem && (
          <img
            key={focusedItem.id} // trigger re-render for smooth fade if needed
            src={focusedItem.backdropUrl || focusedItem.imageUrl}
            alt={focusedItem.title}
            className="w-full h-full object-cover object-top filter brightness-[0.85] transition-opacity duration-700 animate-in fade-in"
          />
        )}
        {/* 深色渐变遮罩：左侧暗化(侧边栏+文字区域)，底部暗化(陈列栏区域) */}
        <div className="absolute inset-0 bg-gradient-to-r from-black/80 via-black/40 to-transparent" />
        <div className="absolute inset-0 bg-gradient-to-t from-black via-black/60 to-transparent" />
      </div>


      {/* 
        ====================================================
        1. 左侧 TV 风格展开式侧边栏 (Collapsed / Expanded Sidebar)
        ====================================================
      */}
      <LibraryTVExpandedSidebar
        activeCategory={activeCategory}
        onSelectCategory={handleSelectCategory}
      />

      {/* 
        ====================================================
        2. 主体验内容区 (Main 10-Foot Canvas)
        ====================================================
      */}
      <div className="flex-1 pl-[128px] flex flex-col min-w-0 z-10">
        {sliceState.kind === "loading" && (
          <div className="grid grid-cols-2 gap-4 px-[32px] pt-[32px] sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6" aria-label="正在加载媒体库">
            {Array.from({ length: 6 }).map((_, i) => (
              <div key={i} className="flex flex-col gap-3">
                <div className="aspect-[2/3] animate-pulse rounded-2xl bg-white/10" />
                <div className="h-3 w-3/4 animate-pulse rounded bg-white/10" />
                <div className="h-3 w-1/2 animate-pulse rounded bg-white/10" />
              </div>
            ))}
          </div>
        )}
        {(sliceState.kind === "retryable_error" || sliceState.kind === "terminal_error") && (
          <div className="px-[32px] pt-[32px] text-sm text-white/80">
            <p>{sliceState.message || "媒体库加载失败"}</p>
            {sliceState.canRetry && (
              <button type="button" onClick={() => void loadLibrary()} className="mt-3 font-semibold underline">
                重试加载
              </button>
            )}
          </div>
        )}
        {sliceState.kind === "offline_partial" && loadError && (
          <div className="px-[32px] pt-[32px] text-sm text-white/80">
            <p>{sliceState.message || "媒体库刷新失败，当前显示的内容可能已过期"}</p>
            {sliceState.canRetry && (
              <button type="button" onClick={() => void loadLibrary()} className="mt-3 font-semibold underline">
                重试刷新
              </button>
            )}
          </div>
        )}
        {sliceState.kind === "empty" && (
          <p className="px-[32px] pt-[32px] text-sm text-white/70">媒体库中还没有内容</p>
        )}
        

        {/* 顶部选中的媒体原信息展示 (Hero Info Section) */}
        {focusedItem && <LibraryTVHeroInfo item={focusedItem} />}

        {/* 
          ====================================================
          3. 陈列栏横排 (Horizontal Scrolling Shelves)
          ====================================================
        */}
        <main className="flex flex-col gap-10 px-[32px] pt-[16px]">
          
          {/* 如果是“全部”或“推荐”，展示多排大分类陈列栏 */}
          {activeCategory === "all" && (
            <>
              <LibraryTVRowShelf
                title="影视热播 (Movies & Shows)"
                items={movies}
                onHoverSpotlight={handleHoverSpotlight}
                onSeeMore={() => navigate("/library/browse/video")}
              />

              <LibraryTVRowShelf
                title="在读书籍与长篇内容 (Books)"
                items={books}
                onHoverSpotlight={handleHoverSpotlight}
                onSeeMore={() => navigate("/library/browse/book")}
              />

              <LibraryTVRowShelf
                title="国漫精选 & 日漫单行本 (Comics)"
                items={comics}
                onHoverSpotlight={handleHoverSpotlight}
                onSeeMore={() => navigate("/library/browse/comic")}
              />

              <LibraryTVRowShelf
                title="商业报刊 & 深度期刊 (Periodicals)"
                items={periodicals}
                onHoverSpotlight={handleHoverSpotlight}
                onSeeMore={() => navigate("/library/browse/periodical")}
              />

              <LibraryTVRowShelf
                title="官方技术规范 & 白皮书 (Documents)"
                items={documents}
                onHoverSpotlight={handleHoverSpotlight}
                onSeeMore={() => navigate("/library/browse/document")}
              />
            </>
          )}

          {/* 如果选择了特定分类（如影视、漫画、资料等），显示精选陈列栏 + 全量分类网格 */}
          {activeCategory !== "all" && (
            <div className="flex flex-col gap-[32px] pt-[16px]">
              <LibraryTVRowShelf
                title={`${getCategoryTitle(activeCategory)} · 热门推荐`}
                items={getCategoryItems(activeCategory)}
                onHoverSpotlight={handleHoverSpotlight}
                onSeeMore={() => navigate(`/library/browse/${activeCategory}`)}
              />

              <div className="flex flex-col gap-[16px] border-t border-white/10 pt-[32px]">
                <h2 className="text-xl md:text-2xl font-black text-white">
                  全部 {getCategoryTitle(activeCategory)} 资源
                </h2>
                <LibraryGrid
                  category={activeCategory}
                  viewMode="grid"
                  items={libraryItems}
                  onHoverItem={handleHoverSpotlight}
                />
              </div>
            </div>
          )}

        </main>
      </div>
    </div>
  )
}

function filterByCategory(items: LibraryMediaItemData[], cat: string) {
  if (cat === "video") return items.filter((i) => i.type === "movie" || i.type === "tv")
  return items.filter((i) => i.type === cat)
}

function getCategoryTitle(cat: string) {
  switch (cat) {
    case "video": return "影视"
    case "comic": return "漫画"
    case "periodical": return "报刊"
    case "document": return "资料"
    case "book": return "图书"
    case "search": return "搜索"
    default: return "推荐"
  }
}
