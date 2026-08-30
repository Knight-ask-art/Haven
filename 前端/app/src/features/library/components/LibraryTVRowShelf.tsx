import { useRef } from "react"
import { useNavigate } from "react-router"
import { MediaItem } from "./MediaItem"
import type { LibraryMediaItemData } from "./MediaItem"
import { ChevronRight, ChevronLeft } from "lucide-react"

interface LibraryTVRowShelfProps {
  title: string
  subCategoryKey?: string
  items: LibraryMediaItemData[]
  onHoverSpotlight?: (item: LibraryMediaItemData) => void
  onSeeMore?: () => void
}

export function LibraryTVRowShelf({
  title,
  items,
  onHoverSpotlight,
  onSeeMore
}: LibraryTVRowShelfProps) {
  const scrollRef = useRef<HTMLDivElement>(null)
  const navigate = useNavigate()

  const scroll = (direction: "left" | "right") => {
    if (!scrollRef.current) return
    const step = 320
    scrollRef.current.scrollBy({
      left: direction === "left" ? -step : step,
      behavior: "smooth"
    })
  }

  return (
    <section className="relative flex flex-col gap-[16px] w-full select-none group/shelf py-[8px]">
      {/* 行标题与右侧“查找更多”控制 */}
      <div className="flex items-center justify-between pr-[16px] md:pr-[48px]">
        <div className="flex items-center gap-3">
          <h2 className="text-xl md:text-2xl font-black tracking-tight text-white drop-shadow-md">
            {title}
          </h2>
        </div>

        <button
          onClick={onSeeMore || (() => navigate(`/library/browse/${items[0]?.type === "movie" || items[0]?.type === "tv" ? "video" : items[0]?.type || "all"}`))}
          className="flex items-center gap-1 text-sm md:text-base font-bold text-white/80 hover:text-white transition-colors cursor-pointer"
        >
          <span>查看更多</span>
          <ChevronRight className="w-5 h-5" />
        </button>
      </div>

      {/* 横向陈列栏：露出一部分提示用户可以滚动 (Peeking effect) */}
      <div className="relative w-full">
        {/* 左右滚动悬浮按键 */}
        <button
          onClick={() => scroll("left")}
          className="absolute left-[8px] top-1/2 -translate-y-1/2 z-30 hidden md:flex items-center justify-center w-10 h-10 rounded-full bg-black/80 text-white border border-white/20 shadow-2xl opacity-0 group-hover/shelf:opacity-100 transition-all hover:scale-110 cursor-pointer"
        >
          <ChevronLeft className="w-5 h-5" />
        </button>
        <button
          onClick={() => scroll("right")}
          className="absolute right-6 top-1/2 -translate-y-1/2 z-30 hidden md:flex items-center justify-center w-10 h-10 rounded-full bg-black/80 text-white border border-white/20 shadow-2xl opacity-0 group-hover/shelf:opacity-100 transition-all hover:scale-110 cursor-pointer"
        >
          <ChevronRight className="w-5 h-5" />
        </button>

        {/* 卡片轨道 */}
        <div
          ref={scrollRef}
          className="flex items-center gap-[16px] md:gap-5 overflow-x-hidden scroll-smooth pb-6 pr-[80px] pt-[16px]"
        >
          {items.map((item) => (
            <div key={item.id} className="w-[150px] md:w-[180px] lg:w-[210px] shrink-0">
              <MediaItem item={item} onHover={onHoverSpotlight} />
            </div>
          ))}
        </div>
      </div>
    </section>
  )
}
