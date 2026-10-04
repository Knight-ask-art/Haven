import { useEffect, useRef, useSyncExternalStore } from "react"
import { useLocation, useNavigate } from "react-router"
import type { ContinueItemDto } from "@/lib/ipc/generated/wire"
import { controlledResourceUri } from "@/lib/artwork-url"
import type { InterfaceFontAsset } from "@/lib/ipc/interface-font-wire"
import { getHavenClientMode, getHavenClient } from "@/lib/ipc/runtime"
import type {
  AppearanceSettingsValue,
  GeneralSettingsValue,
  SettingsValue,
} from "@/lib/ipc/settings-wire"
import { defaultSettingsValue } from "@/lib/ipc/settings-wire"
import { applyCustomThemeTokens, subscribeSystemPreference, systemPrefersDark } from "./appearance-runtime"
import { settingsGateway } from "../ipc/gateway"
import type { SettingsGateway } from "../ipc/gateway"
import { interfaceFontGateway } from "../ipc/interface-font-gateway"
import type { InterfaceFontGateway } from "../ipc/interface-font-gateway"
import { resolveInterfaceFont } from "./interface-font"

/**
 * 运行时投影只承载这两个分区，因此这里没有「分区白名单」——哪一页可导航、哪一页能
 * 渲染，由 settings-registry 的登记表唯一决定（见 `isSettingsSectionId`）。本模块
 * 只负责把已保存的值读进内存并投影到当前 WebView。
 */
export type SettingsRuntimeSnapshot = {
  general: GeneralSettingsValue
  appearance: AppearanceSettingsValue
  /**
   * 已知的导入字体资产（新的在前）。
   *
   * 存在的理由只有一个：`resolveInterfaceFont` 需要判断 `interfaceFontAssetId`
   * 是否仍然有效（已删除的 stale id 必须被拒绝应用），以及导入字体的 MIME/格式。
   * 它不是第二事实源——只是 Rust 返回的元数据在本 WebView 的短生命周期投影。
   */
  interfaceFontAssets: InterfaceFontAsset[]
}

export type SettingsRuntimeStatus = "loading" | "ready" | "degraded"

function defaultGeneralValue(): GeneralSettingsValue {
  const value = defaultSettingsValue("general")
  return value.section === "general" ? value : {
    section: "general",
    launchPage: "home",
    restoreSession: false,
    language: "zh_cn",
    notifications: true,
  }
}

function defaultAppearanceValue(): AppearanceSettingsValue {
  const value = defaultSettingsValue("appearance")
  return value.section === "appearance" ? value : {
    section: "appearance",
    theme: "system",
    density: "comfortable",
    sidebar: "auto",
    reduceMotion: false,
    interfaceFontMode: "system",
  }
}

const DEFAULT_RUNTIME_SNAPSHOT: SettingsRuntimeSnapshot = {
  general: defaultGeneralValue(),
  appearance: defaultAppearanceValue(),
  interfaceFontAssets: [],
}

let runtimeSnapshot: SettingsRuntimeSnapshot = DEFAULT_RUNTIME_SNAPSHOT
const runtimeListeners = new Set<() => void>()

/**
 * 应用根布局与壳层共用的短生命周期设置投影。
 *
 * Rust + SQLite 仍是唯一事实源；这里仅保存当前 WebView 已读取的外观/启动
 * 投影，避免跨页面设置保存后必须刷新才能看到效果。投影不可序列化到
 * localStorage，也不承担业务状态。
 */
export function getSettingsRuntimeSnapshot(): SettingsRuntimeSnapshot {
  return runtimeSnapshot
}

export function subscribeSettingsRuntime(listener: () => void): () => void {
  runtimeListeners.add(listener)
  return () => runtimeListeners.delete(listener)
}

/**
 * 只订阅当前运行时投影，不触发加载、不重复投影外观、也不做启动跳转。
 *
 * 应用根布局（AppRoot）是唯一调用 `useSettingsRuntime` 的地方——它负责读盘、投影
 * 与首屏跳转；只消费快照的组件（AppShell 的密度 / 侧栏 / 减少动效）走这里，因此
 * 导航到沉浸式路由时投影不会被卸载，也不会为了读一份值把加载路径跑第二遍。
 */
export function useSettingsRuntimeSnapshot(): SettingsRuntimeSnapshot {
  return useSyncExternalStore(
    subscribeSettingsRuntime,
    getSettingsRuntimeSnapshot,
    getSettingsRuntimeSnapshot,
  )
}

function emitSettingsRuntime(): void {
  for (const listener of runtimeListeners) listener()
}

// ---------- 启动读取状态（投影失败的对外事实） ----------

/**
 * 启动读盘的**用户可见事实**：loading / ready / degraded。
 *
 * 为什么它必须是一个可订阅的模块级状态，而不是 `useSettingsRuntime` 的局部 state：
 * 读取失败时运行时**安全回退到契约默认值**（外观、密度、导航留白、减少动效都不是
 * 用户保存的那份），而渲染这个回退结果的组件（AppShell / HomePage）没有能力区分
 * 「用户真的这么设置」与「我们没读到」。设置页是唯一能解释这件事的地方，它挂在
 * AppRoot 的**兄弟**路由上，读不到 AppRoot 的局部 state，因此状态必须放在两者都能
 * 读到的地方。
 *
 * 与 `runtimeSnapshot` 同一条边界：只活在当前渲染进程里，不落盘、不进 localStorage。
 */
let runtimeStatus: SettingsRuntimeStatus = "loading"
/**
 * 重试代数：`reloadSettingsRuntime()` 自增它，根布局的启动 effect 以它为依赖重跑。
 *
 * 与分区代数一样只是内存计数器：它的唯一用途是让「重新读取」这件事有一个可观察的
 * 触发点，而不是让设置页自己去 invoke 一次读盘（那会变成第二条启动路径）。
 */
let runtimeReloadToken = 0
const runtimeStatusListeners = new Set<() => void>()

export function getSettingsRuntimeStatus(): SettingsRuntimeStatus {
  return runtimeStatus
}

export function getSettingsRuntimeReloadToken(): number {
  return runtimeReloadToken
}

export function subscribeSettingsRuntimeStatus(listener: () => void): () => void {
  runtimeStatusListeners.add(listener)
  return () => runtimeStatusListeners.delete(listener)
}

/** 只在状态真的变了时广播；重复写同一个值不该让订阅者白重渲染一次。 */
function setSettingsRuntimeStatus(next: SettingsRuntimeStatus): void {
  if (next === runtimeStatus) return
  runtimeStatus = next
  for (const listener of runtimeStatusListeners) listener()
}

/**
 * 请求根布局重跑一次启动读盘（设置页错误提示上的「重新读取」）。
 *
 * 它不读盘、不改投影、更不改磁盘上的设置：只是把根布局的那次启动 effect 叫醒一次。
 */
export function reloadSettingsRuntime(): void {
  runtimeReloadToken += 1
  for (const listener of runtimeStatusListeners) listener()
}

/** 订阅启动读取状态；设置页用它决定要不要显示「本次读取失败」的说明。 */
export function useSettingsRuntimeStatus(): SettingsRuntimeStatus {
  return useSyncExternalStore(
    subscribeSettingsRuntimeStatus,
    getSettingsRuntimeStatus,
    getSettingsRuntimeStatus,
  )
}

function useSettingsRuntimeReloadToken(): number {
  return useSyncExternalStore(
    subscribeSettingsRuntimeStatus,
    getSettingsRuntimeReloadToken,
    getSettingsRuntimeReloadToken,
  )
}

/** 运行时投影只承载这两个分区；发布代数按分区各记一份。 */
type SettingsRuntimeSection = "general" | "appearance"

/** 一次发布前后的代数快照；调用方在异步请求发出前记下，回来时用它判断分区是否已更新。 */
export type SettingsRuntimeEpochs = Readonly<Record<SettingsRuntimeSection, number>> & {
  interfaceFontAssets: number
}

/**
 * 分区发布代数：每次某个分区的值真的被换掉时自增。
 *
 * 唯一用途是让首屏那次异步加载认得「我出发之后这一区被人换过」。加载在请求发出前
 * 记下代数，结果回来时只收口代数没变的分区，其余保留运行时里那份更新的值——没有它，
 * 用户在加载途中改的主题/壁纸会在加载落地的一瞬间被磁盘上的旧值顶回去。
 *
 * 它只是一个内存计数器：不落盘、不进 localStorage、不跨渲染进程，重启后从 0 开始
 * 也无所谓（重启本来就没有「在飞的加载」）。
 */
const runtimeEpochs: Record<SettingsRuntimeSection | "interfaceFontAssets", number> = {
  general: 0,
  appearance: 0,
  interfaceFontAssets: 0,
}

/** 取当前代数快照。 */
export function captureSettingsRuntimeEpochs(): SettingsRuntimeEpochs {
  return {
    general: runtimeEpochs.general,
    appearance: runtimeEpochs.appearance,
    interfaceFontAssets: runtimeEpochs.interfaceFontAssets,
  }
}

/** 只给真的换了值的分区记一笔；同一个值重复发布不该让在飞的加载作废。 */
function bumpEpochsFor(previous: SettingsRuntimeSnapshot, next: SettingsRuntimeSnapshot): void {
  if (next.general !== previous.general) runtimeEpochs.general += 1
  if (next.appearance !== previous.appearance) runtimeEpochs.appearance += 1
  if (!sameAssetList(next.interfaceFontAssets, previous.interfaceFontAssets)) {
    runtimeEpochs.interfaceFontAssets += 1
  }
}

export function publishSettingsRuntimeValue(value: SettingsValue): void {
  if (value.section === "privacy" || value.section === "playback" || value.section === "reading") return
  const next = value.section === "general"
    ? { ...runtimeSnapshot, general: value }
    : value.section === "appearance"
      ? { ...runtimeSnapshot, appearance: value }
      : runtimeSnapshot
  if (next.general === runtimeSnapshot.general && next.appearance === runtimeSnapshot.appearance) return
  bumpEpochsFor(runtimeSnapshot, next)
  runtimeSnapshot = next
  emitSettingsRuntime()
}

/**
 * 导入/删除字体后刷新资产投影。
 *
 * 与设置值同一条发布通道：AppShell 与设置页看到的是同一份资产列表，
 * 因此「刚导入的字体能否立即应用」「已删除的字体是否还被引用」不会出现两套答案。
 */
export function publishInterfaceFontAssets(assets: readonly InterfaceFontAsset[]): void {
  const next = [...assets]
  if (sameAssetList(next, runtimeSnapshot.interfaceFontAssets)) return
  runtimeEpochs.interfaceFontAssets += 1
  runtimeSnapshot = { ...runtimeSnapshot, interfaceFontAssets: next }
  emitSettingsRuntime()
}

function sameAssetList(
  left: readonly InterfaceFontAsset[],
  right: readonly InterfaceFontAsset[],
): boolean {
  if (left.length !== right.length) return false
  return left.every((asset, index) => {
    const other = right[index]
    return other !== undefined && asset.id === other.id && asset.familyName === other.familyName
  })
}

export function publishSettingsRuntimeSnapshot(snapshot: SettingsRuntimeSnapshot): void {
  const next: SettingsRuntimeSnapshot = {
    general: snapshot.general,
    appearance: snapshot.appearance,
    interfaceFontAssets: [...snapshot.interfaceFontAssets],
  }
  if (
    next.general === runtimeSnapshot.general &&
    next.appearance === runtimeSnapshot.appearance &&
    sameAssetList(next.interfaceFontAssets, runtimeSnapshot.interfaceFontAssets)
  ) return
  bumpEpochsFor(runtimeSnapshot, next)
  runtimeSnapshot = next
  emitSettingsRuntime()
}

/**
 * 首屏加载结果收口：只收口「加载期间没有发生过更新发布」的分区。
 *
 * 逐分区判断而不是一刀切：一次用户操作通常只改一个分区，没被改过的分区照常采用
 * 磁盘上的值（启动页跳转依赖 general，不能因为用户顺手换了个主题就一并放弃）。
 */
export function publishLoadedSettingsRuntime(
  loaded: SettingsRuntimeSnapshot,
  epochsAtRequestStart: SettingsRuntimeEpochs,
): void {
  const current = runtimeSnapshot
  publishSettingsRuntimeSnapshot({
    general:
      runtimeEpochs.general === epochsAtRequestStart.general ? loaded.general : current.general,
    appearance:
      runtimeEpochs.appearance === epochsAtRequestStart.appearance
        ? loaded.appearance
        : current.appearance,
    interfaceFontAssets:
      runtimeEpochs.interfaceFontAssets === epochsAtRequestStart.interfaceFontAssets
        ? loaded.interfaceFontAssets
        : current.interfaceFontAssets,
  })
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value.trim().length > 0
}

function routeSegment(value: unknown): string | null {
  return isNonEmptyString(value) ? encodeURIComponent(value) : null
}

/** 将后端明确给出的主操作映射为受控内部路由；未知/损坏值安全回退。 */
export function resolveContinueRoute(item: Pick<ContinueItemDto, "mediaItemId" | "primaryAction"> | null | undefined): string | null {
  if (!item || !isNonEmptyString(item.mediaItemId) || !item.primaryAction) return null
  const mediaItemId = routeSegment(item.mediaItemId)
  if (!mediaItemId) return null
  switch (item.primaryAction.kind) {
    case "playback":
      return `/player/${mediaItemId}`
    case "reader":
      return `/reader/${mediaItemId}`
    case "comic":
      return `/comic/${mediaItemId}`
    case "article":
      return `/article/${mediaItemId}`
    case "open_edition": {
      const editionId = routeSegment(item.primaryAction.editionId)
      return editionId ? `/edition/${editionId}` : null
    }
    default:
      return null
  }
}

/** 只生成已登记的内部启动路由；不接受任意外部路径。 */
export function resolveLaunchRoute(
  general: GeneralSettingsValue,
  continueRoute: string | null = null,
): string {
  if (general.restoreSession && continueRoute) return continueRoute
  switch (general.launchPage) {
    case "library":
      return "/library"
    case "continue":
    case "last_session":
      return continueRoute ?? "/"
    case "home":
    default:
      return "/"
  }
}

function applyTheme(theme: AppearanceSettingsValue["theme"]): () => void {
  const root = document.documentElement
  root.dataset.havenTheme = theme
  // `custom` 也跟随系统明暗：自定义主题自带 light/dark 两套调色板，明暗类名必须与
  // 运行时实际采用的那一套一致，否则暗色下会用到浅色 token。
  const followsSystem = theme === "system" || theme === "custom"
  const apply = () => {
    const isDark = theme === "dark" || (followsSystem && systemPrefersDark())
    root.classList.toggle("dark", isDark)
  }
  apply()
  if (!followsSystem) return () => undefined
  return subscribeSystemPreference("(prefers-color-scheme: dark)", apply)
}

/** 导入字体 `@font-face` 的宿主 `<style>`；同一时刻最多一条。 */
const INTERFACE_FONT_STYLE_ELEMENT_ID = "haven-interface-font-face"

/**
 * 把界面字体落到文档上。
 *
 * - `--ds-font-interface` 写在 documentElement 的内联样式上（最高优先级），
 *   `index.css` 的 body / .settings-page 读取它；未设置时两者都回落到既有字体栈。
 * - 导入字体额外注入一条 `@font-face`，`src` 是 opaque id 生成的受控资源地址
 *   （`haven-resource://font/<id>`，Windows WebView 自动换成兼容形态）。
 *   重启后设置里存的还是同一个 id，因此不需要任何临时状态即可重新生效。
 * - stale id / 非法族名不会产生任何 CSS（见 resolveInterfaceFont）。
 */
export function applyInterfaceFont(
  appearance: AppearanceSettingsValue,
  assets: readonly InterfaceFontAsset[],
): () => void {
  const root = document.documentElement
  const resolved = resolveInterfaceFont(appearance, assets, controlledResourceUri)
  root.dataset.havenInterfaceFont = resolved.appliedKey
  if (resolved.fontFamily === null) {
    root.style.removeProperty("--ds-font-interface")
    root.style.removeProperty("--haven-ui-font-family")
  } else {
    root.style.setProperty("--ds-font-interface", resolved.fontFamily)
    root.style.setProperty("--haven-ui-font-family", resolved.fontFamily)
  }

  const existing = document.getElementById(INTERFACE_FONT_STYLE_ELEMENT_ID)
  if (resolved.fontFaceCss === null) {
    existing?.remove()
    return () => {
      root.style.removeProperty("--ds-font-interface")
      root.style.removeProperty("--haven-ui-font-family")
      root.removeAttribute("data-haven-interface-font")
    }
  }
  const style = existing instanceof HTMLStyleElement
    ? existing
    : document.createElement("style")
  if (!(existing instanceof HTMLStyleElement)) {
    style.id = INTERFACE_FONT_STYLE_ELEMENT_ID
    document.head.appendChild(style)
  }
  if (style.textContent !== resolved.fontFaceCss) {
    style.textContent = resolved.fontFaceCss
  }
  return () => {
    root.style.removeProperty("--ds-font-interface")
    root.style.removeProperty("--haven-ui-font-family")
    root.removeAttribute("data-haven-interface-font")
    document.getElementById(INTERFACE_FONT_STYLE_ELEMENT_ID)?.remove()
  }
}

function applyAppearance(
  value: AppearanceSettingsValue,
  assets: readonly InterfaceFontAsset[],
): () => void {
  const root = document.documentElement
  root.dataset.havenDensity = value.density
  root.dataset.havenSidebar = value.sidebar
  root.dataset.havenReduceMotion = String(value.reduceMotion)
  const cleanupInterfaceFont = applyInterfaceFont(value, assets)
  const cleanupTheme = applyTheme(value.theme)
  const cleanupProjection = applyCustomThemeTokens(value)
  return () => {
    cleanupInterfaceFont()
    cleanupProjection()
    cleanupTheme()
  }
}

export async function loadSettingsRuntimeSnapshot(
  gateway: Pick<SettingsGateway, "settingsGet"> = settingsGateway,
  fonts: Pick<InterfaceFontGateway, "assetList"> = interfaceFontGateway,
): Promise<{ snapshot: SettingsRuntimeSnapshot; degraded: boolean }> {
  const [generalResult, appearanceResult, assetResult] = await Promise.allSettled([
    gateway.settingsGet("general"),
    gateway.settingsGet("appearance"),
    fonts.assetList(),
  ])
  const general = generalResult.status === "fulfilled" && generalResult.value.value.section === "general"
    ? generalResult.value.value
    : DEFAULT_RUNTIME_SNAPSHOT.general
  const appearance = appearanceResult.status === "fulfilled" && appearanceResult.value.value.section === "appearance"
    ? appearanceResult.value.value
    : DEFAULT_RUNTIME_SNAPSHOT.appearance
  // 资产列表读失败不把整个 Shell 判成 degraded：界面按 system 字体渲染仍然完全可用，
  // 只是「导入字体」暂时无法解析（resolveInterfaceFont 会拒绝应用未知 id）。
  const interfaceFontAssets = assetResult.status === "fulfilled" ? assetResult.value : []
  return {
    snapshot: { general, appearance, interfaceFontAssets },
    degraded: generalResult.status === "rejected" || appearanceResult.status === "rejected",
  }
}

/**
 * 应用根布局启动入口：读取本地设置、把外观投影到 documentElement，并只在初始根
 * 路由上执行一次安全启动跳转。任何一个分区读取失败都回退默认值，不阻塞页面交互。
 *
 * 它必须挂在**覆盖全部主要路由**的根布局上，而不是某个壳层里：外观投影（自定义
 * token / 字体 / 明暗 / 密度）的清理函数跟着调用它的组件走，挂在壳层里会让进入
 * player / reader / comic / article 这些壳层外的路由时把主题一起清掉。
 *
 * 返回的 `status` 同时被发布到模块级状态上（`useSettingsRuntimeStatus`），因为
 * 回退默认值这件事必须能被设置页说明——只有它知道刚才读到的是磁盘上的值还是默认值。
 */
export function useSettingsRuntime(): {
  snapshot: SettingsRuntimeSnapshot
  status: SettingsRuntimeStatus
} {
  const navigate = useNavigate()
  const location = useLocation()
  const mode = getHavenClientMode()
  const snapshot = useSettingsRuntimeSnapshot()
  const status = useSettingsRuntimeStatus()
  // 「重新读取」的唯一入口是设置页的那个按钮：它自增代数，下面这个 effect 跟着重跑。
  const reloadToken = useSettingsRuntimeReloadToken()
  const initialPathRef = useRef(location.pathname)
  const initialNavigationHandledRef = useRef(false)

  useEffect(() => {
    if (location.pathname !== initialPathRef.current) initialNavigationHandledRef.current = true
  }, [location.pathname])

  useEffect(() => {
    if (mode !== "tauri" && mode !== "mock") {
      setSettingsRuntimeStatus("degraded")
      return
    }
    let active = true
    setSettingsRuntimeStatus("loading")
    // 在请求发出之前记下代数：这段时间里用户改过的分区不能被这次加载结果顶回去。
    const epochsAtRequestStart = captureSettingsRuntimeEpochs()
    void loadSettingsRuntimeSnapshot().then(({ snapshot: loaded, degraded }) => {
      if (!active) return
      publishLoadedSettingsRuntime(loaded, epochsAtRequestStart)
      setSettingsRuntimeStatus(degraded ? "degraded" : "ready")
    })
    return () => {
      active = false
    }
  }, [mode, reloadToken])

  useEffect(
    () => applyAppearance(snapshot.appearance, snapshot.interfaceFontAssets),
    [snapshot.appearance, snapshot.interfaceFontAssets],
  )

  useEffect(() => {
    if (mode !== "tauri" && mode !== "mock") return
    if (initialPathRef.current !== "/" || initialNavigationHandledRef.current) return
    let active = true
    const general = snapshot.general
    const needsContinue = general.restoreSession || general.launchPage === "continue" || general.launchPage === "last_session"
    if (!needsContinue) {
      const target = resolveLaunchRoute(general)
      if (target !== "/" && active) {
        initialNavigationHandledRef.current = true
        navigate(target, { replace: true })
      }
      return () => {
        active = false
      }
    }

    void getHavenClient().homeGet().then((home) => {
      if (!active || initialNavigationHandledRef.current) return
      const firstContinue = Array.isArray(home.continueItems) ? home.continueItems[0] : undefined
      const target = resolveLaunchRoute(general, resolveContinueRoute(firstContinue))
      if (target !== "/") {
        initialNavigationHandledRef.current = true
        navigate(target, { replace: true })
      } else {
        initialNavigationHandledRef.current = true
      }
    }).catch(() => {
      if (active) initialNavigationHandledRef.current = true
    })
    return () => {
      active = false
    }
  }, [mode, navigate, snapshot.general])

  return { snapshot, status }
}
