import { useCallback, useEffect, useRef } from "react"
import { Outlet, useLocation } from "react-router"
import { HavenBottomNav } from "@/components/ui/navigation/HavenBottomNav"
import {
  reportHomeWallpaperUnavailable,
  useAppearanceWallpaper,
  useHomeWallpaperFailure,
  useSystemPreference,
  useWallpaperRetryEpoch,
} from "@/features/settings/lib/appearance-runtime"
import type { WallpaperLayer } from "@/features/settings/lib/appearance-runtime"
import { useSettingsRuntimeSnapshot } from "@/features/settings/lib/settings-runtime-state"
import { updaterGateway } from "@/features/settings/ipc/updater-gateway"
import { getHavenClientMode } from "@/lib/ipc/runtime"
import { useNotice } from "@/app/notice-center/notice-context"

/**
 * 壁纸媒体层：铺满 shell、不接收指针事件、不进无障碍树。
 *
 * `pointer-events-none` 让这一层永远不吃点击——它铺在交互内容下面，任何位置上的
 * 点击都照常落到内容或浮层 Dock 上。`-z-10` 与 AppShell 只在渲染壁纸时加的
 * `isolate` 一起构成显式的图层顺序：
 *
 *   shell 自身背景 < 壁纸（-z-10） < 页面内容（普通流 / z-auto） < 浮层 Dock（z-50）
 *
 * 壁纸因此只从「页面自身透明」的地方透出来，内容与 Dock 的交互和层叠都不变。
 */
const WALLPAPER_MEDIA_CLASS =
  "pointer-events-none absolute inset-0 -z-10 h-full w-full object-cover"

/**
 * 壁纸渲染层：只消费运行时已经解析过的受控 URI，不接收路径或任意 URL。
 *
 * 静帧用 `img`、动态用静音循环的 `video`。字节读不到（受控协议解析失败、文件已被
 * 移除）时回调让 AppShell 把这次投影收敛回「无壁纸」，而不是留一个永远加载不出来
 * 的元素。
 */
export function AppearanceWallpaperLayer({
  layer,
  onUnavailable,
}: {
  layer: WallpaperLayer
  onUnavailable: () => void
}) {
  if (layer.kind === "static") {
    return (
      <img
        src={layer.uri}
        alt=""
        aria-hidden="true"
        draggable={false}
        onError={onUnavailable}
        className={WALLPAPER_MEDIA_CLASS}
      />
    )
  }
  return (
    <video
      src={layer.uri}
      aria-hidden="true"
      autoPlay
      muted
      loop
      playsInline
      onError={onUnavailable}
      className={WALLPAPER_MEDIA_CLASS}
    />
  )
}

export function AppShell() {
  const location = useLocation()
  // 壳层只**读**运行时投影：读盘、外观投影与首屏跳转都在根布局（AppRoot）里做一次。
  // 壳层自己不再挂外观投影——它是被导航切走的那一层，投影挂在它身上就会在进入
  // player / reader / comic / article 时被一起清掉。
  const snapshot = useSettingsRuntimeSnapshot()
  const { push } = useNotice()
  const updateCheckStarted = useRef(false)
  const appearance = snapshot.appearance
  // 减少动效有两个来源（设置项与系统偏好），在这里合成同一份事实再往下发：
  // shell 的动效类名、data 状态和动态壁纸的抑制用的是同一个值，不会各判各的。
  const systemReducedMotion = useSystemPreference("(prefers-reduced-motion: reduce)")
  const reducedMotion = appearance.reduceMotion || systemReducedMotion
  const wallpaper = useAppearanceWallpaper(appearance, { reducedMotion })
  // 读不出来的壁纸按「哪一份受控 URI」记账，而不是记一个布尔开关：布尔开关要靠
  // effect 才能在换 URI 之后复位，中间会多出一帧「新壁纸被上一次失败挡住」；
  // 记 URI 则让换一份 URI 天然就是一次新投影，而同一份读不出来的 URI 保持收敛。
  //
  // 记账里还带一代重试代数（外观设置页的「重试预览」）：用户对同一份读不出来的资产
  // 按下重试时，这一代失败作废、同一份受控 URI 重新挂载并重新请求字节——不需要重启，
  // 也不需要改一份已经保存的设置。
  const wallpaperRetryEpoch = useWallpaperRetryEpoch()
  const failedWallpaper = useHomeWallpaperFailure()
  const wallpaperUri = wallpaper?.uri ?? null
  const activeWallpaper = useRef({ uri: wallpaperUri, epoch: wallpaperRetryEpoch })
  activeWallpaper.current = { uri: wallpaperUri, epoch: wallpaperRetryEpoch }
  const wallpaperFailed =
    wallpaperUri !== null &&
    failedWallpaper !== null &&
    failedWallpaper.uri === wallpaperUri &&
    failedWallpaper.epoch === wallpaperRetryEpoch
  const wallpaperLayer = wallpaperFailed ? null : wallpaper
  const dismissWallpaper = useCallback(() => {
    const active = activeWallpaper.current
    if (
      wallpaperUri !== null &&
      active.uri === wallpaperUri &&
      active.epoch === wallpaperRetryEpoch
    ) {
      reportHomeWallpaperUnavailable(wallpaperUri, wallpaperRetryEpoch)
    }
  }, [wallpaperUri, wallpaperRetryEpoch])
  const isDetailPage = location.pathname.startsWith("/media/")
  const isHomePage = location.pathname === "/"
  // 外观合同里壁纸是「首页壁纸」：只在根路由渲染，避免在搜索页等半透明页面上透出
  // 并非该页设置的背景。图层与它需要的 `isolate` 同源，因此没有壁纸时 shell 的
  // 层叠上下文、背景和内容层级都保持原样。
  const homeWallpaper = isHomePage ? wallpaperLayer : null
  const shellStackClass = homeWallpaper ? "isolate" : ""
  const isImmersivePage = ["/reader/", "/comic/", "/article/"].some((prefix) => location.pathname.startsWith(prefix))
  const isSettingsPage = location.pathname.startsWith("/settings")
  const isAIWorkbenchPreview = location.pathname.startsWith("/dev/ai-workbench-preview")
  // 设置页也保留全局透明 Dock，用户可以随时回到首页、媒体库、足迹或更多菜单。
  // 首页、详情页和沉浸式阅读器继续由页面自管导航，避免出现两个底部导航。
  const hasBottomNav = !isDetailPage && !isHomePage && !isImmersivePage
  const shellDensityClass = appearance.density === "compact" ? "pb-[80px]" : "pb-[96px]"
  // 设置页与 AI 工作台在各自的全屏画布内部避开 Dock；其他壳层页面统一在主内容区
  // 预留底部空间。Dock 始终由 AppShell 全局渲染，不嵌入具体页面。
  const mainBottomInsetClass = hasBottomNav && !isSettingsPage && !isAIWorkbenchPreview ? shellDensityClass : ""
  // 当前桌面壳使用浮动 Dock 而非独立左侧栏；这里把 sidebar 偏好投影为
  // 内容壳层的导航留白，避免给不存在的侧栏状态制造假视觉。
  const shellSidebarClass = appearance.sidebar === "expanded"
    ? "lg:pl-[16px]"
    : appearance.sidebar === "collapsed"
      ? "lg:pl-[4px]"
      : ""
  const shellMotionClass = reducedMotion
    ? "[&_*]:transition-none [&_*]:duration-0 [&_*]:animate-none"
    : ""
  // 壁纸可见性契约的一半：只有真的渲染出背景层时才是 "true"，index.css 据此让首页
  // 面板（HOME_WALLPAPER_SURFACE_CLASS）交出背景色；读不出来的壁纸同样是 "false"，
  // 首页因此保持原有的不透明外观。
  const wallpaperPresence = homeWallpaper ? "true" : "false"

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
      data-haven-reduce-motion={reducedMotion ? "true" : "false"}
      data-haven-wallpaper={wallpaperPresence}
      className={`relative ${shellStackClass} flex h-screen min-h-0 flex-col overflow-hidden bg-background text-foreground ${shellMotionClass}`}
    >
      {/* 首页壁纸铺在最底层（图层顺序见 WALLPAPER_MEDIA_CLASS）：内容与浮层 Dock
          都压在它之上，媒体层自己不吃指针事件，因此不会抢走任何点击。 */}
      {homeWallpaper && (
        <AppearanceWallpaperLayer layer={homeWallpaper} onUnavailable={dismissWallpaper} />
      )}

      {/* Main Content Area with padding for bottom nav when present */}
      <main className={`min-h-0 flex-1 haven-scroll ${isSettingsPage ? 'overflow-hidden' : 'overflow-y-auto'} ${mainBottomInsetClass} ${shellSidebarClass}`}>
        <Outlet />
      </main>

      {/* Floating Bottom Center Navigation - Hidden on Detail Pages */}
      {hasBottomNav && <HavenBottomNav />}
    </div>
  )
}
