import { describe, expect, it } from "vitest"
import type {
  AppearanceSettingsValue,
  AppTheme,
  AppThemePalette,
  PreferenceGetResult,
  WallpaperSelection,
} from "./settings-wire"
import {
  applySettingsPatch,
  buildSettingsPatch,
  defaultHomeLayout,
  defaultOverviewLayout,
  defaultSettingsValue,
  guardAppearanceAsset,
  guardAppearanceAssets,
  guardAppTheme,
  guardHomeLayout,
  guardHomeLayoutSnapshot,
  guardOverviewLayout,
  guardOverviewLayoutMutationResult,
  guardOverviewLayoutSnapshot,
  guardPreferenceGetResult,
  guardSettingsValue,
  guardWallpaperSelection,
  overviewModuleColumnSpan,
  settingsValuesEqual,
} from "./settings-wire"

const ASSET_ID = "0196f0d2-0000-7000-8000-00000000a101"

const palette = (): AppThemePalette => ({
  background: "#ffffff",
  foreground: "#111111",
  card: "#ffffff",
  cardForeground: "#111111",
  popover: "#ffffff",
  popoverForeground: "#111111",
  primary: "#3c3c43",
  primaryForeground: "#ffffff",
  secondary: "#f4f4f5",
  secondaryForeground: "#111111",
  muted: "#f4f4f5",
  mutedForeground: "#666666",
  accent: "#3c3c43",
  accentForeground: "#ffffff",
  destructive: "#dc2626",
  destructiveForeground: "#ffffff",
  border: "#3c3c4329",
  input: "#3c3c4329",
  ring: "#3c3c43",
})

const customTheme = (): AppTheme => ({ light: palette(), dark: palette(), accentColor: "#2563eb" })

const appearanceWith = (
  extra: Partial<AppearanceSettingsValue> = {},
): AppearanceSettingsValue => ({
  section: "appearance",
  theme: "custom",
  density: "comfortable",
  sidebar: "auto",
  reduceMotion: false,
  interfaceFontMode: "system",
  customTheme: customTheme(),
  wallpaper: { kind: "static", assetId: ASSET_ID },
  customFontAssetId: ASSET_ID,
  uiFontPreset: "system",
  uiFontFamily: null,
  ...extra,
})

const validPreferenceResult = (): PreferenceGetResult => ({
  schemaVersion: 1,
  mediaItemId: "media-1",
  editionId: "edition-1",
  readingPatch: { fontFamily: "serif", pagination: "paginated" },
  comicPatch: { viewMode: "single", direction: "rtl" },
  editionReadingPatch: { fontSize: "large", letterSpacing: "relaxed" },
  editionComicPatch: { pageGap: "twelve" },
  mediaItemReadingPatch: null,
  mediaItemComicPatch: { preloadPages: "three" },
  effectiveReading: {
    section: "reading",
    fontFamily: "serif",
    customFontFamily: null,
    fontSize: "large",
    lineHeight: "comfortable",
    contentWidth: "medium",
    theme: "warm",
    customBackground: null,
    customText: null,
    fontWeight: "regular",
    letterSpacing: "relaxed",
    systemAuto: true,
    pagination: "paginated",
  },
  effectiveComic: {
    section: "comic",
    viewMode: "single",
    direction: "rtl",
    pageGap: "twelve",
    preloadPages: "three",
  },
  mediaItemRevision: null,
  editionRevision: "edition-rev-1",
})
describe("resource preference wire guards", () => {
  it("accepts valid raw edition and media-item patches", () => {
    expect(guardPreferenceGetResult(validPreferenceResult())).toBe(true)
  })

  it("rejects an invalid value in any raw reading patch", () => {
    const value = validPreferenceResult()
    const malformed = {
      ...value,
      editionReadingPatch: { pagination: "book" },
    }
    expect(guardPreferenceGetResult(malformed)).toBe(false)
  })

  it("rejects an invalid value in any raw comic patch", () => {
    const value = validPreferenceResult()
    const malformed = {
      ...value,
      mediaItemComicPatch: { pageGap: "forty_eight" },
    }
    expect(guardPreferenceGetResult(malformed)).toBe(false)
  })

  it("requires all four raw patch fields in the v1 response", () => {
    const value = validPreferenceResult()
    const { mediaItemReadingPatch: _omitted, ...withoutField } = value
    expect(guardPreferenceGetResult(withoutField)).toBe(false)
  })
})

describe("appearance wire guards", () => {
  it("accepts theme=custom with a full palette, a static wallpaper and a font asset", () => {
    expect(guardSettingsValue(appearanceWith())).toBe(true)
  })

  it("accepts an appearance value without the serde-default fields", () => {
    // Rust 侧这三个字段是 `#[serde(default)]`：旧快照缺失它们是合法的，缺失即默认。
    expect(guardSettingsValue({
      section: "appearance",
      theme: "dark",
    density: "compact",
    sidebar: "expanded",
    reduceMotion: true,
    interfaceFontMode: "system",
  })).toBe(true)
  })

  it("rejects an unknown palette token instead of silently dropping it", () => {
    const theme = customTheme()
    expect(guardAppTheme({
      light: { ...theme.light, shadow: "#000000" },
      dark: theme.dark,
      accentColor: theme.accentColor,
    })).toBe(false)
    expect(guardSettingsValue(appearanceWith({
      customTheme: {
        light: { ...palette(), shadow: "#000000" },
        dark: palette(),
        accentColor: "#2563eb",
      } as unknown as AppTheme,
    }))).toBe(false)
  })

  it("rejects colors that are not canonical lowercase #rrggbb / #rrggbbaa", () => {
    for (const invalid of ["#fff", "#FFFFFF", "#1122334", "#gggggg", "white", "rgba(0,0,0,0.5)"]) {
      expect(guardSettingsValue(appearanceWith({
        customTheme: { light: palette(), dark: palette(), accentColor: invalid },
      }))).toBe(false)
    }
    // 带 alpha 的既有 token 形态是被接受的（前端 --border 就是 8 位）。
    expect(guardSettingsValue(appearanceWith({
      customTheme: { light: palette(), dark: palette(), accentColor: "#3c3c4329" },
    }))).toBe(true)
  })

  it("rejects symbol and non-enumerable unknown keys", () => {
    const withSymbol = { ...customTheme(), [Symbol("shadow")]: "#000000" }
    expect(guardAppTheme(withSymbol)).toBe(false)

    const withHidden = { ...customTheme() }
    Object.defineProperty(withHidden, "shadow", { value: "#000000", enumerable: false })
    expect(guardAppTheme(withHidden)).toBe(false)
  })

  it("rejects wallpaper selections whose kind and assetId combination is impossible", () => {
    const invalid: unknown[] = [
      { kind: "none", assetId: ASSET_ID },
      { kind: "static" },
      { kind: "dynamic" },
      { kind: "video", assetId: ASSET_ID },
      { kind: "static", assetId: ASSET_ID.toUpperCase() },
      { kind: "static", assetId: ASSET_ID.replace(/-/g, "") },
    ]
    for (const value of invalid) expect(guardWallpaperSelection(value)).toBe(false)

    expect(guardWallpaperSelection({ kind: "none" })).toBe(true)
    expect(guardWallpaperSelection({ kind: "static", assetId: ASSET_ID })).toBe(true)
    expect(guardWallpaperSelection({ kind: "dynamic", assetId: ASSET_ID })).toBe(true)
  })

  it("rejects a wallpaper or font asset that smuggles a path/url field", () => {
    const smuggledWallpaper = { kind: "static", assetId: ASSET_ID, path: "C:/wall.png" }
    expect(guardWallpaperSelection(smuggledWallpaper)).toBe(false)
    expect(guardSettingsValue(appearanceWith({
      wallpaper: { kind: "static", assetId: ASSET_ID, url: "https://example.invalid/w.png" } as unknown as WallpaperSelection,
    }))).toBe(false)
  })

  it("rejects non-canonical custom font asset ids and accepts null", () => {
    for (const invalid of ["", "not-a-uuid", ASSET_ID.toUpperCase(), ASSET_ID.replace(/-/g, ""), `urn:uuid:${ASSET_ID}`]) {
      expect(guardSettingsValue(appearanceWith({ customFontAssetId: invalid }))).toBe(false)
    }
    expect(guardSettingsValue(appearanceWith({ customFontAssetId: null }))).toBe(true)
  })

  it("accepts safe system font names and rejects unknown presets or CSS fragments", () => {
    expect(guardSettingsValue(appearanceWith({
      customFontAssetId: null,
      uiFontPreset: "custom_system",
      uiFontFamily: "霞鹜文楷",
    }))).toBe(true)
    expect(guardSettingsValue(appearanceWith({ uiFontPreset: "unknown" as never }))).toBe(false)
    for (const uiFontFamily of ["", "../font.woff2", "Arial; color:red", '"Arial"']) {
      expect(guardSettingsValue(appearanceWith({ uiFontFamily })), uiFontFamily).toBe(false)
    }
    expect(guardSettingsValue(appearanceWith({ uiFontFamily: null }))).toBe(true)
  })
})

describe("appearance patch semantics", () => {
  it("treats an explicit null customTheme as a clear, not as leave-unchanged", () => {
    const saved = appearanceWith()
    const draft = appearanceWith({ customTheme: null })

    expect(settingsValuesEqual(saved, draft)).toBe(false)
    const patch = buildSettingsPatch(saved, draft)
    expect(patch).toEqual({ section: "appearance", customTheme: null })

    // 清除只作用于 customTheme：壁纸与字体资产保持原样。
    expect(applySettingsPatch(saved, { section: "appearance", customTheme: null })).toMatchObject({
      section: "appearance",
      customTheme: null,
      wallpaper: { kind: "static", assetId: ASSET_ID },
      customFontAssetId: ASSET_ID,
    })
  })

  it("keeps the custom theme and font asset when the patch omits them", () => {
    const applied = applySettingsPatch(appearanceWith(), { section: "appearance", theme: "dark" })
    expect(applied).toMatchObject({ theme: "dark", customTheme: customTheme(), customFontAssetId: ASSET_ID })
  })

  it("persists the selected preset and system family while clearing an imported font selection", () => {
    const saved = appearanceWith({ customFontAssetId: ASSET_ID, uiFontPreset: "system", uiFontFamily: null })
    const draft = appearanceWith({ customFontAssetId: null, uiFontPreset: "custom_system", uiFontFamily: "霞鹜文楷" })
    const patch = buildSettingsPatch(saved, draft)

    expect(patch).toEqual({
      section: "appearance",
      customFontAssetId: null,
      uiFontPreset: "custom_system",
      uiFontFamily: "霞鹜文楷",
    })
    expect(applySettingsPatch(saved, patch!)).toMatchObject({
      customFontAssetId: null,
      uiFontPreset: "custom_system",
      uiFontFamily: "霞鹜文楷",
    })
  })

  it("normalizes a missing wallpaper to none without reporting a change", () => {
    const legacy = {
      section: "appearance",
      theme: "dark",
      density: "compact",
      sidebar: "auto",
      reduceMotion: true,
    } as AppearanceSettingsValue
    const applied = applySettingsPatch(legacy, { section: "appearance" })

    expect(applied).toMatchObject({ wallpaper: { kind: "none" }, customTheme: null, customFontAssetId: null })
    expect(settingsValuesEqual(legacy, applied)).toBe(true)
    expect(buildSettingsPatch(legacy, applied)).toBeNull()
  })

  it("detects a single drifted palette token and a wallpaper change", () => {
    const drifted = appearanceWith({
      customTheme: { light: { ...palette(), ring: "#000001" }, dark: palette(), accentColor: "#2563eb" },
    })
    expect(settingsValuesEqual(appearanceWith(), drifted)).toBe(false)
    expect(buildSettingsPatch(appearanceWith(), drifted)).toEqual({
      section: "appearance",
      customTheme: drifted.customTheme,
    })

    expect(buildSettingsPatch(appearanceWith(), appearanceWith({ wallpaper: { kind: "none" } }))).toEqual({
      section: "appearance",
      wallpaper: { kind: "none" },
    })
  })

  it("defaults appearance to no custom theme, no wallpaper and no font asset", () => {
    expect(defaultSettingsValue("appearance")).toMatchObject({
      section: "appearance",
      theme: "system",
      customTheme: null,
      wallpaper: { kind: "none" },
      customFontAssetId: null,
      uiFontPreset: "system",
      uiFontFamily: null,
    })
  })
})

describe("home layout wire guards", () => {
  it("accepts the domain default layout built from the three real modules", () => {
    const layout = defaultHomeLayout()
    expect(layout.modules.map((placement) => placement.module)).toEqual([
      "continue",
      "recently_added",
      "shelf-favorites",
    ])
    expect(guardHomeLayout(layout)).toBe(true)
  })

  it("accepts an explicit empty layout and rejects unknown module ids", () => {
    expect(guardHomeLayout({ schemaVersion: 1, modules: [] })).toBe(true)
    // shelf_favorites 是下划线形态：与真实模块 ID（连字符）不是同一个模块。
    expect(guardHomeLayout({
      schemaVersion: 1,
      modules: [{ module: "shelf_favorites", size: "medium", row: 0, column: 0, order: 0 }],
    })).toBe(false)
    expect(guardHomeLayout({ schemaVersion: 1, modules: [{ module: "continue", size: "huge", row: 0, column: 0, order: 0 }] })).toBe(false)
  })

  it("rejects overlapping cells, duplicate orders and out-of-grid coordinates", () => {
    expect(guardHomeLayout({
      schemaVersion: 1,
      modules: [
        { module: "continue", size: "medium", row: 0, column: 0, order: 0 },
        { module: "recently_added", size: "small", row: 0, column: 1, order: 1 },
      ],
    })).toBe(false)
    expect(guardHomeLayout({
      schemaVersion: 1,
      modules: [
        { module: "continue", size: "medium", row: 0, column: 0, order: 0 },
        { module: "recently_added", size: "medium", row: 1, column: 0, order: 0 },
      ],
    })).toBe(false)
    expect(guardHomeLayout({
      schemaVersion: 1,
      modules: [{ module: "continue", size: "large", row: 0, column: 1, order: 0 }],
    })).toBe(false)
    expect(guardHomeLayout({
      schemaVersion: 1,
      modules: [{ module: "continue", size: "medium", row: 12, column: 0, order: 0 }],
    })).toBe(false)
  })

  it("rejects unknown, symbol and non-enumerable keys on the layout and its placements", () => {
    expect(guardHomeLayout({ ...defaultHomeLayout(), revision: "appearance-1" })).toBe(false)

    const hidden = defaultHomeLayout()
    Object.defineProperty(hidden, "path", { value: "C:/layout.json", enumerable: false })
    expect(guardHomeLayout(hidden)).toBe(false)

    const smuggled = { ...defaultHomeLayout().modules[0], url: "https://example.invalid/layout" }
    expect(guardHomeLayout({ schemaVersion: 1, modules: [smuggled] })).toBe(false)
  })

  it("keeps unsaved (revision=null) distinguishable from a saved empty layout", () => {
    expect(guardHomeLayoutSnapshot({ layout: defaultHomeLayout(), revision: null })).toBe(true)
    expect(guardHomeLayoutSnapshot({ layout: { schemaVersion: 1, modules: [] }, revision: "appearance-1" })).toBe(true)
    expect(guardHomeLayoutSnapshot({ layout: defaultHomeLayout(), revision: "" })).toBe(false)
    expect(guardHomeLayoutSnapshot({ layout: defaultHomeLayout() })).toBe(false)
    expect(guardHomeLayoutSnapshot({ layout: defaultHomeLayout(), revision: null, changed: false })).toBe(false)
  })
})

describe("overview layout wire guards", () => {
  it("accepts the domain default layout built from the five real modules", () => {
    const layout = defaultOverviewLayout()
    expect(layout.modules.map((placement) => placement.module)).toEqual([
      "preferences",
      "metrics",
      "reading-minutes",
      "type-share",
      "reading-heatmap",
    ])
    expect(guardOverviewLayout(layout)).toBe(true)
    // 默认版式就是升级前那一屏：两张图同行且宽度比 2:1。
    const minutes = layout.modules.find((placement) => placement.module === "reading-minutes")
    const shares = layout.modules.find((placement) => placement.module === "type-share")
    expect([minutes?.row, minutes?.column, minutes?.size]).toEqual([2, 0, "medium"])
    expect([shares?.row, shares?.column, shares?.size]).toEqual([2, 2, "small"])
  })

  it("mirrors the three-column grid rather than the home layout's four columns", () => {
    // 档位跨度是三列的 1/2/3；medium 从第 2 列开始占满整行，而不是越界。
    expect(overviewModuleColumnSpan("small")).toBe(1)
    expect(overviewModuleColumnSpan("medium")).toBe(2)
    expect(overviewModuleColumnSpan("large")).toBe(3)
    expect(guardOverviewLayout({
      schemaVersion: 1,
      modules: [{ module: "metrics", size: "medium", row: 0, column: 1, order: 0 }],
    })).toBe(true)
    // 同一份放置换到首页的四列网格里是合法的，在总览里必须被拒绝——两侧网格不同。
    expect(guardHomeLayout({
      schemaVersion: 1,
      modules: [{ module: "continue", size: "medium", row: 0, column: 1, order: 0 }],
    })).toBe(true)
    expect(guardOverviewLayout({
      schemaVersion: 1,
      modules: [{ module: "preferences", size: "medium", row: 0, column: 2, order: 0 }],
    })).toBe(false)
    expect(guardOverviewLayout({
      schemaVersion: 1,
      modules: [{ module: "preferences", size: "small", row: 0, column: 3, order: 0 }],
    })).toBe(false)
  })

  it("accepts an explicit empty layout and rejects unknown or cross-layout module ids", () => {
    expect(guardOverviewLayout({ schemaVersion: 1, modules: [] })).toBe(true)
    // 下划线形态与真实模块 ID（连字符）不是同一个模块。
    expect(guardOverviewLayout({
      schemaVersion: 1,
      modules: [{ module: "type_share", size: "small", row: 0, column: 0, order: 0 }],
    })).toBe(false)
    // 首页的模块 ID 不属于总览的闭合集合。
    expect(guardOverviewLayout({
      schemaVersion: 1,
      modules: [{ module: "shelf-favorites", size: "small", row: 0, column: 0, order: 0 }],
    })).toBe(false)
    expect(guardOverviewLayout({
      schemaVersion: 1,
      modules: [{ module: "heatmap", size: "small", row: 0, column: 0, order: 0 }],
    })).toBe(false)
    expect(guardOverviewLayout({
      schemaVersion: 1,
      modules: [{ module: "metrics", size: "huge", row: 0, column: 0, order: 0 }],
    })).toBe(false)
  })

  it("rejects overlapping cells, duplicate orders and out-of-grid coordinates", () => {
    // large 占满整行 0，small 落在它覆盖的最后一列：起始格不同，占格却重叠。
    expect(guardOverviewLayout({
      schemaVersion: 1,
      modules: [
        { module: "metrics", size: "large", row: 0, column: 0, order: 0 },
        { module: "preferences", size: "small", row: 0, column: 2, order: 1 },
      ],
    })).toBe(false)
    expect(guardOverviewLayout({
      schemaVersion: 1,
      modules: [
        { module: "metrics", size: "small", row: 0, column: 0, order: 0 },
        { module: "preferences", size: "small", row: 1, column: 0, order: 0 },
      ],
    })).toBe(false)
    expect(guardOverviewLayout({
      schemaVersion: 1,
      modules: [{ module: "metrics", size: "small", row: 12, column: 0, order: 0 }],
    })).toBe(false)
    expect(guardOverviewLayout({ schemaVersion: 2, modules: [] })).toBe(false)
    expect(guardOverviewLayout({
      schemaVersion: 1,
      modules: Array.from({ length: 17 }, (_, index) => ({
        module: "metrics",
        size: "small",
        row: index,
        column: 0,
        order: index,
      })),
    })).toBe(false)
  })

  it("rejects unknown, symbol and non-enumerable keys on the layout and its placements", () => {
    expect(guardOverviewLayout({ ...defaultOverviewLayout(), revision: "overview-1" })).toBe(false)

    const hidden = defaultOverviewLayout()
    Object.defineProperty(hidden, "path", { value: "C:/overview.json", enumerable: false })
    expect(guardOverviewLayout(hidden)).toBe(false)

    const smuggled = { ...defaultOverviewLayout().modules[0], url: "https://example.invalid/layout" }
    expect(guardOverviewLayout({ schemaVersion: 1, modules: [smuggled] })).toBe(false)

    const symbolKey = { ...defaultOverviewLayout(), [Symbol("extra")]: true }
    expect(guardOverviewLayout(symbolKey)).toBe(false)
  })

  it("keeps unsaved (revision=null) distinguishable from a saved empty layout", () => {
    expect(guardOverviewLayoutSnapshot({ layout: defaultOverviewLayout(), revision: null })).toBe(true)
    expect(guardOverviewLayoutSnapshot({ layout: { schemaVersion: 1, modules: [] }, revision: "overview-1" })).toBe(true)
    expect(guardOverviewLayoutSnapshot({ layout: defaultOverviewLayout(), revision: "" })).toBe(false)
    expect(guardOverviewLayoutSnapshot({ layout: defaultOverviewLayout() })).toBe(false)
    expect(guardOverviewLayoutSnapshot({ layout: defaultOverviewLayout(), revision: null, changed: false })).toBe(false)
  })

  it("keeps changed=false distinguishable from a fresh save in the mutation result", () => {
    expect(guardOverviewLayoutMutationResult({
      layout: defaultOverviewLayout(),
      revision: "overview-1",
      changed: false,
    })).toBe(true)
    expect(guardOverviewLayoutMutationResult({
      layout: defaultOverviewLayout(),
      revision: null,
      changed: true,
    })).toBe(true)
    expect(guardOverviewLayoutMutationResult({
      layout: defaultOverviewLayout(),
      revision: "overview-1",
    })).toBe(false)
    expect(guardOverviewLayoutMutationResult({
      layout: defaultOverviewLayout(),
      revision: "overview-1",
      changed: "yes",
    })).toBe(false)
    // 首页布局的形态不能冒充总览布局的响应。
    expect(guardOverviewLayoutMutationResult({
      layout: defaultHomeLayout(),
      revision: "overview-1",
      changed: true,
    })).toBe(false)
  })
})

describe("appearance asset guards", () => {
  const asset = () => ({
    assetId: ASSET_ID,
    kind: "static_wallpaper",
    state: "validated",
    byteSize: 2048,
    displayName: "落日",
  })

  it("accepts a canonical projection with the numeric byteSize the IPC payload really carries", () => {
    expect(guardAppearanceAsset(asset())).toBe(true)
    // bigint 不是 IPC 载荷的形态（JSON 里没有 bigint），因此守卫不再接受它：
    // 接受它等于让一个 `v is AppearanceAssetDto` 的断言说出与类型不符的话。
    expect(guardAppearanceAsset({ ...asset(), byteSize: 2048n })).toBe(false)
  })

  it("rejects paths, urls, non-canonical ids and impossible byte sizes", () => {
    expect(guardAppearanceAsset({ ...asset(), path: "C:/wall.png" })).toBe(false)
    expect(guardAppearanceAsset({ ...asset(), url: "https://example.invalid/w.png" })).toBe(false)
    expect(guardAppearanceAsset({ ...asset(), assetId: ASSET_ID.toUpperCase() })).toBe(false)
    expect(guardAppearanceAsset({ ...asset(), kind: "video" })).toBe(false)
    expect(guardAppearanceAsset({ ...asset(), state: "trusted" })).toBe(false)
    expect(guardAppearanceAsset({ ...asset(), byteSize: -1 })).toBe(false)
    expect(guardAppearanceAsset({ ...asset(), byteSize: 1.5 })).toBe(false)
  })

  it("keeps the list projection closed and versioned", () => {
    expect(guardAppearanceAssets({ schemaVersion: 1, assets: [asset()] })).toBe(true)
    expect(guardAppearanceAssets({ schemaVersion: 1, assets: [] })).toBe(true)
    expect(guardAppearanceAssets({ schemaVersion: 2, assets: [] })).toBe(false)
    expect(guardAppearanceAssets({ schemaVersion: 1, assets: [asset()], total: 1 })).toBe(false)
  })
})
