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

/**
 * 浮动 Dock 的**材质面**：半透明底、描边与静止态文字都走 index.css 的外观运行时 token
 * （`--haven-floating-*`），不写死颜色。
 *
 * 为什么不是 `bg-background/86`：这几个 token 的值是裸 `var(...)`，Tailwind 解析不出
 * 颜色就会把带 `/NN` 修饰符的工具类整条丢掉（生成不出任何 CSS）。alpha 因此像
 * `--border` 那样写进取值本身，token 名在这里只负责「哪一层材质」这件事。
 *
 * 这些 token 在 `:root` / `.dark` 里各有一套默认取值（与改动前写死的那串颜色逐字相同），
 * 因此在 system / light / dark 下渲染不变；用户启用自定义主题时由 appearance-runtime
 * 用调色板重写它们，浮动 Dock 于是跟着自定义主题走——改动前它是整屏唯一不跟随主题的
 * 一块。`backdrop-blur` 与投影阴影不属于调色板，保持原样。
 */
const DOCK_SURFACE_CLASS = "bg-[var(--haven-floating-surface)]"
const DOCK_RAISED_SURFACE_CLASS = "bg-[var(--haven-floating-surface-raised)]"
const DOCK_BORDER_CLASS = "border-[var(--haven-floating-border)]"
const DOCK_IDLE_TEXT_CLASS = "text-[var(--haven-floating-idle)]"
const DOCK_IDLE_HOVER_CLASS = "hover:bg-[var(--haven-floating-idle-hover)] hover:text-foreground"

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
      className="fixed bottom-[10px] left-1/2 z-50 flex -translate-x-1/2 select-none items-center gap-[32px]"
    >
      {/* 居中 Dock 悬浮主体导航条 */}
      <div className={cn(
        "box-border h-[72px] w-[365.1px] px-[29.55px] flex items-center gap-[16px] rounded-[36px]",
        DOCK_SURFACE_CLASS, "backdrop-blur-2xl",
        `border ${DOCK_BORDER_CLASS}`,
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
        "relative h-[56px] w-[64px] shrink-0 rounded-2xl px-[8px] flex flex-col items-center justify-center gap-1 transition-all duration-200",
        "outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1",
        // 活跃态走主题的 `--primary`，因此自定义强调色（accentColor 同时驱动
        // `--primary`）在这里生效；写死蓝色会让 Dock 成为唯一不跟随主题的一块。
        // 静止态走 Dock 材质自己的弱化文字 token，悬停时才升到 `--foreground`。
        isActive
          ? "text-primary"
          : cn(DOCK_IDLE_TEXT_CLASS, DOCK_IDLE_HOVER_CLASS)
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
            "font-normal"
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
    <div className="relative flex h-[56px] w-[64px] shrink-0 items-center" ref={menuRef}>
      <button
        onClick={() => setIsOpen(!isOpen)}
        className={cn(
          "relative h-full w-[64px] px-[8px] rounded-2xl flex flex-col items-center justify-center gap-1 transition-all duration-200",
          "outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1",
          // 与主导航同一套材质 token：自定义主题对「更多」同样生效。
          isActive
            ? "text-primary"
            : cn(DOCK_IDLE_TEXT_CLASS, DOCK_IDLE_HOVER_CLASS),
          isOpen && !isActive && "text-foreground bg-[var(--haven-floating-idle-hover)]"
        )}
      >
        <div className={cn("flex items-center justify-center h-[24px] mb-0.5 transition-transform duration-300", (isActive || isOpen) && "scale-110")}>
          <Ellipsis className="w-[22px] h-[22px]" />
        </div>
        <span className={cn(
          "text-[11px] leading-none whitespace-nowrap transition-all duration-200",
          "font-normal"
        )}>更多</span>
      </button>

      {isOpen && (
        <div className={cn(
          "absolute bottom-[calc(100%+20px)] left-1/2 -translate-x-1/2 w-[184px] p-2 rounded-2xl z-50 shadow-2xl",
          DOCK_RAISED_SURFACE_CLASS, `border ${DOCK_BORDER_CLASS}`,
          "backdrop-blur-2xl",
          "flex flex-col gap-0.5 animate-in fade-in zoom-in-95 duration-150"
        )}>
          <NavLink
            to="/downloads"
            onClick={() => setIsOpen(false)}
            className={({ isActive }) => cn(
              "flex min-h-[44px] items-center justify-center gap-3.5 rounded-xl px-4 py-[10px] text-sm font-medium transition-colors",
              isActive
                ? "text-primary"
                : "text-foreground hover:bg-[var(--haven-floating-idle-hover)]"
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
                ? "text-primary"
                : "text-foreground hover:bg-[var(--haven-floating-idle-hover)]"
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
  const isSettingsPage = location.pathname.startsWith("/settings")

  // 全局 Ctrl+K / Cmd+K 打开搜索
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      // 设置页把同一快捷键交给本页的设置导航搜索，避免跳出设置工作台。
      if (isSettingsPage) return
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault()
        navigate("/search")
      }
    }
    window.addEventListener("keydown", handleKeyDown)
    return () => window.removeEventListener("keydown", handleKeyDown)
  }, [isSettingsPage, navigate])

  return (
    <button
      type="button"
      onClick={() => navigate("/search")}
      title="全局搜索 (Ctrl+K)"
      aria-label="搜索页"
      className={cn(
        "h-[60px] w-[60px] shrink-0 cursor-pointer rounded-full flex items-center justify-center transition-all duration-300",
        DOCK_SURFACE_CLASS, "backdrop-blur-2xl",
        // 与 Dock 主体同一套材质 token：搜索按钮不是第二块不跟随主题的浮层。
        `border ${DOCK_BORDER_CLASS}`,
        "shadow-[0_8px_24px_-4px_rgba(0,0,0,0.15)] dark:shadow-[0_8px_24px_-4px_rgba(0,0,0,0.6)]",
        "outline-none focus-visible:ring-2 focus-visible:ring-primary focus-visible:ring-offset-2",
        isSearchActive
          ? "text-primary scale-105"
          : "text-foreground hover:text-primary hover:scale-105 active:scale-95"
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
