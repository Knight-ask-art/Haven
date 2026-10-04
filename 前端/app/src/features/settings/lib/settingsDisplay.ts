// Settings 显示映射（wire 枚举值 ↔ 界面文案；FE-SETTINGS-001）。
// 表单状态持有 wire 值（dirty 检测与 CAS 提交的单一事实源），界面只做映射，不存文案副本。

import type {
  DensityWire,
  DownloadConcurrencyWire,
  DownloadSpeedLimitWire,
  LanguageWire,
  LaunchPageWire,
  PlaybackRateWire,
  ReadingContentWidthWire,
  ReadingFontFamilyWire,
  ReadingFontSizeWire,
  ReadingFontWeightWire,
  ReadingLetterSpacingWire,
  ReadingLineHeightWire,
  ReadingPaginationWire,
  ReadingPatchWire,
  ReadingSettingsValue,
  ReadingThemeWire,
  SidebarWire,
  ThemeWire,
} from "../../../lib/ipc/settings-wire";
import {
  MAX_CUSTOM_FONT_FAMILY_LENGTH,
  READING_FONT_CLASSES,
  READING_THEME_PRESENTATION,
  resolveReadingCustomColors,
  sanitizeCustomFontFamilyName,
  sanitizeCustomReadingColor,
  type ReadingPresentation,
} from "../../reader/lib/reading-settings-mapping";
import type { InterfaceFontModeWire } from "../../../lib/ipc/interface-font-wire";

export interface DisplayOption<T extends string> {
  value: T;
  label: string;
}

export const LAUNCH_PAGE_OPTIONS: DisplayOption<LaunchPageWire>[] = [
  { value: "home", label: "首页" },
  { value: "library", label: "媒体库" },
  { value: "continue", label: "继续上次内容" },
  { value: "last_session", label: "继续上次内容" },
];

/** 旧值继续按原契约读取，只在呈现中合并同义选项；不写入或迁移用户配置。 */
export function launchPageChoice(value: LaunchPageWire): LaunchPageWire {
  return value === "last_session" ? "continue" : value;
}
export const LAUNCH_PAGE_CHOICES = LAUNCH_PAGE_OPTIONS.filter((option) => option.value !== "last_session");

export const LANGUAGE_OPTIONS: DisplayOption<LanguageWire>[] = [
  { value: "zh_cn", label: "简体中文" },
  { value: "zh_tw", label: "繁體中文" },
  { value: "en_us", label: "English" },
];

export const THEME_OPTIONS: DisplayOption<ThemeWire>[] = [
  { value: "system", label: "跟随系统" },
  { value: "light", label: "浅色" },
  { value: "dark", label: "深色" },
  { value: "custom", label: "自定义" },
];

export const DENSITY_OPTIONS: DisplayOption<DensityWire>[] = [
  { value: "comfortable", label: "舒适" },
  { value: "compact", label: "紧凑" },
];

export const SIDEBAR_OPTIONS: DisplayOption<SidebarWire>[] = [
  { value: "auto", label: "默认" },
  { value: "expanded", label: "宽松" },
  { value: "collapsed", label: "紧凑" },
];

/**
 * 界面字体四张互斥卡片（外观 → 界面字体）。
 *
 * 字面值与 `haven-domain` 的 `InterfaceFontMode`（snake_case）一一对应；
 * `custom` 卡片不是占位——它展开真实的本机字体列表与导入字体列表。
 */
export const INTERFACE_FONT_MODE_OPTIONS: DisplayOption<InterfaceFontModeWire>[] = [
  { value: "system", label: "跟随系统" },
  { value: "serif", label: "人文衬线" },
  { value: "sans", label: "现代黑体" },
  { value: "custom", label: "自定义系统字体" },
];

/** 卡片副标题：说明该模式实际会用的字体栈，而不是营销文案。 */
export const INTERFACE_FONT_MODE_DETAILS: Record<InterfaceFontModeWire, string> = {
  system: "不覆盖，保持栖阅既有字体栈",
  sans: "清晰、利落的界面字形",
  serif: "温和、耐读的宋体系字形",
  custom: "从本机字体或导入字体中选择",
};

export const PLAYBACK_RATE_OPTIONS: DisplayOption<PlaybackRateWire>[] = [
  { value: "point_seven_five", label: "0.75x" },
  { value: "one", label: "1.0x" },
  { value: "one_point_two_five", label: "1.25x" },
  { value: "one_point_five", label: "1.5x" },
  { value: "two", label: "2.0x" },
];

export const DOWNLOAD_CONCURRENCY_OPTIONS: DisplayOption<DownloadConcurrencyWire>[] = [
  { value: "one", label: "1 个任务" },
  { value: "two", label: "2 个任务" },
  { value: "three", label: "3 个任务" },
  { value: "five", label: "5 个任务" },
];

export const DOWNLOAD_SPEED_LIMIT_OPTIONS: DisplayOption<DownloadSpeedLimitWire>[] = [
  { value: "unlimited", label: "不限速" },
  { value: "kbps512", label: "512 KB/s" },
  { value: "mbps2", label: "2 MB/s" },
  { value: "mbps5", label: "5 MB/s" },
  { value: "mbps10", label: "10 MB/s" },
];

export const READING_FONT_OPTIONS: DisplayOption<ReadingFontFamilyWire>[] = [
  { value: "sans", label: "系统无衬线" },
  { value: "serif", label: "系统衬线" },
  { value: "kai", label: "楷体" },
  { value: "heiti", label: "黑体" },
  { value: "fangsong", label: "仿宋" },
  { value: "mianfei", label: "免费字体" },
  { value: "custom", label: "自定义" },
];

export const READING_FONT_WEIGHT_OPTIONS: DisplayOption<ReadingFontWeightWire>[] = [
  { value: "light", label: "细" },
  { value: "regular", label: "常规" },
  { value: "medium", label: "中等" },
  { value: "semibold", label: "半粗" },
  { value: "bold", label: "粗" },
];

export const READING_LETTER_SPACING_OPTIONS: DisplayOption<ReadingLetterSpacingWire>[] = [
  { value: "tight", label: "紧凑" },
  { value: "normal", label: "正常" },
  { value: "relaxed", label: "宽松" },
  { value: "loose", label: "很松" },
];

export const READING_FONT_SIZE_OPTIONS: DisplayOption<ReadingFontSizeWire>[] = [
  { value: "small", label: "小" },
  { value: "medium", label: "中" },
  { value: "large", label: "大" },
];

export const READING_LINE_HEIGHT_OPTIONS: DisplayOption<ReadingLineHeightWire>[] = [
  { value: "compact", label: "紧凑" },
  { value: "comfortable", label: "舒适" },
  { value: "airy", label: "宽松" },
];

export const READING_WIDTH_OPTIONS: DisplayOption<ReadingContentWidthWire>[] = [
  { value: "narrow", label: "窄" },
  { value: "medium", label: "适中" },
  { value: "wide", label: "宽" },
];

export const READING_THEME_OPTIONS: DisplayOption<ReadingThemeWire>[] = [
  { value: "system", label: "跟随系统" },
  { value: "paper", label: "纸张" },
  { value: "warm", label: "暖光" },
  { value: "slate", label: "石板" },
  { value: "dark", label: "夜间" },
  { value: "sepia", label: "复古" },
  { value: "eyeCare", label: "护眼" },
  { value: "custom", label: "自定义" },
];

export const READING_PAGINATION_OPTIONS: DisplayOption<ReadingPaginationWire>[] = [
  { value: "scroll", label: "连续滚动" },
  { value: "paginated", label: "单页分页" },
  { value: "double", label: "双页分页" },
];

// ---- 阅读模式：顶层「滚动 / 分页」+ 分页下的「单页 / 双页」子选择 ----
//
// 契约里只有一个 `reading.pagination` 字段（scroll / paginated / double），这里不新增
// wire 取值，只把同一组取值呈现成两级：顶层把 scroll 与「分页」分开，子选择只在分页时
// 出现并直接写回 paginated / double。

export type ReadingPaginationMode = "scroll" | "paginated";
export type ReadingPageSubMode = "paginated" | "double";

export const READING_PAGINATION_MODE_OPTIONS: DisplayOption<ReadingPaginationMode>[] = [
  { value: "scroll", label: "连续滚动" },
  { value: "paginated", label: "分页" },
];

export const READING_PAGE_SUBMODE_OPTIONS: DisplayOption<ReadingPageSubMode>[] = [
  { value: "paginated", label: "单页" },
  { value: "double", label: "双页" },
];

/** 顶层模式：scroll 之外的一切都是「分页」。 */
export function readingPaginationMode(mode: ReadingPaginationWire): ReadingPaginationMode {
  return mode === "scroll" ? "scroll" : "paginated";
}

/** 分页子选择；滚动模式下没有子选择，返回默认的单页取值。 */
export function readingPageSubMode(mode: ReadingPaginationWire): ReadingPageSubMode {
  return mode === "double" ? "double" : "paginated";
}

/** 两级选择合成唯一的 wire 取值：顶层选滚动即 scroll，否则由子选择决定。 */
export function readingPaginationWire(
  mode: ReadingPaginationMode,
  subMode: ReadingPageSubMode,
): ReadingPaginationWire {
  if (mode === "scroll") return "scroll";
  return subMode === "double" ? "double" : "paginated";
}

export function optionLabel<T extends string>(options: DisplayOption<T>[], value: T): string {
  return options.find((option) => option.value === value)?.label ?? value;
}

export function optionValue<T extends string>(options: DisplayOption<T>[], label: string): T {
  return options.find((option) => option.label === label)?.value
    ?? (() => { throw new Error(`未知的显示文案（无法映射回 wire 值）：${label}`); })();
}

// ---- 自定义阅读外观（reading.theme=custom / reading.fontFamily=custom）----
//
// Settings 只负责把界面输入翻译成契约里的取值：颜色必须落在 `#rrggbb`，字体族名
// 只做 CSS 族名写法校验 —— 这里不查询本机字体注册表，也不导入字体文件，所以
// 「写进去了」不等于「本机装了这个字体」，族名对不上时由阅读器的字体栈兜底。
// 非法输入一律拒绝写入草稿 —— `guardSettingsValue` 会拒绝含非法
// customBackground/customText 的整份 Reading 快照，草稿写坏会让整区设置都读不出来。

export type CustomReadingColorField = "customBackground" | "customText";

export const READING_CUSTOM_BACKGROUND_LABEL = "自定义背景色";
export const READING_CUSTOM_TEXT_LABEL = "自定义文字色";
export const READING_CUSTOM_FONT_LABEL = "自定义字体族名";

export const READING_CUSTOM_APPEARANCE_HINT = "只作用于文本阅读器，PDF 等非文本资源继续使用各自阅读器；保存后对新的阅读会话生效。背景色与文字色需要先把上方「阅读底色」选为「自定义」才会生效。";
// 这里的文案只说校验器真的做了什么：它检查的是 CSS 族名写法，不查本机字体列表，
// 所以不能写「已确认本机安装了该字体」。
export const READING_CUSTOM_FONT_HINT = `只作用于文本阅读器；需要填写本机已安装字体的族名（如「思源宋体」「Source Han Serif SC」）。这里只校验族名的 CSS 写法，不会查询本机字体列表：族名不存在时正文会退回系统字体栈，也不支持字体文件导入。留空表示清除已保存的字体名。`;
export const READING_CUSTOM_FONT_PLACEHOLDER = "本机字体族名";
export const READING_CUSTOM_FONT_INVALID_HINT = `字数超过 ${MAX_CUSTOM_FONT_FAMILY_LENGTH}，或含有引号、反斜杠、分号等 CSS 特殊字符；这里只校验族名写法、不校验是否已安装，请填写字体族名而不是字体文件路径。`;

/** 自定义配色控件在非「自定义」底色下不可用的原因，直接说明怎么才能调整。 */
export const READING_CUSTOM_COLOR_DISABLED_HINT = "只在上方「阅读底色」选为「自定义」时可调整；选择预设底色不会清除已保存的自定义颜色。";

/**
 * 取色器写入的 patch。输入不是契约要求的 `#rrggbb` 时返回 null，调用方应丢弃这次
 * 输入而不是把非法值写进草稿。
 *
 * 这里**只写颜色本身**：是否使用自定义配色由用户在主题选择里显式决定。取色器不再
 * 顺手把主题改成「自定义」——那会让一次颜色微调静默丢弃用户选中的预设主题；调用方
 * 只在 `theme=custom` 时启用取色器，所以颜色写进去就一定有人消费。
 */
export function customReadingColorPatch(field: CustomReadingColorField, rawValue: string): ReadingPatchWire | null {
  const value = sanitizeCustomReadingColor(rawValue);
  if (!value) return null;
  return field === "customBackground"
    ? { section: "reading", customBackground: value }
    : { section: "reading", customText: value };
}

/** 取色器展示值：未设置或存了非法颜色的快照一律回退到可渲染的兜底色。 */
export function customReadingColorInputValue(raw: string | null, fallback: string): string {
  return sanitizeCustomReadingColor(raw) ?? fallback;
}

export type CustomFontFamilyCommit =
  | { status: "invalid" }
  | { status: "ready"; patch: ReadingPatchWire };

/**
 * 字体族名输入的提交语义：留空只清除已保存的字体名（不动默认字体选项），通过校验
 * 的名称原样保存并同时把默认字体切到「自定义」；其余判为 invalid，调用方只提示、
 * 不写草稿。
 */
export function customFontFamilyCommit(rawInput: string): CustomFontFamilyCommit {
  if (rawInput.trim().length === 0) {
    return { status: "ready", patch: { section: "reading", customFontFamily: null } };
  }
  const fontFamily = sanitizeCustomFontFamilyName(rawInput);
  if (!fontFamily) return { status: "invalid" };
  return { status: "ready", patch: { section: "reading", fontFamily: "custom", customFontFamily: fontFamily } };
}

// ---- 阅读底色：主题卡片写回的 patch ----

/**
 * 主题卡片的 patch。
 *
 * 契约里 `reading.systemAuto` 就是「跟随系统外观」这个开关本身，只在 `theme=system`
 * 时生效（Rust：`system_auto` 只影响 `ReadingTheme::System`）。只写 `theme` 的话，
 * 历史快照里的 `systemAuto=false` 会把选中的「跟随系统」渲染成固定的暖光——卡片说
 * 跟随系统，实际不跟随。所以选「跟随系统」时把两者一起写；已经是 system 时再点一次
 * 同样会写 `systemAuto=true`，用来修复旧快照。固定主题不需要动这个字段。
 */
export function readingThemePatch(theme: ReadingThemeWire): ReadingPatchWire {
  return theme === "system"
    ? { section: "reading", theme, systemAuto: true }
    : { section: "reading", theme };
}

// ---- 阅读排版预览（设置页里的实时示例）----
//
// 预览不自己算一套排版：它吃 `resolveReadingPresentation` 的结果，也就是两个文本阅读器
// 真正用来渲染正文的那份取值（theme / fontSizePx / lineHeight / contentWidthPx /
// customFontFamily）。这里只补阅读器不需要、设置页需要的两件事：样例文字的字体族类名与
// 配色，以及把真实正文宽度（620 / 700 / 820 px）换算成预览框里的相对宽度。

export interface ReadingPreviewPalette {
  /** 预览底色；既可以是 hex，也可以是（跟随系统用的）渐变。 */
  background: string;
  color: string;
}

/** 主题卡片上的色块：「跟随系统」用半浅半深的渐变表示两种落点，其余用主题真实配色。 */
function readingThemePalette(theme: keyof typeof READING_THEME_PRESENTATION): ReadingPreviewPalette {
  const { background, color } = READING_THEME_PRESENTATION[theme];
  return { background, color };
}

const READING_THEME_SWATCH: Record<ReadingThemeWire, ReadingPreviewPalette> = {
  system: { background: "linear-gradient(135deg, #f5efe3 0 50%, #0f0f11 50% 100%)", color: "#3c332b" },
  paper: readingThemePalette("paper"),
  warm: readingThemePalette("warm"),
  slate: readingThemePalette("slate"),
  dark: readingThemePalette("dark"),
  sepia: readingThemePalette("sepia"),
  eyeCare: readingThemePalette("eyeCare"),
  custom: readingThemePalette("custom"),
};

/** 某个主题取值在卡片上的示意色；自定义主题优先显示已保存（并通过校验）的颜色。 */
export function readingThemeSwatch(
  theme: ReadingThemeWire,
  custom: Pick<ReadingSettingsValue, "customBackground" | "customText">,
): ReadingPreviewPalette {
  if (theme !== "custom") return READING_THEME_SWATCH[theme];
  return {
    background: sanitizeCustomReadingColor(custom.customBackground) ?? READING_THEME_PRESENTATION.custom.background,
    color: sanitizeCustomReadingColor(custom.customText) ?? READING_THEME_PRESENTATION.custom.color,
  };
}

/** 预览样例的配色：自定义主题走阅读器同一套回退规则，不在这里另算一遍。 */
export function readingPreviewPalette(presentation: ReadingPresentation): ReadingPreviewPalette {
  if (presentation.theme !== "custom") {
    const { background, color } = READING_THEME_PRESENTATION[presentation.theme];
    return { background, color };
  }
  const colors = resolveReadingCustomColors("custom", {
    background: presentation.customBackground,
    text: presentation.customText,
    fontFamily: presentation.customFontFamily,
  });
  return colors
    ? { background: colors.backgroundColor, color: colors.color }
    : { background: READING_THEME_PRESENTATION.custom.background, color: READING_THEME_PRESENTATION.custom.color };
}

/** 样例文字直接复用两种阅读器的字体映射，避免预览与实际渲染分叉。 */
export const READING_FONT_PREVIEW_CLASS: Record<ReadingFontFamilyWire, string> = READING_FONT_CLASSES;

/**
 * 除了预设类名还会叠加 inline `font-family` 的取值。
 *
 * 这些取值光看类名会误判成等价项，但它们真的会换字体：`custom` 的类名是 `font-sans`，
 * 同时 `resolveCustomFontFamilyCss` 会给正文加真实的族名，所以不算等价。
 */
const READING_FONT_INLINE_FAMILY_VALUES: ReadonlySet<ReadingFontFamilyWire> = new Set(["custom"]);

/**
 * 某个字体取值在阅读器里是否与更早的选项渲染完全相同。
 *
 * 判据是真实渲染结果而不是文案：`READING_FONT_PREVIEW_CLASS` 就是阅读器 FONT_CLASSES
 * 的逐项副本，类名相同、两边又都不叠加 inline 族名时，选中它不会让正文发生任何变化
 * ——`fangsong` 与 `serif`、`mianfei` 与 `sans` 就是这种情况。返回与之等价的取值；
 * 本身就是独立渲染时返回 null。
 */
export function readingFontAlias(value: ReadingFontFamilyWire): ReadingFontFamilyWire | null {
  if (READING_FONT_INLINE_FAMILY_VALUES.has(value)) return null;
  const index = READING_FONT_OPTIONS.findIndex((option) => option.value === value);
  if (index <= 0) return null;
  const className = READING_FONT_PREVIEW_CLASS[value];
  const alias = READING_FONT_OPTIONS
    .slice(0, index)
    .find((option) => !READING_FONT_INLINE_FAMILY_VALUES.has(option.value)
      && READING_FONT_PREVIEW_CLASS[option.value] === className);
  return alias?.value ?? null;
}

/**
 * 字体取值的显示文案。与别的取值渲染相同时如实标注「等同谁」，而不是只报一个在阅读器
 * 里根本不存在的字体——旧设置里存了 `fangsong` / `mianfei` 时，页面不能把它说成正文
 * 真的用了仿宋或某款免费字体。
 */
export function readingFontLabel(value: ReadingFontFamilyWire): string {
  const label = optionLabel(READING_FONT_OPTIONS, value);
  const alias = readingFontAlias(value);
  return alias ? `${label}（等同${optionLabel(READING_FONT_OPTIONS, alias)}）` : label;
}

/**
 * 字体选择器的可选项：隐藏「选了没有任何渲染变化」的等价取值。
 *
 * wire 枚举保持原样（后端、旧快照和两个文本阅读器都还在用 `fangsong` / `mianfei`），
 * 只是不再把它们当新选项提供——把一个和「系统衬线」完全相同的项摆出来叫「仿宋」，
 * 就是一个改了没效果的假控件。
 */
export const READING_FONT_PICKER_OPTIONS: DisplayOption<ReadingFontFamilyWire>[] =
  READING_FONT_OPTIONS.filter((option) => readingFontAlias(option.value) === null);

/**
 * 选择器选项 + 旧保存值的兼容显示。
 *
 * 历史快照里已经存了 `fangsong` / `mianfei` 时，下拉必须仍然显示并回选当前值：否则
 * 用户一进设置就看到一个空选择，一保存就把旧设置静默改掉。兼容项只在当前值就是它时
 * 出现，并注明它实际渲染成什么，所以既不假装它能换字体，也不篡改旧设置。
 */
export function readingFontPickerOptions(current: ReadingFontFamilyWire): DisplayOption<ReadingFontFamilyWire>[] {
  if (readingFontAlias(current) === null) return READING_FONT_PICKER_OPTIONS;
  return [...READING_FONT_PICKER_OPTIONS, { value: current, label: readingFontLabel(current) }];
}

/** 阅读器最宽的正文栏（820 px）。预览以它为满宽等比示意，不改变任何真实取值。 */
export const READING_PREVIEW_FULL_MEASURE_PX = 820;

/** 预览正文栏的相对宽度：窄 / 适中 / 宽分别约为满宽的 76% / 85% / 100%。 */
export function readingPreviewMeasureRatio(presentation: ReadingPresentation): number {
  return Math.min(1, presentation.contentWidthPx / READING_PREVIEW_FULL_MEASURE_PX);
}

/**
 * 预览标题栏里的字体说明。选了自定义字体但族名还没填（或未通过校验）时如实说明
 * 正文会按预设类名渲染，而不是把空的族名说成已经生效。
 */
export function readingPreviewFontLabel(
  settings: Pick<ReadingSettingsValue, "fontFamily" | "customFontFamily">,
): string {
  if (settings.fontFamily !== "custom") return readingFontLabel(settings.fontFamily);
  const family = sanitizeCustomFontFamilyName(settings.customFontFamily);
  return family ? `${family}（自定义）` : "自定义（未填族名，按系统无衬线渲染）";
}
