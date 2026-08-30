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
  ReadingThemeWire,
  SidebarWire,
  ThemeWire,
} from "../../../lib/ipc/settings-wire";

export interface DisplayOption<T extends string> {
  value: T;
  label: string;
}

export const LAUNCH_PAGE_OPTIONS: DisplayOption<LaunchPageWire>[] = [
  { value: "home", label: "首页" },
  { value: "library", label: "媒体库" },
  { value: "continue", label: "继续上次内容" },
  { value: "last_session", label: "上次打开位置" },
];

export const LANGUAGE_OPTIONS: DisplayOption<LanguageWire>[] = [
  { value: "zh_cn", label: "简体中文" },
  { value: "zh_tw", label: "繁體中文" },
  { value: "en_us", label: "English" },
];

export const THEME_OPTIONS: DisplayOption<ThemeWire>[] = [
  { value: "system", label: "跟随系统" },
  { value: "light", label: "浅色" },
  { value: "dark", label: "深色" },
];

export const DENSITY_OPTIONS: DisplayOption<DensityWire>[] = [
  { value: "comfortable", label: "舒适" },
  { value: "compact", label: "紧凑" },
];

export const SIDEBAR_OPTIONS: DisplayOption<SidebarWire>[] = [
  { value: "auto", label: "自动" },
  { value: "expanded", label: "展开" },
  { value: "collapsed", label: "收起" },
];

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

export function optionLabel<T extends string>(options: DisplayOption<T>[], value: T): string {
  return options.find((option) => option.value === value)?.label ?? value;
}

export function optionValue<T extends string>(options: DisplayOption<T>[], label: string): T {
  return options.find((option) => option.label === label)?.value
    ?? (() => { throw new Error(`未知的显示文案（无法映射回 wire 值）：${label}`); })();
}
