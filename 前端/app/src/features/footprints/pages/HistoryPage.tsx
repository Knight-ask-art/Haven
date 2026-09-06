import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { ArrowLeft, Check } from "lucide-react"
import { useNavigate } from "react-router"
import type { MediaCardProps } from "@/components/ui/haven/MediaCard"
import { ArtworkImage } from "@/components/ui/haven/ArtworkImage"
import { defaultCoverCategoryForMediaType } from "@/lib/default-cover"
import { cn } from "@/lib/utils"
import { getHavenClientMode } from "@/lib/ipc/runtime"
import { toHavenError } from "@/lib/ipc/errors"
import type { HavenError } from "@/lib/ipc/errors"
import { getRecentActivityFootprintItems } from "@/features/footprints/ipc/footprints-gateway"
import type { HistoryCardProps } from "@/features/footprints/ipc/footprints-gateway"
import { groupHistoryByDate } from "@/features/footprints/lib/group-history-by-date"
import {
  loadDemoHistoryValue,
  resolveHistoryRuntimeState,
  shouldApplyHistoryRequest,
} from "@/features/footprints/lib/history-runtime-state"
import { primaryActionRoute } from "@/features/media/lib/primary-action-route"

// Mock Data
const todayHistory: MediaCardProps[] = [
  {
    id: "2",
    title: "沙丘2 Dune: Part Two",
    subtitle: "昨天 · 已播放 45%",
    imageUrl: "https://images.unsplash.com/photo-1534447677768-be436bb09401?q=80&w=800&auto=format&fit=crop",
    artworkCategory: "video",
  },
  {
    id: "10",
    title: "星际穿越 Interstellar",
    subtitle: "已看 1h 23m",
    imageUrl: "https://images.unsplash.com/photo-1462331940025-496dfbfc7564?q=80&w=800&auto=format&fit=crop",
    artworkCategory: "video",
  },
]

const yesterdayHistory: MediaCardProps[] = [
  {
    id: "4",
    title: "怪奇物语：1985故事集 第一季",
    subtitle: "S1E4 · 你真正的自己",
    imageUrl: "https://images.unsplash.com/photo-1578632767115-351597cf2477?q=80&w=800&auto=format&fit=crop",
    artworkCategory: "video",
  },
  {
    id: "6",
    title: "奥本海默 Oppenheimer",
    subtitle: "已看完 · 1080P",
    imageUrl: "https://images.unsplash.com/photo-1446776811953-b23d57bd21aa?q=80&w=800&auto=format&fit=crop",
    artworkCategory: "video",
  },
  {
    id: "comic-aot",
    title: "进击的巨人 漫画全集",
    subtitle: "最终卷 · 88%",
    imageUrl: "https://images.unsplash.com/photo-1542451313056-b7c8e626645f?q=80&w=800&auto=format&fit=crop",
    artworkCategory: "comic",
  }
]

const pastHistory: MediaCardProps[] = [
  {
    id: "d1",
    title: "Apple Human Interface Guidelines",
    subtitle: "第 3 部分 · 75%",
    imageUrl: "https://images.unsplash.com/photo-1518770660439-4636190af475?q=80&w=800&auto=format&fit=crop",
    artworkCategory: "book",
  }
]

const HIDDEN_HISTORY_KEY = "haven:hidden-history"

export function HistoryPage() {
  const navigate = useNavigate()
  const clientMode = getHavenClientMode()
  const runtimeState = resolveHistoryRuntimeState(clientMode)
  const demoMode = runtimeState === "demo"
  const productionMode = runtimeState === "production"
  const unavailableMode = runtimeState === "unavailable"
  const [isSelecting, setIsSelecting] = useState(false)
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set())
  const [hiddenHistoryIds, setHiddenHistoryIds] = useState<string[]>(() =>
    loadDemoHistoryValue(clientMode, readHiddenHistoryIds, []),
  )
  const [recentHistoryItems, setRecentHistoryItems] = useState<HistoryCardProps[]>([])
  const [historyLoading, setHistoryLoading] = useState(productionMode)
  const [historyError, setHistoryError] = useState<HavenError | null>(null)
  const [openError, setOpenError] = useState<string | null>(null)
  const historyRequestRef = useRef(0)

  const productionGroups = useMemo(
    () => (productionMode ? groupHistoryByDate(recentHistoryItems) : []),
    [productionMode, recentHistoryItems],
  )

  const loadRecentHistory = useCallback(async () => {
    const requestId = ++historyRequestRef.current
    setHistoryLoading(true)
    setHistoryError(null)
    try {
      const items = await getRecentActivityFootprintItems()
      if (shouldApplyHistoryRequest(historyRequestRef.current, requestId)) {
        setRecentHistoryItems(items)
      }
    } catch (error) {
      if (shouldApplyHistoryRequest(historyRequestRef.current, requestId)) {
        setHistoryError(toHavenError(error))
      }
    } finally {
      if (shouldApplyHistoryRequest(historyRequestRef.current, requestId)) {
        setHistoryLoading(false)
      }
    }
  }, [])

  useEffect(() => {
    if (!productionMode) return
    void loadRecentHistory()
    return () => {
      historyRequestRef.current += 1
    }
  }, [loadRecentHistory, productionMode])

  const historyGroups = loadDemoHistoryValue(
    clientMode,
    () => [
      { title: "今天", items: todayHistory },
      { title: "昨天", items: yesterdayHistory },
      { title: "2026年7月", items: pastHistory },
    ].map((group) => ({
      ...group,
      items: group.items.filter((item) => !hiddenHistoryIds.includes(item.id)),
    })),
    [],
  )

  const visibleItems = historyGroups.flatMap((group) => group.items)
  const allVisibleIds = visibleItems.map((item) => item.id)
  const isAllSelected = allVisibleIds.length > 0 && allVisibleIds.every((id) => selectedIds.has(id))

  const enterSelectionMode = () => {
    setIsSelecting(true)
    setSelectedIds(new Set())
  }

  const exitSelectionMode = () => {
    setIsSelecting(false)
    setSelectedIds(new Set())
  }

  const toggleSelected = (id: string) => {
    setSelectedIds((current) => {
      const next = new Set(current)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  const toggleSelectGroup = (groupItems: MediaCardProps[]) => {
    const groupIds = groupItems.map((i) => i.id)
    const isGroupAllSelected = groupIds.every((id) => selectedIds.has(id))
    
    setSelectedIds((current) => {
      const next = new Set(current)
      groupIds.forEach((id) => {
        if (isGroupAllSelected) next.delete(id)
        else next.add(id)
      })
      return next
    })
  }

  const toggleSelectAll = () => {
    setSelectedIds(isAllSelected ? new Set() : new Set(allVisibleIds))
  }

  const deleteSelected = () => {
    if (!demoMode) return
    if (selectedIds.size === 0) return
    const nextHiddenIds = Array.from(new Set([...hiddenHistoryIds, ...selectedIds]))
    localStorage.setItem(HIDDEN_HISTORY_KEY, JSON.stringify(nextHiddenIds))
    setHiddenHistoryIds(nextHiddenIds)
    exitSelectionMode()
  }

  if (unavailableMode) return <UnavailableHistoryState />

  if (productionMode) {
    const openHistoryItem = (item: HistoryCardProps) => {
      const route = primaryActionRoute(item.primaryAction)
      if (!route) {
        setOpenError(`${item.title}当前没有可用内容`)
        return
      }
      setOpenError(null)
      navigate(route)
    }
    return (
      <div className="w-full min-h-full bg-background selection:bg-primary/20 transition-colors">
        <HistoryHeader onBack={() => navigate(-1)} />
        <main className="mx-auto flex w-full max-w-[1440px] flex-col gap-10 px-6 py-[32px] md:px-[48px] lg:px-[64px]">
          {historyLoading && recentHistoryItems.length === 0 ? (
            <HistoryMessage>正在加载浏览记录…</HistoryMessage>
          ) : historyError && recentHistoryItems.length === 0 ? (
            <HistoryErrorState error={historyError} onRetry={() => void loadRecentHistory()} />
          ) : recentHistoryItems.length === 0 ? (
            <EmptyHistoryState />
          ) : (
            <>
              {productionGroups.map((group) => (
                <HistoryGroup
                  key={group.title}
                  title={group.title}
                  items={group.items}
                  selectionMode={false}
                  selectedIds={new Set()}
                  onToggle={() => undefined}
                  onToggleGroup={() => undefined}
                  onOpen={openHistoryItem}
                />
              ))}
            </>
          )}
          {historyError && recentHistoryItems.length > 0 && (
            <HistoryErrorState error={historyError} onRetry={() => void loadRecentHistory()} compact />
          )}
          {openError && <HistoryMessage>{openError}</HistoryMessage>}
        </main>
      </div>
    )
  }

  return (
    <div className="w-full min-h-full bg-background selection:bg-primary/20 transition-colors">
      {/* 顶部 Sticky Header (无下划分割线) */}
      <header className="sticky top-0 z-50 flex items-center justify-between gap-[16px] bg-background/90 px-6 py-3.5 backdrop-blur-2xl md:px-[48px] transition-all">
        <div className="flex items-center gap-3">
          <button
            type="button"
            onClick={isSelecting ? exitSelectionMode : () => navigate(-1)}
            className="flex h-9 w-9 items-center justify-center rounded-full hover:bg-muted/60 transition-colors cursor-pointer"
            aria-label="返回"
          >
            <ArrowLeft size={18} />
          </button>
          <div>
            <h1 className="text-lg font-extrabold tracking-tight leading-none">浏览记录</h1>
            {isSelecting && (
              <p className="text-xs text-muted-foreground font-medium mt-1">
                {selectedIds.size > 0 ? (
                  <span className="text-primary font-bold">已选择 {selectedIds.size} 项</span>
                ) : (
                  "点击卡片进行选择"
                )}
              </p>
            )}
          </div>
        </div>

        {/* 管理模式下的右侧纯文字控制组 (极简 Apple iOS / macOS 纯文字风格) */}
        {isSelecting ? (
          <div className="flex items-center gap-[16px] sm:gap-5">
            <button
              type="button"
              onClick={toggleSelectAll}
              disabled={visibleItems.length === 0}
              className="text-xs font-semibold text-foreground hover:opacity-75 transition-opacity cursor-pointer disabled:opacity-30"
            >
              {isAllSelected ? "取消全选" : "全选"}
            </button>

            {selectedIds.size > 0 ? (
              <button
                type="button"
                onClick={deleteSelected}
                className="text-xs font-bold text-red-500 hover:text-red-600 dark:text-red-400 transition-colors cursor-pointer"
              >
                删除
              </button>
            ) : (
              <button
                type="button"
                disabled
                className="text-xs font-semibold text-muted-foreground/40 cursor-not-allowed"
              >
                删除
              </button>
            )}

            <button
              type="button"
              onClick={exitSelectionMode}
              className="text-xs font-bold text-primary hover:opacity-80 transition-opacity cursor-pointer"
            >
              完成
            </button>
          </div>
        ) : (
          <button
            type="button"
            onClick={enterSelectionMode}
            disabled={visibleItems.length === 0}
            className="rounded-full bg-muted/80 px-[16px] py-1.5 text-xs font-bold text-foreground transition-all hover:bg-muted disabled:opacity-30 disabled:cursor-not-allowed cursor-pointer"
          >
            管理记录
          </button>
        )}
      </header>

      {/* 历史记录列表 */}
      <main className="mx-auto flex w-full max-w-[1440px] flex-col gap-10 px-6 py-[32px] md:px-[48px] lg:px-[64px]">
        {historyGroups.map((group) => (
          <HistoryGroup
            key={group.title}
            title={group.title}
            items={group.items}
            selectionMode={isSelecting}
            selectedIds={selectedIds}
            onToggle={toggleSelected}
            onToggleGroup={() => toggleSelectGroup(group.items)}
          />
        ))}

        {visibleItems.length === 0 && (
          <div className="flex min-h-[320px] flex-col items-center justify-center rounded-[2.5rem] border border-dashed border-border bg-muted/15 text-center p-[32px]">
            <p className="text-base font-bold">暂无浏览记录</p>
            <p className="mt-[8px] text-xs text-muted-foreground max-w-sm">当你点开作品或阅读内容时，历史足迹会自动保留在这里。</p>
          </div>
        )}
      </main>
    </div>
  )
}

function HistoryHeader({ onBack }: { onBack: () => void }) {
  return (
    <header className="sticky top-0 z-50 flex items-center gap-3 bg-background/90 px-6 py-3.5 backdrop-blur-2xl md:px-[48px] transition-all">
      <button
        type="button"
        onClick={onBack}
        className="flex h-9 w-9 items-center justify-center rounded-full hover:bg-muted/60 transition-colors cursor-pointer"
        aria-label="返回"
      >
        <ArrowLeft size={18} />
      </button>
      <div>
        <h1 className="text-lg font-extrabold tracking-tight leading-none">浏览记录</h1>
        <p className="mt-1 text-xs font-medium text-muted-foreground">最近活动</p>
      </div>
    </header>
  )
}

function HistoryMessage({ children }: { children: string }) {
  return <p className="py-16 text-center text-sm text-muted-foreground">{children}</p>
}

function EmptyHistoryState() {
  return (
    <div className="flex min-h-[320px] flex-col items-center justify-center rounded-[2.5rem] border border-dashed border-border bg-muted/15 text-center p-[32px]">
      <p className="text-base font-bold">暂无浏览记录</p>
      <p className="mt-[8px] text-xs text-muted-foreground max-w-sm">当你点开作品或阅读内容时，历史足迹会自动保留在这里。</p>
    </div>
  )
}

function HistoryErrorState({ error, onRetry, compact = false }: { error: HavenError; onRetry: () => void; compact?: boolean }) {
  return (
    <div className={cn("text-center text-sm text-muted-foreground", compact ? "py-3" : "flex min-h-[320px] flex-col items-center justify-center")}>
      <p>{error.message || "浏览记录加载失败"}</p>
      {error.retryable && (
        <button type="button" onClick={onRetry} className="mt-3 font-semibold text-foreground underline">
          重试加载
        </button>
      )}
    </div>
  )
}

function UnavailableHistoryState() {
  return (
    <section className="flex min-h-full flex-col items-center justify-center px-6 py-16 text-center">
      <h1 className="text-xl font-semibold text-foreground">浏览记录未启用</h1>
      <p className="mt-2 max-w-md text-sm text-muted-foreground">当前浏览器不支持应用数据访问，请在 Haven 应用中打开浏览记录。</p>
    </section>
  )
}

function HistoryGroup({
  title,
  items,
  selectionMode,
  selectedIds,
  onToggle,
  onToggleGroup,
  onOpen,
}: {
  title: string
  items: MediaCardProps[]
  selectionMode: boolean
  selectedIds: Set<string>
  onToggle: (id: string) => void
  onToggleGroup: () => void
  onOpen?: (item: HistoryCardProps) => void
}) {
  if (!items || items.length === 0) return null

  const isGroupAllSelected = items.every((i) => selectedIds.has(i.id))
  const allowExternal = getHavenClientMode() !== "tauri"

  return (
    <section className="flex flex-col gap-[16px]">
      <div className="flex items-center justify-between">
        <h2 className="text-lg sm:text-xl font-bold tracking-tight text-foreground">{title}</h2>
        
        {selectionMode && (
          <button
            type="button"
            onClick={onToggleGroup}
            className="text-xs font-bold text-primary hover:underline cursor-pointer"
          >
            {isGroupAllSelected ? "取消本组" : "全选本组"}
          </button>
        )}
      </div>

      <div className="flex flex-wrap gap-5 md:gap-7">
        {items.map((item) => {
          const isSelected = selectedIds.has(item.id)
          return (
            <div
              key={item.id}
              onClick={selectionMode ? () => onToggle(item.id) : onOpen ? () => onOpen(item as HistoryCardProps) : undefined}
              onKeyDown={(event) => {
                if (event.key !== "Enter" && event.key !== " ") return
                event.preventDefault()
                if (selectionMode) onToggle(item.id)
                else if (onOpen) onOpen(item as HistoryCardProps)
              }}
              role="button"
              tabIndex={0}
              className={cn(
                "relative group flex flex-col gap-3 outline-none shrink-0 w-[140px] md:w-[160px] lg:w-[200px] select-none transition-transform duration-200",
                selectionMode || onOpen ? "cursor-pointer" : "cursor-default",
                selectionMode && isSelected && "scale-[1.02]"
              )}
            >
              {/* 海报图片及浮动打勾控制 (蒙层只局限在图片区域内！) */}
              <div className="relative aspect-[2/3] w-full overflow-hidden rounded-2xl">
                <ArtworkImage
                  src={item.imageUrl}
                  alt={item.title}
                  allowExternal={allowExternal}
                  fallbackCategory={item.artworkCategory ?? defaultCoverCategoryForMediaType(item.typeBadge)}
                  fallbackSeed={item.id}
                  className={cn(
                    "w-full h-full object-cover transition-transform duration-500",
                    selectionMode && isSelected ? "scale-105" : ""
                  )}
                  loading="lazy"
                />

                {/* 仅在海报图片区域上的暗色蒙层与高亮边框 */}
                {selectionMode && (
                  <div
                    className={cn(
                      "absolute inset-0 z-20 flex items-start justify-end p-[10px] transition-all rounded-2xl",
                      isSelected
                        ? "bg-black/35 ring-4 ring-primary ring-inset"
                        : "bg-black/10 hover:bg-black/25"
                    )}
                  >
                    <div
                      className={cn(
                        "flex h-7 w-7 items-center justify-center rounded-full border-2 shadow-xl transition-all duration-200",
                        isSelected
                          ? "border-primary bg-primary text-white scale-110 shadow-primary/50"
                          : "border-white/80 bg-black/40 text-transparent"
                      )}
                    >
                      <Check size={16} strokeWidth={3.5} className={isSelected ? "opacity-100" : "opacity-0"} />
                    </div>
                  </div>
                )}
              </div>

              {/* 卡片下方的标题文本 (完全在图片外部，绝对不受蒙层影响，保持 100% 高清与清晰) */}
              <div className="flex flex-col px-1">
                <h3 className={cn(
                  "text-sm font-semibold truncate transition-colors",
                  selectionMode && isSelected ? "text-primary font-bold" : "text-foreground"
                )}>
                  {item.title}
                </h3>
                {item.subtitle && (
                  <p className="text-xs text-muted-foreground/80 truncate mt-[1px] font-medium">
                    {item.subtitle}
                  </p>
                )}
              </div>
            </div>
          )
        })}
      </div>
    </section>
  )
}

function readHiddenHistoryIds(): string[] {
  try {
    const stored = localStorage.getItem(HIDDEN_HISTORY_KEY)
    const parsed: unknown = stored ? JSON.parse(stored) : []
    return Array.isArray(parsed) && parsed.every((id) => typeof id === "string") ? parsed : []
  } catch {
    return []
  }
}
