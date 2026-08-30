import { isTauriRuntime } from "@/lib/ipc/runtime"

/**
 * 受控内容 URI（haven-resource://artwork|stream|session/<opaque>）→ WebView 可请求地址。
 * Windows Tauri 自定义协议以 http://haven-resource.<authority>/ 形式呈现；
 * 非 Windows Tauri 直接用 haven-resource://；浏览器 mock 原样返回。
 */
export function controlledResourceUri(contentUri: string | null | undefined): string {
  if (!contentUri || !contentUri.startsWith("haven-resource://")) return contentUri ?? ""
  const rest = contentUri.slice("haven-resource://".length)
  const slash = rest.indexOf("/")
  if (slash <= 0) return contentUri
  const authority = rest.slice(0, slash)
  if (!/^[a-z]+$/.test(authority)) return contentUri
  const windowsWebView =
    isTauriRuntime() &&
    typeof navigator !== "undefined" &&
    /Windows/i.test(navigator.userAgent)
  return windowsWebView ? `http://haven-resource.${authority}/${rest.slice(slash + 1)}` : contentUri
}

/**
 * 受控海报 URL（契约 §36 C1）：haven://artwork/<opaque> 别名。
 */
export function artworkRequestUri(posterUri: string | null | undefined): string {
  if (!posterUri || !posterUri.startsWith("haven://artwork/")) return posterUri ?? ""
  const id = posterUri.slice("haven://artwork/".length)
  if (!id || !/^[A-Za-z0-9_-]+$/.test(id)) return ""
  return controlledResourceUri(`haven-resource://artwork/${id}`)
}

/**
 * 多尺寸 srcSet：资源协议只支持列表变体 200/400；详情原图不走这里。
 */
export function artworkSrcSet(posterUri: string | null | undefined): string | undefined {
  const base = artworkRequestUri(posterUri)
  if (!base || !isControlledArtworkRequest(base)) return undefined
  const widths = [200, 400]
  return widths.map((w) => `${base}?w=${w} ${w}w`).join(", ")
}

function isControlledArtworkRequest(value: string): boolean {
  return value.startsWith("haven-resource://artwork/") || value.startsWith("http://haven-resource.artwork/")
}
