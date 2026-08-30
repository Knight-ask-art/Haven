import { useEffect, useMemo, useState } from "react"

import { artworkRequestUri, controlledResourceUri } from "@/lib/artwork-url"
import { defaultCoverPath, type DefaultCoverCategory } from "@/lib/default-cover"
import { cn } from "@/lib/utils"

export interface ArtworkImageProps {
  /** `haven://artwork/<opaque>`、受控 resource URI 或显式允许的兼容本地/外链地址。 */
  src?: string | null
  alt: string
  className?: string
  containerClassName?: string
  srcSet?: string
  sizes?: string
  loading?: "eager" | "lazy"
  fetchPriority?: "high" | "low" | "auto"
  /** 仅给尚未迁移的 MediaCard Demo 调用者使用；热榜不打开此开关。 */
  allowExternal?: boolean
  /** Missing/broken artwork falls back to a bundled, category-specific cover. */
  fallbackCategory?: DefaultCoverCategory
  /** Stable opaque identity used to choose one cover without render flicker. */
  fallbackSeed?: string
}

type ArtworkStatus = "loading" | "loaded" | "failed"

/**
 * 统一处理海报的受控 URI、加载中、失败和本地占位状态。
 *
 * 失败时只隐藏当前图片，不把 src 改成第二个远端地址；这样断网或上游
 * 不稳定时不会产生重复请求、红色破图图标或布局抖动。
 */
export function ArtworkImage({
  src,
  alt,
  className,
  containerClassName,
  srcSet,
  sizes,
  loading = "lazy",
  fetchPriority = "auto",
  allowExternal = false,
  fallbackCategory,
  fallbackSeed,
}: ArtworkImageProps) {
  const normalizedSrc = useMemo(
    () => normalizeArtworkSource(src, allowExternal),
    [allowExternal, src],
  )
  const fallbackSrc = fallbackCategory ? defaultCoverPath(fallbackCategory, fallbackSeed ?? "") : null
  const [fallbackActive, setFallbackActive] = useState(!normalizedSrc)
  const [status, setStatus] = useState<ArtworkStatus>(normalizedSrc || fallbackSrc ? "loading" : "failed")

  useEffect(() => {
    setFallbackActive(!normalizedSrc)
    setStatus(normalizedSrc || fallbackSrc ? "loading" : "failed")
  }, [fallbackSrc, normalizedSrc])

  const displaySrc = fallbackActive ? fallbackSrc : normalizedSrc

  return (
    <span
      className={cn("relative block h-full w-full overflow-hidden bg-muted", containerClassName)}
      data-artwork-status={status}
      data-artwork-source={fallbackActive ? "default" : "controlled"}
    >
      {status !== "loaded" && (!fallbackSrc || (fallbackActive && status === "failed")) && (
        <span
          className="absolute inset-0 flex items-center justify-center bg-gradient-to-br from-muted via-muted/80 to-muted/50 px-2 text-center text-[11px] font-medium text-muted-foreground"
          aria-hidden="true"
        >
          暂无封面
        </span>
      )}
      {displaySrc && (
        <img
          src={displaySrc}
          srcSet={fallbackActive ? undefined : srcSet}
          sizes={sizes}
          alt={alt}
          loading={loading}
          fetchPriority={fetchPriority}
          decoding="async"
          onLoad={() => setStatus("loaded")}
          onError={() => {
            if (!fallbackActive && fallbackSrc) {
              setFallbackActive(true)
              setStatus("loading")
            } else {
              setStatus("failed")
            }
          }}
          className={cn(
            "relative z-[1] h-full w-full object-cover transition-opacity duration-200",
            status === "loaded" ? "opacity-100" : "opacity-0",
            className,
          )}
        />
      )}
    </span>
  )
}

function normalizeArtworkSource(value: string | null | undefined, allowExternal: boolean): string | null {
  if (!value) return null
  if (value.startsWith("haven://artwork/")) {
    return artworkRequestUri(value) || null
  }
  if (value.startsWith("haven-resource://artwork/")) {
    const id = value.slice("haven-resource://artwork/".length)
    if (!/^[A-Za-z0-9_-]+$/.test(id)) return null
    return controlledResourceUri(value)
  }
  if (value.startsWith("data:image/") || value.startsWith("/")) return value
  if (allowExternal && /^https?:\/\//i.test(value)) return value
  return null
}
