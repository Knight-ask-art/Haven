import React, { useState } from "react"
import { cn } from "@/lib/utils"
import { X, Download, Check, Sparkles, Wand2, Terminal, Shield, Flame, Gamepad2, Layers } from "lucide-react"

export interface ShareCardModalProps {
  isOpen: boolean
  onClose: () => void
  data: {
    title: string
    originalTitle?: string
    metadata?: string
    description?: string
    imageUrl?: string
    backdropUrl?: string
    author?: string
  }
}

export type CardStyleType = "holographic" | "cyberpunk" | "fantasy" | "anime" | "pixel" | "glass"

const CARD_STYLES: { id: CardStyleType; label: string; icon: React.ElementType }[] = [
  { id: "holographic", label: "全息炫彩", icon: Wand2 },
  { id: "cyberpunk", label: "赛博霓虹", icon: Terminal },
  { id: "fantasy", label: "暗黑奇幻", icon: Shield },
  { id: "anime", label: "热血动漫", icon: Flame },
  { id: "pixel", label: "复古像素", icon: Gamepad2 },
  { id: "glass", label: "极简玻璃", icon: Layers },
]

export function ShareCardModal({ isOpen, onClose, data }: ShareCardModalProps) {
  const [saved, setSaved] = useState(false)
  const [cardStyle, setCardStyle] = useState<CardStyleType>("holographic")
  const [tilt, setTilt] = useState({ x: 0, y: 0, opacity: 0 })

  if (!isOpen) return null

  const handleMouseMove = (e: React.MouseEvent<HTMLDivElement>) => {
    const rect = e.currentTarget.getBoundingClientRect()
    const x = e.clientX - rect.left
    const y = e.clientY - rect.top
    const centerX = rect.width / 2
    const centerY = rect.height / 2
    const rotateX = ((y - centerY) / centerY) * -15
    const rotateY = ((x - centerX) / centerX) * 15
    setTilt({ x: rotateX, y: rotateY, opacity: 1 })
  }

  const handleMouseLeave = () => {
    setTilt({ x: 0, y: 0, opacity: 0 })
  }

  const handleSave = () => {
    setSaved(true)
    setTimeout(() => setSaved(false), 2200)
  }

  const bgImage = data.backdropUrl || data.imageUrl || "https://images.unsplash.com/photo-1534447677768-be436bb09401?q=80&w=1200&auto=format&fit=crop"
  const authorName = data.author || data.originalTitle || "Haven Collection"

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-[16px] md:p-[32px] bg-black/85 backdrop-blur-2xl animate-in fade-in duration-300">
      {/* Backdrop overlay */}
      <div className="absolute inset-0" onClick={onClose} />

      {/* Main Container */}
      <div className="relative z-10 flex flex-col md:flex-row items-center justify-center gap-[32px] lg:gap-[48px] max-w-5xl w-full">
        
        {/* ==========================================
            6 DEDICATED ARCHITECTURE TEMPLATES
            (Distinct Internal Layouts + Strict Rules)
           ========================================== */}
        <div 
          id="share-card-canvas"
          onMouseMove={handleMouseMove}
          onMouseLeave={handleMouseLeave}
          style={{
            transform: `perspective(1000px) rotateX(${tilt.x}deg) rotateY(${tilt.y}deg)`,
            transition: tilt.opacity === 0 ? "transform 0.5s ease" : "none"
          }}
          className={cn(
            "relative w-[300px] sm:w-[330px] aspect-[3/4] shrink-0 select-none cursor-pointer group transition-all duration-500",
            // Remove border rules here, apply inside templates natively
          )}
        >
          {/* Style 1: 全息炫彩 (TCG Holographic Foil Card) */}
          {cardStyle === "holographic" && (
            <div className="relative w-full h-full bg-slate-300 rounded-[24px] shadow-[inset_0_0_20px_rgba(255,255,255,0.8),0_32px_80px_rgba(0,0,0,0.9)] border-[8px] border-slate-200">
              {/* Inner content box */}
              <div className="absolute inset-[8px] bg-zinc-100 rounded-[14px] flex flex-col p-3 border border-zinc-300 shadow-inner">
                {/* Artwork window */}
                <div className="relative w-full aspect-square bg-black rounded-[8px] overflow-hidden border-[3px] border-slate-300 shadow-[0_4px_12px_rgba(0,0,0,0.2)]">
                  <img src={bgImage} className="w-full h-full object-cover saturate-150" alt={data.title} />
                </div>
                {/* Minimal Info */}
                <div className="mt-5 flex flex-col gap-1 px-1">
                  <h2 className="text-[22px] font-black text-zinc-800 leading-tight tracking-tight">{data.title}</h2>
                  <p className="text-xs font-bold text-zinc-500">{authorName}</p>
                </div>
              </div>
              
              {/* Dynamic Holographic Overlay */}
              <div 
                className="absolute inset-0 rounded-[16px] mix-blend-color-dodge transition-opacity duration-300 pointer-events-none"
                style={{
                  background: `linear-gradient(${115 + tilt.y * 5}deg, rgba(255,100,180,0.6) 0%, rgba(100,255,255,0.6) 33%, rgba(255,255,100,0.6) 66%, rgba(180,100,255,0.6) 100%)`,
                  opacity: tilt.opacity > 0 ? 0.9 : 0.5
                }}
              />
            </div>
          )}

          {/* Style 2: 赛博霓虹 (Cyberpunk HUD) */}
          {cardStyle === "cyberpunk" && (
            <div 
              className="relative w-full h-full bg-zinc-950 overflow-hidden shadow-[0_32px_80px_rgba(0,0,0,0.9),0_0_50px_rgba(0,240,255,0.3)] border-l-4 border-r-4 border-cyan-500"
              style={{ clipPath: "polygon(8% 0, 100% 0, 100% 92%, 92% 100%, 0 100%, 0 8%)" }}
            >
              <div className="absolute inset-0 bg-[radial-gradient(circle_at_center,rgba(0,240,255,0.15)_0%,transparent_100%)]" />
              
              <div className="absolute inset-3 flex flex-col">
                <div className="relative w-full h-[60%] border-b-2 border-cyan-500/50 p-1">
                  <img 
                    src={bgImage} 
                    className="w-full h-full object-cover filter contrast-125 saturate-150" 
                    style={{ clipPath: "polygon(0 0, 100% 0, 100% 100%, 8% 100%, 0 92%)" }} 
                    alt={data.title}
                  />
                </div>
                
                <div className="flex-1 pt-[16px] pb-[8px] flex flex-col justify-end">
                  <p className="text-[10px] text-cyan-500/60 font-mono mb-1 tracking-widest uppercase">ID // {authorName}</p>
                  <h2 className="text-2xl sm:text-[28px] font-black text-cyan-400 font-mono uppercase tracking-tighter drop-shadow-[0_0_12px_rgba(0,240,255,0.8)] leading-none">
                    {data.title}
                  </h2>
                </div>
              </div>
            </div>
          )}

          {/* Style 3: 暗黑奇幻 (MTG style) */}
          {cardStyle === "fantasy" && (
            <div className="relative w-full h-full bg-stone-900 rounded-[12px] p-3 border-[6px] border-stone-950 shadow-[0_32px_80px_rgba(0,0,0,0.9)]">
               <div className="relative w-full h-[55%] border-[4px] border-amber-700/80 rounded-t-[40%] overflow-hidden shadow-2xl bg-black">
                 <img src={bgImage} className="w-full h-full object-cover filter sepia-[0.4] contrast-[1.1]" alt={data.title} />
               </div>
               
               <div className="mt-[16px] bg-stone-800 border-[2px] border-amber-900/50 rounded-md p-[16px] text-center shadow-[inset_0_0_20px_rgba(0,0,0,0.8)] h-[35%] flex flex-col items-center justify-center relative">
                  {/* Decorative corners */}
                  <div className="absolute top-1 left-1 w-[8px] h-[8px] border-t border-l border-amber-700/50" />
                  <div className="absolute top-1 right-1 w-[8px] h-[8px] border-t border-r border-amber-700/50" />
                  <div className="absolute bottom-1 left-1 w-[8px] h-[8px] border-b border-l border-amber-700/50" />
                  <div className="absolute bottom-1 right-1 w-[8px] h-[8px] border-b border-r border-amber-700/50" />

                  <h2 className="text-xl sm:text-2xl font-serif font-black text-amber-50 tracking-wider mb-3 leading-tight drop-shadow-md">
                    {data.title}
                  </h2>
                  <div className="w-[64px] h-[1px] bg-amber-700/50 mx-auto mb-3" />
                  <p className="text-xs font-serif font-bold text-amber-500/80 tracking-widest uppercase">
                    {authorName}
                  </p>
               </div>
            </div>
          )}

          {/* Style 4: 热血动漫 (Manga Panel) */}
          {cardStyle === "anime" && (
            <div className="relative w-full h-full bg-white rounded-[4px] p-[8px] border-[8px] border-black overflow-hidden shadow-[0_32px_80px_rgba(0,0,0,0.9),8px_8px_0px_rgba(255,255,255,0.2)]">
               {/* Background manga speed lines */}
               <div className="absolute inset-0 bg-[repeating-linear-gradient(45deg,transparent,transparent_8px,#000_8px,#000_10px)] opacity-5" />
               
               <div className="relative w-full h-[65%] border-b-[6px] border-black overflow-hidden bg-black">
                 <img 
                    src={bgImage} 
                    className="w-full h-full object-cover filter grayscale contrast-[1.2] group-hover:grayscale-0 transition-all duration-700" 
                    alt={data.title} 
                 />
                 <div className="absolute inset-0 shadow-[inset_0_0_20px_rgba(0,0,0,0.5)] pointer-events-none" />
               </div>
               
               <div className="relative h-[30%] flex flex-col justify-center items-center p-[8px] transform -rotate-3 scale-[1.05]">
                  <h2 className="text-3xl sm:text-4xl font-black text-black italic tracking-tighter uppercase text-center leading-none drop-shadow-[2px_2px_0px_#fff,4px_4px_0px_#ef4444]">
                    {data.title}
                  </h2>
                  <div className="mt-3 bg-black text-white px-3 py-1 font-black text-[11px] tracking-widest skew-x-[-12deg]">
                    <span className="block skew-x-[12deg]">{authorName}</span>
                  </div>
               </div>
            </div>
          )}

          {/* Style 5: 复古像素 (GameBoy/8-Bit) */}
          {cardStyle === "pixel" && (
            <div className="relative w-full h-full bg-[#c4cfa1] rounded-b-[40px] rounded-t-[12px] p-[16px] border-b-[12px] border-r-[8px] border-[#8b956d] shadow-[0_32px_80px_rgba(0,0,0,0.9)]">
               <div className="relative w-full h-[55%] bg-[#4e5b41] rounded-[4px] p-[8px] border-[4px] border-[#333c2a] shadow-inner mt-[16px]">
                 <div className="relative w-full h-full overflow-hidden bg-black">
                   <img 
                      src={bgImage} 
                      className="w-full h-full object-cover opacity-80 mix-blend-luminosity filter contrast-125" 
                      style={{ imageRendering: "pixelated" }} 
                      alt={data.title} 
                   />
                   {/* Scanlines */}
                   <div className="absolute inset-0 bg-[linear-gradient(to_bottom,transparent_50%,rgba(0,0,0,0.2)_50%)] bg-[size:100%_4px] pointer-events-none" />
                 </div>
               </div>
               
               <div className="mt-10 flex flex-col items-center">
                  <h2 className="text-xl sm:text-2xl font-black text-[#333c2a] font-mono tracking-tighter text-center leading-tight">
                    {data.title}
                  </h2>
                  <p className="text-[10px] font-bold text-[#4e5b41] mt-3 font-mono tracking-widest">
                    [{authorName}]
                  </p>
               </div>
            </div>
          )}

          {/* Style 6: 极简玻璃 (Apple Vision) */}
          {cardStyle === "glass" && (
            <div className="relative w-full h-full rounded-[32px] overflow-hidden bg-black shadow-[0_32px_80px_rgba(0,0,0,0.9)] border border-white/20">
               <img 
                 src={bgImage} 
                 className="absolute inset-0 w-full h-full object-cover transition-transform duration-700 group-hover:scale-105" 
                 alt={data.title} 
               />
               <div className="absolute inset-0 bg-gradient-to-t from-black/80 via-transparent to-transparent pointer-events-none" />
               
               {/* Floating Glass Pill for Info */}
               <div className="absolute bottom-[16px] left-[16px] right-[16px] rounded-[20px] bg-white/10 backdrop-blur-2xl border border-white/20 p-5 shadow-2xl flex flex-col gap-0.5">
                  <h2 className="text-2xl sm:text-[26px] font-bold text-white tracking-tight leading-tight drop-shadow-md">
                    {data.title}
                  </h2>
                  <p className="text-xs font-semibold text-white/60 tracking-wide">
                    {authorName}
                  </p>
               </div>
            </div>
          )}

        </div>

        {/* ==========================================
            RIGHT CONTROLS PANEL: STYLE SELECTOR
           ========================================== */}
        <div className="flex flex-col gap-6 w-full md:w-[320px] p-6 sm:p-7 rounded-[28px] bg-zinc-900/90 border border-white/15 shadow-2xl backdrop-blur-2xl text-white">
          {/* Header */}
          <div className="flex items-center justify-between border-b border-white/10 pb-[16px]">
            <div className="flex items-center gap-[8px]">
              <Sparkles className="w-[16px] h-[16px] text-amber-400" />
              <h3 className="text-base font-extrabold tracking-tight">卡片模板切换</h3>
            </div>
            <button 
              onClick={onClose}
              className="p-1.5 rounded-full bg-white/10 hover:bg-white/20 transition-colors text-white/70 hover:text-white"
            >
              <X className="w-[18px] h-[18px]" />
            </button>
          </div>

          {/* Style Selector Grid */}
          <div className="flex flex-col gap-[10px]">
            <label className="text-xs font-bold text-white/90 tracking-wide">专属极简风格版式</label>
            <div className="grid grid-cols-2 gap-[8px]">
              {CARD_STYLES.map((style) => {
                const Icon = style.icon
                const isSelected = cardStyle === style.id
                return (
                  <button
                    key={style.id}
                    onClick={() => setCardStyle(style.id)}
                    className={cn(
                      "flex items-center gap-[10px] p-[10px] rounded-xl border text-xs font-bold transition-all text-left relative overflow-hidden group cursor-pointer",
                      isSelected
                        ? "bg-white text-zinc-950 border-white shadow-lg scale-[1.02]"
                        : "bg-white/5 border-white/10 text-white/80 hover:bg-white/10 hover:text-white"
                    )}
                  >
                    <div className={cn(
                      "w-6 h-6 rounded-lg flex items-center justify-center shrink-0 border",
                      isSelected ? "bg-zinc-950 text-white border-zinc-800" : "bg-white/10 border-white/10 text-white"
                    )}>
                      <Icon className="w-3.5 h-3.5" />
                    </div>
                    <span className="truncate">{style.label}</span>
                  </button>
                )
              })}
            </div>
          </div>

          {/* Action Button */}
          <div className="pt-[8px] border-t border-white/10">
            <button
              onClick={handleSave}
              className={cn(
                "flex items-center justify-center gap-[10px] w-full py-3.5 px-5 rounded-2xl font-black text-sm transition-all shadow-xl active:scale-98 cursor-pointer",
                saved 
                  ? "bg-emerald-500 text-white shadow-emerald-500/25" 
                  : "bg-white text-zinc-950 hover:bg-white/90 shadow-white/10"
              )}
            >
              {saved ? <Check className="w-5 h-5" /> : <Download className="w-5 h-5" />}
              {saved ? "已保存至本地" : "保存到本地"}
            </button>
          </div>
        </div>

      </div>
    </div>
  )
}
