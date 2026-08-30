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
  PreferenceComicPatchDto,
  PreferenceGetRequest as GeneratedPreferenceGetRequest,
  PreferenceGetResult as GeneratedPreferenceGetResult,
  PreferenceReadingPatchDto,
  PreferenceTargetDto,
  PreferenceUpdateRequest as GeneratedPreferenceUpdateRequest,
  PreferenceUpdateResult as GeneratedPreferenceUpdateResult,
} from "./generated/wire";

export type SettingsSectionWire = "general" | "appearance" | "playback" | "reading" | "comic" | "downloads" | "privacy";

export type LaunchPageWire = "home" | "library" | "continue" | "last_session";
export type LanguageWire = "zh_cn" | "en_us" | "zh_tw";
export type ThemeWire = "system" | "light" | "dark";
export type DensityWire = "comfortable" | "compact";
export type SidebarWire = "expanded" | "collapsed" | "auto";
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

// ---- 守卫：运行时验证契约不变量（闭合枚举 / 形状 / schemaVersion），禁止裸 as ----

export function parseSettingsSection(raw: string): SettingsSectionWire | null {
  return raw === "general" || raw === "appearance" || raw === "playback" || raw === "reading" || raw === "comic" || raw === "downloads" || raw === "privacy" ? raw : null;
}

const LAUNCH_PAGES: readonly LaunchPageWire[] = ["home", "library", "continue", "last_session"];
const LANGUAGES: readonly LanguageWire[] = ["zh_cn", "en_us", "zh_tw"];
const THEMES: readonly ThemeWire[] = ["system", "light", "dark"];
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
    return (
      isOneOf(value.theme, THEMES) &&
      isOneOf(value.density, DENSITIES) &&
      isOneOf(value.sidebar, SIDEBARS) &&
      typeof value.reduceMotion === "boolean"
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
      a.reduceMotion === b.reduceMotion
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
    return { section: "appearance", theme: "system", density: "comfortable", sidebar: "auto", reduceMotion: false };
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
