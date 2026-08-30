import React, { useState } from "react"
import { Sparkles, Film, BookOpen, Image as ComicIcon, Newspaper, FileText, LayoutGrid } from "lucide-react"
import { cn } from "@/lib/utils"

export interface TVSidebarItem {
  id: string
  label: string
  icon: React.ElementType
}

export const TV_SIDEBAR_ITEMS: TVSidebarItem[] = [
  { id: "all", label: "推荐", icon: Sparkles },
  { id: "video", label: "影视", icon: Film },
  { id: "book", label: "图书", icon: BookOpen },
  { id: "comic", label: "漫画", icon: ComicIcon },
  { id: "periodical", label: "报刊", icon: Newspaper },
  { id: "document", label: "资料", icon: FileText },
]

interface LibraryTVExpandedSidebarProps {
  activeCategory: string
  onSelectCategory: (id: string) => void
}

export function LibraryTVExpandedSidebar({ activeCategory, onSelectCategory }: LibraryTVExpandedSidebarProps) {
  const [isHovered, setIsHovered] = useState(false)

  return (
    <>
      {/* 展开时的全局暗化遮罩 (移除了极耗性能的 backdrop-blur，改用纯色遮罩) */}
      <div 
        className={cn(
          "fixed inset-0 z-40 bg-black/60 transition-opacity duration-300 pointer-events-none",
          isHovered ? "opacity-100" : "opacity-0"
        )}
      />

      <aside
        onMouseEnter={() => setIsHovered(true)}
        onMouseLeave={() => setIsHovered(false)}
        className={cn(
          "fixed left-0 top-0 bottom-0 z-50 flex flex-col justify-center select-none transition-all duration-300 ease-out",
          isHovered ? "w-[320px] bg-gradient-to-r from-black/95 via-black/85 to-transparent" : "w-[128px] bg-transparent"
        )}
      >
        {/* 收起状态：图标向右靠，离开最左墙面 (pl-[64px]) */}
        <div 
          className={cn(
            "absolute inset-0 flex items-center justify-start pl-[64px] transition-opacity duration-200",
            isHovered ? "opacity-0 pointer-events-none" : "opacity-100"
          )}
        >
          <div className="text-white/80 hover:text-white transition-colors cursor-pointer p-[8px]">
            <LayoutGrid size={32} />
          </div>
        </div>

        {/* 展开状态：图标与文字间留出合理空隙 (gap-6) */}
        <div 
          className={cn(
            "absolute inset-0 flex flex-col justify-center gap-5 pl-14 pr-[32px] transition-all duration-300",
            isHovered ? "opacity-100 translate-x-0" : "opacity-0 -translate-x-[16px] pointer-events-none"
          )}
        >
          {TV_SIDEBAR_ITEMS.map((item) => {
            const Icon = item.icon
            const isActive = activeCategory === item.id

            return (
              <button
                key={item.id}
                onClick={() => onSelectCategory(item.id)}
                className="relative flex items-center gap-6 transition-all duration-200 cursor-pointer outline-none group py-[8px]"
                title={item.label}
              >
                <Icon 
                  size={26}
                  strokeWidth={isActive ? 2.5 : 2}
                  className={cn(
                    "shrink-0 transition-colors duration-200", 
                    isActive ? "text-white" : "text-neutral-400 group-hover:text-white"
                  )} 
                />
                
                <span 
                  className={cn(
                    "text-xl whitespace-nowrap tracking-wider transition-all duration-200",
                    isActive ? "font-bold text-white scale-105 origin-left drop-shadow-md" : "font-medium text-neutral-400 group-hover:text-white"
                  )}
                >
                  {item.label}
                </span>
              </button>
            )
          })}
        </div>
      </aside>
    </>
  )
}
