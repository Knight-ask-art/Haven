export type Oml2dRuntimeModule = Pick<typeof import("oh-my-live2d"), "loadOml2d">

type Oml2dRuntimeLoader = () => Promise<Oml2dRuntimeModule>
type CubismRuntimeLoader = () => Promise<void>
type StylesheetLoader = (signal?: AbortSignal) => Promise<void>

const CUBISM_RUNTIME_URL = "/live2d/vendor/cubism2-runtime.js"
const OML2D_STYLESHEET_URL = "/live2d/vendor/oml2d.css"
const CUBISM_RUNTIME_TIMEOUT_MS = 10000
const OML2D_STYLESHEET_TIMEOUT_MS = 10000

type Live2dWindow = Window & {
  Live2D?: unknown
}

let cubismRuntimePromise: Promise<void> | null = null
let stylesheetPromise: Promise<void> | null = null

function hasCubismRuntime(): boolean {
  return typeof window !== "undefined" && Boolean((window as Live2dWindow).Live2D)
}

function loadLocalCubismRuntime(): Promise<void> {
  if (hasCubismRuntime()) return Promise.resolve()
  if (cubismRuntimePromise) return cubismRuntimePromise

  cubismRuntimePromise = new Promise<void>((resolve, reject) => {
    const script = document.createElement("script")
    let settled = false
    let timeoutId: number

    const finish = (error?: Error) => {
      if (settled) return
      settled = true
      window.clearTimeout(timeoutId)
      script.onload = null
      script.onerror = null
      if (error) {
        script.remove()
        reject(error)
        return
      }
      resolve()
    }

    timeoutId = window.setTimeout(
      () => finish(new Error("Local Cubism runtime timed out")),
      CUBISM_RUNTIME_TIMEOUT_MS,
    )

    script.async = true
    script.src = CUBISM_RUNTIME_URL
    script.dataset.havenLive2dRuntime = "cubism2"
    script.onload = () => {
      finish(
        hasCubismRuntime()
          ? undefined
          : new Error("Local Cubism runtime did not initialize"),
      )
    }
    script.onerror = () => finish(new Error("Local Cubism runtime failed to load"))
    document.head.append(script)
  }).catch((error: unknown) => {
    cubismRuntimePromise = null
    throw error
  })

  return cubismRuntimePromise
}

function loadLocalStylesheet(signal?: AbortSignal): Promise<void> {
  if (!stylesheetPromise) {
    stylesheetPromise = new Promise<void>((resolve, reject) => {
      const existingLink = document.querySelector<HTMLLinkElement>(
        'link[data-haven-live2d-stylesheet="oml2d"]',
      )
      const link = existingLink ?? document.createElement("link")
      let settled = false
      let timeoutId: number

      const finish = (error?: Error) => {
        if (settled) return
        settled = true
        window.clearTimeout(timeoutId)
        link.onload = null
        link.onerror = null
        if (error) {
          link.remove()
          reject(error)
          return
        }
        link.dataset.havenLive2dStylesheetLoaded = "true"
        resolve()
      }

      if (link.dataset.havenLive2dStylesheetLoaded === "true") {
        resolve()
        return
      }

      timeoutId = window.setTimeout(
        () => finish(new Error("Local oh-my-live2d stylesheet timed out")),
        OML2D_STYLESHEET_TIMEOUT_MS,
      )
      link.rel = "stylesheet"
      link.href = OML2D_STYLESHEET_URL
      link.dataset.havenLive2dStylesheet = "oml2d"
      link.onload = () => finish()
      link.onerror = () => finish(new Error("Local oh-my-live2d stylesheet failed to load"))
      if (!existingLink) document.head.append(link)
    }).catch((error: unknown) => {
      stylesheetPromise = null
      throw error
    })
  }

  if (!signal) return stylesheetPromise
  if (signal.aborted) return Promise.reject(new DOMException("Aborted", "AbortError"))

  return new Promise<void>((resolve, reject) => {
    const abort = () => reject(new DOMException("Aborted", "AbortError"))
    const cleanup = () => signal.removeEventListener("abort", abort)
    signal.addEventListener("abort", abort, { once: true })
    void stylesheetPromise?.then(
      () => {
        cleanup()
        resolve()
      },
      (error: unknown) => {
        cleanup()
        reject(error)
      },
    )
  })
}

/**
 * Cubism 运行时必须先通过同源外部脚本加载。Vite 构建插件会从锁定的
 * oh-my-live2d 包中提取这段运行时，并移除原包的内联注入和远程版本检查。
 * 任一阶段失败都只降级 Live2D，不能阻止 App Shell 挂载。
 */
export async function loadOptionalOml2dRuntime(
  loader: Oml2dRuntimeLoader = () => import("oh-my-live2d"),
  loadCubismRuntime: CubismRuntimeLoader = loadLocalCubismRuntime,
  loadStylesheet: StylesheetLoader = loadLocalStylesheet,
  signal?: AbortSignal,
): Promise<Oml2dRuntimeModule | null> {
  try {
    await loadStylesheet(signal)
    if (signal?.aborted) return null
    await loadCubismRuntime()
    if (signal?.aborted) return null
    return await loader()
  } catch {
    return null
  }
}
