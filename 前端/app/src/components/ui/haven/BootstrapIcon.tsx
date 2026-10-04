import type { CSSProperties, HTMLAttributes } from "react"
import { cn } from "@/lib/utils"

/**
 * Bootstrap Icons bundled with Haven.
 *
 * The SVGs are kept in public so the icon source stays local and auditable.
 * A CSS mask lets the icon inherit the active navigation color without
 * rewriting the upstream SVG files or relying on an external icon service.
 */
const BOOTSTRAP_ICON_NAMES = [
  "activity",
  "arrow-clockwise",
  "book",
  "check",
  "clock",
  "clock-history",
  "cloud-arrow-up",
  "database",
  "download",
  "gear",
  "grid",
  "info-circle",
  "link-45deg",
  "palette2",
  "play-circle",
  "plus-lg",
  "search",
  "shield-lock",
  "stars",
] as const

export type BootstrapIconName = (typeof BOOTSTRAP_ICON_NAMES)[number]

export interface BootstrapIconProps extends HTMLAttributes<HTMLSpanElement> {
  name: BootstrapIconName
  size?: number
}

export function BootstrapIcon({ name, size = 18, className, style, ...props }: BootstrapIconProps) {
  const source = `/icons/bootstrap/${name}.svg`
  const iconStyle: CSSProperties = {
    height: size,
    maskImage: `url("${source}")`,
    maskPosition: "center",
    maskRepeat: "no-repeat",
    maskSize: "contain",
    WebkitMaskImage: `url("${source}")`,
    WebkitMaskPosition: "center",
    WebkitMaskRepeat: "no-repeat",
    WebkitMaskSize: "contain",
    width: size,
    ...style,
  }

  return <span aria-hidden="true" className={cn("inline-block shrink-0 bg-current", className)} style={iconStyle} {...props} />
}
