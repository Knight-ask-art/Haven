import { Outlet } from "react-router"
import { useSettingsRuntime } from "@/features/settings/lib/settings-runtime-state"

/**
 * 应用根布局：外观运行时的挂载点。
 *
 * 为什么需要它：外观投影（自定义调色板 token、界面字体、明暗类名、密度 / 侧栏 /
 * 减少动效的 data 属性）由 `useSettingsRuntime` 装到 `document.documentElement` 上，
 * 它的清理函数跟着调用它的组件走。player / reader / comic / article 是 AppShell 的
 * **兄弟**路由，被挂进 AppShell 时，一进这些页面投影就被清理，自定义主题与字体在
 * 沉浸式阅读里消失。因此启动入口上移到覆盖全部主要路由的根布局，AppShell 只保留
 * 自己的导航、首页壁纸与壳层行为。
 *
 * 这里刻意不渲染任何 DOM：壳层的背景、层叠上下文与布局仍由 AppShell 提供，沉浸式
 * 路由也照旧不套壳层。根布局只负责「读设置、投影外观、首屏跳转一次」。
 */
export function AppRoot() {
  // 读盘 + 外观投影 + 首屏启动跳转都在这个 Hook 里；它只在这里被调用一次，
  // 下游组件用 useSettingsRuntimeSnapshot 读同一份内存投影，不重复跑启动路径。
  useSettingsRuntime()
  return <Outlet />
}
