import { useState, useRef, useEffect } from "react"
import { NavLink, useNavigate, useLocation } from "react-router"
import { HavenIcon } from "@/components/ui/haven/HavenIcon"
import { Ellipsis } from "lucide-react"
import { cn } from "@/lib/utils"

interface NavItemDef {
  to: string
  icon: string
  label: string
}

// 核心主要页面 (主 Dock)
const mainTabs: NavItemDef[] = [
  { to: "/", icon: "home", label: "首页" },
  { to: "/library", icon: "library", label: "媒体库" },
  { to: "/footprints", icon: "history", label: "足迹" }
]

export function HavenBottomNav() {
  return (
    <nav 
      aria-label="全局浮动导航栏"
      className="fixed bottom-6 left-1/2 -translate-x-1/2 z-50 flex items-center gap-[32px] select-none"
    >
      {/* 居中 Dock 悬浮主体导航条 */}
      <div className={cn(
        "h-[72px] px-7 flex items-center gap-[16px] rounded-[36px]",
        "bg-white/85 dark:bg-zinc-950/85 backdrop-blur-2xl",
        "border border-black/10 dark:border-white/15",
        "shadow-[0_12px_40px_-8px_rgba(0,0,0,0.18)] dark:shadow-[0_12px_40px_-8px_rgba(0,0,0,0.7)]",
        "transition-all duration-300"
      )}>
        {mainTabs.map((item) => (
          <DockNavItem key={item.to} item={item} />
        ))}
        {/* 更多菜单 (下载、设置) */}
        <DockMoreMenu />
      </div>

      {/* 右侧独立圆形按钮组 (只有搜索) */}
      <div className="flex items-center shrink-0">
        <SearchActionItem />
      </div>
    </nav>
  )
}

function DockNavItem({ item }: { item: NavItemDef }) {
  return (
    <NavLink
      to={item.to}
      end={item.to === "/"}
      className={({ isActive }) => cn(
        "relative h-[56px] w-[64px] px-[8px] rounded-2xl flex flex-col items-center justify-center gap-1 transition-all duration-200",
        "outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1",
        isActive 
          ? "text-primary" 
          : "text-muted-foreground hover:text-foreground hover:bg-black/5 dark:hover:bg-white/10"
      )}
    >
      {({ isActive }) => (
        <>
          <HavenIcon 
            symbol={item.icon} 
            size={24}
            weight={isActive ? "emphasized" : "regular"}
            role="navigation" 
            className={cn("mb-0.5 transition-transform duration-300", isActive && "scale-110")}
          />
          <span className={cn(
            "text-[11px] leading-none whitespace-nowrap transition-all duration-200",
            isActive ? "font-bold" : "font-medium"
          )}>{item.label}</span>
        </>
      )}
    </NavLink>
  )
}

function DockMoreMenu() {
  const [isOpen, setIsOpen] = useState(false)
  const menuRef = useRef<HTMLDivElement>(null)
  const location = useLocation()

  // 如果当前在下载或设置页，高亮更多按钮
  const isActive = location.pathname === "/downloads" || location.pathname.startsWith("/settings")

  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(event.target as Node)) {
        setIsOpen(false)
      }
    }
    if (isOpen) {
      document.addEventListener("mousedown", handleClickOutside)
    }
    return () => {
      document.removeEventListener("mousedown", handleClickOutside)
    }
  }, [isOpen])

  return (
    <div className="relative h-[56px] flex items-center" ref={menuRef}>
      <button
        onClick={() => setIsOpen(!isOpen)}
        className={cn(
          "relative h-full w-[64px] px-[8px] rounded-2xl flex flex-col items-center justify-center gap-1 transition-all duration-200",
          "outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1",
          isActive 
            ? "text-primary" 
            : "text-muted-foreground hover:text-foreground hover:bg-black/5 dark:hover:bg-white/10",
          isOpen && !isActive && "text-foreground bg-black/5 dark:bg-white/10"
        )}
      >
        <div className={cn("flex items-center justify-center h-[24px] mb-0.5 transition-transform duration-300", (isActive || isOpen) && "scale-110")}>
          <Ellipsis className="w-[22px] h-[22px]" />
        </div>
        <span className={cn(
          "text-[11px] leading-none whitespace-nowrap transition-all duration-200",
          isActive ? "font-bold" : "font-medium"
        )}>更多</span>
      </button>

      {isOpen && (
        <div className={cn(
          "absolute bottom-[calc(100%+20px)] left-1/2 -translate-x-1/2 w-[184px] p-2 rounded-2xl z-50 shadow-2xl",
          "bg-white/90 dark:bg-zinc-900/90 backdrop-blur-2xl border border-black/10 dark:border-white/15",
          "flex flex-col gap-0.5 animate-in fade-in zoom-in-95 duration-150"
        )}>
          <NavLink
            to="/downloads"
            onClick={() => setIsOpen(false)}
            className={({ isActive }) => cn(
              "flex min-h-[44px] items-center justify-center gap-3.5 rounded-xl px-4 py-[10px] text-sm font-medium transition-colors",
              isActive 
                ? "text-primary bg-primary/10" 
                : "text-foreground hover:bg-black/5 dark:hover:bg-white/10"
            )}
          >
            <HavenIcon symbol="download" size={16} className="shrink-0" />
            下载
          </NavLink>
          <NavLink
            to="/settings"
            onClick={() => setIsOpen(false)}
            className={({ isActive }) => cn(
              "flex min-h-[44px] items-center justify-center gap-3.5 rounded-xl px-4 py-[10px] text-sm font-medium transition-colors",
              isActive 
                ? "text-primary bg-primary/10" 
                : "text-foreground hover:bg-black/5 dark:hover:bg-white/10"
            )}
          >
            <HavenIcon symbol="settings" size={16} className="shrink-0" />
            设置
          </NavLink>
        </div>
      )}
    </div>
  )
}

function SearchActionItem() {
  const navigate = useNavigate()
  const location = useLocation()
  const isSearchActive = location.pathname === "/search"

  // 全局 Ctrl+K / Cmd+K 打开搜索
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault()
        navigate("/search")
      }
    }
    window.addEventListener("keydown", handleKeyDown)
    return () => window.removeEventListener("keydown", handleKeyDown)
  }, [navigate])

  return (
    <button
      type="button"
      onClick={() => navigate("/search")}
      title="全局搜索 (Ctrl+K)"
      aria-label="搜索页"
      className={cn(
        "w-[60px] h-[60px] rounded-full flex items-center justify-center transition-all duration-300 cursor-pointer shrink-0",
        "bg-white/85 dark:bg-zinc-950/85 backdrop-blur-2xl",
        "shadow-[0_8px_24px_-4px_rgba(0,0,0,0.15)] dark:shadow-[0_8px_24px_-4px_rgba(0,0,0,0.6)]",
        "border outline-none focus-visible:ring-2 focus-visible:ring-primary focus-visible:ring-offset-2",
        isSearchActive
          ? "text-primary border-blue-500/40 dark:border-blue-400/40 scale-105"
          : "text-foreground border-black/10 dark:border-white/15 hover:text-primary hover:border-blue-500/40 hover:scale-105 active:scale-95"
      )}
    >
      <HavenIcon 
        symbol="search" 
        size={24} 
        weight={isSearchActive ? "emphasized" : "regular"}
        className="transition-transform duration-300 group-hover:scale-110"
      />
    </button>
  )
}
