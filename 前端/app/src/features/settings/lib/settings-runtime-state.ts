import { useEffect, useRef, useState, useSyncExternalStore } from "react"
import { useLocation, useNavigate } from "react-router"
import type { ContinueItemDto } from "@/lib/ipc/generated/wire"
import { getHavenClientMode, getHavenClient, type HavenClientMode } from "@/lib/ipc/runtime"
import type {
  AppearanceSettingsValue,
  GeneralSettingsValue,
  SettingsValue,
} from "@/lib/ipc/settings-wire"
import { defaultSettingsValue } from "@/lib/ipc/settings-wire"
import { settingsGateway } from "../ipc/gateway"
import type { SettingsGateway } from "../ipc/gateway"

export type SettingsSectionId =
  | "general"
  | "appearance"
  | "playback"
  | "reading"
  | "comic"
  | "sources"
  | "storage"
  | "downloads"
  | "sync"
  | "ai"
  | "updates"
  | "privacy"
  | "about"

/** Tauri 中只开放已有真实数据通道的设置。
 * sources：V2-B 起接入来源注册表（source_registry_* 真实 IPC），加入白名单。
 * about：V02-SETTINGS-ABOUT-DIAGNOSTICS-008 通过 app_info/open_* 真实 IPC；
 * privacy：V02-SETTINGS-PRIVACY-DATA-007 仅开放搜索历史设置与 Artwork Cache 清理；
 * playback：V02-SETTINGS-PLAYBACK-004A 仅开放默认倍速与自动继续；
 * reading：V02-SETTINGS-READER-005A 开放文本类 Reader 的全局排版偏好；
 * comic：V02-SETTINGS-READER-COMIC-005B 开放 Comic Reader 已消费的全局模式、方向、间距和预加载窗口；
 * downloads：V02-SETTINGS-DOWNLOAD-006A 开放 Worker 已消费的并发与限速策略；
 * updates：V02-UPDATE-FOUNDATION-001 接入 Tauri 官方签名更新检查/安装。 */
export const PRODUCTION_SETTINGS_SECTIONS: readonly SettingsSectionId[] = [
  "general",
  "appearance",
  "playback",
  "reading",
  "comic",
  "downloads",
  "storage",
  "sources",
  "privacy",
  "about",
  "updates",
]

export function canUseSettingsSection(mode: HavenClientMode, section: SettingsSectionId): boolean {
  if (mode === "mock") return true
  return mode === "tauri" && PRODUCTION_SETTINGS_SECTIONS.includes(section)
}

export function visibleSettingsSectionIds(mode: HavenClientMode): readonly SettingsSectionId[] {
  return mode === "mock" ? ALL_SETTINGS_SECTIONS : PRODUCTION_SETTINGS_SECTIONS
}

export type SettingsRuntimeSnapshot = {
  general: GeneralSettingsValue
  appearance: AppearanceSettingsValue
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
  }
}

const DEFAULT_RUNTIME_SNAPSHOT: SettingsRuntimeSnapshot = {
  general: defaultGeneralValue(),
  appearance: defaultAppearanceValue(),
}

let runtimeSnapshot: SettingsRuntimeSnapshot = DEFAULT_RUNTIME_SNAPSHOT
const runtimeListeners = new Set<() => void>()

/**
 * App Shell 使用的短生命周期设置投影。
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

function emitSettingsRuntime(): void {
  for (const listener of runtimeListeners) listener()
}

export function publishSettingsRuntimeValue(value: SettingsValue): void {
  if (value.section === "privacy" || value.section === "playback" || value.section === "reading") return
  const next = value.section === "general"
    ? { ...runtimeSnapshot, general: value }
    : value.section === "appearance"
      ? { ...runtimeSnapshot, appearance: value }
      : runtimeSnapshot
  if (next.general === runtimeSnapshot.general && next.appearance === runtimeSnapshot.appearance) return
  runtimeSnapshot = next
  emitSettingsRuntime()
}

export function publishSettingsRuntimeSnapshot(snapshot: SettingsRuntimeSnapshot): void {
  runtimeSnapshot = {
    general: snapshot.general,
    appearance: snapshot.appearance,
  }
  emitSettingsRuntime()
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
  const media = typeof window.matchMedia === "function"
    ? window.matchMedia("(prefers-color-scheme: dark)")
    : null
  const apply = () => {
    const isDark = theme === "dark" || (theme === "system" && Boolean(media?.matches))
    root.classList.toggle("dark", isDark)
  }
  apply()
  if (theme !== "system" || !media) return () => undefined
  const listener = () => apply()
  media.addEventListener?.("change", listener)
  return () => media.removeEventListener?.("change", listener)
}

function applyAppearance(value: AppearanceSettingsValue): () => void {
  const root = document.documentElement
  root.dataset.havenDensity = value.density
  root.dataset.havenSidebar = value.sidebar
  root.dataset.havenReduceMotion = String(value.reduceMotion)
  return applyTheme(value.theme)
}

export async function loadSettingsRuntimeSnapshot(
  gateway: Pick<SettingsGateway, "settingsGet"> = settingsGateway,
): Promise<{ snapshot: SettingsRuntimeSnapshot; degraded: boolean }> {
  const [generalResult, appearanceResult] = await Promise.allSettled([
    gateway.settingsGet("general"),
    gateway.settingsGet("appearance"),
  ])
  const general = generalResult.status === "fulfilled" && generalResult.value.value.section === "general"
    ? generalResult.value.value
    : DEFAULT_RUNTIME_SNAPSHOT.general
  const appearance = appearanceResult.status === "fulfilled" && appearanceResult.value.value.section === "appearance"
    ? appearanceResult.value.value
    : DEFAULT_RUNTIME_SNAPSHOT.appearance
  return {
    snapshot: { general, appearance },
    degraded: generalResult.status === "rejected" || appearanceResult.status === "rejected",
  }
}

/**
 * App Shell 启动入口：读取本地设置、应用外观，并只在初始根路由上执行一次
 * 安全启动跳转。任何一个分区读取失败都回退默认值，不阻塞页面交互。
 */
export function useSettingsRuntime(): {
  snapshot: SettingsRuntimeSnapshot
  status: SettingsRuntimeStatus
} {
  const navigate = useNavigate()
  const location = useLocation()
  const mode = getHavenClientMode()
  const snapshot = useSyncExternalStore(
    subscribeSettingsRuntime,
    getSettingsRuntimeSnapshot,
    getSettingsRuntimeSnapshot,
  )
  const [status, setStatus] = useState<SettingsRuntimeStatus>("loading")
  const initialPathRef = useRef(location.pathname)
  const initialNavigationHandledRef = useRef(false)

  useEffect(() => {
    if (location.pathname !== initialPathRef.current) initialNavigationHandledRef.current = true
  }, [location.pathname])

  useEffect(() => {
    if (mode !== "tauri" && mode !== "mock") {
      setStatus("degraded")
      return
    }
    let active = true
    setStatus("loading")
    void loadSettingsRuntimeSnapshot().then(({ snapshot: loaded, degraded }) => {
      if (!active) return
      publishSettingsRuntimeSnapshot(loaded)
      setStatus(degraded ? "degraded" : "ready")
    })
    return () => {
      active = false
    }
  }, [mode])

  useEffect(() => applyAppearance(snapshot.appearance), [snapshot.appearance])

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

const ALL_SETTINGS_SECTIONS: readonly SettingsSectionId[] = [
  "general",
  "appearance",
  "playback",
  "reading",
  "comic",
  "sources",
  "storage",
  "downloads",
  "sync",
  "ai",
  "updates",
  "privacy",
  "about",
]
