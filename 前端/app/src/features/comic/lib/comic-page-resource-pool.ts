import { isTauriRuntime } from "@/lib/ipc/runtime"
import type { ComicPageModel } from "./comic-reader-model"

export type ComicPageLoadResult =
  | { status: "loaded"; resource: ComicPageResource }
  | { status: "unavailable" | "error" | "cancelled"; resource: null }

export interface ComicPageResource {
  /** The original controlled resource URI (or its Windows WebView2 alias). */
  src: string
  width: number | null
  height: number | null
}

interface PendingLoad {
  pageNumber: number
  state: "queued" | "granted"
  resource: ComicPageResource | null
  resolvers: Array<(result: ComicPageLoadResult) => void>
}

export interface ComicPageResourcePoolOptions {
  maxConcurrent?: number
  onChange?: () => void
}

const COMIC_PAGE_URI_PATTERN = /^haven-resource:\/\/comic-page\/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

function requestUri(contentUri: string): string {
  const windowsWebView = isTauriRuntime()
    && typeof navigator !== "undefined"
    && /Windows/i.test(navigator.userAgent)
  return windowsWebView
    ? contentUri.replace("haven-resource://comic-page/", "http://haven-resource.comic-page/")
    : contentUri
}

/**
 * Schedules permits for the actual DOM images that render Comic pages.
 *
 * The pool deliberately does not fetch, decode, create Blob/Object URLs, or
 * mount hidden Image nodes. A permit resolves to the original controlled URI;
 * the mounted `<img>` owns the one network read and calls release() from its
 * load/error handler. This keeps the no-store resource protocol inside the
 * four-read budget without requiring a `blob:` CSP exception.
 */
export class ComicPageResourcePool {
  private readonly pages: ReadonlyMap<number, ComicPageModel>
  private readonly maxConcurrent: number
  private readonly onChange?: () => void
  private readonly pending = new Map<number, PendingLoad>()
  private readonly queue: number[] = []
  private retained = new Set<number>()
  private active = 0
  private disposed = false

  constructor(pages: readonly ComicPageModel[], options: ComicPageResourcePoolOptions = {}) {
    this.pages = new Map(pages.map((page) => [page.pageNumber, page]))
    this.retained = new Set(this.pages.keys())
    this.maxConcurrent = Math.max(1, Math.min(4, options.maxConcurrent ?? 4))
    this.onChange = options.onChange
  }

  /** Number of permits currently owned by mounted/in-flight image elements. */
  get activeCount(): number {
    return this.active
  }

  isLoading(pageNumber: number): boolean {
    return this.pending.has(pageNumber)
  }

  load(pageNumber: number): Promise<ComicPageLoadResult> {
    if (this.disposed) return Promise.resolve({ status: "cancelled", resource: null })
    const page = this.pages.get(pageNumber)
    if (!page || page.availability !== "ready" || !page.contentUri) {
      return Promise.resolve({ status: "unavailable", resource: null })
    }
    if (!COMIC_PAGE_URI_PATTERN.test(page.contentUri)) {
      return Promise.resolve({ status: "error", resource: null })
    }

    const existing = this.pending.get(pageNumber)
    if (existing) {
      return new Promise((resolve) => { existing.resolvers.push(resolve) })
    }

    const promise = new Promise<ComicPageLoadResult>((resolve) => {
      this.pending.set(pageNumber, {
        pageNumber,
        state: "queued",
        resource: null,
        resolvers: [resolve],
      })
      this.queue.push(pageNumber)
      this.pump()
    })
    return promise
  }

  prefetch(pageNumbers: readonly number[]): void {
    for (const pageNumber of new Set(pageNumbers)) void this.load(pageNumber)
  }

  /**
   * Keep only the currently mounted/soon-to-be-mounted window. Queued and
   * granted permits outside it are cancelled, and their callers receive the
   * independent `cancelled` result (never an image failure).
   */
  retain(pageNumbers: readonly number[]): void {
    this.retained = new Set(pageNumbers)
    for (const [pageNumber, job] of this.pending) {
      if (this.retained.has(pageNumber)) continue
      this.pending.delete(pageNumber)
      if (job.state === "granted") this.active = Math.max(0, this.active - 1)
      for (const resolve of job.resolvers) resolve({ status: "cancelled", resource: null })
    }
    this.onChange?.()
    this.pump()
  }

  release(pageNumber: number): void {
    const job = this.pending.get(pageNumber)
    if (!job) return
    if (job.state === "queued") {
      this.pending.delete(pageNumber)
      for (const resolve of job.resolvers) resolve({ status: "cancelled", resource: null })
      return
    }
    this.pending.delete(pageNumber)
    this.active = Math.max(0, this.active - 1)
    this.onChange?.()
    this.pump()
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    this.queue.length = 0
    for (const job of this.pending.values()) {
      for (const resolve of job.resolvers) resolve({ status: "cancelled", resource: null })
    }
    this.pending.clear()
    this.active = 0
    this.onChange?.()
  }

  private pump(): void {
    while (!this.disposed && this.active < this.maxConcurrent && this.queue.length > 0) {
      const pageNumber = this.queue.shift()!
      const job = this.pending.get(pageNumber)
      const page = this.pages.get(pageNumber)
      if (!job || job.state !== "queued" || !page?.contentUri) continue

      const resource: ComicPageResource = {
        src: requestUri(page.contentUri),
        width: null,
        height: null,
      }
      job.state = "granted"
      job.resource = resource
      this.active += 1
      for (const resolve of job.resolvers) resolve({ status: "loaded", resource })
      this.onChange?.()
    }
  }

}
