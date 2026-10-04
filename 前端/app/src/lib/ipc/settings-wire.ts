// Settings 最小 IPC 类型消费层（FE-SETTINGS-001）。
// 单一事实源为冻结契约 plan/FRONTEND_BACKEND_CONTRACT.md §26 + 后端生成物：
//   - haven-domain/src/settings.rs（SettingsValue / SettingsPatch，serde tag="section"，
//     snake_case 枚举值，字段 camelCase，deny_unknown_fields）
//   - haven-application/src/services/settings.rs（SettingsSnapshot / SettingsUpdateResult）
//   - src-tauri/src/ipc/mod.rs（SettingsChangedDto，camelCase）
// 资源偏好 DTO 由后端 ts-rs 生成并在本层复用；本文件只保留设置值的运行时守卫
// 与前端的安全默认/patch 语义，避免组件直接依赖 Domain 或 DB Row。
// 禁止把 Domain Entity / DB Row 当作 IPC 类型；secret 永不进入设置 DTO。

import type {
  AppearanceAssetDeleteRequest,
  AppearanceAssetDeleteResultDto,
  AppearanceAssetDto,
  AppearanceAssetImportRequest,
  AppearanceAssetsDto,
  AppearanceAssetsListRequest,
  HomeLayoutDto,
  HomeLayoutMutationResultDto,
  HomeLayoutResetRequest,
  HomeLayoutSaveRequest,
  HomeLayoutSnapshotDto,
  OverviewLayoutDto,
  OverviewLayoutMutationResultDto,
  OverviewLayoutResetRequest,
  OverviewLayoutSaveRequest,
  OverviewLayoutSnapshotDto,
  PreferenceComicPatchDto,
  PreferenceGetRequest as GeneratedPreferenceGetRequest,
  PreferenceGetResult as GeneratedPreferenceGetResult,
  PreferenceReadingPatchDto,
  PreferenceTargetDto,
  PreferenceUpdateRequest as GeneratedPreferenceUpdateRequest,
  PreferenceUpdateResult as GeneratedPreferenceUpdateResult,
  ReadingCategoryTotalDto,
  ReadingDailyBucketDto,
  ReadingHeatmapCellDto,
  ReadingOverviewDto,
  ReadingOverviewGetRequest,
  ReadingOverviewRangeDto,
  ReadingSessionCategoryDto,
} from "./generated/wire";
import {
  INTERFACE_FONT_MODES,
  isCanonicalInterfaceFontAssetId,
  isSafeInterfaceFontFamily,
  type InterfaceFontModeWire,
} from "./interface-font-wire.js";

export type SettingsSectionWire = "general" | "appearance" | "playback" | "reading" | "comic" | "downloads" | "privacy";

export type LaunchPageWire = "home" | "library" | "continue" | "last_session";
export type LanguageWire = "zh_cn" | "en_us" | "zh_tw";
export type ThemeWire = "system" | "light" | "dark" | "custom";
export type DensityWire = "comfortable" | "compact";
export type SidebarWire = "expanded" | "collapsed" | "auto";
export type UiFontPresetWire = "system" | "humanist_serif" | "modern_sans" | "custom_system";
export type PlaybackRateWire = "point_seven_five" | "one" | "one_point_two_five" | "one_point_five" | "two";
export type ReadingFontFamilyWire = "sans" | "serif" | "kai" | "heiti" | "fangsong" | "mianfei" | "custom";
export type ReadingFontWeightWire = "light" | "regular" | "medium" | "semibold" | "bold";
export type ReadingLetterSpacingWire = "tight" | "normal" | "relaxed" | "loose";
export type ReadingFontSizeWire = "small" | "medium" | "large";
export type ReadingLineHeightWire = "compact" | "comfortable" | "airy";
export type ReadingContentWidthWire = "narrow" | "medium" | "wide";
export type ReadingThemeWire = "system" | "paper" | "warm" | "slate" | "dark" | "sepia" | "eyeCare" | "custom";
/** 文本阅读布局；缺失值兼容 024 之前的设置快照并按 scroll 处理。 */
export type ReadingPaginationWire = "scroll" | "paginated" | "double";
export type ComicViewModeWire = "single" | "double" | "strip";
export type ComicDirectionWire = "rtl" | "ltr";
export type ComicPageGapWire = "zero" | "twelve" | "twenty_four";
export type ComicPreloadPagesWire = "one" | "three" | "five" | "unlimited";
export type DownloadConcurrencyWire = "one" | "two" | "three" | "five";
export type DownloadSpeedLimitWire = "unlimited" | "kbps512" | "mbps2" | "mbps5" | "mbps10";

export type GeneralSettingsValue = {
  section: "general";
  launchPage: LaunchPageWire;
  restoreSession: boolean;
  language: LanguageWire;
  notifications: boolean;
};

export type AppearanceSettingsValue = {
  section: "appearance";
  theme: ThemeWire;
  density: DensityWire;
  sidebar: SidebarWire;
  reduceMotion: boolean;
  /** 界面字体模式；Rust 侧始终序列化（旧设置行按 system 回落）。 */
  interfaceFontMode: InterfaceFontModeWire;
  /** 仅 custom 且选择本机字体时存在；缺失即「未选择」（不是 null）。 */
  interfaceFontFamily?: string;
  /** 仅 custom 且选择导入字体时存在；opaque 资产 id，永不保存路径。 */
  interfaceFontAssetId?: string;
  // 以下三个字段在 Rust 侧是 `#[serde(default)]`（settings.rs）：序列化总会写出，
  // 但反序列化容忍缺失（缺失 = 从未创建 / 无壁纸 / 无自定义字体）。类型与守卫
  // 因此同「pagination」一样按可选处理，缺失即取默认，不把「旧快照」判成非法。
  /** 用户自定义的明/暗调色板 + 强调色；null = 尚未创建（theme=custom 时才被消费）。 */
  customTheme?: AppTheme | null;
  /** 首页壁纸；只引用不透明资产 ID，`none` 是明确的无壁纸状态。 */
  wallpaper?: WallpaperSelection;
  /** 阅读运行时加载的字体资产；null = 使用预设字体。 */
  customFontAssetId?: string | null;
  /** 界面字体方案；旧设置缺失时等同于跟随系统。 */
  uiFontPreset?: UiFontPresetWire;
  /** 仅自定义系统字体方案使用的字体族名，不含文件路径或 CSS。 */
  uiFontFamily?: string | null;
};

export type PlaybackSettingsValue = {
  section: "playback";
  defaultPlaybackRate: PlaybackRateWire;
  autoResume: boolean;
  autoNext: boolean;
};

export type ReadingSettingsValue = {
  section: "reading";
  fontFamily: ReadingFontFamilyWire;
  customFontFamily: string | null;
  fontSize: ReadingFontSizeWire;
  lineHeight: ReadingLineHeightWire;
  contentWidth: ReadingContentWidthWire;
  theme: ReadingThemeWire;
  customBackground: string | null;
  customText: string | null;
  fontWeight: ReadingFontWeightWire;
  letterSpacing: ReadingLetterSpacingWire;
  systemAuto: boolean;
  /** 后端完成迁移前允许缺失，缺失即连续滚动；新写入值总是显式保存。 */
  pagination?: ReadingPaginationWire;
};

export type ComicSettingsValue = {
  section: "comic";
  viewMode: ComicViewModeWire;
  direction: ComicDirectionWire;
  pageGap: ComicPageGapWire;
  preloadPages: ComicPreloadPagesWire;
};

export type DownloadSettingsValue = {
  section: "downloads";
  concurrentTasks: DownloadConcurrencyWire;
  speedLimit: DownloadSpeedLimitWire;
  autoContinue: boolean;
};

export type PrivacySettingsValue = {
  section: "privacy";
  searchHistory: boolean;
  playbackHistory: boolean;
};

/** 分区设置值（闭合联合；JSON 形状 `{"section":"general", ...}`）。 */
export type SettingsValue = GeneralSettingsValue | AppearanceSettingsValue | PlaybackSettingsValue | ReadingSettingsValue | ComicSettingsValue | DownloadSettingsValue | PrivacySettingsValue;

export type GeneralPatchWire = {
  section: "general";
  launchPage?: LaunchPageWire;
  restoreSession?: boolean;
  language?: LanguageWire;
  notifications?: boolean;
};

export type AppearancePatchWire = {
  section: "appearance";
  theme?: ThemeWire;
  density?: DensityWire;
  sidebar?: SidebarWire;
  reduceMotion?: boolean;
  interfaceFontMode?: InterfaceFontModeWire;
  /** 空字符串表示「清除本机字体选择」（与 reading 的 custom 字段同一语义）。 */
  interfaceFontFamily?: string;
  /** 空字符串表示「清除导入字体选择」。 */
  interfaceFontAssetId?: string;
  // 「缺失 = 不改；显式 null = 清除」与 Rust `Option<Option<T>>` 同形，因此这三个字段
  // 必须用 `!== undefined` 判断，不能用 `??`（会把清除当成不改）。
  customTheme?: AppTheme | null;
  /** 显式 `null` = 清除壁纸（等价于 `{ kind: "none" }`）；缺失 = 不改。 */
  wallpaper?: WallpaperSelection | null;
  customFontAssetId?: string | null;
  uiFontPreset?: UiFontPresetWire;
  uiFontFamily?: string | null;
};

export type PlaybackPatchWire = {
  section: "playback";
  defaultPlaybackRate?: PlaybackRateWire;
  autoResume?: boolean;
  autoNext?: boolean;
};

export type ReadingPatchWire = {
  section: "reading";
  fontFamily?: ReadingFontFamilyWire | null;
  customFontFamily?: string | null;
  fontSize?: ReadingFontSizeWire | null;
  lineHeight?: ReadingLineHeightWire | null;
  contentWidth?: ReadingContentWidthWire | null;
  theme?: ReadingThemeWire | null;
  customBackground?: string | null;
  customText?: string | null;
  fontWeight?: ReadingFontWeightWire | null;
  letterSpacing?: ReadingLetterSpacingWire | null;
  systemAuto?: boolean | null;
  pagination?: ReadingPaginationWire | null;
};

export type ComicPatchWire = {
  section: "comic";
  viewMode?: ComicViewModeWire | null;
  direction?: ComicDirectionWire | null;
  pageGap?: ComicPageGapWire | null;
  preloadPages?: ComicPreloadPagesWire | null;
};

export type DownloadPatchWire = {
  section: "downloads";
  concurrentTasks?: DownloadConcurrencyWire;
  speedLimit?: DownloadSpeedLimitWire;
  autoContinue?: boolean;
};

export type PrivacyPatchWire = {
  section: "privacy";
  searchHistory?: boolean;
  playbackHistory?: boolean;
};

/** 分区部分更新（闭合联合；JSON 形状 `{"section":"general","launchPage":"library"}`）。 */
export type SettingsPatch = GeneralPatchWire | AppearancePatchWire | PlaybackPatchWire | ReadingPatchWire | ComicPatchWire | DownloadPatchWire | PrivacyPatchWire;

/** `settings_get` 响应：当前值 + 状态版本（从未保存 → 默认值 + revision: null）。 */
export type SettingsSnapshot = {
  value: SettingsValue;
  revision: string | null;
};

/** `settings_update` 请求（Tauri 命令参数形状：section + expected_revision + patch）。 */
export type SettingsUpdateRequest = {
  section: SettingsSectionWire;
  expectedRevision: string | null;
  patch: SettingsPatch;
};

/** `settings_update` 成功响应（changed=false = 幂等重复更新，不发布 settings.changed）。 */
export type SettingsUpdateResult = {
  value: SettingsValue;
  revision: string | null;
  changed: boolean;
};

export type PreferenceTargetWire = PreferenceTargetDto;

/** Resource preference nested patches omit the SettingsPatch discriminator. */
export type PreferenceReadingPatchWire = PreferenceReadingPatchDto;
export type PreferenceComicPatchWire = PreferenceComicPatchDto;

export type PreferenceGetRequest = GeneratedPreferenceGetRequest;

export type PreferenceUpdateRequest = GeneratedPreferenceUpdateRequest;

/** Resource Preference 的真实读模型；effective 值由 Rust 合并后返回。 */
export type PreferenceGetResult = GeneratedPreferenceGetResult;

export type PreferenceUpdateResult = GeneratedPreferenceUpdateResult;

/** `settings.changed` 事件负载（仅 changed=true 时发布；revision 与 Result 同源）。 */
export type SettingsChangedDto = {
  schemaVersion: 1;
  at: string;
  operationId: string;
  sequence: number;
  section: SettingsSectionWire;
  revision: string;
};

// ---- 外观（appearance）运行时类型（单一事实源：haven-domain/src/appearance.rs）----
//
// 这一组是「前端自持」的 wire 类型：AppearanceAsset* / HomeLayout* 由 ts-rs 生成，
// 但 AppTheme / AppThemePalette / WallpaperSelection 属于设置值内部结构，生成器
// 不产出它们，所以在这里手写镜像。任何字段漂移都会被下面的守卫挡在 UI 之外。

/** 严格 `#rrggbb` 或 `#rrggbbaa`（小写归一化后的规范十六进制颜色）。 */
export type HexColorWire = string;

/**
 * 自定义主题调色板：与前端 CSS token 一一对应的 19 个字段。
 *
 * `border` / `input` 这两个既有 token 实际带 alpha（前端是 `rgba(60, 60, 67, 0.16)`），
 * 所以它们的取值是 8 位形态（`#3c3c4329`），不会被压成不透明色。
 */
export type AppThemePalette = {
  background: HexColorWire;
  foreground: HexColorWire;
  card: HexColorWire;
  cardForeground: HexColorWire;
  popover: HexColorWire;
  popoverForeground: HexColorWire;
  primary: HexColorWire;
  primaryForeground: HexColorWire;
  secondary: HexColorWire;
  secondaryForeground: HexColorWire;
  muted: HexColorWire;
  mutedForeground: HexColorWire;
  accent: HexColorWire;
  accentForeground: HexColorWire;
  destructive: HexColorWire;
  destructiveForeground: HexColorWire;
  border: HexColorWire;
  input: HexColorWire;
  ring: HexColorWire;
};

/** 自定义主题：明/暗两套调色板 + 独立强调色（后者与调色板里的 `accent` 是两件事）。 */
export type AppTheme = {
  light: AppThemePalette;
  dark: AppThemePalette;
  accentColor: HexColorWire;
};

/**
 * 壁纸选择（闭合形状）：`none` 不携带 assetId，`static` / `dynamic` 必须携带。
 *
 * 只持有不透明资产 ID，没有 path / url 字段：真实字节由后端受控存储按 ID 解析。
 */
export type WallpaperSelection =
  | { kind: "none"; assetId?: undefined }
  | { kind: "static"; assetId: string }
  | { kind: "dynamic"; assetId: string };

export type AppearanceAssetKindWire = AppearanceAssetDto["kind"];

/** 资产列表请求；`kind` 为 `null` 表示不按种类过滤。 */
export type AppearanceAssetsListRequestWire = AppearanceAssetsListRequest;
export type AppearanceAssetImportRequestWire = AppearanceAssetImportRequest;
export type AppearanceAssetDeleteRequestWire = AppearanceAssetDeleteRequest;
export type AppearanceAssetWire = AppearanceAssetDto;
export type AppearanceAssetsWire = AppearanceAssetsDto;
export type AppearanceAssetDeleteResultWire = AppearanceAssetDeleteResultDto;
export type HomeLayoutWire = HomeLayoutDto;
export type HomeLayoutSnapshotWire = HomeLayoutSnapshotDto;
export type HomeLayoutMutationResultWire = HomeLayoutMutationResultDto;
export type HomeLayoutSaveRequestWire = HomeLayoutSaveRequest;
export type HomeLayoutResetRequestWire = HomeLayoutResetRequest;

// 设置页总览布局（048）与首页布局是两组平行但独立的 wire 类型：三列网格对四列网格、
// 闭合模块集合不同、存储表与 revision 各自独立（见 haven-domain/src/appearance.rs）。
export type OverviewLayoutWire = OverviewLayoutDto;
export type OverviewLayoutSnapshotWire = OverviewLayoutSnapshotDto;
export type OverviewLayoutMutationResultWire = OverviewLayoutMutationResultDto;
export type OverviewLayoutSaveRequestWire = OverviewLayoutSaveRequest;
export type OverviewLayoutResetRequestWire = OverviewLayoutResetRequest;

// ---- 阅读总览（reading overview）运行时类型（单一事实源：haven-domain/src/reading_activity.rs）----
//
// 这一组直接复用 ts-rs 生成物：请求、窗口、逐日桶、分类合计、热力图格子与总览聚合
// 都在 `generate.rs` 的导出清单里，因此前端不需要手写镜像。下面的守卫只补充生成类型
// 表达不了的**范围**约束（Rust 侧由 `ReadingOverviewRequest::new` 与领域构造保证）。

/** 总览读取请求：窗口天数 + 显式 UTC 偏移（分钟）。 */
export type ReadingOverviewGetRequestWire = ReadingOverviewGetRequest;
export type ReadingOverviewRangeWire = ReadingOverviewRangeDto;
export type ReadingDailyBucketWire = ReadingDailyBucketDto;
export type ReadingCategoryTotalWire = ReadingCategoryTotalDto;
export type ReadingHeatmapCellWire = ReadingHeatmapCellDto;
export type ReadingOverviewWire = ReadingOverviewDto;

/** 会话归属的一级分类（闭合枚举，与领域 `ReadingSessionCategory` 一一对应）。 */
export type ReadingSessionCategoryWire = ReadingSessionCategoryDto;

// ---- 守卫：运行时验证契约不变量（闭合枚举 / 形状 / schemaVersion），禁止裸 as ----

export function parseSettingsSection(raw: string): SettingsSectionWire | null {
  return raw === "general" || raw === "appearance" || raw === "playback" || raw === "reading" || raw === "comic" || raw === "downloads" || raw === "privacy" ? raw : null;
}

const LAUNCH_PAGES: readonly LaunchPageWire[] = ["home", "library", "continue", "last_session"];
const LANGUAGES: readonly LanguageWire[] = ["zh_cn", "en_us", "zh_tw"];
const THEMES: readonly ThemeWire[] = ["system", "light", "dark", "custom"];
const UI_FONT_PRESETS: readonly UiFontPresetWire[] = ["system", "humanist_serif", "modern_sans", "custom_system"];
const DENSITIES: readonly DensityWire[] = ["comfortable", "compact"];
const SIDEBARS: readonly SidebarWire[] = ["expanded", "collapsed", "auto"];
const PLAYBACK_RATES: readonly PlaybackRateWire[] = ["point_seven_five", "one", "one_point_two_five", "one_point_five", "two"];
const READING_FONT_FAMILIES: readonly ReadingFontFamilyWire[] = ["sans", "serif", "kai", "heiti", "fangsong", "mianfei", "custom"];
const READING_FONT_WEIGHTS: readonly ReadingFontWeightWire[] = ["light", "regular", "medium", "semibold", "bold"];
const READING_LETTER_SPACINGS: readonly ReadingLetterSpacingWire[] = ["tight", "normal", "relaxed", "loose"];
const READING_FONT_SIZES: readonly ReadingFontSizeWire[] = ["small", "medium", "large"];
const READING_LINE_HEIGHTS: readonly ReadingLineHeightWire[] = ["compact", "comfortable", "airy"];
const READING_CONTENT_WIDTHS: readonly ReadingContentWidthWire[] = ["narrow", "medium", "wide"];
const READING_THEMES: readonly ReadingThemeWire[] = ["system", "paper", "warm", "slate", "dark", "sepia", "eyeCare", "custom"];
const READING_PAGINATIONS: readonly ReadingPaginationWire[] = ["scroll", "paginated", "double"];
const COMIC_VIEW_MODES: readonly ComicViewModeWire[] = ["single", "double", "strip"];
const COMIC_DIRECTIONS: readonly ComicDirectionWire[] = ["rtl", "ltr"];
const COMIC_PAGE_GAPS: readonly ComicPageGapWire[] = ["zero", "twelve", "twenty_four"];
const COMIC_PRELOAD_PAGES: readonly ComicPreloadPagesWire[] = ["one", "three", "five", "unlimited"];
const DOWNLOAD_CONCURRENCIES: readonly DownloadConcurrencyWire[] = ["one", "two", "three", "five"];
const DOWNLOAD_SPEED_LIMITS: readonly DownloadSpeedLimitWire[] = ["unlimited", "kbps512", "mbps2", "mbps5", "mbps10"];

function isOneOf<T extends string>(value: unknown, closed: readonly T[]): value is T {
  return typeof value === "string" && (closed as readonly string[]).includes(value);
}

// ---- 外观守卫（闭合形状；未知字段一律拒绝，禁止裸 as）----

/** 规范十六进制颜色：恰好 `#rrggbb` 或 `#rrggbbaa`（小写，不接受 3/4 位简写与 CSS 文本）。 */
const HEX_COLOR_PATTERN = /^#[0-9a-f]{6}([0-9a-f]{2})?$/;
/** 规范小写连字符 UUID：同一个资产只有一种文本身份（大写/无连字符/URN/花括号都拒绝）。 */
const CANONICAL_ASSET_ID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

const PALETTE_FIELDS = [
  "background",
  "foreground",
  "card",
  "cardForeground",
  "popover",
  "popoverForeground",
  "primary",
  "primaryForeground",
  "secondary",
  "secondaryForeground",
  "muted",
  "mutedForeground",
  "accent",
  "accentForeground",
  "destructive",
  "destructiveForeground",
  "border",
  "input",
  "ring",
] as const;

const APP_THEME_FIELDS = new Set<string>(["light", "dark", "accentColor"]);
const WALLPAPER_FIELDS = new Set<string>(["kind", "assetId"]);
const PALETTE_KEYS = new Set<string>(PALETTE_FIELDS);
const APPEARANCE_ASSET_KINDS: readonly AppearanceAssetKindWire[] = [
  "font",
  "static_wallpaper",
  "dynamic_wallpaper",
];
const APPEARANCE_ASSET_STATES: readonly AppearanceAssetDto["state"][] = [
  "pending",
  "validated",
  "rejected",
];

/**
 * 未知字段检查基于 `Reflect.ownKeys`（字符串 + symbol，含不可枚举）：`Object.keys`
 * 看不到 symbol 键和 `defineProperty` 造出的非枚举键，`{...合法字段, [Symbol()]: x}`
 * 会被当成闭合 wire 数据透传，而 Rust 侧这些结构是 `deny_unknown_fields`。
 */
function hasExactOwnKeys(value: object, allowed: ReadonlySet<string>): boolean {
  const ownKeys = Reflect.ownKeys(value);
  return ownKeys.length === allowed.size
    && ownKeys.every((key) => typeof key === "string" && allowed.has(key));
}

function isPlainRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isHexColor(value: unknown): value is string {
  return typeof value === "string" && HEX_COLOR_PATTERN.test(value);
}

function isCanonicalAssetId(value: unknown): value is string {
  return typeof value === "string" && CANONICAL_ASSET_ID_PATTERN.test(value);
}

function isNullableAssetId(value: unknown): value is string | null {
  return value === null || isCanonicalAssetId(value);
}

/** 字体族名只能是一个安全的本机名称，不能携带 CSS 语句或路径。 */
export function isSafeUiFontFamilyName(value: unknown): value is string {
  if (typeof value !== "string") return false;
  const normalized = value.trim();
  if (normalized.length === 0 || [...normalized].length > 64) return false;
  return [...normalized].every((character) =>
    /[\p{L}\p{N}]/u.test(character) || " ._()+&'-".includes(character),
  );
}

function isNullableUiFontFamily(value: unknown): value is string | null | undefined {
  return value === undefined || value === null || isSafeUiFontFamilyName(value);
}

/** 调色板：19 个既有 token 一个不少、一个不多，取值必须是规范十六进制颜色。 */
export function guardAppThemePalette(value: unknown): value is AppThemePalette {
  if (!isPlainRecord(value) || !hasExactOwnKeys(value, PALETTE_KEYS)) return false;
  return PALETTE_FIELDS.every((field) => isHexColor(value[field]));
}

/** 自定义主题：明/暗两套调色板 + 独立的 `accentColor`。 */
export function guardAppTheme(value: unknown): value is AppTheme {
  if (!isPlainRecord(value) || !hasExactOwnKeys(value, APP_THEME_FIELDS)) return false;
  return guardAppThemePalette(value.light)
    && guardAppThemePalette(value.dark)
    && isHexColor(value.accentColor);
}

/**
 * 壁纸选择：`kind=none` 不得携带 assetId，`static` / `dynamic` 必须携带规范资产 ID。
 * 不存在 `path` / `url` 字段，多写任何一个键都会在这里被拒绝。
 */
export function guardWallpaperSelection(value: unknown): value is WallpaperSelection {
  if (!isPlainRecord(value)) return false;
  if (value.kind === "none") {
    return hasExactOwnKeys(value, new Set(["kind"]));
  }
  if (value.kind !== "static" && value.kind !== "dynamic") return false;
  return hasExactOwnKeys(value, WALLPAPER_FIELDS) && isCanonicalAssetId(value.assetId);
}

/** 外观资产投影：身份是不透明 ID，投影里没有路径、URL 或原始文件名。 */
export function guardAppearanceAsset(v: unknown): v is AppearanceAssetDto {
  if (!isPlainRecord(v)) return false;
  if (!hasExactOwnKeys(v, new Set(["assetId", "kind", "state", "byteSize", "displayName"]))) return false;
  if (!isCanonicalAssetId(v.assetId)) return false;
  if (!isOneOf(v.kind, APPEARANCE_ASSET_KINDS)) return false;
  if (!isOneOf(v.state, APPEARANCE_ASSET_STATES)) return false;
  // IPC 的 JSON 载荷到前端是 number（ts-rs 的 u64 → bigint 只是类型标注的旧口径，已经
  // 在 DTO 上改成 `#[ts(type = "number")]`）。这里只接受非负安全整数：资产上限 256 MiB，
  // 远小于 2^53，所以「安全整数」这条界不会误伤任何真实资产。
  const byteSize = v.byteSize;
  if (typeof byteSize !== "number" || !Number.isSafeInteger(byteSize) || byteSize < 0) return false;
  return typeof v.displayName === "string" || v.displayName === null;
}

export function guardAppearanceAssets(v: unknown): v is AppearanceAssetsDto {
  if (!isPlainRecord(v) || !hasExactOwnKeys(v, new Set(["schemaVersion", "assets"]))) return false;
  return v.schemaVersion === 1 && Array.isArray(v.assets) && v.assets.every(guardAppearanceAsset);
}

/** 删除结果：两部分事实分开报告（登记行是否删除 / 字节是否同时清理成功）。 */
export function guardAppearanceAssetDeleteResult(v: unknown): v is AppearanceAssetDeleteResultDto {
  if (!isPlainRecord(v) || !hasExactOwnKeys(v, new Set(["deleted", "fileRemoved"]))) return false;
  return typeof v.deleted === "boolean" && typeof v.fileRemoved === "boolean";
}

const HOME_MODULE_IDS: readonly HomeLayoutDto["modules"][number]["module"][] = [
  "continue",
  "recently_added",
  "shelf-favorites",
];
const HOME_MODULE_SIZES: readonly HomeLayoutDto["modules"][number]["size"][] = [
  "small",
  "medium",
  "large",
];
const HOME_LAYOUT_GRID_COLUMNS = 4;
const HOME_LAYOUT_GRID_ROWS = 12;
const HOME_LAYOUT_MAX_MODULES = 16;
const PLACEMENT_FIELDS = new Set<string>(["module", "size", "row", "column", "order"]);

/** 档位占用的列数（镜像 Rust `HomeModuleSize::column_span`；所有档位都只占一行）。 */
export function homeModuleColumnSpan(size: HomeLayoutDto["modules"][number]["size"]): number {
  if (size === "small") return 1;
  if (size === "medium") return 2;
  return HOME_LAYOUT_GRID_COLUMNS;
}

function isGridCoordinate(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

/**
 * 首页布局：镜像 Rust `HomeLayout::validate` 的全部不变量——schema 版本、模块数上限、
 * 坐标落在固定网格内、模块与排序号唯一、占用的格子不重叠。
 *
 * 越界坐标与重叠布局在 Rust 侧是 `APPEARANCE_INVALID_HOME_LAYOUT`，前端绝不能把一份
 * 后端会拒绝的布局当成合法数据渲染或提交。
 */
export function guardHomeLayout(v: unknown): v is HomeLayoutDto {
  if (!isPlainRecord(v) || !hasExactOwnKeys(v, new Set(["schemaVersion", "modules"]))) return false;
  if (v.schemaVersion !== 1 || !Array.isArray(v.modules)) return false;
  if (v.modules.length > HOME_LAYOUT_MAX_MODULES) return false;

  const occupied = new Set<string>();
  const modules = new Set<string>();
  const orders = new Set<number>();
  for (const placement of v.modules) {
    if (!isPlainRecord(placement) || !hasExactOwnKeys(placement, PLACEMENT_FIELDS)) return false;
    if (!isOneOf(placement.module, HOME_MODULE_IDS)) return false;
    if (!isOneOf(placement.size, HOME_MODULE_SIZES)) return false;
    const { row, column, order } = placement;
    if (!isGridCoordinate(row) || !isGridCoordinate(column) || !isGridCoordinate(order)) return false;
    if (row >= HOME_LAYOUT_GRID_ROWS) return false;
    if (order >= HOME_LAYOUT_MAX_MODULES) return false;
    const span = homeModuleColumnSpan(placement.size);
    if (column + span > HOME_LAYOUT_GRID_COLUMNS) return false;
    if (modules.has(placement.module) || orders.has(order)) return false;
    modules.add(placement.module);
    orders.add(order);
    for (let offset = 0; offset < span; offset += 1) {
      const cell = `${row}:${column + offset}`;
      if (occupied.has(cell)) return false;
      occupied.add(cell);
    }
  }
  return true;
}

/** `revision` 允许 null（从未保存过自定义布局）；非 null 时必须是非空 string。 */
function isNullableRevision(value: unknown): value is string | null {
  return value === null || (typeof value === "string" && value.length > 0);
}

/** 首页布局读取结果：`revision=null`（从未保存）与「保存了空布局」是两种不同事实。 */
export function guardHomeLayoutSnapshot(v: unknown): v is HomeLayoutSnapshotDto {
  if (!isPlainRecord(v) || !hasExactOwnKeys(v, new Set(["layout", "revision"]))) return false;
  return guardHomeLayout(v.layout) && isNullableRevision(v.revision);
}

/** 首页布局写入结果：`changed=false` 表示幂等重放（此时 revision 不变）。 */
export function guardHomeLayoutMutationResult(v: unknown): v is HomeLayoutMutationResultDto {
  if (!isPlainRecord(v) || !hasExactOwnKeys(v, new Set(["layout", "revision", "changed"]))) return false;
  return guardHomeLayout(v.layout)
    && isNullableRevision(v.revision)
    && typeof v.changed === "boolean";
}

/**
 * 领域默认首页布局（从未保存过自定义布局时后端返回的那一份）。
 *
 * 与 Rust `HomeLayout::default()` 逐字段一致：三个真实模块各自 medium（占 2 列），
 * 前两项在桌面网格首行并排，第三项在下一行；顺序为 0/1/2。Mock 与守卫测试共用它。
 */
export function defaultHomeLayout(): HomeLayoutDto {
  return {
    schemaVersion: 1,
    modules: [
      { module: "continue", size: "medium", row: 0, column: 0, order: 0 },
      { module: "recently_added", size: "medium", row: 0, column: 2, order: 1 },
      { module: "shelf-favorites", size: "medium", row: 1, column: 0, order: 2 },
    ],
  };
}

// ---- 总览布局守卫（与首页布局同形，但网格是三列）----
//
// 这一组镜像的是 **048 / OverviewLayout**，不是 045/046 / HomeLayout：两侧的列数、
// 闭合模块集合与档位跨度都不同。守卫必须按各自那一份规则判断，共用一套会让「总览允许
// 的布局」被首页的网格悄悄放宽（或收紧）。

const OVERVIEW_MODULE_IDS: readonly OverviewLayoutDto["modules"][number]["module"][] = [
  "preferences",
  "metrics",
  "reading-minutes",
  "type-share",
  "reading-heatmap",
];
const OVERVIEW_MODULE_SIZE_VALUES: readonly OverviewLayoutDto["modules"][number]["size"][] = [
  "small",
  "medium",
  "large",
];
/** 总览网格：3 列 × 12 行，最多 16 个模块（镜像 haven-domain 的 OVERVIEW_LAYOUT_*）。 */
const OVERVIEW_LAYOUT_GRID_COLUMNS = 3;
const OVERVIEW_LAYOUT_GRID_ROWS = 12;
const OVERVIEW_LAYOUT_MAX_MODULES = 16;

/** 档位占用的列数（镜像 Rust `OverviewModuleSize::column_span`；所有档位都只占一行）。 */
export function overviewModuleColumnSpan(
  size: OverviewLayoutDto["modules"][number]["size"],
): number {
  if (size === "small") return 1;
  if (size === "medium") return 2;
  return OVERVIEW_LAYOUT_GRID_COLUMNS;
}

/**
 * 总览布局：镜像 Rust `OverviewLayout::validate` 的全部不变量——schema 版本、模块数上限、
 * 坐标落在**三列**网格内、模块与排序号唯一、占用的格子不重叠。
 *
 * 越界坐标与重叠布局在 Rust 侧是 `APPEARANCE_INVALID_OVERVIEW_LAYOUT`，前端绝不能把一份
 * 后端会拒绝的布局当成合法数据渲染或提交。
 */
export function guardOverviewLayout(v: unknown): v is OverviewLayoutDto {
  if (!isPlainRecord(v) || !hasExactOwnKeys(v, new Set(["schemaVersion", "modules"]))) return false;
  if (v.schemaVersion !== 1 || !Array.isArray(v.modules)) return false;
  if (v.modules.length > OVERVIEW_LAYOUT_MAX_MODULES) return false;

  const occupied = new Set<string>();
  const modules = new Set<string>();
  const orders = new Set<number>();
  for (const placement of v.modules) {
    if (!isPlainRecord(placement) || !hasExactOwnKeys(placement, PLACEMENT_FIELDS)) return false;
    if (!isOneOf(placement.module, OVERVIEW_MODULE_IDS)) return false;
    if (!isOneOf(placement.size, OVERVIEW_MODULE_SIZE_VALUES)) return false;
    const { row, column, order } = placement;
    if (!isGridCoordinate(row) || !isGridCoordinate(column) || !isGridCoordinate(order)) return false;
    if (row >= OVERVIEW_LAYOUT_GRID_ROWS) return false;
    if (order >= OVERVIEW_LAYOUT_MAX_MODULES) return false;
    const span = overviewModuleColumnSpan(placement.size);
    if (column + span > OVERVIEW_LAYOUT_GRID_COLUMNS) return false;
    if (modules.has(placement.module) || orders.has(order)) return false;
    modules.add(placement.module);
    orders.add(order);
    for (let offset = 0; offset < span; offset += 1) {
      const cell = `${row}:${column + offset}`;
      if (occupied.has(cell)) return false;
      occupied.add(cell);
    }
  }
  return true;
}

/** 总览布局读取结果：`revision=null`（从未保存）与「保存了空布局」是两种不同事实。 */
export function guardOverviewLayoutSnapshot(v: unknown): v is OverviewLayoutSnapshotDto {
  if (!isPlainRecord(v) || !hasExactOwnKeys(v, new Set(["layout", "revision"]))) return false;
  return guardOverviewLayout(v.layout) && isNullableRevision(v.revision);
}

/** 总览布局写入结果：`changed=false` 表示幂等重放（此时 revision 不变）。 */
export function guardOverviewLayoutMutationResult(
  v: unknown,
): v is OverviewLayoutMutationResultDto {
  if (!isPlainRecord(v) || !hasExactOwnKeys(v, new Set(["layout", "revision", "changed"]))) return false;
  return guardOverviewLayout(v.layout)
    && isNullableRevision(v.revision)
    && typeof v.changed === "boolean";
}

/**
 * 领域默认总览布局（从未保存过自定义布局时后端返回的那一份）。
 *
 * 与 Rust `OverviewLayout::default()` 逐字段一致，也**就是升级前 `SettingsOverview` 的实际
 * 版式**：当前偏好与阅读指标各占一整行（3 列），两张图同行且宽度比 2:1（medium 2 列 +
 * small 1 列，对应旧的 `1.92fr:1fr`），热力图占一整行。三列网格存在的理由就是这一档
 * 2:1——四列网格只能给出均分。Mock、守卫测试与「布局读不到时的安全默认」共用它。
 */
export function defaultOverviewLayout(): OverviewLayoutDto {
  return {
    schemaVersion: 1,
    modules: [
      { module: "preferences", size: "large", row: 0, column: 0, order: 0 },
      { module: "metrics", size: "large", row: 1, column: 0, order: 1 },
      { module: "reading-minutes", size: "medium", row: 2, column: 0, order: 2 },
      { module: "type-share", size: "small", row: 2, column: 2, order: 3 },
      { module: "reading-heatmap", size: "large", row: 3, column: 0, order: 4 },
    ],
  };
}

// ---- 阅读总览守卫（闭合形状；范围与领域常量逐字对齐）----
/** 领域 `MIN_OVERVIEW_DAYS` / `MAX_OVERVIEW_DAYS`。 */
const READING_OVERVIEW_MIN_DAYS = 1;
const READING_OVERVIEW_MAX_DAYS = 90;
/** 领域 `MAX_UTC_OFFSET_MINUTES`（±14 小时）。 */
const READING_OVERVIEW_MAX_UTC_OFFSET_MINUTES = 14 * 60;
const READING_OVERVIEW_DAY_MS = 86_400_000;
const LOCAL_DATE_PATTERN = /^\d{4}-\d{2}-\d{2}$/;
const READING_SESSION_CATEGORIES: readonly ReadingSessionCategoryWire[] = [
  "video",
  "book",
  "comic",
  "periodical",
];

function isBoundedInteger(value: unknown, min: number, max: number): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= min && value <= max;
}

/** 时长 / 计数类字段：wire 上是 `number`（Rust `u64`），必须是非负安全整数。 */
function isDurationMs(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function isLocalDate(value: unknown): value is string {
  return typeof value === "string" && LOCAL_DATE_PATTERN.test(value);
}

/** 可选统计量：`null` 与数字是两种不同事实（`null` = 没有记录，不是 0）。 */
function isNullableDuration(value: unknown): boolean {
  return value === null || isDurationMs(value);
}

function isNullableBoundedInteger(value: unknown, min: number, max: number): boolean {
  return value === null || isBoundedInteger(value, min, max);
}

/**
 * 总览请求：越界窗口在 Rust 侧是 `READING_OVERVIEW_INVALID_RANGE`，
 * 前端不该把一份后端必然拒绝的请求发出去。
 */
export function guardReadingOverviewGetRequest(v: unknown): v is ReadingOverviewGetRequest {
  if (!isPlainRecord(v) || !hasExactOwnKeys(v, new Set(["days", "utcOffsetMinutes"]))) return false;
  return isBoundedInteger(v.days, READING_OVERVIEW_MIN_DAYS, READING_OVERVIEW_MAX_DAYS)
    && isBoundedInteger(
      v.utcOffsetMinutes,
      -READING_OVERVIEW_MAX_UTC_OFFSET_MINUTES,
      READING_OVERVIEW_MAX_UTC_OFFSET_MINUTES,
    );
}

/** 统计窗口：起止都是本地日期，天数与偏移与请求同源。 */
export function guardReadingOverviewRange(v: unknown): v is ReadingOverviewRangeWire {
  if (!isPlainRecord(v)) return false;
  if (!hasExactOwnKeys(v, new Set(["startLocalDate", "endLocalDate", "days", "utcOffsetMinutes"]))) {
    return false;
  }
  return isLocalDate(v.startLocalDate)
    && isLocalDate(v.endLocalDate)
    && isBoundedInteger(v.days, READING_OVERVIEW_MIN_DAYS, READING_OVERVIEW_MAX_DAYS)
    && isBoundedInteger(
      v.utcOffsetMinutes,
      -READING_OVERVIEW_MAX_UTC_OFFSET_MINUTES,
      READING_OVERVIEW_MAX_UTC_OFFSET_MINUTES,
    );
}

/** 单日时长：窗口内每一天一条（未阅读的空日时长就是 0，属于真实观测值）。 */
export function guardReadingDailyBucket(v: unknown): v is ReadingDailyBucketWire {
  if (!isPlainRecord(v) || !hasExactOwnKeys(v, new Set(["localDate", "weekday", "durationMs"]))) {
    return false;
  }
  return isLocalDate(v.localDate) && isBoundedInteger(v.weekday, 1, 7) && isDurationMs(v.durationMs);
}

/** 分类合计：只出现在真的有时长的分类上，时长恒为正。 */
export function guardReadingCategoryTotal(v: unknown): v is ReadingCategoryTotalWire {
  if (!isPlainRecord(v) || !hasExactOwnKeys(v, new Set(["category", "durationMs"]))) return false;
  return isOneOf(v.category, READING_SESSION_CATEGORIES)
    && isDurationMs(v.durationMs)
    && v.durationMs > 0;
}

/** 热力图格子：只出现在真的有时长的「星期 × 小时」上。 */
export function guardReadingHeatmapCell(v: unknown): v is ReadingHeatmapCellWire {
  if (!isPlainRecord(v) || !hasExactOwnKeys(v, new Set(["weekday", "hour", "durationMs"]))) {
    return false;
  }
  return isBoundedInteger(v.weekday, 1, 7)
    && isBoundedInteger(v.hour, 0, 23)
    && isDurationMs(v.durationMs)
    && v.durationMs > 0;
}

/**
 * 总览聚合：镜像领域 `aggregate` 的结构不变量——schema 版本、闭合分类枚举、
 * 星期/小时范围，以及「可选统计量在无记录时为 `null`」这一空态口径。
 *
 * `peakEndHour` 的右端点是起点 + 1 小时，所以合法上界是 24（不是 23）。
 */
export function guardReadingOverview(v: unknown): v is ReadingOverviewWire {
  if (!isPlainRecord(v)) return false;
  if (!hasExactOwnKeys(v, new Set([
    "schemaVersion",
    "range",
    "sessionCount",
    "totalDurationMs",
    "daily",
    "categories",
    "heatmapCells",
    "peakStartHour",
    "peakEndHour",
    "longestStreakDays",
    "averageDailyDurationMs",
    "recentWeekDurationMs",
  ]))) return false;
  if (v.schemaVersion !== 1) return false;
  if (!guardReadingOverviewRange(v.range)) return false;
  if (!isDurationMs(v.sessionCount) || !isDurationMs(v.totalDurationMs)) return false;
  if (!Array.isArray(v.daily) || !v.daily.every(guardReadingDailyBucket)) return false;
  // 领域对窗口内**每一天**都产出一条桶（未阅读的日子时长为 0），长度必须与窗口一致。
  if (v.daily.length !== v.range.days) return false;
  if (!Array.isArray(v.categories) || !v.categories.every(guardReadingCategoryTotal)) return false;
  if (!Array.isArray(v.heatmapCells) || !v.heatmapCells.every(guardReadingHeatmapCell)) return false;
  if (!isNullableBoundedInteger(v.peakStartHour, 0, 23)) return false;
  if (!isNullableBoundedInteger(v.peakEndHour, 1, 24)) return false;
  if (!isNullableBoundedInteger(v.longestStreakDays, 0, READING_OVERVIEW_MAX_DAYS)) return false;
  if (!isNullableDuration(v.averageDailyDurationMs) || !isNullableDuration(v.recentWeekDurationMs)) {
    return false;
  }

  // An empty fact set has an explicit wire shape.  Zero is a valid value for
  // each daily bucket (the bucket describes the requested window), but the
  // aggregate-level metrics must stay null so a producer cannot turn
  // "nothing has been read" into a fabricated 0-minute statistic.
  if (v.sessionCount === 0) {
    return v.totalDurationMs === 0
      && v.categories.length === 0
      && v.heatmapCells.length === 0
      && v.peakStartHour === null
      && v.peakEndHour === null
      && v.longestStreakDays === null
      && v.averageDailyDurationMs === null
      && v.recentWeekDurationMs === null;
  }

  return v.totalDurationMs > 0;
}

/** 本地日序号 → `YYYY-MM-DD`（镜像领域 `local_date_of_day_index`）。 */
function localDateOfDayIndex(dayIndex: number): string {
  const date = new Date(dayIndex * READING_OVERVIEW_DAY_MS);
  const year = String(date.getUTCFullYear()).padStart(4, "0");
  const month = String(date.getUTCMonth() + 1).padStart(2, "0");
  const day = String(date.getUTCDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

/** ISO 星期序号：1 = 周一 … 7 = 周日（`Date#getUTCDay` 的 0 = 周日要绕回 7）。 */
function isoWeekdayOfDayIndex(dayIndex: number): number {
  const weekday = new Date(dayIndex * READING_OVERVIEW_DAY_MS).getUTCDay();
  return weekday === 0 ? 7 : weekday;
}

/**
 * 显式空态总览：没有任何会话事实时的唯一合法形态。
 *
 * 逐字段镜像领域 `aggregate` 在 `session_count == 0` 时的输出——可选统计量全部 `null`
 * （不是 0），分类与热力图为空；`daily` 仍然按请求天数逐日展开，因为它描述的是**窗口
 * 本身**（哪些天落在范围内是确定的），而不是观察到的阅读。
 *
 * `now` 显式传入，保证同一份输入得到同一份结果（Mock 与测试共用同一个入口）。
 */
export function emptyReadingOverview(
  request: ReadingOverviewGetRequestWire,
  now: number,
): ReadingOverviewWire {
  const offsetMs = request.utcOffsetMinutes * 60_000;
  // 与 Rust `div_euclid` 同语义：本地日序号按偏移平移后再按天取整。
  const endDay = Math.floor((now + offsetMs) / READING_OVERVIEW_DAY_MS);
  const startDay = endDay - (request.days - 1);
  const daily: ReadingDailyBucketWire[] = [];
  for (let offset = 0; offset < request.days; offset += 1) {
    const dayIndex = startDay + offset;
    daily.push({
      localDate: localDateOfDayIndex(dayIndex),
      weekday: isoWeekdayOfDayIndex(dayIndex),
      durationMs: 0,
    });
  }
  return {
    schemaVersion: 1,
    range: {
      startLocalDate: localDateOfDayIndex(startDay),
      endLocalDate: localDateOfDayIndex(endDay),
      days: request.days,
      utcOffsetMinutes: request.utcOffsetMinutes,
    },
    sessionCount: 0,
    totalDurationMs: 0,
    daily,
    categories: [],
    heatmapCells: [],
    peakStartHour: null,
    peakEndHour: null,
    longestStreakDays: null,
    averageDailyDurationMs: null,
    recentWeekDurationMs: null,
  };
}

export function guardSettingsValue(v: unknown): v is SettingsValue {
  if (typeof v !== "object" || v === null) return false;
  const value = v as Record<string, unknown>;
  if (value.section === "general") {
    return (
      isOneOf(value.launchPage, LAUNCH_PAGES) &&
      typeof value.restoreSession === "boolean" &&
      isOneOf(value.language, LANGUAGES) &&
      typeof value.notifications === "boolean"
    );
  }
  if (value.section === "appearance") {
    const family = value.interfaceFontFamily;
    const familyOk =
      family === undefined || (typeof family === "string" && isSafeInterfaceFontFamily(family));
    const assetId = value.interfaceFontAssetId;
    const assetOk =
      assetId === undefined ||
      (typeof assetId === "string" && isCanonicalInterfaceFontAssetId(assetId));
    return (
      isOneOf(value.theme, THEMES) &&
      isOneOf(value.density, DENSITIES) &&
      isOneOf(value.sidebar, SIDEBARS) &&
      typeof value.reduceMotion === "boolean" &&
      isOneOf(value.interfaceFontMode, INTERFACE_FONT_MODES) &&
      familyOk &&
      assetOk &&
      // 缺失 = Rust `#[serde(default)]` 的默认值；显式 null = 尚未创建自定义主题；
      // 只有真正提供调色板对象时才必须严格校验。
      (value.customTheme === undefined || value.customTheme === null || guardAppTheme(value.customTheme)) &&
      (value.wallpaper === undefined || guardWallpaperSelection(value.wallpaper)) &&
      (value.customFontAssetId === undefined || isNullableAssetId(value.customFontAssetId)) &&
      (value.uiFontPreset === undefined || isOneOf(value.uiFontPreset, UI_FONT_PRESETS)) &&
      isNullableUiFontFamily(value.uiFontFamily)
    );
  }
  if (value.section === "playback") {
    return isOneOf(value.defaultPlaybackRate, PLAYBACK_RATES)
      && typeof value.autoResume === "boolean"
      && typeof value.autoNext === "boolean";
  }
  if (value.section === "reading") {
    const customFontOk = value.customFontFamily == null || typeof value.customFontFamily === "string";
    const customBgOk = value.customBackground == null || (typeof value.customBackground === "string" && /^#[0-9a-fA-F]{6}$/.test(value.customBackground));
    const customTextOk = value.customText == null || (typeof value.customText === "string" && /^#[0-9a-fA-F]{6}$/.test(value.customText));
    return (
      isOneOf(value.fontFamily, READING_FONT_FAMILIES) &&
      customFontOk &&
      isOneOf(value.fontSize, READING_FONT_SIZES) &&
      isOneOf(value.lineHeight, READING_LINE_HEIGHTS) &&
      isOneOf(value.contentWidth, READING_CONTENT_WIDTHS) &&
      isOneOf(value.theme, READING_THEMES) &&
      customBgOk &&
      customTextOk &&
      isOneOf(value.fontWeight, READING_FONT_WEIGHTS) &&
      isOneOf(value.letterSpacing, READING_LETTER_SPACINGS) &&
      typeof value.systemAuto === "boolean" &&
      (value.pagination === undefined || isOneOf(value.pagination, READING_PAGINATIONS))
    );
  }
  if (value.section === "downloads") {
    return isOneOf(value.concurrentTasks, DOWNLOAD_CONCURRENCIES)
      && isOneOf(value.speedLimit, DOWNLOAD_SPEED_LIMITS)
      && typeof value.autoContinue === "boolean";
  }
  if (value.section === "comic") {
    return isOneOf(value.viewMode, COMIC_VIEW_MODES)
      && isOneOf(value.direction, COMIC_DIRECTIONS)
      && isOneOf(value.pageGap, COMIC_PAGE_GAPS)
      && isOneOf(value.preloadPages, COMIC_PRELOAD_PAGES);
  }
  if (value.section === "privacy") {
    return typeof value.searchHistory === "boolean" && typeof value.playbackHistory === "boolean";
  }
  return false;
}

export function guardSettingsSnapshot(v: unknown): v is SettingsSnapshot {
  if (typeof v !== "object" || v === null) return false;
  const snapshot = v as Record<string, unknown>;
  if (!guardSettingsValue(snapshot.value)) return false;
  // R-SETTINGS-001：revision 允许 null（从未保存）；非 null 时必须为非空 string。
  if (snapshot.revision !== null && (typeof snapshot.revision !== "string" || snapshot.revision.length === 0)) {
    return false;
  }
  return true;
}

export function guardSettingsUpdateResult(v: unknown): v is SettingsUpdateResult {
  if (typeof v !== "object" || v === null) return false;
  const result = v as Record<string, unknown>;
  if (!guardSettingsValue(result.value)) return false;
  if (typeof result.changed !== "boolean") return false;
  if (result.revision !== null && (typeof result.revision !== "string" || result.revision.length === 0)) {
    return false;
  }
  return true;
}

function isNullableString(value: unknown): value is string | null {
  return value === null || (typeof value === "string" && value.length > 0);
}

function guardReadingPatch(value: unknown): value is PreferenceReadingPatchWire | null {
  if (value === null) return true;
  if (typeof value !== "object") return false;
  const patch = value as Record<string, unknown>;
  if (patch.fontFamily != null && !isOneOf(patch.fontFamily, READING_FONT_FAMILIES)) return false;
  if (patch.customFontFamily !== undefined && patch.customFontFamily !== null && typeof patch.customFontFamily !== "string") return false;
  if (patch.fontSize != null && !isOneOf(patch.fontSize, READING_FONT_SIZES)) return false;
  if (patch.lineHeight != null && !isOneOf(patch.lineHeight, READING_LINE_HEIGHTS)) return false;
  if (patch.contentWidth != null && !isOneOf(patch.contentWidth, READING_CONTENT_WIDTHS)) return false;
  if (patch.theme != null && !isOneOf(patch.theme, READING_THEMES)) return false;
  if (patch.customBackground !== undefined && patch.customBackground !== null && (typeof patch.customBackground !== "string" || !/^#[0-9a-fA-F]{6}$/.test(patch.customBackground))) return false;
  if (patch.customText !== undefined && patch.customText !== null && (typeof patch.customText !== "string" || !/^#[0-9a-fA-F]{6}$/.test(patch.customText))) return false;
  if (patch.fontWeight != null && !isOneOf(patch.fontWeight, READING_FONT_WEIGHTS)) return false;
  if (patch.letterSpacing != null && !isOneOf(patch.letterSpacing, READING_LETTER_SPACINGS)) return false;
  if (patch.systemAuto !== undefined && typeof patch.systemAuto !== "boolean") return false;
  if (patch.pagination != null && !isOneOf(patch.pagination, READING_PAGINATIONS)) return false;
  return true;
}

function guardComicPatch(value: unknown): value is PreferenceComicPatchWire | null {
  if (value === null) return true;
  if (typeof value !== "object") return false;
  const patch = value as Record<string, unknown>;
  return (patch.viewMode == null || isOneOf(patch.viewMode, COMIC_VIEW_MODES))
    && (patch.direction == null || isOneOf(patch.direction, COMIC_DIRECTIONS))
    && (patch.pageGap == null || isOneOf(patch.pageGap, COMIC_PAGE_GAPS))
    && (patch.preloadPages == null || isOneOf(patch.preloadPages, COMIC_PRELOAD_PAGES));
}

/**
 * `PreferenceGetResult` 同时返回两个作用域的原始覆盖。每个字段都必须经过
 * 与请求 patch 相同的闭合枚举/颜色校验；否则一个漂移的后端响应会绕过
 * 设置页的 safe fallback，最终把无效值带进 Reader/Comic。
 */
function guardPreferencePatches(result: Record<string, unknown>): boolean {
  return guardReadingPatch(result.editionReadingPatch)
    && guardComicPatch(result.editionComicPatch)
    && guardReadingPatch(result.mediaItemReadingPatch)
    && guardComicPatch(result.mediaItemComicPatch);
}

export function guardPreferenceGetResult(v: unknown): v is PreferenceGetResult {
  if (typeof v !== "object" || v === null) return false;
  const result = v as Record<string, unknown>;
  return result.schemaVersion === 1
    && typeof result.mediaItemId === "string"
    && typeof result.editionId === "string"
    && guardReadingPatch(result.readingPatch)
    && guardComicPatch(result.comicPatch)
    && guardPreferencePatches(result)
    && guardSettingsValue(result.effectiveReading)
    && guardSettingsValue(result.effectiveComic)
    && (result.effectiveReading as { section?: unknown }).section === "reading"
    && (result.effectiveComic as { section?: unknown }).section === "comic"
    && isNullableString(result.mediaItemRevision)
    && isNullableString(result.editionRevision);
}

export function guardPreferenceUpdateResult(v: unknown): v is PreferenceUpdateResult {
  if (typeof v !== "object" || v === null) return false;
  const result = v as Record<string, unknown>;
  return guardPreferenceGetResult(result.result)
    && isOneOf(result.target, ["edition", "media_item"])
    && isNullableString(result.revision)
    && typeof result.changed === "boolean";
}

export function guardSettingsChanged(v: unknown): v is SettingsChangedDto {
  if (typeof v !== "object" || v === null) return false;
  const dto = v as Record<string, unknown>;
  return (
    dto.schemaVersion === 1 &&
    typeof dto.at === "string" &&
    typeof dto.operationId === "string" &&
    dto.operationId.length > 0 &&
    typeof dto.sequence === "number" &&
    dto.sequence >= 1 &&
    isOneOf(dto.section, ["general", "appearance", "playback", "reading", "comic", "downloads", "privacy"]) &&
    typeof dto.revision === "string" &&
    dto.revision.length > 0
  );
}

// ---- Wire 语义辅助（镜像后端 apply_to / 值比较；Mock CAS 与表单 dirty 检测共用）----

/**
 * 可清除的可选选择字段（本机字体族名 / 导入资产 id）的合并语义。
 *
 * 与后端 `SettingsPatch::apply_to` 一致：
 * - `undefined` = patch 未提及 → 保留当前值；
 * - 空/纯空白字符串 = 显式清除 → `undefined`（后端存 None，序列化时字段消失）；
 * - 其它 → trim 后的新值。
 */
function mergeOptionalSelection(
  patchValue: string | undefined,
  current: string | undefined,
): string | undefined {
  if (patchValue === undefined) return current;
  const trimmed = patchValue.trim();
  return trimmed === "" ? undefined : trimmed;
}

/** 「无壁纸」是明确状态（不是缺失），apply/比较/patch 三处共用同一份字面量。 */
const NONE_WALLPAPER: WallpaperSelection = { kind: "none" };

/** 深浅两套调色板逐 token 比较（缺一个 token 也会判为不等，N/A 时用 `??` 归一为 null）。 */
function appThemesEqual(a: AppTheme | null | undefined, b: AppTheme | null | undefined): boolean {
  if (a == null || b == null) return (a ?? null) === (b ?? null);
  if (a.accentColor !== b.accentColor) return false;
  return PALETTE_FIELDS.every(
    (field) => a.light[field] === b.light[field] && a.dark[field] === b.dark[field],
  );
}

/** 壁纸比较：kind 与 assetId 都必须一致（缺失归一等同 `none`）。 */
function wallpapersEqual(
  a: WallpaperSelection | undefined,
  b: WallpaperSelection | undefined,
): boolean {
  const left = a ?? NONE_WALLPAPER;
  const right = b ?? NONE_WALLPAPER;
  return left.kind === right.kind && (left.assetId ?? null) === (right.assetId ?? null);
}

/** 把 patch 应用到当前值（部分更新；未知 section 组合防御性返回原值）。 */
export function applySettingsPatch(value: SettingsValue, patch: SettingsPatch): SettingsValue {
  if (value.section === "general" && patch.section === "general") {
    return {
      section: "general",
      launchPage: patch.launchPage ?? value.launchPage,
      restoreSession: patch.restoreSession ?? value.restoreSession,
      language: patch.language ?? value.language,
      notifications: patch.notifications ?? value.notifications,
    };
  }
  if (value.section === "appearance" && patch.section === "appearance") {
    return {
      section: "appearance",
      theme: patch.theme ?? value.theme,
      density: patch.density ?? value.density,
      sidebar: patch.sidebar ?? value.sidebar,
      reduceMotion: patch.reduceMotion ?? value.reduceMotion,
      interfaceFontMode: patch.interfaceFontMode ?? value.interfaceFontMode,
      interfaceFontFamily: mergeOptionalSelection(patch.interfaceFontFamily, value.interfaceFontFamily),
      interfaceFontAssetId: mergeOptionalSelection(patch.interfaceFontAssetId, value.interfaceFontAssetId),
      // 「显式 null = 清除」必须用 `!== undefined` 判断：`??` 会把清除当成不改。
      customTheme: patch.customTheme !== undefined ? patch.customTheme : (value.customTheme ?? null),
      wallpaper: patch.wallpaper !== undefined
        ? (patch.wallpaper ?? NONE_WALLPAPER)
        : (value.wallpaper ?? NONE_WALLPAPER),
      customFontAssetId: patch.customFontAssetId !== undefined
        ? patch.customFontAssetId
        : (value.customFontAssetId ?? null),
      uiFontPreset: patch.uiFontPreset ?? value.uiFontPreset ?? "system",
      uiFontFamily: patch.uiFontFamily !== undefined
        ? (patch.uiFontFamily?.trim() ? patch.uiFontFamily.trim() : null)
        : (value.uiFontFamily ?? null),
    };
  }
  if (value.section === "playback" && patch.section === "playback") {
    return {
      section: "playback",
      defaultPlaybackRate: patch.defaultPlaybackRate ?? value.defaultPlaybackRate,
      autoResume: patch.autoResume ?? value.autoResume,
      autoNext: patch.autoNext ?? value.autoNext,
    };
  }
  if (value.section === "reading" && patch.section === "reading") {
    const customFontFamily = patch.customFontFamily !== undefined
      ? (patch.customFontFamily?.trim() ? patch.customFontFamily.trim() : null)
      : value.customFontFamily;
    const customBackground = patch.customBackground !== undefined
      ? (patch.customBackground?.trim() ? patch.customBackground.trim() : null)
      : value.customBackground;
    const customText = patch.customText !== undefined
      ? (patch.customText?.trim() ? patch.customText.trim() : null)
      : value.customText;
    return {
      section: "reading",
      fontFamily: patch.fontFamily ?? value.fontFamily,
      customFontFamily,
      fontSize: patch.fontSize ?? value.fontSize,
      lineHeight: patch.lineHeight ?? value.lineHeight,
      contentWidth: patch.contentWidth ?? value.contentWidth,
      theme: patch.theme ?? value.theme,
      customBackground,
      customText,
      fontWeight: patch.fontWeight ?? value.fontWeight,
      letterSpacing: patch.letterSpacing ?? value.letterSpacing,
      systemAuto: patch.systemAuto ?? value.systemAuto,
      pagination: patch.pagination ?? value.pagination ?? "scroll",
    };
  }
  if (value.section === "downloads" && patch.section === "downloads") {
    return {
      section: "downloads",
      concurrentTasks: patch.concurrentTasks ?? value.concurrentTasks,
      speedLimit: patch.speedLimit ?? value.speedLimit,
      autoContinue: patch.autoContinue ?? value.autoContinue,
    };
  }
  if (value.section === "comic" && patch.section === "comic") {
    return {
      section: "comic",
      viewMode: patch.viewMode ?? value.viewMode,
      direction: patch.direction ?? value.direction,
      pageGap: patch.pageGap ?? value.pageGap,
      preloadPages: patch.preloadPages ?? value.preloadPages,
    };
  }
  if (value.section === "privacy" && patch.section === "privacy") {
    return {
      section: "privacy",
      searchHistory: patch.searchHistory ?? value.searchHistory,
      playbackHistory: patch.playbackHistory ?? value.playbackHistory,
    };
  }
  return value;
}

/** 语义值比较（决定 dirty 与幂等；与后端 `next_value == current_value` 对齐）。 */
export function settingsValuesEqual(a: SettingsValue, b: SettingsValue): boolean {
  if (a.section !== b.section) return false;
  if (a.section === "general" && b.section === "general") {
    return (
      a.launchPage === b.launchPage &&
      a.restoreSession === b.restoreSession &&
      a.language === b.language &&
      a.notifications === b.notifications
    );
  }
  if (a.section === "appearance" && b.section === "appearance") {
    return (
      a.theme === b.theme &&
      a.density === b.density &&
      a.sidebar === b.sidebar &&
      a.reduceMotion === b.reduceMotion &&
      a.interfaceFontMode === b.interfaceFontMode &&
      a.interfaceFontFamily === b.interfaceFontFamily &&
      a.interfaceFontAssetId === b.interfaceFontAssetId &&
      appThemesEqual(a.customTheme, b.customTheme) &&
      wallpapersEqual(a.wallpaper, b.wallpaper) &&
      (a.customFontAssetId ?? null) === (b.customFontAssetId ?? null) &&
      (a.uiFontPreset ?? "system") === (b.uiFontPreset ?? "system") &&
      (a.uiFontFamily ?? null) === (b.uiFontFamily ?? null)
    );
  }
  if (a.section === "playback" && b.section === "playback") {
    return a.defaultPlaybackRate === b.defaultPlaybackRate
      && a.autoResume === b.autoResume
      && a.autoNext === b.autoNext;
  }
  if (a.section === "reading" && b.section === "reading") {
    return a.fontFamily === b.fontFamily && a.customFontFamily === b.customFontFamily
      && a.fontSize === b.fontSize && a.lineHeight === b.lineHeight
      && a.contentWidth === b.contentWidth && a.theme === b.theme
      && a.customBackground === b.customBackground && a.customText === b.customText
      && a.fontWeight === b.fontWeight && a.letterSpacing === b.letterSpacing
      && a.systemAuto === b.systemAuto
      && (a.pagination ?? "scroll") === (b.pagination ?? "scroll");
  }
  if (a.section === "downloads" && b.section === "downloads") {
    return a.concurrentTasks === b.concurrentTasks
      && a.speedLimit === b.speedLimit
      && a.autoContinue === b.autoContinue;
  }
  if (a.section === "comic" && b.section === "comic") {
    return a.viewMode === b.viewMode && a.direction === b.direction
      && a.pageGap === b.pageGap && a.preloadPages === b.preloadPages;
  }
  if (a.section === "privacy" && b.section === "privacy") {
    return a.searchHistory === b.searchHistory && a.playbackHistory === b.playbackHistory;
  }
  return false;
}

/** 未保存 Section 的默认值（与后端 SettingsValue::default_for 对齐）。 */
export function defaultSettingsValue(section: SettingsSectionWire): SettingsValue {
  if (section === "general") {
    return { section: "general", launchPage: "home", restoreSession: false, language: "zh_cn", notifications: true };
  }
  if (section === "appearance") {
    return {
      section: "appearance",
      theme: "system",
      density: "comfortable",
      sidebar: "auto",
      reduceMotion: false,
      interfaceFontMode: "system",
      interfaceFontFamily: undefined,
      interfaceFontAssetId: undefined,
      customTheme: null,
      wallpaper: { kind: "none" },
      customFontAssetId: null,
      uiFontPreset: "system",
      uiFontFamily: null,
    };
  }
  if (section === "playback") {
    return { section: "playback", defaultPlaybackRate: "one", autoResume: true, autoNext: true };
  }
  if (section === "reading") {
    return {
      section: "reading",
      fontFamily: "serif",
      customFontFamily: null,
      fontSize: "medium",
      lineHeight: "comfortable",
      contentWidth: "medium",
      theme: "warm",
      customBackground: null,
      customText: null,
      fontWeight: "regular",
      letterSpacing: "normal",
      systemAuto: true,
      pagination: "scroll",
    };
  }
  if (section === "downloads") {
    return { section: "downloads", concurrentTasks: "three", speedLimit: "unlimited", autoContinue: true };
  }
  if (section === "comic") {
    return { section: "comic", viewMode: "single", direction: "rtl", pageGap: "twelve", preloadPages: "three" };
  }
  return { section: "privacy", searchHistory: true, playbackHistory: true };
}

/** 构造只含已变化字段的 patch；无变化（相同值/空 patch）返回 null。 */
export function buildSettingsPatch(saved: SettingsValue, draft: SettingsValue): SettingsPatch | null {
  if (settingsValuesEqual(saved, draft)) return null;
  if (saved.section === "general" && draft.section === "general") {
    const patch: GeneralPatchWire = { section: "general" };
    if (draft.launchPage !== saved.launchPage) patch.launchPage = draft.launchPage;
    if (draft.restoreSession !== saved.restoreSession) patch.restoreSession = draft.restoreSession;
    if (draft.language !== saved.language) patch.language = draft.language;
    if (draft.notifications !== saved.notifications) patch.notifications = draft.notifications;
    return patch;
  }
  if (saved.section === "appearance" && draft.section === "appearance") {
    const patch: AppearancePatchWire = { section: "appearance" };
    if (draft.theme !== saved.theme) patch.theme = draft.theme;
    if (draft.density !== saved.density) patch.density = draft.density;
    if (draft.sidebar !== saved.sidebar) patch.sidebar = draft.sidebar;
    if (draft.reduceMotion !== saved.reduceMotion) patch.reduceMotion = draft.reduceMotion;
    if (draft.interfaceFontMode !== saved.interfaceFontMode) patch.interfaceFontMode = draft.interfaceFontMode;
    // 选择字段用空字符串表达「清除」：后端只在字段出现时才覆盖，
    // 因此「清空选择」必须显式发一个空串，不能靠省略字段。
    if (draft.interfaceFontFamily !== saved.interfaceFontFamily) {
      patch.interfaceFontFamily = draft.interfaceFontFamily ?? "";
    }
    if (draft.interfaceFontAssetId !== saved.interfaceFontAssetId) {
      patch.interfaceFontAssetId = draft.interfaceFontAssetId ?? "";
    }
    // 清除（draft=null）与「不改」必须能分辨，所以显式写 null 而不是省略字段。
    if (!appThemesEqual(draft.customTheme, saved.customTheme)) patch.customTheme = draft.customTheme ?? null;
    if (!wallpapersEqual(draft.wallpaper, saved.wallpaper)) patch.wallpaper = draft.wallpaper ?? NONE_WALLPAPER;
    if ((draft.customFontAssetId ?? null) !== (saved.customFontAssetId ?? null)) {
      patch.customFontAssetId = draft.customFontAssetId ?? null;
    }
    if ((draft.uiFontPreset ?? "system") !== (saved.uiFontPreset ?? "system")) {
      patch.uiFontPreset = draft.uiFontPreset ?? "system";
    }
    if ((draft.uiFontFamily ?? null) !== (saved.uiFontFamily ?? null)) {
      patch.uiFontFamily = draft.uiFontFamily ?? null;
    }
    return patch;
  }
  if (saved.section === "playback" && draft.section === "playback") {
    const patch: PlaybackPatchWire = { section: "playback" };
    if (draft.defaultPlaybackRate !== saved.defaultPlaybackRate) patch.defaultPlaybackRate = draft.defaultPlaybackRate;
    if (draft.autoResume !== saved.autoResume) patch.autoResume = draft.autoResume;
    if (draft.autoNext !== saved.autoNext) patch.autoNext = draft.autoNext;
    return patch;
  }
  if (saved.section === "reading" && draft.section === "reading") {
    const patch: ReadingPatchWire = { section: "reading" };
    if (draft.fontFamily !== saved.fontFamily) patch.fontFamily = draft.fontFamily;
    if (draft.customFontFamily !== saved.customFontFamily) patch.customFontFamily = draft.customFontFamily ?? null;
    if (draft.fontSize !== saved.fontSize) patch.fontSize = draft.fontSize;
    if (draft.lineHeight !== saved.lineHeight) patch.lineHeight = draft.lineHeight;
    if (draft.contentWidth !== saved.contentWidth) patch.contentWidth = draft.contentWidth;
    if (draft.theme !== saved.theme) patch.theme = draft.theme;
    if (draft.customBackground !== saved.customBackground) patch.customBackground = draft.customBackground ?? null;
    if (draft.customText !== saved.customText) patch.customText = draft.customText ?? null;
    if (draft.fontWeight !== saved.fontWeight) patch.fontWeight = draft.fontWeight;
    if (draft.letterSpacing !== saved.letterSpacing) patch.letterSpacing = draft.letterSpacing;
    if (draft.systemAuto !== saved.systemAuto) patch.systemAuto = draft.systemAuto;
    if ((draft.pagination ?? "scroll") !== (saved.pagination ?? "scroll")) patch.pagination = draft.pagination ?? "scroll";
    return patch;
  }
  if (saved.section === "downloads" && draft.section === "downloads") {
    const patch: DownloadPatchWire = { section: "downloads" };
    if (draft.concurrentTasks !== saved.concurrentTasks) patch.concurrentTasks = draft.concurrentTasks;
    if (draft.speedLimit !== saved.speedLimit) patch.speedLimit = draft.speedLimit;
    if (draft.autoContinue !== saved.autoContinue) patch.autoContinue = draft.autoContinue;
    return patch;
  }
  if (saved.section === "comic" && draft.section === "comic") {
    const patch: ComicPatchWire = { section: "comic" };
    if (draft.viewMode !== saved.viewMode) patch.viewMode = draft.viewMode;
    if (draft.direction !== saved.direction) patch.direction = draft.direction;
    if (draft.pageGap !== saved.pageGap) patch.pageGap = draft.pageGap;
    if (draft.preloadPages !== saved.preloadPages) patch.preloadPages = draft.preloadPages;
    return patch;
  }
  if (saved.section === "privacy" && draft.section === "privacy") {
    const patch: PrivacyPatchWire = { section: "privacy" };
    if (draft.searchHistory !== saved.searchHistory) patch.searchHistory = draft.searchHistory;
    if (draft.playbackHistory !== saved.playbackHistory) patch.playbackHistory = draft.playbackHistory;
    return patch;
  }
  return null;
}
