
import { useNavigate } from "react-router"
import { cn } from "@/lib/utils"
import { Star } from "lucide-react"
import { ArtworkImage } from "@/components/ui/haven/ArtworkImage"
import { defaultCoverCategoryForMediaType } from "@/lib/default-cover"
import { getHavenClientMode } from "@/lib/ipc/runtime"
import type { LocatorDto } from "@/lib/ipc/generated/wire"

export interface LibraryMediaItemData {
  id: string
  title: string
  originalTitle?: string
  type: string
  year: number
  imageUrl: string
  backdropUrl?: string
  rating?: string
  badge?: string
  size?: string
  description?: string
  /** 服务端收藏投影（WorkCardDto.favorite）；随导航传给详情页作初始值。 */
  favorite?: boolean
  /** Server-selected progress target; absent means batch completion is unavailable. */
  progressMediaItemId?: string
  progressLocator?: LocatorDto | null
}

interface MediaItemProps {
  item: LibraryMediaItemData
  onHover?: (item: LibraryMediaItemData) => void
  density?: "regular" | "compact"
  selectionMode?: boolean
  selected?: boolean
  onSelect?: (id: string) => void
}

const getTypeLabel = (type: string) => {
  switch (type) {
    case "movie": return "影视"
    case "book": return "图书"
    case "comic": return "漫画"
    case "periodical": return "报刊"
    case "document": return "资料"
    default: return "媒体"
  }
}

export function MediaItem({ item, onHover, density = "regular", selectionMode = false, selected = false, onSelect }: MediaItemProps) {
  const navigate = useNavigate()
  const isCompact = density === "compact"
  const allowExternal = getHavenClientMode() !== "tauri"
  const canBatchComplete = Boolean(item.progressMediaItemId && item.progressLocator)

  return (
    <div 
      onClick={() => navigate(`/work/${item.id}`)}
      onMouseEnter={() => onHover?.(item)}
      className={cn(
        "group relative flex cursor-pointer select-none outline-none",
        isCompact ? "flex-col gap-4 py-8" : "flex-col gap-[8px] py-[16px]"
      )}
      tabIndex={0}
      role="button"
    >
      {/* Cover Area */}
      <div 
        className={cn(
          "relative aspect-[2/3] w-full rounded-sm overflow-hidden bg-muted/40 shadow-sm",
          "transition-[transform,border-color,box-shadow] duration-300 ease-out border-[2px] border-transparent origin-center",
          "group-hover:scale-105 group-hover:border-primary/50 group-hover:shadow-xl group-hover:z-30",
          "group-focus-visible:scale-105 group-focus-visible:border-primary/50 group-focus-visible:shadow-xl group-focus-visible:z-30"
        )}
      >
        {selectionMode && (
          <button
            type="button"
            aria-label={canBatchComplete
              ? (selected ? `取消选择 ${item.title}` : `选择 ${item.title}`)
              : `${item.title}没有可用的阅读定位`}
            aria-pressed={selected}
            disabled={!canBatchComplete}
            onClick={(event) => { event.stopPropagation(); onSelect?.(item.id) }}
            className={cn(
              "absolute left-3 top-3 z-20 flex h-8 w-8 items-center justify-center rounded-full border-2 shadow-lg",
              selected ? "border-primary bg-primary text-primary-foreground" : "border-white bg-black/45 text-transparent",
              !canBatchComplete && "cursor-not-allowed opacity-45",
            )}
          >
            <span aria-hidden="true">✓</span>
          </button>
        )}
        <ArtworkImage
          src={item.imageUrl}
          alt={item.title}
          allowExternal={allowExternal}
          fallbackCategory={defaultCoverCategoryForMediaType(item.type)}
          fallbackSeed={item.id}
          className="w-full h-full object-cover transition-transform duration-500"
          loading="lazy"
        />

        {/* 底部红色提示条 (Recently Added / Badge) */}
        {item.badge && (
          <div className="absolute bottom-0 inset-x-0 py-1 bg-red-600/95 backdrop-blur-xs text-center shadow-lg z-10">
            <span className="block text-[10px] md:text-xs font-black text-white tracking-wider uppercase leading-none">
              {item.badge}
            </span>
          </div>
        )}

        {/* 光影遮罩 */}
        <div className="absolute inset-0 bg-gradient-to-t from-black/20 via-transparent to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-300" />
      </div>

      {/* Metadata Area */}
      <div className={cn(
        "flex flex-col px-1 transition-opacity duration-300 group-hover:opacity-100",
        isCompact ? "gap-0.5 mt-0" : "gap-0.5 mt-1"
      )}>
        <h3 
          className={cn(
            "font-bold leading-tight text-foreground group-hover:text-primary transition-colors truncate",
            isCompact ? "text-xs md:text-sm" : "text-sm md:text-base"
          )}
          title={item.title}
        >
          {item.title}
        </h3>
        
        <div className={cn(
          "flex items-center justify-between font-semibold text-neutral-400",
          isCompact ? "text-[11px]" : "text-xs"
        )}>
          <span>{item.year} • {getTypeLabel(item.type)}</span>
          {item.rating && (
            <span className="flex items-center gap-1 text-amber-400">
              <Star className={cn("fill-current", isCompact ? "h-[12px] w-[12px]" : "w-3.5 h-3.5")} />
              {item.rating}
            </span>
          )}
        </div>
      </div>
    </div>
  )
}
