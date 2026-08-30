import * as React from "react"
import type { LucideIcon } from "lucide-react"
import {
  Search, LibraryBig, Download, Settings, ChevronRight, Play, Pause, X, CheckCircle,
  Home, Bookmark, Tag, Heart, Sparkles, Layers, LayoutGrid, Film, BookOpen, Star, History
} from "lucide-react"
import { cn } from "@/lib/utils"

// Centralized symbol dictionary mapping string identifiers to Lucide components
const SymbolDictionary: Record<string, LucideIcon> = {
  "home": Home,
  "search": Search,
  "library": LayoutGrid,
  "library-big": LibraryBig,
  "layers": Layers,
  "book-open": BookOpen,
  "film": Film,
  "bookmark": Bookmark,
  "star": Star,
  "tag": Tag,
  "heart": Heart,
  "sparkles": Sparkles,
  "download": Download,
  "settings": Settings,
  "chevron-right": ChevronRight,
  "play": Play,
  "pause": Pause,
  "x": X,
  "check-circle": CheckCircle,
  "footprints": History,
  "history": History,
}

export type SymbolSize = 12 | 14 | 16 | 18 | 20 | 24 | 32 | 48 | 64
export type SymbolWeight = "subtle" | "regular" | "emphasized"
export type SymbolRole = "navigation" | "toolbar" | "inline" | "status" | "decorative"

export interface HavenIconProps extends React.SVGProps<SVGSVGElement> {
  symbol: string | LucideIcon
  role?: SymbolRole
  size?: SymbolSize
  weight?: SymbolWeight
  className?: string
}

// Maps our conceptual weight to Lucide's absolute stroke width
const weightToStrokeMap: Record<SymbolWeight, number> = {
  subtle: 1.5,
  regular: 1.75, // Apple-like baseline instead of Lucide's 2.0
  emphasized: 2.25,
}

export function HavenIcon({
  symbol,
  role = "inline",
  size = 16,
  weight = "regular",
  className,
  ...props
}: HavenIconProps) {
  
  // Resolve the icon component either from dictionary or direct prop
  const IconComponent = typeof symbol === "string" ? SymbolDictionary[symbol] : symbol

  if (!IconComponent) {
    console.warn(`HavenIcon: Symbol '${symbol}' not found in dictionary.`)
    return null
  }

  const strokeWidth = weightToStrokeMap[weight]

  return (
    <IconComponent
      size={size}
      strokeWidth={strokeWidth}
      className={cn(
        "shrink-0 transition-colors",
        // Apply default opacity/colors based on role if needed, though usually inherited from parent
        role === "decorative" && "opacity-50",
        className
      )}
      {...props}
    />
  )
}
