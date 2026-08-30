import { NavLink } from "react-router"
import { HavenIcon } from "@/components/ui/haven/HavenIcon"

import { cn } from "@/lib/utils"

export function HavenSidebar() {
  return (
    <aside className={cn(
      "w-sidebar-expanded h-screen flex flex-col border-r border-border bg-sidebar",
      "backdrop-blur-xl bg-sidebar/70" // True Apple-style Liquid Glass effect
    )}>
      {/* Brand / Window Drag Handle Area */}
      <div className="h-14 flex items-center px-[16px] drag-region">
        <div className="flex items-center gap-[8px] font-semibold text-lg text-sidebar-foreground">
          <HavenIcon symbol="library-big" size={20} className="text-primary" />
          <span> Haven</span>
        </div>
      </div>

      {/* Primary Navigation */}
      <nav className="flex-1 px-3 py-[8px] space-y-1">
        <div className="flex flex-col gap-1 mt-[8px]">
          <NavItem to="/" icon="library" label="资料库" />
          <NavItem to="/search" icon="search" label="搜索" />
          <NavItem to="/downloads" icon="download" label="下载" />
        </div>
      </nav>

      {/* Secondary / Bottom */}
      <div className="flex-none p-3 pb-[16px]">
        <NavItem to="/settings" icon="settings" label="设置" />
      </div>
    </aside>
  )
}

function NavItem({ 
  to, 
  icon, 
  label 
}: { 
  to: string
  icon: string
  label: string 
}) {
  return (
    <NavLink
      to={to}
      className={({ isActive }) => cn(
        "flex items-center gap-icon-text h-10 px-3 rounded-lg text-sm transition-colors",
        "outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-sidebar",
        isActive 
          ? "bg-sidebar-accent text-sidebar-accent-foreground font-medium" 
          : "text-sidebar-foreground hover:bg-sidebar-accent/50"
      )}
    >
      <HavenIcon symbol={icon} role="navigation" />
      <span>{label}</span>
    </NavLink>
  )
}
