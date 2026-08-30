import { useState } from "react"
import { Sparkles } from "lucide-react"
import { cn } from "@/lib/utils"
import type { BallType } from "@/lib/mascotState"

interface PokeBallWidgetProps {
  ballType: BallType
  characterName: string
  onSummon: () => void
}

export function PokeBallWidget({ ballType, characterName, onSummon }: PokeBallWidgetProps) {
  const [isHovered, setIsHovered] = useState(false)
  const [isOpening, setIsOpening] = useState(false)

  const handleClick = () => {
    setIsOpening(true)
    setTimeout(() => {
      onSummon()
    }, 450)
  }

  return (
    <div className="relative group select-none flex flex-col items-center">
      {/* 悬停气泡提示 */}
      <div
        className={cn(
          "absolute -top-[48px] whitespace-nowrap px-3.5 py-1.5 rounded-2xl text-xs font-bold shadow-xl border backdrop-blur-2xl transition-all duration-300 pointer-events-none z-50 flex items-center gap-1.5",
          "bg-background/95 border-border/80 text-foreground",
          isHovered ? "opacity-100 -translate-y-1 scale-100" : "opacity-0 translate-y-[8px] scale-90"
        )}
      >
        <Sparkles size={13} className="text-amber-500 fill-amber-500 animate-spin" style={{ animationDuration: "4s" }} />
        <span>点击召唤 <strong className="text-primary">{characterName}</strong> 伴读</span>
      </div>

      {/* 宝可梦球主体 */}
      <button
        type="button"
        onClick={handleClick}
        onMouseEnter={() => setIsHovered(true)}
        onMouseLeave={() => setIsHovered(false)}
        aria-label={`召唤 ${characterName}`}
        className={cn(
          "relative flex items-center justify-center cursor-pointer outline-none transition-transform duration-300",
          "h-[64px] w-[64px] md:h-[80px] md:w-[80px] rounded-full shadow-[0_12px_32px_rgba(0,0,0,0.25)] hover:shadow-[0_16px_40px_rgba(0,122,255,0.35)]",
          isOpening ? "scale-125 rotate-12 brightness-150 animate-pulse" : isHovered ? "scale-110 -translate-y-1" : "hover:scale-105"
        )}
      >
        <PokeBallSvg type={ballType} isHovered={isHovered} />
        
        {/* 球体外围微弱光晕 */}
        <div className="absolute inset-0 rounded-full bg-primary/10 blur-md pointer-events-none opacity-0 group-hover:opacity-100 transition-opacity" />
      </button>

      {/* 底部微小投影 */}
      <div
        className={cn(
          "w-[48px] h-[8px] rounded-[100%] bg-black/20 dark:bg-black/50 blur-[3px] mt-[8px] transition-all duration-300",
          isHovered ? "scale-75 opacity-40 translate-y-1" : "scale-100 opacity-60"
        )}
      />
    </div>
  )
}

function PokeBallSvg({ type, isHovered }: { type: BallType; isHovered: boolean }) {
  return (
    <svg
      viewBox="0 0 100 100"
      className={cn(
        "w-full h-full drop-shadow-md transition-transform duration-300",
        isHovered && "animate-[wobble_0.8s_ease-in-out_infinite]"
      )}
    >
      <defs>
        {/* 顶部球壳高光渐变 */}
        <radialGradient id="sphereHighlight" cx="30%" cy="30%" r="70%">
          <stop offset="0%" stopColor="#ffffff" stopOpacity="0.45" />
          <stop offset="50%" stopColor="#ffffff" stopOpacity="0" />
          <stop offset="100%" stopColor="#000000" stopOpacity="0.3" />
        </radialGradient>

        {/* 经典精灵球红顶渐变 */}
        <linearGradient id="pokeRed" x1="0%" y1="0%" x2="0%" y2="100%">
          <stop offset="0%" stopColor="#ff3b30" />
          <stop offset="100%" stopColor="#d70015" />
        </linearGradient>

        {/* 超级球蓝顶渐变 */}
        <linearGradient id="greatBlue" x1="0%" y1="0%" x2="0%" y2="100%">
          <stop offset="0%" stopColor="#0a84ff" />
          <stop offset="100%" stopColor="#0058d0" />
        </linearGradient>

        {/* 高级球黑黄渐变 */}
        <linearGradient id="ultraBlack" x1="0%" y1="0%" x2="0%" y2="100%">
          <stop offset="0%" stopColor="#2c2c2e" />
          <stop offset="100%" stopColor="#1c1c1e" />
        </linearGradient>

        {/* 大师球紫顶渐变 */}
        <linearGradient id="masterPurple" x1="0%" y1="0%" x2="0%" y2="100%">
          <stop offset="0%" stopColor="#af52de" />
          <stop offset="100%" stopColor="#7823b5" />
        </linearGradient>

        {/* 下半球白底渐变 */}
        <linearGradient id="ballWhite" x1="0%" y1="0%" x2="0%" y2="100%">
          <stop offset="0%" stopColor="#ffffff" />
          <stop offset="100%" stopColor="#e5e5ea" />
        </linearGradient>
      </defs>

      {/* 下半球（白色金属） */}
      <path d="M 5 50 A 45 45 0 0 0 95 50 Z" fill="url(#ballWhite)" />

      {/* 上半球（依球类型涂装） */}
      {type === "poke" && (
        <path d="M 5 50 A 45 45 0 0 1 95 50 Z" fill="url(#pokeRed)" />
      )}
      {type === "great" && (
        <g>
          <path d="M 5 50 A 45 45 0 0 1 95 50 Z" fill="url(#greatBlue)" />
          {/* 超级球红色两翼条纹 */}
          <path d="M 18 30 Q 35 15 42 48" stroke="#ff3b30" strokeWidth="6" fill="none" strokeLinecap="round" />
          <path d="M 82 30 Q 65 15 58 48" stroke="#ff3b30" strokeWidth="6" fill="none" strokeLinecap="round" />
        </g>
      )}
      {type === "ultra" && (
        <g>
          <path d="M 5 50 A 45 45 0 0 1 95 50 Z" fill="url(#ultraBlack)" />
          {/* 高级球金色 H 型装甲条纹 */}
          <path d="M 28 8 Q 50 16 72 8 L 76 22 Q 50 28 24 22 Z" fill="#ffd60a" />
          <path d="M 26 22 L 32 48 L 22 48 Z" fill="#ffd60a" />
          <path d="M 74 22 L 68 48 L 78 48 Z" fill="#ffd60a" />
        </g>
      )}
      {type === "master" && (
        <g>
          <path d="M 5 50 A 45 45 0 0 1 95 50 Z" fill="url(#masterPurple)" />
          {/* 大师球粉色突出凸起 */}
          <circle cx="28" cy="30" r="10" fill="#ff2d55" />
          <circle cx="72" cy="30" r="10" fill="#ff2d55" />
          {/* 大师球标志性的 M 字母 */}
          <text x="50" y="32" textAnchor="middle" fill="#ffffff" fontSize="13" fontWeight="900" fontFamily="sans-serif">M</text>
        </g>
      )}

      {/* 中间黑色环带 */}
      <rect x="5" y="47" width="90" height="6" fill="#1c1c1e" />

      {/* 立体高光覆盖层 */}
      <circle cx="50" cy="50" r="45" fill="url(#sphereHighlight)" pointerEvents="none" />

      {/* 外边框 */}
      <circle cx="50" cy="50" r="45" fill="none" stroke="#1c1c1e" strokeWidth="4" />

      {/* 中心按钮外黑圈 */}
      <circle cx="50" cy="50" r="14" fill="#ffffff" stroke="#1c1c1e" strokeWidth="4" />

      {/* 中心发光按钮 */}
      <circle
        cx="50"
        cy="50"
        r="7"
        fill="#ffffff"
        stroke="#8e8e93"
        strokeWidth="1.5"
        className={cn(isHovered && "fill-cyan-300 drop-shadow-[0_0_6px_rgba(56,189,248,0.9)]")}
      />
    </svg>
  )
}
