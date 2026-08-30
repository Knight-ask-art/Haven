
import { Trash2, X } from "lucide-react"
import { cn } from "@/lib/utils"

export interface SearchHistoryProps {
  className?: string
  history?: string[]
  onClear?: () => void
  onRemoveItem?: (item: string) => void
  onItemClick?: (item: string) => void
}

export function SearchHistory({ 
  className, 
  history = [], 
  onClear, 
  onRemoveItem, 
  onItemClick 
}: SearchHistoryProps) {
  return (
    <section className={cn("flex flex-col gap-[16px] w-full", className)}>
      <div className="flex items-center justify-between">
        <h2 className="text-xl font-bold tracking-tight">历史搜索</h2>
        {history.length > 0 && (
          <button 
            onClick={onClear}
            title="清空全部历史"
            className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-semibold text-muted-foreground hover:text-destructive hover:bg-destructive/10 rounded-full transition-colors outline-none focus-visible:ring-2 focus-visible:ring-primary cursor-pointer"
          >
            <Trash2 className="w-3.5 h-3.5" />
            <span>清空全部</span>
          </button>
        )}
      </div>

      {history.length === 0 ? (
        <div className="flex flex-col gap-[8px]">
          <p className="text-foreground font-medium">暂无历史搜索</p>
          <p className="text-sm text-muted-foreground">
            搜索过的关键词会显示在这里，方便你快速回到上次看的内容。
          </p>
        </div>
      ) : (
        <div className="flex flex-wrap gap-[10px]">
          {history.map((item) => (
            <div
              key={item}
              className="inline-flex items-center gap-1.5 pl-3.5 pr-[8px] py-1.5 text-sm bg-white dark:bg-zinc-900 border border-black/10 dark:border-white/10 rounded-full hover:border-black/20 dark:hover:border-white/20 transition-all group shadow-sm"
            >
              <button
                onClick={() => onItemClick?.(item)}
                className="text-foreground hover:text-primary font-medium cursor-pointer"
              >
                {item}
              </button>
              <button
                onClick={(e) => {
                  e.stopPropagation()
                  onRemoveItem?.(item)
                }}
                title={`删除 "${item}"`}
                className="p-0.5 rounded-full text-muted-foreground/60 hover:text-destructive hover:bg-black/5 dark:hover:bg-white/10 transition-colors cursor-pointer"
              >
                <X className="w-3.5 h-3.5" />
              </button>
            </div>
          ))}
        </div>
      )}
    </section>
  )
}
