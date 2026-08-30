
import type { LibraryMediaItemData } from "./MediaItem"
import { Star } from "lucide-react"

interface LibraryTVHeroInfoProps {
  item: LibraryMediaItemData
}

export function LibraryTVHeroInfo({ item }: LibraryTVHeroInfoProps) {
  return (
    <div className="relative w-full pt-[64px] md:pt-[96px] px-[32px] max-w-5xl select-none z-10 min-h-[45vh] flex flex-col justify-end pb-[32px]">
      <div className="flex flex-col gap-[16px] animate-in fade-in duration-500">
        
        {/* 顶栏元数据 Badge 组: New, Year, Rating, Quality, Audio */}
        <div className="flex flex-wrap items-center gap-3 text-lg md:text-xl font-bold text-white/90 drop-shadow-md">
          {item.badge && (
            <span className="px-[8px] py-0.5 rounded bg-red-600 text-white font-black text-sm uppercase tracking-widest">
              {item.badge}
            </span>
          )}
          <span>{item.year}</span>
          <span className="px-[8px] py-0.5 rounded border border-white/40 text-sm font-extrabold uppercase text-white/80 tracking-wider">
            {item.type === "movie" ? "HD" : item.type === "tv" ? "4K" : "TEXT"}
          </span>
          <span className="px-[8px] py-0.5 rounded border border-white/40 text-sm font-extrabold uppercase text-white/80 tracking-wider">
            5.1
          </span>
          {item.rating && (
            <span className="flex items-center gap-1.5 text-amber-400">
              <Star className="w-5 h-5 fill-current" />
              {item.rating}
            </span>
          )}
        </div>

        {/* 选中的媒体标题 */}
        <div className="flex flex-col gap-1 mt-[8px]">
          {item.originalTitle && (
            <p className="text-xl md:text-2xl font-bold text-white/80 tracking-widest uppercase drop-shadow-md">
              {item.originalTitle}
            </p>
          )}
          <h1 className="text-5xl md:text-7xl font-black tracking-tight text-white drop-shadow-2xl leading-tight">
            {item.title}
          </h1>
        </div>

        {/* 简介短述 */}
        {item.description && (
          <p className="text-xl md:text-2xl text-white/90 line-clamp-3 leading-relaxed max-w-3xl font-medium drop-shadow-lg mt-[16px] text-shadow-sm">
            {item.description}
          </p>
        )}
      </div>
    </div>
  )
}
