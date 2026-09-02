import { isTauriRuntime } from "@/lib/ipc/runtime"
import type { ComicPageModel } from "./comic-reader-model"

export type ComicPageLoadResult =
  | { status: "loaded"; resource: ComicPageResource }
  | { status: "unavailable" | "error" | "cancelled"; resource: null }

/**
 * A load request is still a Promise for callers that only need the result,
 * but exposes cancellation for a DOM node that unmounts before its permit is
 * granted. Keeping cancellation on the request prevents one page's cleanup
 * from accidentally releasing another consumer of the same page.
 */
export type ComicPageLoadRequest = Promise<ComicPageLoadResult> & {
  cancel: () => void
}

export interface ComicPageResource {
  /** The original controlled resource URI (or its Windows WebView2 alias). */
  src: string
  width: number | null
  height: number | null
  /** Releases this individual DOM consumer's permit. Idempotent. */
  release: () => void
}

type ConsumerState = "waiting" | "loaded" | "released"

interface PendingConsumer {
  resolve: (result: ComicPageLoadResult) => void
  state: ConsumerState
}

interface PendingLoad {
  pageNumber: number
  state: "queued" | "granted"
  resource: Omit<ComicPageResource, "release"> | null
  consumers: Set<PendingConsumer>
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
 * the mounted `<img>` owns the one network read and calls its lease's
 * release() from its load/error handler. A page may have more than one DOM
 * consumer (for example, the main image and a thumbnail), so leases are
 * reference-counted and the permit is held until every consumer releases it.
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

  load(pageNumber: number): ComicPageLoadRequest {
    if (this.disposed) return cancelledRequest()
    const page = this.pages.get(pageNumber)
    if (!page || page.availability !== "ready" || !page.contentUri) {
      return immediateRequest({ status: "unavailable", resource: null })
    }
    if (!COMIC_PAGE_URI_PATTERN.test(page.contentUri)) {
      return immediateRequest({ status: "error", resource: null })
    }

    let consumer: PendingConsumer | undefined
    const promise = new Promise<ComicPageLoadResult>((resolve) => {
      consumer = { resolve, state: "waiting" }
    }) as ComicPageLoadRequest

    const existing = this.pending.get(pageNumber)
    const job: PendingLoad = existing ?? {
      pageNumber,
      state: "queued",
      resource: null,
      consumers: new Set<PendingConsumer>(),
    }
    if (!existing) {
      this.pending.set(pageNumber, job)
      this.queue.push(pageNumber)
    }
    const requestConsumer = consumer!
    job.consumers.add(requestConsumer)
    promise.cancel = () => this.releaseConsumer(job, requestConsumer)

    // A thumbnail can mount after the main image already owns the permit.
    // Resolve it immediately with its own idempotent lease instead of leaving
    // a resolver behind that `pump()` will never visit again.
    if (job.state === "granted" && job.resource) {
      this.resolveLoadedConsumer(job, requestConsumer)
    } else if (!existing) {
      this.pump()
    }
    return promise
  }

  prefetch(pageNumbers: readonly number[]): void {
    // The pool only grants permits to mounted DOM images; a prefetch request
    // therefore releases its permit as soon as the controlled URI is issued.
    // This keeps the convenience API from pinning the bounded pool forever.
    for (const pageNumber of new Set(pageNumbers)) {
      const request = this.load(pageNumber)
      void request.then((result) => {
        if (result.status === "loaded") result.resource.release()
      })
    }
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
      this.cancelJob(job)
    }
    this.onChange?.()
    this.pump()
  }

  /**
   * Compatibility path for older page-only callers. New DOM callers use the
   * request cancel() method and the resource lease release() method so a
   * sibling consumer cannot be released by mistake.
   */
  release(pageNumber: number): void {
    const job = this.pending.get(pageNumber)
    if (!job) return
    const consumer = [...job.consumers].find((item) => item.state !== "released")
    if (consumer) this.releaseConsumer(job, consumer)
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    this.queue.length = 0
    for (const job of [...this.pending.values()]) this.cancelJob(job)
    this.onChange?.()
  }

  private pump(): void {
    while (!this.disposed && this.active < this.maxConcurrent && this.queue.length > 0) {
      const pageNumber = this.queue.shift()!
      const job = this.pending.get(pageNumber)
      const page = this.pages.get(pageNumber)
      if (!job || job.state !== "queued" || !page?.contentUri) continue

      const resource: Omit<ComicPageResource, "release"> = {
        src: requestUri(page.contentUri),
        width: null,
        height: null,
      }
      job.state = "granted"
      job.resource = resource
      this.active += 1
      for (const consumer of job.consumers) this.resolveLoadedConsumer(job, consumer)
      this.onChange?.()
    }
  }

  private resolveLoadedConsumer(job: PendingLoad, consumer: PendingConsumer): void {
    if (consumer.state !== "waiting" || !job.resource) return
    consumer.state = "loaded"
    const resource: ComicPageResource = {
      ...job.resource,
      release: () => this.releaseConsumer(job, consumer),
    }
    consumer.resolve({ status: "loaded", resource })
  }

  private releaseConsumer(job: PendingLoad, consumer: PendingConsumer): void {
    if (consumer.state === "released") return
    if (consumer.state === "waiting") {
      consumer.resolve({ status: "cancelled", resource: null })
    }
    consumer.state = "released"
    job.consumers.delete(consumer)

    // A queued request with no remaining consumers can be removed without
    // consuming a permit. A granted request stays active until all DOM
    // consumers release their individual leases.
    if (job.consumers.size > 0) return
    this.finishJob(job)
  }

  private cancelJob(job: PendingLoad): void {
    if (this.pending.get(job.pageNumber) !== job) return
    this.pending.delete(job.pageNumber)
    if (job.state === "granted") this.active = Math.max(0, this.active - 1)

    for (const consumer of job.consumers) {
      if (consumer.state === "waiting") {
        consumer.state = "released"
        consumer.resolve({ status: "cancelled", resource: null })
      } else {
        consumer.state = "released"
      }
    }
    job.consumers.clear()
  }

  private finishJob(job: PendingLoad): void {
    if (this.pending.get(job.pageNumber) !== job) return
    this.pending.delete(job.pageNumber)
    if (job.state === "granted") this.active = Math.max(0, this.active - 1)
    this.onChange?.()
    this.pump()
  }
}

function immediateRequest(result: ComicPageLoadResult): ComicPageLoadRequest {
  const promise = Promise.resolve(result) as ComicPageLoadRequest
  promise.cancel = () => undefined
  return promise
}

function cancelledRequest(): ComicPageLoadRequest {
  return immediateRequest({ status: "cancelled", resource: null })
}
