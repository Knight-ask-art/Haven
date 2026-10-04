import { describe, expect, it } from "vitest"
import { MockHavenClient } from "./mock-client"
import { defaultHomeLayout, defaultOverviewLayout, guardAppearanceAsset } from "./settings-wire"
import type { AppearanceAssetWire, HomeLayoutWire, OverviewLayoutWire } from "./settings-wire"

const request = { mediaItemId: "4", engine: "playback" as const }
const uuidPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/

const FONT_ASSET_ID = "0196f0d2-0000-7000-8000-00000000a201"
const WALLPAPER_ASSET_ID = "0196f0d2-0000-7000-8000-00000000a202"

// 预置投影是进程内构造的 typed 值：`byteSize` 与 IPC 的 JSON 载荷同形，都是 number
// （DTO 上的 u64 已按 `#[ts(type = "number")]` 对齐真实载荷，不再是 bigint 标注）。
function fontAsset(): AppearanceAssetWire {
  return {
    assetId: FONT_ASSET_ID,
    kind: "font",
    state: "validated",
    byteSize: 1024,
    displayName: "霞鹜文楷",
  }
}

function wallpaperAsset(): AppearanceAssetWire {
  return {
    assetId: WALLPAPER_ASSET_ID,
    kind: "static_wallpaper",
    state: "validated",
    byteSize: 2048,
    displayName: null,
  }
}

describe("MockHavenClient session lifecycle", () => {
  it("creates unique canonical tokens and closes known, unknown, and repeated tokens idempotently", async () => {
    const client = new MockHavenClient()
    const first = await client.sessionOpen(request)
    const second = await client.sessionOpen(request)

    expect(first.sessionId).toMatch(uuidPattern)
    expect(second.sessionId).toMatch(uuidPattern)
    expect(second.sessionId).not.toBe(first.sessionId)
    await expect(client.sessionClose({ sessionId: first.sessionId })).resolves.toEqual({ schemaVersion: 1, closed: true })
    await expect(client.sessionClose({ sessionId: first.sessionId })).resolves.toEqual({ schemaVersion: 1, closed: true })
    await expect(client.sessionClose({ sessionId: "0196f0d2-0000-7000-8000-ffffffffffff" }))
      .resolves.toEqual({ schemaVersion: 1, closed: true })
  })

  it("merges sparse edition and media-item preference patches field by field", async () => {
    const client = new MockHavenClient(false, { seedSettings: false })
    const identity = { mediaItemId: "media-1", editionId: "edition-1" }

    await expect(client.preferenceGet(identity)).resolves.toMatchObject({
      effectiveReading: { fontSize: "medium", theme: "warm" },
    })

    await client.preferenceUpdate({
      ...identity,
      target: "edition",
      readingPatch: { fontSize: "large" },
      comicPatch: null,
      expectedRevision: null,
    })
    await client.preferenceUpdate({
      ...identity,
      target: "media_item",
      readingPatch: { theme: "dark" },
      comicPatch: null,
      expectedRevision: null,
    })

    const result = await client.preferenceGet(identity)
    expect(result.effectiveReading).toMatchObject({ fontSize: "large", theme: "dark" })
    expect(result.effectiveReading.section).toBe("reading")
    expect(result.effectiveComic.section).toBe("comic")
  })

  it("keeps comic manifests stable while active and rotates every runtime identity after close", async () => {
    const client = new MockHavenClient()
    const firstSession = await client.sessionOpen({ mediaItemId: "4", engine: "comic" })
    expect(firstSession.contentUri).toBeNull()

    const first = await client.comicPageManifestGet({ sessionId: firstSession.sessionId })
    const repeated = await client.comicPageManifestGet({ sessionId: firstSession.sessionId })
    expect(repeated).toEqual(first)
    expect(first.pageCount).toBe(first.pages.length)
    expect(new Set(first.pages.map((page) => page.pageId)).size).toBe(first.pages.length)
    const grants = first.pages.flatMap((page) => page.contentUri ? [page.contentUri] : [])
    expect(new Set(grants).size).toBe(grants.length)

    await client.sessionClose({ sessionId: firstSession.sessionId })
    await expect(client.comicPageManifestGet({ sessionId: firstSession.sessionId }))
      .rejects.toHaveProperty("code", "RESOURCE_NOT_FOUND")

    const reopened = await client.sessionOpen({ mediaItemId: "4", engine: "comic" })
    const second = await client.comicPageManifestGet({ sessionId: reopened.sessionId })
    expect(reopened.sessionId).not.toBe(firstSession.sessionId)
    expect(second.pages.map((page) => page.pageId)).not.toEqual(first.pages.map((page) => page.pageId))
    expect(second.pages.map((page) => page.contentUri)).not.toEqual(first.pages.map((page) => page.contentUri))
  })

  it("returns the sanitized multi-chapter catalog fixture with source-level states", async () => {
    const client = new MockHavenClient()
    const catalog = await client.comicChapterCatalogGet({
      sourceId: "mangadex",
      remoteWorkId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    })
    expect(catalog.chapters).toHaveLength(3)
    expect(catalog.chapters.map((chapter) => chapter.availability)).toEqual([
      "available",
      "temporarily_unavailable",
      "external_only",
    ])
    expect(catalog.chapters[0].editionProfile.scanGroupKind).toBe("content_line")
  })

  it("keeps the explicit refresh method available in browser mocks", async () => {
    const client = new MockHavenClient()
    await expect(client.comicChapterCatalogRefresh({
      sourceId: "mangadex",
      remoteWorkId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    })).resolves.toEqual(await client.comicChapterCatalogGet({
      sourceId: "mangadex",
      remoteWorkId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    }))
  })

  it("returns an explicit empty source-candidate projection in browser mocks", async () => {
    const client = new MockHavenClient()
    const source = {
      sourceId: "mangadex",
      remoteWorkId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
      remoteChapterId: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
    } as const
    await expect(client.comicChapterSourceCandidatesGet({ source })).resolves.toEqual({
      schemaVersion: 1,
      source,
      currentMediaItemId: "0196f0d2-0000-7000-8000-000000000000",
      candidates: [],
      truncated: false,
    })
  })

  it("projects the persisted media identities and refresh generation", async () => {
    const client = new MockHavenClient()
    const catalog = await client.comicChapterCatalogRegisteredGet({
      sourceId: "mangadex",
      remoteWorkId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    })
    expect(catalog.refreshState).toMatchObject({ generation: 1, truncated: false })
    expect(catalog.chapters).toHaveLength(3)
    expect(catalog.chapters.map((chapter) => chapter.sourceOrder)).toEqual([0, 1, 2])
    expect(catalog.chapters.every((chapter) => uuidPattern.test(chapter.mediaItemId))).toBe(true)
    expect(catalog.chapters.some((chapter) => chapter.availability === "missing")).toBe(false)
  })

  it("rejects manifest access for a non-comic session", async () => {
    const client = new MockHavenClient()
    const session = await client.sessionOpen(request)
    await expect(client.comicPageManifestGet({ sessionId: session.sessionId }))
      .rejects.toHaveProperty("code", "FORMAT_UNSUPPORTED")
  })

  it("returns deterministic reader TOC for reader sessions and rejects unknown/foreign sessions", async () => {
    const client = new MockHavenClient()
    const readerSession = await client.sessionOpen({ mediaItemId: "4", engine: "reader" })
    const toc = await client.readerTocGet({ sessionId: readerSession.sessionId })
    expect(toc.schemaVersion).toBe(1)
    expect(toc.sessionId).toBe(readerSession.sessionId)
    expect(toc.items.length).toBeGreaterThan(0)
    expect(toc.items.every((item) => /^[0-9a-f]{16}$/.test(item.id))).toBe(true)
    expect(toc.items.every((item) => item.progression >= 0 && item.progression <= 1)).toBe(true)
    const repeated = await client.readerTocGet({ sessionId: readerSession.sessionId })
    expect(repeated).toEqual(toc)

    await expect(client.readerTocGet({ sessionId: "0196f0d2-0000-7000-8000-ffffffffffff" }))
      .rejects.toHaveProperty("code", "RESOURCE_NOT_FOUND")

    const playbackSession = await client.sessionOpen(request)
    await expect(client.readerTocGet({ sessionId: playbackSession.sessionId }))
      .rejects.toHaveProperty("code", "FORMAT_UNSUPPORTED")
  })

  it("returns explicit Mock About facts and rejects directory access", async () => {
    const client = new MockHavenClient()
    const info = await client.appInfoGet()
    expect(info.appVersion).toBe("Mock")
    expect(info.directories.every((directory) => directory.canOpen === false)).toBe(true)
    await expect(client.openDataDirectory()).rejects.toHaveProperty("code", "APP_DIRECTORY_UNAVAILABLE")
  })

  it("keeps updater unavailable in Browser Mock", async () => {
    const client = new MockHavenClient()
    await expect(client.updateCheck()).rejects.toMatchObject({
      code: "UPDATER_UNAVAILABLE",
      retryable: false,
    })
    await expect(client.updateInstall()).rejects.toMatchObject({
      code: "UPDATER_UNAVAILABLE",
      retryable: false,
    })
  })

  it("projects custom sources into the registry and keeps their enabled state scoped", async () => {
    const client = new MockHavenClient()
    const added = await client.sourceAdd({
      displayName: "我的 OPDS 书库",
      endpoint: "https://example.invalid/opds",
    })
    let registry = await client.sourceRegistryList()
    let custom = registry.sources.find((source) => source.sourceId === added.sourceId)
    expect(custom).toMatchObject({
      displayName: "我的 OPDS 书库",
      categories: ["book"],
      mode: "single",
      endpointConfigured: true,
      enabled: false,
    })

    await client.sourceRegistrySet({ sourceId: added.sourceId, enabled: true })
    registry = await client.sourceRegistryList()
    custom = registry.sources.find((source) => source.sourceId === added.sourceId)
    expect(custom?.enabled).toBe(true)

    await client.sourceRemove({ sourceId: added.sourceId })
    registry = await client.sourceRegistryList()
    expect(registry.sources.some((source) => source.sourceId === added.sourceId)).toBe(false)
  })

  it("projects RSS/Atom feed sources as periodical and refuses their credential path", async () => {
    const client = new MockHavenClient()
    const added = await client.sourceAdd({
      displayName: "我的订阅",
      endpoint: "https://feeds.example.invalid/rss.xml",
      kind: "feed",
    })
    expect(added.sourceId.startsWith("custom-feed-")).toBe(true)

    const registry = await client.sourceRegistryList()
    const feed = registry.sources.find((source) => source.sourceId === added.sourceId)
    expect(feed).toMatchObject({
      displayName: "我的订阅",
      categories: ["periodical"],
      kinds: ["search", "online_read", "offline_download"],
      mode: "single",
      endpointConfigured: true,
      enabled: false,
    })
    expect(feed?.notes.includes("feeds.example.invalid")).toBe(false)

    // 订阅源没有受控的 HTTP 认证路径：不得写入一条不会被消费的凭据。
    await expect(
      client.sourceSetCredential({ sourceId: added.sourceId, secret: "s3cret" }),
    ).rejects.toHaveProperty("code", "INVALID_ARGUMENT")

    await client.sourceRemove({ sourceId: added.sourceId })
    const after = await client.sourceRegistryList()
    expect(after.sources.some((source) => source.sourceId === added.sourceId)).toBe(false)
  })

  it("rejects feed endpoints that are not HTTPS or that carry a query string", async () => {
    const client = new MockHavenClient()
    await expect(
      client.sourceAdd({ displayName: "明文", endpoint: "http://feeds.example.invalid/rss.xml", kind: "feed" }),
    ).rejects.toHaveProperty("code", "INVALID_ARGUMENT")
    await expect(
      client.sourceAdd({
        displayName: "令牌",
        endpoint: "https://feeds.example.invalid/rss.xml?token=secret",
        kind: "feed",
      }),
    ).rejects.toHaveProperty("code", "INVALID_ARGUMENT")
    // 缺省 kind 仍是 OPDS，保持既有行为不变。
    const legacy = await client.sourceAdd({
      displayName: "旧调用",
      endpoint: "https://example.invalid/opds",
    })
    expect(legacy.sourceId.startsWith("custom-")).toBe(true)
    expect(legacy.sourceId.startsWith("custom-feed-")).toBe(false)
  })

  it("starts with the broker disabled and only reaches listening after an explicit enable", async () => {
    const client = new MockHavenClient()
    await expect(client.agentBrokerStatus()).resolves.toEqual({
      schemaVersion: 1,
      status: "disabled",
      endpoint: null,
      reason: null,
    })

    const enabled = await client.agentBrokerEnable()
    expect(enabled).toEqual({
      schemaVersion: 1,
      status: "listening",
      endpoint: expect.any(String),
      reason: null,
    })
    expect(Object.keys(enabled).sort()).toEqual(["endpoint", "reason", "schemaVersion", "status"])
    // 幂等：重复 enable 返回同一端点，且状态是查询出来的事实而不是一次性返回值。
    await expect(client.agentBrokerEnable()).resolves.toEqual(enabled)
    await expect(client.agentBrokerStatus()).resolves.toEqual(enabled)

    // 未开启时 disable 同样幂等；关闭后端点从投影里撤下。
    await expect(client.agentBrokerDisable()).resolves.toEqual({
      schemaVersion: 1,
      status: "disabled",
      endpoint: null,
      reason: null,
    })
    await expect(client.agentBrokerDisable()).resolves.toEqual({
      schemaVersion: 1,
      status: "disabled",
      endpoint: null,
      reason: null,
    })
  })

  it("marks its broker endpoint as a browser-preview marker instead of faking a local endpoint", async () => {
    const endpoint = (await new MockHavenClient().agentBrokerEnable()).endpoint ?? ""
    // 浏览器里没有 Rust Broker：返回 `\\.\pipe\…` 或 `*.sock` 形状的假地址，会让
    // 页面把一段永远连不上的路径当成可用端点展示并复制给用户。
    expect(endpoint).toContain("mock")
    expect(endpoint).not.toMatch(/^\\\\\.\\pipe\\/)
    expect(endpoint).not.toMatch(/\.sock$/)
  })
})

describe("MockHavenClient appearance state", () => {
  it("starts with an empty asset list and no saved home layout", async () => {
    const client = new MockHavenClient()

    await expect(client.appearanceAssetsList({ kind: null }))
      .resolves.toEqual({ schemaVersion: 1, assets: [] })
    await expect(client.homeLayoutGet())
      .resolves.toEqual({ layout: defaultHomeLayout(), revision: null })
  })

  it("lists only preset assets and filters them by kind", async () => {
    const client = new MockHavenClient(false, {
      appearanceAssets: [fontAsset(), wallpaperAsset()],
    })

    expect((await client.appearanceAssetsList({ kind: null })).assets).toHaveLength(2)
    expect((await client.appearanceAssetsList({ kind: "font" })).assets.map((asset) => asset.assetId))
      .toEqual([FONT_ASSET_ID])
    expect((await client.appearanceAssetsList({ kind: "dynamic_wallpaper" })).assets).toEqual([])
  })

  it("keeps preset assets on the guard-accepted side of the byteSize boundary", async () => {
    expect(guardAppearanceAsset(fontAsset())).toBe(true)
    // 接受 number 不等于放宽守卫：负数、小数、非安全整数与 bigint 仍然被拒绝
    // （bigint 从来不是 JSON 载荷的形态，见 settings-wire 的守卫说明）。
    expect(guardAppearanceAsset({ ...fontAsset(), byteSize: -1 })).toBe(false)
    expect(guardAppearanceAsset({ ...fontAsset(), byteSize: 1.5 })).toBe(false)
    expect(guardAppearanceAsset({ ...fontAsset(), byteSize: Number.MAX_SAFE_INTEGER + 2 })).toBe(false)

    // Mock 原样透传预置投影，不做静默转换。
    const client = new MockHavenClient(false, { appearanceAssets: [fontAsset()] })
    const listed = (await client.appearanceAssetsList({ kind: null })).assets
    expect(listed).toEqual([fontAsset()])
    expect(typeof listed[0].byteSize).toBe("number")
  })

  it("never pretends to pick a file on import", async () => {
    const client = new MockHavenClient()

    await expect(client.appearanceAssetImport({ kind: "font", displayName: null }))
      .rejects.toMatchObject({ code: "OPERATION_CANCELLED", retryable: false })
    // 没有选中文件就不可能凭空多出一个资产。
    expect((await client.appearanceAssetsList({ kind: null })).assets).toEqual([])
  })

  it("deletes known assets, treats unknown ids as a no-op and rejects non-canonical ids", async () => {
    const client = new MockHavenClient(false, {
      appearanceAssets: [fontAsset(), wallpaperAsset()],
    })

    await expect(client.appearanceAssetDelete({ assetId: WALLPAPER_ASSET_ID }))
      .resolves.toEqual({ deleted: true, fileRemoved: true })
    await expect(client.appearanceAssetDelete({ assetId: WALLPAPER_ASSET_ID }))
      .resolves.toEqual({ deleted: false, fileRemoved: false })
    await expect(client.appearanceAssetDelete({ assetId: FONT_ASSET_ID.toUpperCase() }))
      .rejects.toMatchObject({ code: "INVALID_ID" })
    expect((await client.appearanceAssetsList({ kind: null })).assets).toEqual([fontAsset()])
  })

  it("saves an explicit empty layout, replays it idempotently and resets through revision CAS", async () => {
    const client = new MockHavenClient()
    const empty: HomeLayoutWire = { schemaVersion: 1, modules: [] }

    const saved = await client.homeLayoutSave({ expectedRevision: null, layout: empty })
    expect(saved.changed).toBe(true)
    expect(saved.layout).toEqual(empty)
    expect(saved.revision).toEqual(expect.any(String))

    // 「保存了空布局」是显式事实：空 modules + 非空 revision，与「从未保存」不同。
    await expect(client.homeLayoutGet())
      .resolves.toEqual({ layout: empty, revision: saved.revision })

    // 幂等重放：同值不产生新 revision。
    const repeated = await client.homeLayoutSave({ expectedRevision: saved.revision, layout: empty })
    expect(repeated.changed).toBe(false)
    expect(repeated.revision).toBe(saved.revision)

    // 陈旧 revision → REVISION_CONFLICT 且零写入（布局与 revision 保持原样）。
    await expect(client.homeLayoutReset({ expectedRevision: null }))
      .rejects.toMatchObject({ code: "REVISION_CONFLICT" })
    await expect(client.homeLayoutSave({ expectedRevision: null, layout: defaultHomeLayout() }))
      .rejects.toMatchObject({ code: "REVISION_CONFLICT" })
    await expect(client.homeLayoutGet())
      .resolves.toEqual({ layout: empty, revision: saved.revision })

    const reset = await client.homeLayoutReset({ expectedRevision: saved.revision })
    expect(reset.changed).toBe(true)
    expect(reset.revision).toBeNull()
    expect(reset.layout).toEqual(defaultHomeLayout())

    // 重置后回到「从未保存」：读取不再带 revision。
    await expect(client.homeLayoutGet())
      .resolves.toEqual({ layout: defaultHomeLayout(), revision: null })
  })

  it("is idempotent when resetting a layout that was never saved", async () => {
    const client = new MockHavenClient()

    await expect(client.homeLayoutReset({ expectedRevision: null }))
      .resolves.toEqual({ layout: defaultHomeLayout(), revision: null, changed: false })
  })

  it("canonicalizes module order so a reordered identical layout is a no-op", async () => {
    const client = new MockHavenClient()
    const layout = defaultHomeLayout()

    const saved = await client.homeLayoutSave({ expectedRevision: null, layout })
    expect(saved.changed).toBe(true)

    const reordered: HomeLayoutWire = { schemaVersion: 1, modules: [...layout.modules].reverse() }
    const again = await client.homeLayoutSave({ expectedRevision: saved.revision, layout: reordered })
    expect(again.changed).toBe(false)
    expect(again.revision).toBe(saved.revision)
  })

  it("rejects an invalid layout before any CAS write", async () => {
    const client = new MockHavenClient()
    const overlapping = {
      schemaVersion: 1,
      modules: [
        { module: "continue", size: "medium", row: 0, column: 0, order: 0 },
        { module: "recently_added", size: "small", row: 0, column: 1, order: 1 },
      ],
    } as HomeLayoutWire

    await expect(client.homeLayoutSave({ expectedRevision: null, layout: overlapping }))
      .rejects.toMatchObject({ code: "APPEARANCE_INVALID_HOME_LAYOUT" })
    await expect(client.homeLayoutGet())
      .resolves.toEqual({ layout: defaultHomeLayout(), revision: null })
  })

  it("does not leak its internal layout state to callers", async () => {
    const client = new MockHavenClient()
    const saved = await client.homeLayoutSave({
      expectedRevision: null,
      layout: { schemaVersion: 1, modules: [] },
    })

    saved.layout.modules.push({ module: "continue", size: "medium", row: 0, column: 0, order: 0 })
    await expect(client.homeLayoutGet())
      .resolves.toEqual({ layout: { schemaVersion: 1, modules: [] }, revision: saved.revision })
  })

  it("starts with no saved overview layout and keeps it independent from the home layout", async () => {
    const client = new MockHavenClient()

    await expect(client.overviewLayoutGet())
      .resolves.toEqual({ layout: defaultOverviewLayout(), revision: null })

    // 保存总览不得在首页那一侧造出状态。
    const saved = await client.overviewLayoutSave({
      expectedRevision: null,
      layout: defaultOverviewLayout(),
    })
    expect(saved.changed).toBe(true)
    await expect(client.homeLayoutGet())
      .resolves.toEqual({ layout: defaultHomeLayout(), revision: null })

    // 首页的 revision 也不是总览的 CAS token。
    await expect(client.overviewLayoutSave({
      expectedRevision: saved.revision,
      layout: { schemaVersion: 1, modules: [] },
    })).resolves.toMatchObject({ changed: true })
    await expect(client.overviewLayoutReset({ expectedRevision: null }))
      .rejects.toMatchObject({ code: "REVISION_CONFLICT" })
  })

  it("saves an explicit empty overview layout and resets it through revision CAS", async () => {
    const client = new MockHavenClient()
    const empty: OverviewLayoutWire = { schemaVersion: 1, modules: [] }

    const saved = await client.overviewLayoutSave({ expectedRevision: null, layout: empty })
    expect(saved.changed).toBe(true)
    expect(saved.layout).toEqual(empty)
    expect(saved.revision).toEqual(expect.any(String))

    await expect(client.overviewLayoutGet())
      .resolves.toEqual({ layout: empty, revision: saved.revision })

    // 幂等重放：同值不产生新 revision。
    const repeated = await client.overviewLayoutSave({ expectedRevision: saved.revision, layout: empty })
    expect(repeated.changed).toBe(false)
    expect(repeated.revision).toBe(saved.revision)

    // 陈旧 revision → REVISION_CONFLICT 且零写入。
    await expect(client.overviewLayoutReset({ expectedRevision: null }))
      .rejects.toMatchObject({ code: "REVISION_CONFLICT" })
    await expect(client.overviewLayoutSave({ expectedRevision: null, layout: defaultOverviewLayout() }))
      .rejects.toMatchObject({ code: "REVISION_CONFLICT" })
    await expect(client.overviewLayoutGet())
      .resolves.toEqual({ layout: empty, revision: saved.revision })

    const reset = await client.overviewLayoutReset({ expectedRevision: saved.revision })
    expect(reset.changed).toBe(true)
    expect(reset.revision).toBeNull()
    expect(reset.layout).toEqual(defaultOverviewLayout())

    await expect(client.overviewLayoutGet())
      .resolves.toEqual({ layout: defaultOverviewLayout(), revision: null })
  })

  it("is idempotent when resetting an overview layout that was never saved", async () => {
    const client = new MockHavenClient()

    await expect(client.overviewLayoutReset({ expectedRevision: null }))
      .resolves.toEqual({ layout: defaultOverviewLayout(), revision: null, changed: false })
  })

  it("canonicalizes overview module order so a reordered identical layout is a no-op", async () => {
    const client = new MockHavenClient()
    const layout = defaultOverviewLayout()

    const saved = await client.overviewLayoutSave({ expectedRevision: null, layout })
    expect(saved.changed).toBe(true)

    const reordered: OverviewLayoutWire = { schemaVersion: 1, modules: [...layout.modules].reverse() }
    const again = await client.overviewLayoutSave({ expectedRevision: saved.revision, layout: reordered })
    expect(again.changed).toBe(false)
    expect(again.revision).toBe(saved.revision)
  })

  it("rejects an invalid overview layout before any CAS write", async () => {
    const client = new MockHavenClient()
    // large 占满整行 0，small 落在它覆盖的最后一列：起始格不同，占格却重叠。
    const overlapping = {
      schemaVersion: 1,
      modules: [
        { module: "metrics", size: "large", row: 0, column: 0, order: 0 },
        { module: "preferences", size: "small", row: 0, column: 2, order: 1 },
      ],
    } as OverviewLayoutWire

    await expect(client.overviewLayoutSave({ expectedRevision: null, layout: overlapping }))
      .rejects.toMatchObject({ code: "APPEARANCE_INVALID_OVERVIEW_LAYOUT" })
    // 首页布局的非法代码不是这一个：两份布局各有自己的错误码。
    await expect(client.overviewLayoutGet())
      .resolves.toEqual({ layout: defaultOverviewLayout(), revision: null })
  })

  it("does not leak its internal overview layout state to callers", async () => {
    const client = new MockHavenClient()
    const saved = await client.overviewLayoutSave({
      expectedRevision: null,
      layout: { schemaVersion: 1, modules: [] },
    })

    saved.layout.modules.push({ module: "metrics", size: "large", row: 0, column: 0, order: 0 })
    await expect(client.overviewLayoutGet())
      .resolves.toEqual({ layout: { schemaVersion: 1, modules: [] }, revision: saved.revision })
  })
})

describe("MockHavenClient tvbox_config_preview", () => {
  it("validates the address like the backend, then fails closed", async () => {
    const client = new MockHavenClient()

    // 输入校验与后端同形：空地址 / 超长地址都是 INVALID_ARGUMENT。
    for (const url of ["", "   "]) {
      await expect(client.tvboxConfigPreview({ url }))
        .rejects.toMatchObject({ code: "INVALID_ARGUMENT", retryable: false })
    }
    await expect(client.tvboxConfigPreview({ url: `https://e.invalid/${"a".repeat(2048)}` }))
      .rejects.toMatchObject({ code: "INVALID_ARGUMENT" })

    // 合法地址也不编造摘要：浏览器预览里没有受控 HTTP，假装取回过配置就是假能力。
    await expect(client.tvboxConfigPreview({ url: "https://config.example.invalid/tvbox.json" }))
      .rejects.toMatchObject({ code: "HAVEN_CAPABILITY_UNAVAILABLE", retryable: false })
  })
})

describe("MockHavenClient tvbox_config_save", () => {
  it("validates the inputs like the backend, then fails closed", async () => {
    const client = new MockHavenClient()

    // 输入校验与后端同形：空/超长显示名与地址都是 INVALID_ARGUMENT。
    for (const request of [
      { displayName: "", url: "https://config.example.invalid/tvbox.json" },
      { displayName: "   ", url: "https://config.example.invalid/tvbox.json" },
      { displayName: "名".repeat(101), url: "https://config.example.invalid/tvbox.json" },
      { displayName: "电视源", url: "" },
      { displayName: "电视源", url: "   " },
      { displayName: "电视源", url: `https://e.invalid/${"a".repeat(2048)}` },
    ]) {
      await expect(client.tvboxConfigSave(request))
        .rejects.toMatchObject({ code: "INVALID_ARGUMENT", retryable: false })
    }

    // 合法输入也不编造 sourceId：没有 Rust 侧取回/解析/缓存，假装导入成功就是假能力。
    await expect(
      client.tvboxConfigSave({
        displayName: "电视源",
        url: "https://config.example.invalid/tvbox.json",
      }),
    ).rejects.toMatchObject({ code: "HAVEN_CAPABILITY_UNAVAILABLE", retryable: false })
  })
})
