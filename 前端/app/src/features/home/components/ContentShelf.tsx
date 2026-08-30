import React, { useRef, useEffect, useState } from "react"
import { cn } from "@/lib/utils"
import { MediaCard } from "@/components/ui/haven/MediaCard"
import type { MediaCardProps } from "@/components/ui/haven/MediaCard"
import { ChevronLeft, ChevronRight } from "lucide-react"

interface ContentShelfProps {
  title: string
  items: MediaCardProps[]
  className?: string
  actionRight?: React.ReactNode
}

export function ContentShelf({ title, items, className, actionRight }: ContentShelfProps) {
  const scrollRef = useRef<HTMLDivElement>(null)
  const [canScrollLeft, setCanScrollLeft] = useState(false)
  const [canScrollRight, setCanScrollRight] = useState(true)

  const checkScroll = () => {
    const el = scrollRef.current
    if (!el) return
    setCanScrollLeft(el.scrollLeft > 10)
    setCanScrollRight(el.scrollLeft < el.scrollWidth - el.clientWidth - 10)
  }

  // 1. 监听滚动更新左右悬浮按钮状态
  useEffect(() => {
    const el = scrollRef.current
    if (!el) return

    el.addEventListener("scroll", checkScroll, { passive: true })
    
    // 初始化检查 (延迟确保 DOM 渲染完成)
    const timer = setTimeout(checkScroll, 100)

    return () => {
      el.removeEventListener("scroll", checkScroll)
      clearTimeout(timer)
    }
  }, [])



  // 3. 左右导航按键点击平滑滚动 (精准滑动单个项目)
  const scrollTo = (direction: "left" | "right") => {
    const el = scrollRef.current
    if (!el) return

    // 动态获取第一张卡片的真实宽度 + 间距(gap)，确保单次点击只滑动“一个项目”
    const cardEl = el.querySelector('[role="button"]') as HTMLElement
    const singleItemStep = cardEl ? cardEl.offsetWidth + 24 : 320

    el.scrollBy({
      left: direction === "left" ? -singleItemStep : singleItemStep,
      behavior: "smooth"
    })
  }

  return (
    <section className={cn("group/shelf relative flex flex-col gap-3 py-[16px] w-full select-none", className)}>
      {/* 标题栏 */}
      <div className="px-[48px] md:px-[64px] lg:px-[96px] flex items-center justify-between">
        <h2 className="text-xl md:text-2xl font-bold tracking-tight">{title}</h2>
        {actionRight && <div>{actionRight}</div>}
      </div>
      
      {/* 轨道包装容器：将左右侧侧边悬浮按钮限制在卡片图片高度的垂直正中心 */}
      <div className="relative w-full">
        {/* 左侧悬浮按钮与渐隐遮罩 */}
        {canScrollLeft && (
          <div className="absolute left-0 top-0 bottom-6 z-30 hidden md:flex items-center pl-6 pr-[48px] bg-gradient-to-r from-background via-background/60 to-transparent pointer-events-none opacity-0 group-hover/shelf:opacity-100 transition-opacity duration-300">
            <button
              onClick={() => scrollTo("left")}
              className="pointer-events-auto cursor-pointer flex items-center justify-center w-11 h-11 rounded-full bg-white/90 dark:bg-zinc-900/90 backdrop-blur-xl border border-black/10 dark:border-white/15 text-foreground shadow-2xl transition-all hover:scale-110 active:scale-90 hover:bg-white dark:hover:bg-zinc-800"
              aria-label="Previous item"
            >
              <ChevronLeft className="w-5 h-5" />
            </button>
          </div>
        )}

        {/* 右侧悬浮按钮与渐隐遮罩 */}
        {canScrollRight && (
          <div className="absolute right-0 top-0 bottom-6 z-30 hidden md:flex items-center pr-6 pl-[48px] bg-gradient-to-l from-background via-background/60 to-transparent pointer-events-none opacity-0 group-hover/shelf:opacity-100 transition-opacity duration-300">
            <button
              onClick={() => scrollTo("right")}
              className="pointer-events-auto cursor-pointer flex items-center justify-center w-11 h-11 rounded-full bg-white/90 dark:bg-zinc-900/90 backdrop-blur-xl border border-black/10 dark:border-white/15 text-foreground shadow-2xl transition-all hover:scale-110 active:scale-90 hover:bg-white dark:hover:bg-zinc-800"
              aria-label="Next item"
            >
              <ChevronRight className="w-5 h-5" />
            </button>
          </div>
        )}

        {/* 隐藏滚动条但保留全套流畅交互 (依靠按钮滚动) */}
        <div 
          ref={scrollRef}
          className={cn(
            "flex overflow-x-hidden overflow-y-hidden pb-[16px] pt-[8px] -mt-[8px]",
            "scroll-smooth"
          )}
        >
          <div className="w-[48px] md:w-[64px] lg:w-[96px] shrink-0" aria-hidden="true" />
          
          <div className="flex gap-[16px] md:gap-6 shrink-0">
            {items.map((item) => (
              <MediaCard key={item.id} {...item} />
            ))}
          </div>

          <div className="w-[48px] md:w-[64px] lg:w-[96px] shrink-0" aria-hidden="true" />
        </div>
      </div>
    </section>
  )
}
