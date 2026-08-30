import { useNavigate } from "react-router"
import { cn } from "@/lib/utils"
import { artworkSrcSet } from "@/lib/artwork-url"
import type { DefaultCoverCategory } from "@/lib/default-cover"
import { ArtworkImage } from "./ArtworkImage"

export type CardTheme = 
  | "default"
  | "holographic" 
  | "cyberpunk" 
  | "dark-fantasy" 
  | "shounen" 
  | "pixel-retro" 
  | "glassmorphism"

export interface MediaCardProps {
  id: string
  title: string
  subtitle?: string
  imageUrl: string
  layout?: "portrait" | "landscape"
  progress?: number // 0 to 100
  typeBadge?: string
  theme?: CardTheme
  className?: string
  onClick?: () => void
  description?: string
  releaseYear?: number
  /** Category used when the controlled artwork is missing or fails to load. */
  artworkCategory?: DefaultCoverCategory
}

export function MediaCard({ 
  id,
  title, 
  subtitle, 
  imageUrl, 
  layout = "portrait",
  progress, 
  typeBadge,
  theme = "default",
  className,
  onClick,
  artworkCategory,
}: MediaCardProps) {
  const navigate = useNavigate()
  const srcSet = (() => {
    if (!imageUrl) return undefined
    // haven 受控海报：补 w 参数占位，后端后续按 w 转码
    if (imageUrl.includes("haven-resource.artwork") || imageUrl.startsWith("haven://artwork/")) {
      return artworkSrcSet(imageUrl)
    }
    // 外链海报（如 unsplash/picsum）不加 srcSet，避免 CORS
    return undefined
  })()

  const handleClick = () => {
    if (onClick) {
      onClick()
    } else {
      navigate(`/work/${id}`)
    }
  }

  // --- 依据主题生成图片外框与遮罩层样式 ---
  const getCoverThemeStyles = () => {
    switch (theme) {
      case "holographic":
        return "border border-white/20 rounded-2xl relative overflow-hidden group-hover:shadow-[0_0_20px_rgba(124,58,237,0.5)] transition-all after:absolute after:inset-0 after:bg-[linear-gradient(125deg,rgba(255,255,255,0.3)_0%,rgba(255,255,255,0)_40%,rgba(255,255,255,0.1)_60%,rgba(255,255,255,0)_100%)] after:opacity-0 group-hover:after:opacity-100 after:transition-opacity after:duration-700"
      case "cyberpunk":
        return "border border-cyan-500/50 rounded-lg group-hover:border-pink-500 group-hover:shadow-[0_0_15px_rgba(236,72,153,0.6)] transition-all relative overflow-hidden before:absolute before:inset-0 before:bg-[linear-gradient(transparent_1px,rgba(0,0,0,0)_1px),linear-gradient(90deg,rgba(0,255,255,0.05)_1px,transparent_1px)] before:bg-[size:4px_4px]"
      case "dark-fantasy":
        return "border-2 border-[#8b6b3e]/40 rounded-sm group-hover:border-[#d4af37] group-hover:shadow-[0_0_20px_rgba(212,175,55,0.2)] transition-all relative after:absolute after:inset-[8px] after:border after:border-[#d4af37]/20 after:pointer-events-none"
      case "shounen":
        return "border-4 border-black dark:border-white rounded-none shadow-[4px_4px_0_0_rgba(0,0,0,1)] dark:shadow-[4px_4px_0_0_rgba(255,255,255,1)] group-hover:translate-x-[-2px] group-hover:translate-y-[-2px] group-hover:shadow-[6px_6px_0_0_rgba(0,0,0,1)] dark:group-hover:shadow-[6px_6px_0_0_rgba(255,255,255,1)] transition-all"
      case "pixel-retro":
        return "border-[3px] border-emerald-500 rounded-none shadow-[2px_2px_0_0_#10b981] group-hover:shadow-[4px_4px_0_0_#10b981] transition-all relative before:absolute before:inset-0 before:bg-[repeating-linear-gradient(0deg,transparent,transparent_2px,rgba(0,0,0,0.1)_2px,rgba(0,0,0,0.1)_4px)] before:pointer-events-none"
      case "glassmorphism":
        return "border border-white/10 dark:border-white/5 rounded-3xl bg-white/5 backdrop-blur-2xl shadow-[0_8px_32px_0_rgba(31,38,135,0.07)] group-hover:bg-white/10 group-hover:shadow-[0_8px_32px_0_rgba(31,38,135,0.15)] transition-all"
      default:
        return "border border-black/5 dark:border-white/5 rounded-2xl shadow-sm group-hover:shadow-xl transition-all duration-300"
    }
  }

  // --- 依据主题生成文本区域样式 ---
  const getTextThemeStyles = () => {
    switch (theme) {
      case "cyberpunk": return "text-cyan-400 group-hover:text-pink-400 group-hover:drop-shadow-[0_0_5px_rgba(236,72,153,0.8)]"
      case "dark-fantasy": return "font-serif text-[#d4af37] tracking-wider"
      case "shounen": return "font-black tracking-tighter uppercase"
      case "pixel-retro": return "font-mono text-emerald-500 tracking-tight"
      default: return "text-foreground group-hover:text-primary transition-colors"
    }
  }

  return (
    <div 
      className={cn(
        "group relative flex flex-col gap-3 cursor-pointer outline-none shrink-0",
        layout === "portrait" 
          ? "w-[140px] md:w-[160px] lg:w-[200px]" 
          : "w-[240px] md:w-[280px] lg:w-[340px]",
        className
      )}
      onClick={handleClick}
      tabIndex={0}
      role="button"
    >
      {/* 封面容器 */}
      <div className={cn(
        "relative bg-muted shrink-0 w-full overflow-hidden",
        layout === "portrait" ? "aspect-[2/3]" : "aspect-video",
        getCoverThemeStyles()
      )}>
        <ArtworkImage
          src={imageUrl}
          srcSet={srcSet}
          sizes="(max-width: 640px) 140px, (max-width: 768px) 160px, 200px"
          alt={title} 
          loading="lazy"
          allowExternal
          fallbackCategory={artworkCategory ?? inferArtworkCategory(typeBadge) ?? "article"}
          fallbackSeed={id}
          className={cn(
            "w-full h-full object-cover transition-transform duration-700 ease-out",
            theme === "shounen" || theme === "pixel-retro" ? "group-hover:scale-100" : "group-hover:scale-[1.03]"
          )}
        />
        
        {/* 顶部标签 - 高对比度深色胶囊，确保任意底色背景下均清晰可见 */}
        {typeBadge && (
          <div className="absolute top-[8px] right-[8px] px-1.5 py-0.5 rounded-[6px] bg-black/75 backdrop-blur-md border border-white/20 shadow-md z-10">
            <span className="block text-[9px] font-bold text-white tracking-widest uppercase leading-none mt-[1px]">
              {typeBadge}
            </span>
          </div>
        )}

        {/* 底部进度条 */}
        {progress !== undefined && (
          <div className="absolute bottom-0 left-0 right-0 h-1 bg-black/40 backdrop-blur-md">
            <div 
              className="h-full bg-primary/90 transition-all duration-500" 
              style={{ width: `${Math.min(100, Math.max(0, progress))}%` }}
            />
          </div>
        )}
      </div>

      {/* 文本信息 (严格限制卡片展示元素: 图片、标题、子标题) */}
      <div className="flex flex-col px-1">
        <h3 className={cn(
          "text-sm font-semibold truncate transition-colors",
          getTextThemeStyles()
        )}>
          {title}
        </h3>
        {subtitle && (
          <p className="text-xs text-muted-foreground/80 truncate mt-[1px] font-medium">
            {subtitle}
          </p>
        )}
      </div>
    </div>
  )
}

function inferArtworkCategory(typeBadge: string | undefined): DefaultCoverCategory | undefined {
  if (!typeBadge) return undefined
  const normalized = typeBadge.toLocaleLowerCase()
  if (/漫画|manga|cbz|comic/.test(normalized)) return "comic"
  if (/图书|book|epub/.test(normalized)) return "book"
  if (/报刊|文章|article|magazine|html/.test(normalized)) return "article"
  if (/影视|影片|剧集|movie|series|episode|4k|1080|hdr/.test(normalized)) return "video"
  return undefined
}
