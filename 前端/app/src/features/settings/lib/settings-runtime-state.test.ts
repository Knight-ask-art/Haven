import { describe, expect, it } from "vitest"
import {
  canUseSettingsSection,
  loadSettingsRuntimeSnapshot,
  resolveContinueRoute,
  resolveLaunchRoute,
  visibleSettingsSectionIds,
} from "./settings-runtime-state"

describe("settings runtime boundary", () => {
  it("keeps the complete settings Demo available only for the mock client", () => {
    expect(canUseSettingsSection("mock", "ai")).toBe(true)
    expect(canUseSettingsSection("mock", "sync")).toBe(true)
    expect(visibleSettingsSectionIds("mock")).toContain("ai")
  })

  it("opens only backed settings in Tauri（Comic/Privacy/Playback/Reading/Downloads 已接真实 IPC）", () => {
    expect(canUseSettingsSection("tauri", "general")).toBe(true)
    expect(canUseSettingsSection("tauri", "appearance")).toBe(true)
    expect(canUseSettingsSection("tauri", "playback")).toBe(true)
    expect(canUseSettingsSection("tauri", "reading")).toBe(true)
    expect(canUseSettingsSection("tauri", "comic")).toBe(true)
    expect(canUseSettingsSection("tauri", "downloads")).toBe(true)
    expect(canUseSettingsSection("tauri", "storage")).toBe(true)
    expect(canUseSettingsSection("tauri", "sources")).toBe(true)
    expect(canUseSettingsSection("tauri", "privacy")).toBe(true)
    expect(canUseSettingsSection("tauri", "about")).toBe(true)
    expect(canUseSettingsSection("tauri", "sync")).toBe(false)
    expect(canUseSettingsSection("tauri", "ai")).toBe(false)
    expect(visibleSettingsSectionIds("tauri")).toEqual([
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
    ])
  })

  it("fails closed for a production browser", () => {
    expect(canUseSettingsSection("unavailable", "general")).toBe(false)
    expect(canUseSettingsSection("unavailable", "storage")).toBe(false)
    expect(canUseSettingsSection("unavailable", "sources")).toBe(false)
    // 静态展示列表同源；可用性由 canUseSettingsSection 按 runtime 收口。
    expect(visibleSettingsSectionIds("unavailable")).toEqual([
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
    ])
  })

  it("maps only known continue actions to internal routes", () => {
    expect(resolveContinueRoute({ mediaItemId: "video-1", primaryAction: { kind: "playback", editionId: "edition-1", mediaItemId: "video-1", labelHint: "continue", locator: null } })).toBe("/player/video-1")
    expect(resolveContinueRoute({ mediaItemId: "book-1", primaryAction: { kind: "reader", editionId: "edition-1", mediaItemId: "book-1", labelHint: "continue", locator: null } })).toBe("/reader/book-1")
    expect(resolveContinueRoute({ mediaItemId: "comic-1", primaryAction: { kind: "comic", editionId: "edition-1", mediaItemId: "comic-1", labelHint: "continue", locator: null } })).toBe("/comic/comic-1")
    expect(resolveContinueRoute({ mediaItemId: "article-1", primaryAction: { kind: "article", editionId: "edition-1", mediaItemId: "article-1", labelHint: "continue", locator: null } })).toBe("/article/article-1")
    expect(resolveContinueRoute({ mediaItemId: "edition-item", primaryAction: { kind: "open_edition", editionId: "edition-2", mediaItemId: null, labelHint: "open", locator: null } })).toBe("/edition/edition-2")
    expect(resolveContinueRoute(null)).toBeNull()
    expect(resolveContinueRoute({ mediaItemId: "", primaryAction: { kind: "playback", editionId: "edition-1", mediaItemId: "", labelHint: "continue", locator: null } })).toBeNull()
  })

  it("gives restore-session a safe continue route and falls back to home", () => {
    const general = { section: "general" as const, launchPage: "library" as const, restoreSession: false, language: "zh_cn" as const, notifications: true }
    expect(resolveLaunchRoute(general)).toBe("/library")
    expect(resolveLaunchRoute({ ...general, restoreSession: true }, "/reader/book-1")).toBe("/reader/book-1")
    expect(resolveLaunchRoute({ ...general, launchPage: "continue" }, null)).toBe("/")
    expect(resolveLaunchRoute({ ...general, launchPage: "last_session" }, "/player/video-1")).toBe("/player/video-1")
  })

  it("degrades per section when a snapshot is unavailable or corrupted", async () => {
    const result = await loadSettingsRuntimeSnapshot({
      settingsGet: async (section) => {
        if (section === "general") throw new Error("corrupted settings row")
        return {
          value: { section: "appearance", theme: "dark", density: "compact", sidebar: "collapsed", reduceMotion: true },
          revision: "appearance-1",
        }
      },
    })
    expect(result.degraded).toBe(true)
    expect(result.snapshot.general).toMatchObject({ section: "general", launchPage: "home", restoreSession: false })
    expect(result.snapshot.appearance).toMatchObject({ section: "appearance", theme: "dark", density: "compact", sidebar: "collapsed", reduceMotion: true })
  })
})
