import React from "react"
import { Search, X } from "lucide-react"
import { cn } from "@/lib/utils"

export interface SearchBarProps {
  value?: string
  onChange?: (e: React.ChangeEvent<HTMLInputElement>) => void
  onSearch?: () => void
  onClear?: () => void
  onFocus?: () => void
  onBlur?: () => void
  placeholder?: string
  className?: string
  disabled?: boolean
}

export function SearchBar({
  value,
  onChange,
  onSearch,
  onClear,
  onFocus,
  onBlur,
  placeholder = "搜搜动漫或影视",
  className,
  disabled = false,
}: SearchBarProps) {
  return (
    <div className={cn("flex items-center gap-[16px] w-full", className)}>
      <div className="relative flex-1 group">
        <div className="absolute inset-y-0 left-0 pl-6 flex items-center pointer-events-none">
          <Search className="w-5 h-5 text-muted-foreground transition-colors group-focus-within:text-primary" />
        </div>
        <input
          type="text"
          value={value}
          onChange={onChange}
          onFocus={onFocus}
          onBlur={onBlur}
          placeholder={placeholder}
          disabled={disabled}
          onKeyDown={(event) => {
            if (event.key === "Enter") onSearch?.()
            if (event.key === "Escape" && value) onClear?.()
          }}
          className={cn(
            "w-full pl-[56px] pr-[52px] h-[56px] md:h-[64px] rounded-full bg-white dark:bg-zinc-900 border border-black/5 dark:border-white/5",
            "text-base md:text-lg text-foreground placeholder:text-muted-foreground",
            "shadow-sm transition-all duration-300",
            "focus:outline-none focus:ring-4 focus:ring-primary/20 focus:border-primary/30 focus:shadow-md disabled:cursor-not-allowed disabled:opacity-60"
          )}
        />
        {value && !disabled ? (
          <button
            type="button"
            onClick={onClear}
            className="absolute right-[16px] top-1/2 flex h-[32px] w-[32px] -translate-y-1/2 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
            title="清空搜索"
            aria-label="清空搜索"
          >
            <X className="h-[16px] w-[16px]" aria-hidden="true" />
          </button>
        ) : null}
      </div>
      <button 
        type="button"
        onClick={onSearch}
        disabled={disabled}
        className="hidden sm:block shrink-0 px-[8px] text-muted-foreground hover:text-foreground font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-50"
      >
        搜索
      </button>
    </div>
  )
}
