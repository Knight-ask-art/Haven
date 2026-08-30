
import { cn } from "@/lib/utils"
import { TrendingItem } from "./TrendingItem"
import type { TrendingItemProps } from "./TrendingItem"

export interface TrendingBoardProps {
  title: string
  subtitle?: string
  items: TrendingItemProps[]
  className?: string
}

export function TrendingBoard({ title, subtitle, items, className }: TrendingBoardProps) {
  return (
    <div className={cn(
      "flex flex-col bg-white dark:bg-zinc-900 rounded-3xl p-6 shadow-sm border border-black/5 dark:border-white/5",
      className
    )}>
      {/* 榜单头部 */}
      <div className="flex flex-col gap-1 mb-6 px-[8px]">
        <h2 className="text-xl font-extrabold tracking-tight text-foreground">{title}</h2>
        {subtitle && (
          <span className="text-xs font-semibold text-muted-foreground uppercase tracking-wider">
            {subtitle}
          </span>
        )}
      </div>

      {/* 榜单列表 */}
      <div className="flex flex-col gap-[16px]">
        {items.map((item, idx) => (
          <TrendingItem
            key={idx}
            {...item}
            loading={idx < 2 ? "eager" : "lazy"}
            fetchPriority={idx < 2 ? "high" : "auto"}
          />
        ))}
      </div>
    </div>
  )
}
