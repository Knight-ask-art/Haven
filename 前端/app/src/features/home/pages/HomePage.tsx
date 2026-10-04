import { useState, useSyncExternalStore } from "react"
import { Link } from "react-router"
import {
  ArrowUpRight,
  Moon,
  Sun,
} from "lucide-react"
import { cn } from "@/lib/utils"
import { HavenMascot } from "../components/mascot"
import { HOME_WALLPAPER_SURFACE_CLASS, useSystemPreference } from "@/features/settings/lib/appearance-runtime"
import { getSettingsRuntimeSnapshot, publishSettingsRuntimeValue, subscribeSettingsRuntime } from "@/features/settings/lib/settings-runtime-state"
import { settingsGateway } from "@/features/settings/ipc/gateway"

const navLinks = [
  { label: "首页", to: "/" },
  { label: "媒体库", to: "/library" },
  { label: "搜索", to: "/search" },
  { label: "足迹", to: "/footprints" },
  { label: "下载", to: "/downloads" },
]

/**
 * 首页面板的外观层：底走 index.css 的外观运行时 token，前景直接复用 `--foreground`。
 *
 * 深色下**不能**用 Tailwind 的 `text-white/55` 这种「裸色 + /NN」写法去换 token：这些
 * token 的值是裸 `var(...)`，Tailwind 解析不出颜色就会把带修饰符的工具类整条丢掉。
 * 因此带 alpha 的取值都写进 token 本身（见 index.css 的「外观运行时面」）。
 *
 * `--haven-home-surface` 的默认取值与改动前写死的 `#f7f7f5` / `#171717` 逐字相同，所以
 * 默认外观不变；自定义主题下由 appearance-runtime 用调色板重写它，首页因此跟着
 * 自定义主题走。同一屏的弱化/次级文字保留原有层次。
 */
const HOME_SURFACE_CLASS = "bg-[var(--haven-home-surface)]"
/** 面板前景：两套主题下都恰好等于 `--foreground`，直接复用既有 token。 */
const HOME_FOREGROUND_CLASS = "text-foreground"

export function HomePage() {
  const settings = useSyncExternalStore(subscribeSettingsRuntime, getSettingsRuntimeSnapshot, getSettingsRuntimeSnapshot)
  const [isUpdatingTheme, setIsUpdatingTheme] = useState(false)
  // 与 settings-runtime-state 的 applyTheme 用同一份判断：`system` 与 `custom` 都跟随
  // 系统明暗（自定义主题自带浅色/深色两套调色板），所以这里也必须跟随，否则首页会在
  // 系统深色下继续用浅色文案与浅色背景。
  const prefersDark = useSystemPreference("(prefers-color-scheme: dark)")
  const theme = settings.appearance.theme
  const isDark = theme === "dark" || ((theme === "system" || theme === "custom") && prefersDark)

  const toggleTheme = () => {
    if (isUpdatingTheme) return
    const nextTheme = isDark ? "light" : "dark"
    setIsUpdatingTheme(true)
    void settingsGateway.settingsGet("appearance")
      .then((current) => settingsGateway.settingsUpdate({
        section: "appearance",
        expectedRevision: current.revision,
        patch: { section: "appearance", theme: nextTheme },
      }))
      .then((result) => publishSettingsRuntimeValue(result.value))
      .catch(() => undefined)
      .finally(() => setIsUpdatingTheme(false))
  }

  return (
    <div className={cn(
      "relative flex min-h-full flex-col overflow-hidden transition-colors duration-500",
      // 选中首页壁纸时这块不透明背景要让开，壁纸才透得出来：AppShell 把媒体层挂在
      // shell 的 -z-10 上，本层的底色是唯一压住它的东西。规则见 index.css 的
      // [data-haven-wallpaper="true"] .haven-wallpaper-surface ——没有壁纸时属性是
      // "false"，规则不生效，首页维持原样。
      HOME_WALLPAPER_SURFACE_CLASS,
      // 底与前景走外观 token：浅色 `#f7f7f5` / 深色 `#171717`，前景两套都等于
      // `--foreground`，因此默认外观与改动前逐字一致，自定义主题下则由调色板接管。
      HOME_SURFACE_CLASS,
      HOME_FOREGROUND_CLASS
    )}>
      {/* 云朵伴读背景层：居中呼吸云 */}
      <div className="pointer-events-none absolute inset-0 overflow-hidden">
        {/* z-20 提到 main(z-10) 之上，保证空白区域点击能落到云朵上 */}
        <div className="absolute left-1/2 top-[42%] z-20 aspect-square w-[clamp(280px,26vw,420px)] -translate-x-1/2 -translate-y-1/2">
          <div className="h-full w-full animate-cloud-breathe motion-reduce:animate-none">
            <HavenMascot isDark={isDark} className="h-full w-full" />
          </div>
        </div>
      </div>

      <a
        href="#main-content"
        className="sr-only focus:not-sr-only focus:fixed focus:left-5 focus:top-5 focus:z-50 focus:rounded-full focus:bg-primary focus:px-[16px] focus:py-[8px] focus:text-sm focus:font-semibold focus:text-primary-foreground"
      >
        跳转到主要内容
      </a>

      <header className="relative z-10 mx-auto flex w-full max-w-[1440px] shrink-0 items-center justify-between gap-6 px-6 py-7 sm:px-10 lg:px-14">
        <Link to="/" className="group flex items-center" aria-label="栖阅首页">
          <span className="flex h-[64px] w-[64px] shrink-0 items-center justify-center overflow-hidden rounded-2xl border border-black/[0.07] bg-white shadow-sm">
            <img
              src="/logo.png"
              alt="栖阅 Logo"
              className="h-full w-full object-contain"
            />
          </span>
        </Link>

        <nav className="hidden items-center gap-7 md:flex" aria-label="首页导航">
          {navLinks.map((link) => (
            <Link
              key={link.to}
              to={link.to}
              className={cn(
                "text-[13px] font-medium transition-colors",
                isDark ? "text-white/55 hover:text-white" : "text-[#6e6e73] hover:text-[#1d1d1f]"
              )}
            >
              {link.label}
            </Link>
          ))}
        </nav>

        <div className="flex items-center gap-[8px]">
          <button
            type="button"
            onClick={toggleTheme}
            disabled={isUpdatingTheme}
            aria-label={isDark ? "切换浅色主题" : "切换深色主题"}
            className={cn(
              "flex h-9 w-9 items-center justify-center rounded-full transition-colors",
              isDark ? "text-white/60 hover:bg-white/10 hover:text-white" : "text-[#6e6e73] hover:bg-black/[0.05] hover:text-[#1d1d1f]"
            )}
          >
            {isDark ? <Sun size={18} className="shrink-0" /> : <Moon size={18} className="shrink-0" />}
          </button>
        </div>
      </header>

      <main id="main-content" className="relative z-10 mx-auto flex w-full max-w-[1440px] flex-1 flex-col px-6 pb-10 sm:px-10 lg:px-14">
        <section className="flex flex-1 flex-col justify-center py-[24px] sm:py-[32px]">
          <div className="w-full">
            <p className={cn(
              "max-w-xl text-sm leading-7 sm:text-base",
              isDark ? "text-white/55" : "text-[#6e6e73]"
            )}>
              收藏影视、图书、漫画与资料，也收藏那些值得再次回到的时刻。
            </p>

            <div className="mt-10 flex flex-col">
              <span className={cn(
                "text-[clamp(3.5rem,16vh,10rem)] font-semibold leading-[0.82] tracking-[-0.09em]",
                isDark ? "text-white" : "text-[#1d1d1f]"
              )}>
                栖阅
              </span>
              <span className={cn(
                "mt-7 text-[clamp(1rem,2vw,1.45rem)] font-medium uppercase tracking-[0.42em]",
                isDark ? "text-white/45" : "text-[#86868b]"
              )}>
                <span className="font-haven-marker">H A V E N</span>
              </span>
            </div>

            <div className="mt-[48px] grid w-full gap-[32px] lg:grid-cols-[minmax(0,1fr)_auto] lg:items-end">
              <p className={cn(
                "max-w-xl text-xl font-medium leading-9 tracking-[-0.02em] sm:text-2xl",
                isDark ? "text-white/85" : "text-[#3a3a3c]"
              )}>
                让所有故事，<br className="sm:hidden" />在一个地方继续。
              </p>
              <aside aria-label="首页寄语" className={cn(
                "justify-self-start border-l pl-5 text-xs leading-6 lg:justify-self-end",
                isDark ? "border-white/20 text-white/45" : "border-black/15 text-[#86868b]"
              )}>
                <p className="font-haven-salt text-[10px] tracking-[0.08em]">KEEP WHAT MATTERS.</p>
                <p>RETURN WHENEVER YOU WANT.</p>
                <p className="mt-1 font-semibold tracking-[0.18em]">栖阅 / 2026</p>
              </aside>
            </div>
          </div>

          <div className="mt-[40px] flex flex-wrap items-center gap-3 sm:mt-[48px]">
            <Link
              to="/library"
              // 主按钮走主题的 `--primary`（自定义强调色同时驱动它）：写死蓝色会让首页
              // 成为唯一不跟随强调色的一块。悬停用中性亮度变化而不是第二个蓝色——
              // `--ds-color-action-hover` 不跟随强调色，写死它会在自定义主题下漏出蓝边。
              className="group inline-flex items-center gap-[8px] rounded-full bg-primary px-5 py-3 text-sm font-semibold text-primary-foreground shadow-[0_8px_22px_rgba(0,122,255,0.2)] transition-all hover:-translate-y-0.5 hover:brightness-90"
            >
              <span>进入媒体库</span>
              <ArrowUpRight size={16} className="shrink-0 transition-transform group-hover:translate-x-0.5 group-hover:-translate-y-0.5" />
            </Link>
            <Link
              to="/search"
              className={cn(
                "inline-flex items-center rounded-full px-5 py-3 text-sm font-semibold transition-colors",
                isDark ? "text-white/65 hover:bg-white/10 hover:text-white" : "text-[#6e6e73] hover:bg-black/[0.05] hover:text-[#1d1d1f]"
              )}
            >
              搜索一部作品
            </Link>
          </div>
        </section>

        {/* 欢迎区下方只保留留白，不渲染内容模块或占位面板。 */}
        <div aria-hidden="true" className="h-[96px] shrink-0" />

        <section className={cn(
          "flex flex-col gap-6 border-t pt-6 sm:flex-row sm:items-center sm:justify-end",
          isDark ? "border-white/10" : "border-black/[0.1]"
        )} aria-label="栖阅信息">
          <p className={cn("text-xs", isDark ? "text-white/35" : "text-[#86868b]")}>© 2026 栖阅 Haven. 个人内容空间。</p>
        </section>
      </main>

    </div>
  )
}
