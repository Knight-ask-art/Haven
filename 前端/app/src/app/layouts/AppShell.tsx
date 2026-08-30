import { useEffect, useRef } from "react"
import { Outlet, useLocation } from "react-router"
import { HavenBottomNav } from "@/components/ui/navigation/HavenBottomNav"
import { useSettingsRuntime } from "@/features/settings/lib/settings-runtime-state"
import { updaterGateway } from "@/features/settings/ipc/updater-gateway"
import { getHavenClientMode } from "@/lib/ipc/runtime"
import { useNotice } from "@/app/notice-center/notice-context"

export function AppShell() {
  const location = useLocation()
  const { snapshot } = useSettingsRuntime()
  const { push } = useNotice()
  const updateCheckStarted = useRef(false)
  const appearance = snapshot.appearance
  const isDetailPage = location.pathname.startsWith("/media/")
  const isHomePage = location.pathname === "/"
  const isImmersivePage = ["/reader/", "/comic/", "/article/"].some((prefix) => location.pathname.startsWith(prefix))
  const isSettingsPage = location.pathname.startsWith("/settings")
  // 底部导航真实渲染条件；底部预留只在导航存在且页面非自管布局时生效
  // （设置页自带内滚壳、首页/详情页不渲染导航——此前首页被强加 96px 死 padding 导致必现滚动条）
  const hasBottomNav = !isDetailPage && !isHomePage && !isImmersivePage
  const shellDensityClass = appearance.density === "compact" ? "pb-[80px]" : "pb-[96px]"
  // 当前桌面壳使用浮动 Dock 而非独立左侧栏；这里把 sidebar 偏好投影为
  // 内容壳层的导航留白，避免给不存在的侧栏状态制造假视觉。
  const shellSidebarClass = appearance.sidebar === "expanded"
    ? "lg:pl-[16px]"
    : appearance.sidebar === "collapsed"
      ? "lg:pl-[4px]"
      : ""
  const shellMotionClass = appearance.reduceMotion
    ? "[&_*]:transition-none [&_*]:duration-0 [&_*]:animate-none"
    : ""

  useEffect(() => {
    if (getHavenClientMode() !== "tauri" || updateCheckStarted.current) return
    updateCheckStarted.current = true
    void updaterGateway.check().then((result) => {
      if (result.status !== "available") return
      push({
        kind: "info",
        title: "栖阅有新版本",
        message: `${result.availableVersion ?? "新版本"} 已准备好，可在设置 → 更新中安装。`,
        dedupeKey: "updater:available",
      })
    }).catch(() => {
      // 启动检查是非阻塞能力；详细错误和重试入口在设置页展示。
    })
  }, [push])

  return (
    <div
      data-haven-density={appearance.density}
      data-haven-sidebar={appearance.sidebar}
      data-haven-reduce-motion={appearance.reduceMotion ? "true" : "false"}
      className={`relative flex h-screen min-h-0 flex-col overflow-hidden bg-background text-foreground ${shellMotionClass}`}
    >
      {/* Main Content Area with padding for bottom nav when present */}
      <main className={`min-h-0 flex-1 haven-scroll ${isSettingsPage ? 'overflow-hidden' : 'overflow-y-auto'} ${hasBottomNav && !isSettingsPage ? shellDensityClass : ''} ${shellSidebarClass}`}>
        <Outlet />
      </main>

      {/* Floating Bottom Center Navigation - Hidden on Detail Pages */}
      {hasBottomNav && <HavenBottomNav />}
    </div>
  )
}
