/**
 * Bundled cover fallbacks for media whose controlled artwork is unavailable.
 *
 * The files live in Vite's public directory so the fallback never depends on
 * a provider, the Artwork Cache, or a network request.  Selection is a stable
 * hash of the item identity: it is random-looking across items but does not
 * flicker to a different cover every time React re-renders the card.
 */

export type DefaultCoverCategory = "video" | "book" | "comic" | "article"

const DEFAULT_COVER_FILES: Record<DefaultCoverCategory, readonly string[]> = {
  video: ["/默认图片/影视/影视01.png"],
  book: ["/默认图片/图书/图书01.png"],
  comic: [
    "/默认图片/漫画/漫画01.png",
    "/默认图片/漫画/漫画02.png",
    "/默认图片/漫画/漫画03.png",
  ],
  article: ["/默认图片/报刊文章/报刊文章01.png"],
}
/** Map the application's media/category labels to the four bundled folders. */
export function defaultCoverCategoryForMediaType(value: string | null | undefined): DefaultCoverCategory {
  switch (value) {
    case "video":
    case "movie":
    case "tv":
    case "series":
    case "episode":
      return "video"
    case "comic":
      return "comic"
    case "book":
      return "book"
    case "article":
    case "periodical":
    case "document":
    default:
      return "article"
  }
}

/**
 * Return a bundled cover path for one stable item identity.
 * `seed` should be an opaque work/media/task id, never a URL or user content.
 */
export function defaultCoverPath(category: DefaultCoverCategory, seed = ""): string {
  const files = DEFAULT_COVER_FILES[category]
  const index = stableHash(seed || category) % files.length
  return files[index]
}

export function defaultCoverForMediaType(value: string | null | undefined, seed = ""): string {
  return defaultCoverPath(defaultCoverCategoryForMediaType(value), seed)
}

/** Exposed for tests and asset checks; callers should use defaultCoverPath. */
export function defaultCoverFiles(category: DefaultCoverCategory): readonly string[] {
  return DEFAULT_COVER_FILES[category]
}

function stableHash(value: string): number {
  // FNV-1a keeps the mapping deterministic without pulling a crypto/runtime
  // dependency into the render path.
  let hash = 0x811c9dc5
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index)
    hash = Math.imul(hash, 0x01000193)
  }
  return hash >>> 0
}
