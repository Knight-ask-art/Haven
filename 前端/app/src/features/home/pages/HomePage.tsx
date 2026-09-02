import { useState } from "react"
import { Link } from "react-router"
import {
  ArrowUpRight,
  Languages,
  Moon,
  Rss,
  Sun,
} from "lucide-react"
import { cn } from "@/lib/utils"
import { HavenMascot } from "../components/mascot"

function GithubIcon({ size = 16, className }: { size?: number; className?: string }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="currentColor"
      className={className}
      aria-hidden="true"
    >
      <path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12" />
    </svg>
  )
}

// Replace this placeholder with the Haven repository when it is public.

const navLinks = [
  { label: "首页", to: "/" },
  { label: "媒体库", to: "/library" },
  { label: "搜索", to: "/search" },
  { label: "足迹", to: "/footprints" },
  { label: "下载", to: "/downloads" },
]

export function HomePage() {
  const [isDark, setIsDark] = useState(false)

  return (
    <div className={cn(
      "relative flex min-h-full flex-col overflow-hidden transition-colors duration-500",
      isDark ? "bg-[#171717] text-[#f5f5f7]" : "bg-[#f7f7f5] text-[#1d1d1f]"
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
        className="sr-only focus:not-sr-only focus:fixed focus:left-5 focus:top-5 focus:z-50 focus:rounded-full focus:bg-[#007aff] focus:px-[16px] focus:py-[8px] focus:text-sm focus:font-semibold focus:text-white"
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
          {/* 生产版本无外部仓库可跳转：占位链接按死入口治理改为静态徽标，避免"可点击无效果" */}
          <span
            title="GitHub 仓库地址待配置"
            className={cn(
              "hidden cursor-default items-center gap-1.5 rounded-full px-3 py-[8px] text-xs font-semibold sm:flex",
              isDark ? "text-white/60" : "text-[#6e6e73]"
            )}
          >
            <GithubIcon size={16} className="shrink-0" />
            <span>GitHub</span>
            <ArrowUpRight size={14} className="shrink-0" />
          </span>
          <button
            type="button"
            onClick={() => setIsDark((current) => !current)}
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
              className="group inline-flex items-center gap-[8px] rounded-full bg-[#007aff] px-5 py-3 text-sm font-semibold text-white shadow-[0_8px_22px_rgba(0,122,255,0.2)] transition-all hover:-translate-y-0.5 hover:bg-[#006fe6]"
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

        <section className={cn(
          "flex flex-col gap-6 border-t pt-6 sm:flex-row sm:items-center sm:justify-between",
          isDark ? "border-white/10" : "border-black/[0.1]"
        )} aria-label="栖阅链接">
          <div className="flex items-center gap-[8px]">
            <span
              title="GitHub 仓库地址待配置"
              className={cn(
                "inline-flex cursor-default items-center gap-[8px] rounded-full px-3 py-[8px] text-xs font-semibold",
                isDark ? "text-white/60" : "text-[#6e6e73]"
              )}
            >
              <GithubIcon size={16} className="shrink-0" />
              <span>GitHub</span>
            </span>
            <span
              title="多语言界面将在后续版本提供"
              className={cn(
                "inline-flex cursor-default items-center gap-[8px] rounded-full px-3 py-[8px] text-xs font-semibold",
                isDark ? "text-white/60" : "text-[#6e6e73]"
              )}
            >
              <Languages size={16} className="shrink-0" />
              <span>中文</span>
            </span>
            <span
              title="RSS 订阅将在后续版本提供"
              className={cn(
                "inline-flex cursor-default items-center gap-[8px] rounded-full px-3 py-[8px] text-xs font-semibold",
                isDark ? "text-white/60" : "text-[#6e6e73]"
              )}
            >
              <Rss size={16} className="shrink-0" />
              <span>RSS</span>
            </span>
          </div>
          <p className={cn("text-xs", isDark ? "text-white/35" : "text-[#86868b]")}>© 2026 栖阅 Haven. 个人内容空间。</p>
        </section>
      </main>

    </div>
  )
}
