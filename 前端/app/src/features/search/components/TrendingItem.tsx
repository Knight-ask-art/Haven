
import { cn } from "@/lib/utils"
import { ArtworkImage } from "@/components/ui/haven/ArtworkImage"
import type { DefaultCoverCategory } from "@/lib/default-cover"

export interface TrendingItemProps {
  title: string
  subtitle: string
  description: string
  imageUrl?: string | null
  loading?: "eager" | "lazy"
  fetchPriority?: "high" | "low" | "auto"
  statusBadge?: string
  fallbackCategory?: DefaultCoverCategory
  fallbackSeed?: string
  className?: string
  onClick?: () => void
}

export function TrendingItem({
  title,
  subtitle,
  description,
  imageUrl,
  loading = "lazy",
  fetchPriority = "auto",
  statusBadge,
  fallbackCategory = "video",
  fallbackSeed,
  className,
  onClick
}: TrendingItemProps) {
  return (
    <div 
      onClick={onClick}
      className={cn(
        "group flex gap-[16px] p-[8px] -mx-[8px] rounded-xl cursor-pointer outline-none transition-colors",
        "hover:bg-black/5 dark:hover:bg-white/5 focus-visible:ring-2 focus-visible:ring-primary",
        className
      )}
      tabIndex={0}
      role="button"
    >
      {/* 竖版海报 */}
      <div className="relative shrink-0 overflow-hidden rounded-lg aspect-[2/3] w-[80px] md:w-[96px] shadow-sm bg-muted transition-transform duration-300 group-hover:shadow-md group-hover:scale-105">
        <ArtworkImage
          src={imageUrl}
          alt={title}
          className="w-full h-full object-cover"
          loading={loading}
          fetchPriority={fetchPriority}
          fallbackCategory={fallbackCategory}
          fallbackSeed={fallbackSeed}
        />
      </div>

      {/* 信息流 */}
      <div className="flex flex-col flex-1 min-w-0 justify-center">
        <h3 className="font-bold text-base md:text-lg text-foreground truncate group-hover:text-primary transition-colors">
          {title}
        </h3>
        
        <p className="text-xs md:text-sm text-muted-foreground truncate mt-0.5">
          {subtitle}
        </p>

        {statusBadge && (
          <div className="mt-[8px]">
            <span className="inline-flex items-center px-[8px] py-0.5 rounded text-[11px] font-medium bg-blue-500/10 text-blue-600 dark:text-blue-400">
              {statusBadge}
            </span>
          </div>
        )}

        <p className="text-xs text-muted-foreground line-clamp-2 mt-[8px] leading-relaxed text-pretty">
          {description}
        </p>
      </div>
    </div>
  )
}
