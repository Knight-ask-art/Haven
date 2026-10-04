import { describe, expect, it } from "vitest"
import {
  loadSettingsRuntimeSnapshot,
  resolveContinueRoute,
  resolveLaunchRoute,
} from "./settings-runtime-state"

// 这里刻意不再有「哪些分区可用」的用例：运行时状态模块已经不再持有分区白名单，
// 可导航分区由 settings-registry 的登记表唯一回答（见 settings-registry.test.ts
// 里对「第二套分区真相」的回归断言）。
describe("settings runtime boundary", () => {
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
          value: { section: "appearance", theme: "dark", density: "compact", sidebar: "collapsed", reduceMotion: true, interfaceFontMode: "system" },
          revision: "appearance-1",
        }
      },
    })
    expect(result.degraded).toBe(true)
    expect(result.snapshot.general).toMatchObject({ section: "general", launchPage: "home", restoreSession: false })
    expect(result.snapshot.appearance).toMatchObject({ section: "appearance", theme: "dark", density: "compact", sidebar: "collapsed", reduceMotion: true })
  })
})
