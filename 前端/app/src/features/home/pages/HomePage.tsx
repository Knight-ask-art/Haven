import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react"
import type { CSSProperties } from "react"
import { Link } from "react-router"
import {
  ArrowUpRight,
  Moon,
  Sun,
} from "lucide-react"
import type { HomeDto } from "@/lib/ipc/generated/wire"
import { cn } from "@/lib/utils"
import { toHavenError } from "@/lib/ipc/errors"
import { MediaCard } from "@/components/ui/haven/MediaCard"
import type { MediaCardProps } from "@/components/ui/haven/MediaCard"
import { HavenMascot } from "../components/mascot"
import { continueItemToCard, getHomeProjection, workCardToMediaCard } from "../ipc/home-gateway"
import { HOME_WALLPAPER_SURFACE_CLASS, useSystemPreference } from "@/features/settings/lib/appearance-runtime"
import { getSettingsRuntimeSnapshot, publishSettingsRuntimeValue, subscribeSettingsRuntime } from "@/features/settings/lib/settings-runtime-state"
import { settingsGateway } from "@/features/settings/ipc/gateway"
import { appearanceGateway } from "@/features/settings/ipc/appearance-gateway"
import { layoutToHomeModulePlacements } from "@/features/settings/lib/home-layout"
import type { HomeModulePlacement, HomeModuleSetting, HomeModuleSize } from "@/features/settings/lib/home-layout"

const navLinks = [
  { label: "首页", to: "/" },
  { label: "媒体库", to: "/library" },
  { label: "搜索", to: "/search" },
  { label: "足迹", to: "/footprints" },
  { label: "下载", to: "/downloads" },
]

/**
 * 首页模块区的状态。
 *
 * `modules` 是**已保存布局**里可见的模块（顺序、档位都来自后端），`home` 是 home_get
 * 的真实投影。两者必须一起到位才能渲染：只有布局没有内容会得到一排空壳，只有内容
 * 没有布局则会把用户隐藏掉的模块重新显示出来。
 */
type HomeModulesState =
  | { status: "loading" }
  | { status: "ready"; modules: HomeModulePlacement[]; home: HomeDto }
  | { status: "error"; message: string }

type HomeModulePositionStyle = CSSProperties & {
  "--haven-home-module-row": number
  "--haven-home-module-column": number
}

/** 档位 → 模块在 4 列网格里占几列（与 settings-wire 的 homeModuleColumnSpan 同源）。 */
const MODULE_SPAN_CLASS: Record<HomeModuleSize, string> = {
  small: "lg:col-span-1",
  medium: "lg:col-span-2",
  large: "lg:col-span-4",
}

/** 模块内部的卡片网格：窄模块一行少放几张，宽模块铺开到六列。 */
const MODULE_GRID_CLASS: Record<HomeModuleSize, string> = {
  small: "grid-cols-1 sm:grid-cols-2",
  medium: "grid-cols-2 sm:grid-cols-3",
  large: "grid-cols-2 sm:grid-cols-3 lg:grid-cols-6",
}

const MODULE_TITLES: Record<HomeModuleSetting["module"], string> = {
  continue: "继续",
  recently_added: "最近添加",
  "shelf-favorites": "收藏",
}

/**
 * 首页面板的外观层：底走 index.css 的外观运行时 token，前景直接复用 `--foreground`。
 *
 * 深色下**不能**用 Tailwind 的 `text-white/55` 这种「裸色 + /NN」写法去换 token：这些
 * token 的值是裸 `var(...)`，Tailwind 解析不出颜色就会把带修饰符的工具类整条丢掉。
 * 因此带 alpha 的取值都写进 token 本身（见 index.css 的「外观运行时面」）。
 *
 * `--haven-home-surface` 的默认取值与改动前写死的 `#f7f7f5` / `#171717` 逐字相同，所以
 * 默认外观不变；自定义主题下由 appearance-runtime 用调色板重写它，首页面板因此跟着
 * 自定义主题走（改动前它永远是一块写死的中性色）。同一屏的弱化/次级文字层次刻意不动，
 * 理由见 `HomeModules` 里的说明。
 */
const HOME_SURFACE_CLASS = "bg-[var(--haven-home-surface)]"
/** 面板前景：两套主题下都恰好等于 `--foreground`，直接复用既有 token。 */
const HOME_FOREGROUND_CLASS = "text-foreground"

/**
 * 首页模块区：读取真实投影与已保存布局，并按布局渲染。
 *
 * 加载 / 失败 / 空三种结果都显式呈现；任何模块没有内容时都不渲染空壳，也不会补一个
 * 假的占位卡片。用户把全部模块隐藏（显式空布局）时这里什么都不渲染——那是用户的选择，
 * 不是「没有数据」。
 */
function HomeModules({ isDark }: { isDark: boolean }) {
  const [state, setState] = useState<HomeModulesState>({ status: "loading" })
  const requestId = useRef(0)

  const load = useCallback(async (): Promise<void> => {
    const id = ++requestId.current
    setState({ status: "loading" })
    try {
      const [layout, home] = await Promise.all([
        appearanceGateway.homeLayoutGet(),
        getHomeProjection(),
      ])
      if (id !== requestId.current) return
      setState({
        status: "ready",
        modules: layoutToHomeModulePlacements(layout.layout),
        home,
      })
    } catch (error) {
      if (id !== requestId.current) return
      setState({ status: "error", message: toHavenError(error).message })
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  // 首页的弱化/次级文字刻意保持不变：这里是首页自己的一套四级文字层次
  // （`#6e6e73`/`#86868b`/`#3a3a3c` 与深色下的 `white/55`、`white/45`、`white/85`），
  // 调色板里没有与之等价的一组 token。把它们压成 `--muted-foreground` 会把这套层次
  // 抹平成两级，改动已确认的首页排版；只转换其中几级又会同屏混两套来源。因此这一轮
  // 只让**背景 / 前景 / 强调色**这三样跟随主题（见本文件顶部的 HOME_* 常量与 CTA）。
  const mutedText = isDark ? "text-white/55" : "text-[#6e6e73]"
  const headingText = isDark ? "text-white" : "text-[#1d1d1f]"
  const errorTone = isDark
    ? "border-white/15 bg-white/[0.04] text-white/70"
    : "border-black/[0.08] bg-white/60 text-[#7c5a2d]"

  if (state.status === "loading") {
    return (
      <section className="mx-auto w-full max-w-[1440px] px-6 pb-[96px] sm:px-10 lg:px-14" aria-label="首页模块" aria-busy="true">
        <p className={cn("text-[13px]", mutedText)} role="status">正在读取首页内容…</p>
      </section>
    )
  }

  if (state.status === "error") {
    return (
      <section className="mx-auto w-full max-w-[1440px] px-6 pb-[96px] sm:px-10 lg:px-14" aria-label="首页模块">
        <div role="alert" className={cn("flex flex-wrap items-center justify-between gap-3 rounded-[16px] border px-[16px] py-[12px] text-[13px]", errorTone)}>
          <span>首页内容暂时不可用：{state.message}</span>
          <button type="button" onClick={() => void load()} className="shrink-0 rounded-full border border-current px-[12px] py-[4px] text-[12px] font-semibold">重试</button>
        </div>
      </section>
    )
  }

  const { modules, home } = state
  // 显式空布局：用户隐藏了全部模块，这里保持安静。
  if (modules.length === 0) return null

  // Continue 卡片需要 WorkCard 才能显示标题与封面；投影里可用的作品卡来自
  // 最近添加与内容架预览。找不到作品卡的条目不会渲染成一个「无名」卡片。
  const cardPool = [...home.recentlyAdded, ...home.shelves.flatMap((shelf) => shelf.preview)]
  const continueCards = home.continueItems
    .map((item) => continueItemToCard(item, cardPool))
    .filter((card): card is MediaCardProps => card !== null)
  const favoriteShelf = home.shelves.find((shelf) => shelf.shelfId === "shelf-favorites")
  const favoriteCards = (favoriteShelf?.preview ?? []).map(workCardToMediaCard)

  const moduleItems: Record<HomeModuleSetting["module"], MediaCardProps[]> = {
    continue: continueCards,
    recently_added: home.recentlyAdded.map(workCardToMediaCard),
    "shelf-favorites": favoriteCards,
  }
  const visibleModules = modules.filter((module) => moduleItems[module.module].length > 0)

  return (
    <section
      className="mx-auto w-full max-w-[1440px] px-6 pb-[96px] sm:px-10 lg:px-14"
      aria-label="首页模块"
    >
      {visibleModules.length === 0 ? (
        <div className={cn("rounded-[20px] border border-dashed px-[20px] py-[28px] text-center", isDark ? "border-white/15" : "border-black/[0.12]")}>
          <p className={cn("text-[14px] font-semibold", headingText)}>暂无内容</p>
          <p className={cn("mt-[6px] text-[12px]", mutedText)}>已保存的布局里没有可显示的内容。首次使用时先扫描来源或添加本地目录；桌面端会在这里显示继续、最近添加与收藏。</p>
        </div>
      ) : (
        <div className="grid grid-cols-1 gap-[32px] lg:grid-cols-4">
          {visibleModules.map((module) => (
            <section
              key={module.module}
              data-module={module.module}
              data-module-size={module.size}
              data-module-row={module.row}
              data-module-column={module.column}
              style={{
                "--haven-home-module-row": module.row + 1,
                "--haven-home-module-column": module.column + 1,
              } as HomeModulePositionStyle}
              className={cn("haven-home-module-positioned min-w-0", MODULE_SPAN_CLASS[module.size])}
              aria-label={MODULE_TITLES[module.module]}
            >
              <h2 className={cn("text-[18px] font-bold tracking-[-0.02em]", headingText)}>{MODULE_TITLES[module.module]}</h2>
              <div className={cn("mt-[14px] grid gap-[16px]", MODULE_GRID_CLASS[module.size])}>
                {moduleItems[module.module].map((card) => (
                  <MediaCard key={card.id} {...card} />
                ))}
              </div>
            </section>
          ))}
        </div>
      )}
    </section>
  )
}

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
          <div className="max-w-4xl">
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

            <div className="mt-[48px] grid max-w-3xl gap-[32px] sm:grid-cols-[1fr_auto] sm:items-end">
              <p className={cn(
                "max-w-xl text-xl font-medium leading-9 tracking-[-0.02em] sm:text-2xl",
                isDark ? "text-white/85" : "text-[#3a3a3c]"
              )}>
                让所有故事，<br className="sm:hidden" />在一个地方继续。
              </p>
              <div className={cn(
                "border-l pl-5 text-xs leading-6",
                isDark ? "border-white/20 text-white/45" : "border-black/15 text-[#86868b]"
              )}>
                <p className="font-haven-salt text-[10px] tracking-[0.08em]">KEEP WHAT MATTERS.</p>
                <p>RETURN WHENEVER YOU WANT.</p>
                <p className="mt-1 font-semibold tracking-[0.18em]">栖阅 / 2026</p>
              </div>
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

        {/* 真实首页内容：已保存布局里的模块 + home_get 投影。欢迎区不受它影响，
            模块区按顺序与档位排在欢迎区下方；加载/失败/空各有明确呈现。 */}
        <HomeModules isDark={isDark} />

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
