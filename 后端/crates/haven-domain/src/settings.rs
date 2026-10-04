//! Settings 领域模型（BE-SETTINGS-001）。
//!
//! - Section 使用**闭合枚举**：不接受任意字符串 section；未知 Section 在 parse 时拒绝。
//! - 每个 Section 是 **Typed DTO**（serde `deny_unknown_fields`）：未知字段/非法枚举/越界值
//!   在反序列化边界拒绝，禁止任意 JSON Map 无校验写入。
//! - Patch 版本字段全为 `Option`：未提供的字段保持原值（部分更新）。
//! - Secret 禁止进入设置数据（凭据走 CredentialStore，只存 credential_ref）。
//! - 分区只包含已经具备真实消费闭环的字段；Comic 仅开放已由漫画阅读器消费的
//!   全局默认偏好，OCR/翻译等 Foundation 能力不进入设置事实源。

use serde::{Deserialize, Serialize};

use crate::appearance::{AppTheme, AppearanceAssetId, WallpaperSelection};

/// 设置分区（闭合枚举；新增分区为向后兼容扩展，未知字符串一律拒绝）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsSection {
    General,
    Appearance,
    Playback,
    Reading,
    Comic,
    Downloads,
    Privacy,
}

impl SettingsSection {
    /// 从 wire 字符串解析（未知 section 拒绝）。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "general" => Some(Self::General),
            "appearance" => Some(Self::Appearance),
            "playback" => Some(Self::Playback),
            "reading" => Some(Self::Reading),
            "comic" => Some(Self::Comic),
            "downloads" => Some(Self::Downloads),
            "privacy" => Some(Self::Privacy),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Appearance => "appearance",
            Self::Playback => "playback",
            Self::Reading => "reading",
            Self::Comic => "comic",
            Self::Downloads => "downloads",
            Self::Privacy => "privacy",
        }
    }
}

/// 启动页（general.launchPage）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchPage {
    Home,
    Library,
    Continue,
    LastSession,
}

/// 界面语言（general.language；BCP-47 子集）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    ZhCn,
    EnUs,
    ZhTw,
}

/// 主题（appearance.theme）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    System,
    Light,
    Dark,
    Custom,
}

/// 密度（appearance.density）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    Comfortable,
    Compact,
}

/// 侧边栏偏好（appearance.sidebar）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidebarPreference {
    Expanded,
    Collapsed,
    Auto,
}

/// 应用界面字体方案（appearance.uiFontPreset）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiFontPreset {
    #[default]
    System,
    HumanistSerif,
    ModernSans,
    CustomSystem,
}

/// general 分区设置（Typed DTO；JSON 字段 camelCase，与 wire 规则一致）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GeneralSettings {
    pub launch_page: LaunchPage,
    pub restore_session: bool,
    pub language: Language,
    pub notifications: bool,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            launch_page: LaunchPage::Home,
            restore_session: false,
            language: Language::ZhCn,
            notifications: true,
        }
    }
}

/// 界面字体模式（appearance.interfaceFontMode）。
///
/// 四个互斥模式对应设置页的四张字体卡片，各自有真实 CSS 字体栈消费者
/// （`applyInterfaceFont` → `--ds-font-interface`），不是占位枚举：
/// - `System`：随操作系统界面字体（等价于栖阅既有字体栈）；
/// - `Sans` / `Serif`：显式指定无衬线 / 衬线优先栈；
/// - `Custom`：由用户在本机字体或已导入字体中选择，选择结果分别落在
///   `interfaceFontFamily` / `interfaceFontAssetId`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterfaceFontMode {
    System,
    Sans,
    Serif,
    Custom,
}

fn default_interface_font_mode() -> InterfaceFontMode {
    InterfaceFontMode::System
}

/// 界面字体族名安全上界（与设置页输入提示一致）。
const MAX_INTERFACE_FONT_FAMILY_LEN: usize = 120;

/// 界面字体族名是否可安全拼进 CSS `font-family` 列表。
///
/// 族名只允许来自本机字体枚举或用户显式输入；这里拒绝控制字符、引号、分号、
/// 大括号、反斜杠和 `/*`，避免保存出可以逃出 `font-family` 声明的值。
/// 该函数是**保存边界校验**，渲染端仍会再做一次同样的判断。
pub fn is_safe_interface_font_family(name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_INTERFACE_FONT_FAMILY_LEN {
        return false;
    }
    !trimmed.chars().any(|c| {
        c.is_control()
            || c == '"'
            || c == '\''
            || c == ';'
            || c == '{'
            || c == '}'
            || c == '\\'
            || c == '<'
            || c == '>'
    }) && !trimmed.contains("/*")
}

/// 把任意来源（字体 `name` 表或文件名）的族名修成可安全保存的形式。
///
/// 导入字体的族名来自第三方文件，必须能落进 `font-family`：
/// 去掉不安全字符、折叠空白、截断到长度上限；结果为空时退化为 `fallback`。
/// 与 [`is_safe_interface_font_family`] 互为「修 → 验」，修复后必然通过校验。
pub fn sanitize_interface_font_family(name: &str, fallback: &str) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    let mut length = 0usize;
    for ch in name.chars() {
        if ch.is_control() || matches!(ch, '"' | '\'' | ';' | '{' | '}' | '\\' | '<' | '>') {
            continue;
        }
        if ch.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        let additional = if pending_space { 2 } else { 1 };
        if length + additional > MAX_INTERFACE_FONT_FAMILY_LEN {
            break;
        }
        if pending_space {
            out.push(' ');
            length += 1;
            pending_space = false;
        }
        out.push(ch);
        length += 1;
    }
    let cleaned = out.trim();
    // `/*` 只能由 `/` 与 `*` 相邻产生；两者本身合法，因此单独再兜一次。
    let cleaned = cleaned.replace("/*", " ");
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || !is_safe_interface_font_family(cleaned) {
        return fallback.to_owned();
    }
    cleaned.to_owned()
}

/// appearance 分区设置（Typed DTO；JSON 字段 camelCase，与 wire 规则一致）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AppearanceSettings {
    pub theme: Theme,
    pub density: Density,
    pub sidebar: SidebarPreference,
    pub reduce_motion: bool,
    /// 旧版本设置行没有界面字体字段时，按原观感回落到系统默认字体。
    #[serde(default = "default_interface_font_mode")]
    pub interface_font_mode: InterfaceFontMode,
    /// 仅 `Custom` 且选择本机字体时生效：已安装字体族名（永不保存路径）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface_font_family: Option<String>,
    /// 仅 `Custom` 且选择导入字体时生效：`font_assets` 的 opaque id。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface_font_asset_id: Option<String>,
    /// 用户保存的应用级明/暗调色板与强调色。`None` 表示尚未创建自定义主题。
    /// 大调色板独立分配，避免放大所有分区设置；序列化仍是原有主题对象。
    #[serde(default)]
    pub custom_theme: Option<Box<AppTheme>>,
    /// 首页壁纸只引用不透明资产 ID；`none` 是明确的无壁纸状态。
    #[serde(default)]
    pub wallpaper: WallpaperSelection,
    /// 阅读运行时加载的自定义字体资产；字体资产本身由 Appearance Repository 管理。
    #[serde(default)]
    pub custom_font_asset_id: Option<AppearanceAssetId>,
    /// 应用 UI 使用的字体方案；缺失时跟随系统默认字体。
    #[serde(default)]
    pub ui_font_preset: UiFontPreset,
    /// 仅 `customSystem` 方案下读取；只保存系统字体族名，不包含路径或 CSS 声明。
    #[serde(default, deserialize_with = "deserialize_ui_font_family")]
    pub ui_font_family: Option<String>,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            density: Density::Comfortable,
            sidebar: SidebarPreference::Auto,
            reduce_motion: false,
            interface_font_mode: InterfaceFontMode::System,
            interface_font_family: None,
            interface_font_asset_id: None,
            custom_theme: None,
            wallpaper: WallpaperSelection::None,
            custom_font_asset_id: None,
            ui_font_preset: UiFontPreset::System,
            ui_font_family: None,
        }
    }
}

/// playback 分区设置。
///
/// 这里只保存播放器已经具备真实消费者的默认倍速、进度恢复和自动下一集开关。
/// 字幕、音轨、硬件解码和截图目录仍属于后续引擎能力，不进入本分区，
/// 避免设置被保存却没有任何播放引擎消费。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PlaybackSettings {
    pub default_playback_rate: PlaybackRate,
    pub auto_resume: bool,
    /// 播放到当前 Edition 的最后一项后是否自动打开下一项。
    /// 旧设置行没有该字段时默认开启，保持播放器原有的连续播放行为。
    #[serde(default = "default_auto_next")]
    pub auto_next: bool,
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
            default_playback_rate: PlaybackRate::One,
            auto_resume: true,
            auto_next: true,
        }
    }
}

fn default_auto_next() -> bool {
    true
}

/// 播放倍速（闭合集合；与 Player/VideoControls 的可选值一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackRate {
    PointSevenFive,
    One,
    OnePointTwoFive,
    OnePointFive,
    Two,
}

/// 阅读字体（reading.fontFamily）— 6 预设 + custom。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadingFontFamily {
    Sans,
    Serif,
    Kai,
    Heiti,
    Fangsong,
    Mianfei,
    Custom,
}

/// 阅读字号档位（reading.fontSize）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadingFontSize {
    Small,
    Medium,
    Large,
}

/// 阅读行高档位（reading.lineHeight）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadingLineHeight {
    Compact,
    Comfortable,
    Airy,
}

/// 阅读正文宽度档位（reading.contentWidth）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadingContentWidth {
    Narrow,
    Medium,
    Wide,
}

/// 阅读字重档位（reading.fontWeight）— 300..700 闭合。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadingFontWeight {
    Light,    // 300
    Regular,  // 400
    Medium,   // 500
    Semibold, // 600
    Bold,     // 700
}

impl ReadingFontWeight {
    pub const fn as_u16(self) -> u16 {
        match self {
            Self::Light => 300,
            Self::Regular => 400,
            Self::Medium => 500,
            Self::Semibold => 600,
            Self::Bold => 700,
        }
    }
}

/// 阅读字距档位（reading.letterSpacing）— -0.02..0.12em 闭合。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadingLetterSpacing {
    Tight,   // -0.02
    Normal,  // 0.0
    Relaxed, // 0.06
    Loose,   // 0.12
}

impl ReadingLetterSpacing {
    pub const fn as_f32(self) -> f32 {
        match self {
            Self::Tight => -0.02,
            Self::Normal => 0.0,
            Self::Relaxed => 0.06,
            Self::Loose => 0.12,
        }
    }
}

fn default_reading_font_weight() -> ReadingFontWeight {
    ReadingFontWeight::Regular
}

fn default_reading_letter_spacing() -> ReadingLetterSpacing {
    ReadingLetterSpacing::Normal
}

/// 阅读主题（reading.theme）— 6 预设 + custom/system。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadingTheme {
    System,
    Paper,
    Warm,
    Slate,
    Dark,
    Sepia,
    EyeCare,
    Custom,
}

/// 文本阅读分页模式（reading.pagination）。
///
/// `scroll` 保持现有连续滚动行为；`paginated` 使用单栏分页，`double`
/// 在宽屏上最多并排两栏。分页只作用于文本类 Reader，PDF 使用独立渲染器。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadingPagination {
    Scroll,
    Paginated,
    Double,
}

fn default_reading_pagination() -> ReadingPagination {
    ReadingPagination::Scroll
}

/// reading 分区设置。
///
/// 该分区保存文本类 Reader（EPUB/TXT/Markdown/文章）的全局默认偏好，含 6 字体/6 主题
/// + 本机/上传 + 字重/字距 + 双取色器 + systemAuto，以及文本分页布局。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReadingSettings {
    pub font_family: ReadingFontFamily,
    /// 仅 `Custom` 时生效的本机/上传字体族名。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_font_family: Option<String>,
    pub font_size: ReadingFontSize,
    pub line_height: ReadingLineHeight,
    pub content_width: ReadingContentWidth,
    pub theme: ReadingTheme,
    /// 仅 `Custom` 时生效的背景/文字色（`#rrggbb`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_background: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_text: Option<String>,
    /// 旧版本设置行没有这些扩展字段时，按原有阅读观感补安全默认值。
    #[serde(default = "default_reading_font_weight")]
    pub font_weight: ReadingFontWeight,
    #[serde(default = "default_reading_letter_spacing")]
    pub letter_spacing: ReadingLetterSpacing,
    /// 是否跟随系统 `prefers-color-scheme`（`theme=System` 时生效）。
    #[serde(default = "default_system_auto")]
    pub system_auto: bool,
    /// 文本阅读布局模式。旧设置行缺失时保持连续滚动。
    #[serde(default = "default_reading_pagination")]
    pub pagination: ReadingPagination,
}

fn default_system_auto() -> bool {
    true
}

impl Default for ReadingSettings {
    fn default() -> Self {
        Self {
            font_family: ReadingFontFamily::Serif,
            custom_font_family: None,
            font_size: ReadingFontSize::Medium,
            line_height: ReadingLineHeight::Comfortable,
            content_width: ReadingContentWidth::Medium,
            theme: ReadingTheme::Warm,
            custom_background: None,
            custom_text: None,
            font_weight: ReadingFontWeight::Regular,
            letter_spacing: ReadingLetterSpacing::Normal,
            system_auto: true,
            pagination: ReadingPagination::Scroll,
        }
    }
}

/// 漫画阅读模式（comic.viewMode）。
///
/// 这些值与 Comic Reader 已有的会话渲染模式一一对应；设置只提供新会话
/// 的默认值，阅读器内的临时切换不会回写全局设置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComicViewMode {
    Single,
    Double,
    Strip,
}

/// 漫画翻页方向（comic.direction）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComicDirection {
    Rtl,
    Ltr,
}

/// 漫画页面间距（像素档位，避免把任意 CSS 数值写入设置）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComicPageGap {
    Zero,
    Twelve,
    TwentyFour,
}

impl ComicPageGap {
    pub const fn as_pixels(self) -> u8 {
        match self {
            Self::Zero => 0,
            Self::Twelve => 12,
            Self::TwentyFour => 24,
        }
    }
}

/// 漫画预加载窗口档位。`unlimited` 在前端仍受固定安全上限约束，
/// 不能通过设置让资源池一次挂载全部页面。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComicPreloadPages {
    One,
    Three,
    Five,
    Unlimited,
}

impl ComicPreloadPages {
    pub const fn radius(self) -> usize {
        match self {
            Self::One => 1,
            Self::Three => 3,
            Self::Five => 5,
            // 读取侧将无限预加载限制在固定窗口内，避免恶意/损坏配置导致
            // 千页漫画一次性进入 DOM 或资源许可池。
            Self::Unlimited => 12,
        }
    }
}

/// comic 分区设置。
///
/// 只包含 Comic Reader 已有真实消费者的全局默认偏好。OCR 与翻译依赖
/// 尚未完成的 AI Foundation，不进入设置事实源。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ComicSettings {
    pub view_mode: ComicViewMode,
    pub direction: ComicDirection,
    pub page_gap: ComicPageGap,
    pub preload_pages: ComicPreloadPages,
}

impl Default for ComicSettings {
    fn default() -> Self {
        Self {
            view_mode: ComicViewMode::Single,
            direction: ComicDirection::Rtl,
            page_gap: ComicPageGap::Twelve,
            preload_pages: ComicPreloadPages::Three,
        }
    }
}

/// 下载并发档位。下载 Worker 只接受闭合集合，避免通过设置写入任意资源占用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadConcurrency {
    One,
    Two,
    Three,
    Five,
}

impl DownloadConcurrency {
    pub const fn as_usize(self) -> usize {
        match self {
            Self::One => 1,
            Self::Two => 2,
            Self::Three => 3,
            Self::Five => 5,
        }
    }
}

/// 下载限速档位。值只用于本地 Worker，Wire 不传递任意数值或路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadSpeedLimit {
    Unlimited,
    Kbps512,
    Mbps2,
    Mbps5,
    Mbps10,
}

impl DownloadSpeedLimit {
    pub const fn as_bytes_per_second(self) -> Option<u64> {
        match self {
            Self::Unlimited => None,
            Self::Kbps512 => Some(512 * 1024),
            Self::Mbps2 => Some(2 * 1024 * 1024),
            Self::Mbps5 => Some(5 * 1024 * 1024),
            Self::Mbps10 => Some(10 * 1024 * 1024),
        }
    }
}

/// downloads 分区设置。
///
/// 这些字段由本地 Download Worker/DownloadService 真实消费；计费网络、通知和视频质量
/// 仍不进入设置事实源，待对应 Foundation 建立后再扩展。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DownloadSettings {
    pub concurrent_tasks: DownloadConcurrency,
    pub speed_limit: DownloadSpeedLimit,
    /// 应用重启时是否自动恢复被中断的任务。队列中的新任务仍由
    /// DownloadService 按用户动作和并发策略启动。
    #[serde(default = "default_auto_continue")]
    pub auto_continue: bool,
}

impl Default for DownloadSettings {
    fn default() -> Self {
        Self {
            concurrent_tasks: DownloadConcurrency::Three,
            speed_limit: DownloadSpeedLimit::Unlimited,
            auto_continue: true,
        }
    }
}

fn default_auto_continue() -> bool {
    true
}

/// privacy 分区设置。当前只承载已经有真实消费者的本地历史开关；
/// 网络诊断、代理和跟踪限制仍由各自 Foundation 接入后再扩展。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PrivacySettings {
    pub search_history: bool,
    /// 是否在打开媒体 Session 时记录播放/阅读历史。旧设置行没有该字段时
    /// 默认开启，保持升级前的历史记录行为。
    #[serde(default = "default_playback_history")]
    pub playback_history: bool,
}

impl Default for PrivacySettings {
    fn default() -> Self {
        Self {
            search_history: true,
            playback_history: true,
        }
    }
}

fn default_playback_history() -> bool {
    true
}

/// 分区设置值（闭合联合；JSON 形状 `{"section":"general", ...}`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "section", rename_all = "snake_case")]
pub enum SettingsValue {
    General(GeneralSettings),
    Appearance(AppearanceSettings),
    Playback(PlaybackSettings),
    Reading(ReadingSettings),
    Comic(ComicSettings),
    Downloads(DownloadSettings),
    Privacy(PrivacySettings),
}

impl SettingsValue {
    pub fn section(&self) -> SettingsSection {
        match self {
            Self::General(_) => SettingsSection::General,
            Self::Appearance(_) => SettingsSection::Appearance,
            Self::Playback(_) => SettingsSection::Playback,
            Self::Reading(_) => SettingsSection::Reading,
            Self::Comic(_) => SettingsSection::Comic,
            Self::Downloads(_) => SettingsSection::Downloads,
            Self::Privacy(_) => SettingsSection::Privacy,
        }
    }

    pub fn default_for(section: SettingsSection) -> Self {
        match section {
            SettingsSection::General => Self::General(GeneralSettings::default()),
            SettingsSection::Appearance => Self::Appearance(AppearanceSettings::default()),
            SettingsSection::Playback => Self::Playback(PlaybackSettings::default()),
            SettingsSection::Reading => Self::Reading(ReadingSettings::default()),
            SettingsSection::Comic => Self::Comic(ComicSettings::default()),
            SettingsSection::Downloads => Self::Downloads(DownloadSettings::default()),
            SettingsSection::Privacy => Self::Privacy(PrivacySettings::default()),
        }
    }
}

/// general 分区部分更新（字段全 Option；未知字段在反序列化边界拒绝）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GeneralPatch {
    pub launch_page: Option<LaunchPage>,
    pub restore_session: Option<bool>,
    pub language: Option<Language>,
    pub notifications: Option<bool>,
}

/// appearance 分区部分更新。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AppearancePatch {
    pub theme: Option<Theme>,
    pub density: Option<Density>,
    pub sidebar: Option<SidebarPreference>,
    pub reduce_motion: Option<bool>,
    pub interface_font_mode: Option<InterfaceFontMode>,
    pub interface_font_family: Option<String>,
    pub interface_font_asset_id: Option<String>,
    /// 缺失 = 不改；显式 null = 清除自定义主题。
    #[serde(
        default,
        deserialize_with = "deserialize_nullable_patch_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub custom_theme: Option<Option<Box<AppTheme>>>,
    /// 缺失 = 不改；显式 null = 清除壁纸（等价于 `WallpaperSelection::None`）。
    ///
    /// 与 `custom_theme` / `custom_font_asset_id` 同形是刻意的：三个字段都是
    /// 「可以真的被清空」的外观字段，如果壁纸单独用 `Option<WallpaperSelection>`，
    /// 那么 `{"wallpaper": null}` 会在这一项上静默变成「不改」，而在另外两项上变成
    /// 「清除」——同一个 JSON 形状在两个相邻字段上表达两种不同的意思。
    /// （清空壁纸也可以显式写 `{"kind":"none"}`，两种写法结果一致。）
    #[serde(
        default,
        deserialize_with = "deserialize_nullable_patch_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub wallpaper: Option<Option<WallpaperSelection>>,
    /// 缺失 = 不改；显式 null = 回到预设字体。
    #[serde(
        default,
        deserialize_with = "deserialize_nullable_patch_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub custom_font_asset_id: Option<Option<AppearanceAssetId>>,
    /// 缺失 = 不改；字体方案是闭合枚举。
    pub ui_font_preset: Option<UiFontPreset>,
    /// 缺失 = 不改；显式 null = 清除自定义系统字体名。
    #[serde(
        default,
        deserialize_with = "deserialize_nullable_ui_font_family_patch",
        skip_serializing_if = "Option::is_none"
    )]
    pub ui_font_family: Option<Option<String>>,
}

const MAX_UI_FONT_FAMILY_CHARS: usize = 64;

fn normalize_ui_font_family(value: String) -> Result<String, &'static str> {
    let normalized = value.trim();
    if normalized.is_empty()
        || normalized.chars().count() > MAX_UI_FONT_FAMILY_CHARS
        || !normalized.chars().all(|character| {
            character.is_alphanumeric()
                || character.is_whitespace()
                || " ._-()+&'".contains(character)
        })
    {
        return Err("invalid UI font family name");
    }
    Ok(normalized.to_owned())
}

fn deserialize_ui_font_family<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?
        .map(normalize_ui_font_family)
        .transpose()
        .map_err(serde::de::Error::custom)
}

fn deserialize_nullable_ui_font_family_patch<'de, D>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<String>::deserialize(deserializer)? {
        Some(value) => normalize_ui_font_family(value)
            .map(|value| Some(Some(value)))
            .map_err(serde::de::Error::custom),
        None => Ok(Some(None)),
    }
}

/// 保留 patch 字段的三态：缺失 = 不修改，JSON null = 清除，有值 = 替换。
///
/// serde 对普通 `Option<Option<T>>` 的派生反序列化会把缺失与显式 null 都解成外层
/// `None`；自定义反序列化器只在字段出现时调用，因此在这里把字段存在性包成外层 `Some`。
fn deserialize_nullable_patch_field<'de, T, D>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// playback 分区部分更新。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PlaybackPatch {
    pub default_playback_rate: Option<PlaybackRate>,
    pub auto_resume: Option<bool>,
    pub auto_next: Option<bool>,
}

/// reading 分区部分更新。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReadingPatch {
    pub font_family: Option<ReadingFontFamily>,
    pub custom_font_family: Option<String>,
    pub font_size: Option<ReadingFontSize>,
    pub line_height: Option<ReadingLineHeight>,
    pub content_width: Option<ReadingContentWidth>,
    pub theme: Option<ReadingTheme>,
    pub custom_background: Option<String>,
    pub custom_text: Option<String>,
    pub font_weight: Option<ReadingFontWeight>,
    pub letter_spacing: Option<ReadingLetterSpacing>,
    pub system_auto: Option<bool>,
    pub pagination: Option<ReadingPagination>,
}

/// comic 分区部分更新。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ComicPatch {
    pub view_mode: Option<ComicViewMode>,
    pub direction: Option<ComicDirection>,
    pub page_gap: Option<ComicPageGap>,
    pub preload_pages: Option<ComicPreloadPages>,
}

/// downloads 分区部分更新。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DownloadPatch {
    pub concurrent_tasks: Option<DownloadConcurrency>,
    pub speed_limit: Option<DownloadSpeedLimit>,
    pub auto_continue: Option<bool>,
}

/// privacy 分区部分更新。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PrivacyPatch {
    pub search_history: Option<bool>,
    pub playback_history: Option<bool>,
}

/// 资源级偏好数据。只保存全局设置的窄范围 Patch；`None` 表示该作用域没有覆盖，
/// 空 Patch 仍是合法且可幂等存储的显式覆盖。该结构禁止承载 Secret、路径或正文。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PreferenceData {
    pub reading: Option<ReadingPatch>,
    pub comic: Option<ComicPatch>,
}

impl PreferenceData {
    /// 将一个资源级部分 patch 合并到 authoritative 覆盖数据。
    ///
    /// 这个方法专门服务于 Agent 提案的“只保存 patch、应用时重读并合并”语义。
    /// 现有 `ResourcePreference` 仍表示完整覆盖数据，不能拿它替代本方法：
    /// `None` 在完整覆盖里表示清除该分区，而在 patch 里表示“不触碰该分区”。
    pub fn apply_patch(&self, patch: &Self) -> Self {
        let mut next = self.clone();
        if let Some(reading) = &patch.reading {
            let mut merged = next.reading.clone().unwrap_or_default();
            merge_reading_patch(&mut merged, reading);
            next.reading = Some(merged);
        }
        if let Some(comic) = &patch.comic {
            let mut merged = next.comic.clone().unwrap_or_default();
            merge_comic_patch(&mut merged, comic);
            next.comic = Some(merged);
        }
        next
    }
}

fn merge_reading_patch(current: &mut ReadingPatch, patch: &ReadingPatch) {
    if let Some(value) = patch.font_family {
        current.font_family = Some(value);
    }
    merge_resource_text(&mut current.custom_font_family, &patch.custom_font_family);
    if let Some(value) = patch.font_size {
        current.font_size = Some(value);
    }
    if let Some(value) = patch.line_height {
        current.line_height = Some(value);
    }
    if let Some(value) = patch.content_width {
        current.content_width = Some(value);
    }
    if let Some(value) = patch.theme {
        current.theme = Some(value);
    }
    merge_resource_text(&mut current.custom_background, &patch.custom_background);
    merge_resource_text(&mut current.custom_text, &patch.custom_text);
    if let Some(value) = patch.font_weight {
        current.font_weight = Some(value);
    }
    if let Some(value) = patch.letter_spacing {
        current.letter_spacing = Some(value);
    }
    if let Some(value) = patch.system_auto {
        current.system_auto = Some(value);
    }
    if let Some(value) = patch.pagination {
        current.pagination = Some(value);
    }
}

/// 资源级 Agent patch 的自由文本规范化。
///
/// 全局设置的 patch 用空字符串表示清除；资源级 Agent patch 的 `None` 已经被
/// 固定为“不触碰”，因此没有第二个“清除”状态可用。这里沿用全局的 trim 规则，
/// 但把空白值视为“不触碰”，从而避免产生其它写入路径不会产生的 `Some("")`。
fn merge_resource_text(current: &mut Option<String>, patch: &Option<String>) {
    let Some(value) = patch else { return };
    let value = value.trim();
    if !value.is_empty() {
        *current = Some(value.to_owned());
    }
}

fn merge_comic_patch(current: &mut ComicPatch, patch: &ComicPatch) {
    if let Some(value) = patch.view_mode {
        current.view_mode = Some(value);
    }
    if let Some(value) = patch.direction {
        current.direction = Some(value);
    }
    if let Some(value) = patch.page_gap {
        current.page_gap = Some(value);
    }
    if let Some(value) = patch.preload_pages {
        current.preload_pages = Some(value);
    }
}

/// 分区部分更新（闭合联合；JSON 形状 `{"section":"general","launchPage":"library"}`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "section", rename_all = "snake_case")]
pub enum SettingsPatch {
    General(GeneralPatch),
    Appearance(AppearancePatch),
    Playback(PlaybackPatch),
    Reading(ReadingPatch),
    Comic(ComicPatch),
    Downloads(DownloadPatch),
    Privacy(PrivacyPatch),
}

impl SettingsPatch {
    pub fn section(&self) -> SettingsSection {
        match self {
            Self::General(_) => SettingsSection::General,
            Self::Appearance(_) => SettingsSection::Appearance,
            Self::Playback(_) => SettingsSection::Playback,
            Self::Reading(_) => SettingsSection::Reading,
            Self::Comic(_) => SettingsSection::Comic,
            Self::Downloads(_) => SettingsSection::Downloads,
            Self::Privacy(_) => SettingsSection::Privacy,
        }
    }

    /// 把 patch 应用到当前值（部分更新；空 patch 视为幂等）。
    pub fn apply_to(&self, current: &SettingsValue) -> SettingsValue {
        match (self, current) {
            (Self::General(patch), SettingsValue::General(current)) => {
                SettingsValue::General(GeneralSettings {
                    launch_page: patch.launch_page.unwrap_or(current.launch_page),
                    restore_session: patch.restore_session.unwrap_or(current.restore_session),
                    language: patch.language.unwrap_or(current.language),
                    notifications: patch.notifications.unwrap_or(current.notifications),
                })
            }
            (Self::Appearance(patch), SettingsValue::Appearance(current)) => {
                SettingsValue::Appearance(AppearanceSettings {
                    theme: patch.theme.unwrap_or(current.theme),
                    density: patch.density.unwrap_or(current.density),
                    sidebar: patch.sidebar.unwrap_or(current.sidebar),
                    reduce_motion: patch.reduce_motion.unwrap_or(current.reduce_motion),
                    interface_font_mode: patch
                        .interface_font_mode
                        .unwrap_or(current.interface_font_mode),
                    // 空字符串表示“清除该项选择”，与 reading 的 custom 字段同一语义。
                    interface_font_family: match &patch.interface_font_family {
                        Some(s) if s.trim().is_empty() => None,
                        Some(s) => Some(s.trim().to_owned()),
                        None => current.interface_font_family.clone(),
                    },
                    interface_font_asset_id: match &patch.interface_font_asset_id {
                        Some(s) if s.trim().is_empty() => None,
                        Some(s) => Some(s.trim().to_owned()),
                        None => current.interface_font_asset_id.clone(),
                    },
                    custom_theme: match &patch.custom_theme {
                        Some(value) => value.clone(),
                        None => current.custom_theme.clone(),
                    },
                    wallpaper: match &patch.wallpaper {
                        // 显式 null 与 `{"kind":"none"}` 是同一种「清除」。
                        Some(value) => (*value).unwrap_or(WallpaperSelection::None),
                        None => current.wallpaper,
                    },
                    custom_font_asset_id: match &patch.custom_font_asset_id {
                        Some(value) => *value,
                        None => current.custom_font_asset_id,
                    },
                    ui_font_preset: patch.ui_font_preset.unwrap_or(current.ui_font_preset),
                    ui_font_family: match &patch.ui_font_family {
                        Some(value) => value.clone(),
                        None => current.ui_font_family.clone(),
                    },
                })
            }
            (Self::Playback(patch), SettingsValue::Playback(current)) => {
                SettingsValue::Playback(PlaybackSettings {
                    default_playback_rate: patch
                        .default_playback_rate
                        .unwrap_or(current.default_playback_rate),
                    auto_resume: patch.auto_resume.unwrap_or(current.auto_resume),
                    auto_next: patch.auto_next.unwrap_or(current.auto_next),
                })
            }
            (Self::Reading(patch), SettingsValue::Reading(current)) => {
                SettingsValue::Reading(ReadingSettings {
                    font_family: patch.font_family.unwrap_or(current.font_family),
                    custom_font_family: match &patch.custom_font_family {
                        Some(s) if s.trim().is_empty() => None,
                        Some(s) => Some(s.trim().to_owned()),
                        None => current.custom_font_family.clone(),
                    },
                    font_size: patch.font_size.unwrap_or(current.font_size),
                    line_height: patch.line_height.unwrap_or(current.line_height),
                    content_width: patch.content_width.unwrap_or(current.content_width),
                    theme: patch.theme.unwrap_or(current.theme),
                    custom_background: match &patch.custom_background {
                        Some(s) if s.trim().is_empty() => None,
                        Some(s) => Some(s.trim().to_owned()),
                        None => current.custom_background.clone(),
                    },
                    custom_text: match &patch.custom_text {
                        Some(s) if s.trim().is_empty() => None,
                        Some(s) => Some(s.trim().to_owned()),
                        None => current.custom_text.clone(),
                    },
                    font_weight: patch.font_weight.unwrap_or(current.font_weight),
                    letter_spacing: patch.letter_spacing.unwrap_or(current.letter_spacing),
                    system_auto: patch.system_auto.unwrap_or(current.system_auto),
                    pagination: patch.pagination.unwrap_or(current.pagination),
                })
            }
            (Self::Comic(patch), SettingsValue::Comic(current)) => {
                SettingsValue::Comic(ComicSettings {
                    view_mode: patch.view_mode.unwrap_or(current.view_mode),
                    direction: patch.direction.unwrap_or(current.direction),
                    page_gap: patch.page_gap.unwrap_or(current.page_gap),
                    preload_pages: patch.preload_pages.unwrap_or(current.preload_pages),
                })
            }
            (Self::Downloads(patch), SettingsValue::Downloads(current)) => {
                SettingsValue::Downloads(DownloadSettings {
                    concurrent_tasks: patch.concurrent_tasks.unwrap_or(current.concurrent_tasks),
                    speed_limit: patch.speed_limit.unwrap_or(current.speed_limit),
                    auto_continue: patch.auto_continue.unwrap_or(current.auto_continue),
                })
            }
            (Self::Privacy(patch), SettingsValue::Privacy(current)) => {
                SettingsValue::Privacy(PrivacySettings {
                    search_history: patch.search_history.unwrap_or(current.search_history),
                    playback_history: patch.playback_history.unwrap_or(current.playback_history),
                })
            }
            // section 不匹配不可能发生（patch 与 current 由调用方按同一 section 构造）；
            // 防御性返回当前值。
            _ => current.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app_theme() -> AppTheme {
        let palette = serde_json::json!({
            "background": "#111111",
            "foreground": "#222222",
            "card": "#333333",
            "cardForeground": "#444444",
            "popover": "#555555",
            "popoverForeground": "#666666",
            "primary": "#777777",
            "primaryForeground": "#888888",
            "secondary": "#999999",
            "secondaryForeground": "#aaaaaa",
            "muted": "#bbbbbb",
            "mutedForeground": "#cccccc",
            "accent": "#dddddd",
            "accentForeground": "#eeeeee",
            "destructive": "#123123",
            "destructiveForeground": "#234234",
            "border": "#3c3c4329",
            "input": "#3c3c4329",
            "ring": "#007aff",
        });
        serde_json::from_value(serde_json::json!({
            "light": palette.clone(),
            "dark": palette,
            "accentColor": "#007aff",
        }))
        .unwrap()
    }

    #[test]
    fn preference_data_patch_merges_only_present_fields() {
        let authoritative = PreferenceData {
            reading: Some(ReadingPatch {
                font_size: Some(ReadingFontSize::Large),
                custom_font_family: Some("D:/private/font.ttf".to_owned()),
                custom_background: Some("#101418".to_owned()),
                ..ReadingPatch::default()
            }),
            comic: Some(ComicPatch {
                direction: Some(ComicDirection::Rtl),
                page_gap: Some(ComicPageGap::TwentyFour),
                ..ComicPatch::default()
            }),
        };
        let patch = PreferenceData {
            reading: Some(ReadingPatch {
                font_size: Some(ReadingFontSize::Small),
                ..ReadingPatch::default()
            }),
            comic: None,
        };

        let merged = authoritative.apply_patch(&patch);
        let reading = merged.reading.expect("reading patch keeps the section");
        assert_eq!(reading.font_size, Some(ReadingFontSize::Small));
        assert_eq!(
            reading.custom_font_family.as_deref(),
            Some("D:/private/font.ttf")
        );
        assert_eq!(reading.custom_background.as_deref(), Some("#101418"));
        assert_eq!(merged.comic, authoritative.comic);

        // 空 patch 不会把已有 section 清掉；`None` 表示“不触碰”而不是“清除”。
        assert_eq!(
            authoritative.apply_patch(&PreferenceData::default()),
            authoritative
        );
    }

    #[test]
    fn preference_data_patch_can_create_missing_section_and_preserve_other_section() {
        let authoritative = PreferenceData {
            reading: None,
            comic: Some(ComicPatch {
                view_mode: Some(ComicViewMode::Double),
                ..ComicPatch::default()
            }),
        };
        let patch = PreferenceData {
            reading: Some(ReadingPatch {
                font_size: Some(ReadingFontSize::Medium),
                ..ReadingPatch::default()
            }),
            comic: Some(ComicPatch {
                direction: Some(ComicDirection::Ltr),
                ..ComicPatch::default()
            }),
        };

        let merged = authoritative.apply_patch(&patch);
        assert_eq!(
            merged.reading.and_then(|reading| reading.font_size),
            Some(ReadingFontSize::Medium)
        );
        let comic = merged.comic.expect("comic patch keeps the section");
        assert_eq!(comic.view_mode, Some(ComicViewMode::Double));
        assert_eq!(comic.direction, Some(ComicDirection::Ltr));
    }

    #[test]
    fn preference_data_patch_normalizes_resource_text_and_never_writes_empty_strings() {
        let authoritative = PreferenceData {
            reading: Some(ReadingPatch {
                custom_font_family: Some("Source Han Serif".to_owned()),
                custom_background: Some("#101418".to_owned()),
                ..ReadingPatch::default()
            }),
            comic: None,
        };
        let patch = PreferenceData {
            reading: Some(ReadingPatch {
                custom_font_family: Some("   ".to_owned()),
                custom_background: Some("  #f7f1e3  ".to_owned()),
                custom_text: Some(String::new()),
                ..ReadingPatch::default()
            }),
            comic: None,
        };

        let merged = authoritative.apply_patch(&patch);
        let reading = merged.reading.expect("reading section remains present");
        assert_eq!(
            reading.custom_font_family.as_deref(),
            Some("Source Han Serif")
        );
        assert_eq!(reading.custom_background.as_deref(), Some("#f7f1e3"));
        assert_eq!(reading.custom_text, None);
    }

    #[test]
    fn defaults_are_stable() {
        assert_eq!(
            SettingsValue::default_for(SettingsSection::General),
            SettingsValue::General(GeneralSettings {
                launch_page: LaunchPage::Home,
                restore_session: false,
                language: Language::ZhCn,
                notifications: true,
            })
        );
        assert_eq!(
            SettingsValue::default_for(SettingsSection::Appearance),
            SettingsValue::Appearance(AppearanceSettings {
                theme: Theme::System,
                density: Density::Comfortable,
                sidebar: SidebarPreference::Auto,
                reduce_motion: false,
                interface_font_mode: InterfaceFontMode::System,
                interface_font_family: None,
                interface_font_asset_id: None,
                custom_theme: None,
                wallpaper: WallpaperSelection::None,
                custom_font_asset_id: None,
                ui_font_preset: UiFontPreset::System,
                ui_font_family: None,
            })
        );
        assert_eq!(
            SettingsValue::default_for(SettingsSection::Playback),
            SettingsValue::Playback(PlaybackSettings {
                default_playback_rate: PlaybackRate::One,
                auto_resume: true,
                auto_next: true,
            })
        );
        assert_eq!(
            SettingsValue::default_for(SettingsSection::Reading),
            SettingsValue::Reading(ReadingSettings {
                font_family: ReadingFontFamily::Serif,
                custom_font_family: None,
                font_size: ReadingFontSize::Medium,
                line_height: ReadingLineHeight::Comfortable,
                content_width: ReadingContentWidth::Medium,
                theme: ReadingTheme::Warm,
                custom_background: None,
                custom_text: None,
                font_weight: ReadingFontWeight::Regular,
                letter_spacing: ReadingLetterSpacing::Normal,
                system_auto: true,
                pagination: ReadingPagination::Scroll,
            })
        );
        assert_eq!(
            SettingsValue::default_for(SettingsSection::Comic),
            SettingsValue::Comic(ComicSettings {
                view_mode: ComicViewMode::Single,
                direction: ComicDirection::Rtl,
                page_gap: ComicPageGap::Twelve,
                preload_pages: ComicPreloadPages::Three,
            })
        );
        assert_eq!(
            SettingsValue::default_for(SettingsSection::Downloads),
            SettingsValue::Downloads(DownloadSettings {
                concurrent_tasks: DownloadConcurrency::Three,
                speed_limit: DownloadSpeedLimit::Unlimited,
                auto_continue: true,
            })
        );
        assert_eq!(
            SettingsValue::default_for(SettingsSection::Privacy),
            SettingsValue::Privacy(PrivacySettings {
                search_history: true,
                playback_history: true,
            })
        );
    }

    #[test]
    fn unknown_section_is_rejected() {
        assert_eq!(
            SettingsSection::parse("general"),
            Some(SettingsSection::General)
        );
        assert_eq!(
            SettingsSection::parse("appearance"),
            Some(SettingsSection::Appearance)
        );
        assert_eq!(
            SettingsSection::parse("playback"),
            Some(SettingsSection::Playback)
        );
        assert_eq!(
            SettingsSection::parse("reading"),
            Some(SettingsSection::Reading)
        );
        assert_eq!(
            SettingsSection::parse("comic"),
            Some(SettingsSection::Comic)
        );
        assert_eq!(
            SettingsSection::parse("downloads"),
            Some(SettingsSection::Downloads)
        );
        assert_eq!(
            SettingsSection::parse("privacy"),
            Some(SettingsSection::Privacy)
        );
        assert_eq!(SettingsSection::parse("bogus"), None);
        assert_eq!(SettingsSection::parse(""), None);
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let err = serde_json::from_str::<GeneralPatch>(r#"{"launchPage":"home","bogus":1}"#);
        assert!(err.is_err(), "未知字段必须拒绝（deny_unknown_fields）");
        let err = serde_json::from_str::<AppearancePatch>(r#"{"theme":"dark","extra":true}"#);
        assert!(err.is_err());
        let err = serde_json::from_str::<ComicPatch>(r#"{"viewMode":"paged"}"#);
        assert!(err.is_err());
        // 未知 section tag 拒绝
        let err =
            serde_json::from_str::<SettingsPatch>(r#"{"section":"bogus","launchPage":"home"}"#);
        assert!(err.is_err(), "未知 section 必须拒绝");
    }

    #[test]
    fn invalid_enum_values_are_rejected() {
        let err =
            serde_json::from_str::<SettingsPatch>(r#"{"section":"general","language":"klingon"}"#);
        assert!(err.is_err(), "非法枚举必须拒绝");
        let err =
            serde_json::from_str::<SettingsPatch>(r#"{"section":"appearance","theme":"neon"}"#);
        assert!(err.is_err());
        let err = serde_json::from_str::<SettingsPatch>(r#"{"section":"reading","theme":"neon"}"#);
        assert!(err.is_err());
        let err = serde_json::from_str::<SettingsPatch>(r#"{"section":"general","launchPage":42}"#);
        assert!(err.is_err(), "错误类型必须拒绝");
    }

    #[test]
    fn patch_applies_partial_update() {
        let current = SettingsValue::default_for(SettingsSection::General);
        let patch: SettingsPatch =
            serde_json::from_str(r#"{"section":"general","launchPage":"library"}"#).unwrap();
        let next = patch.apply_to(&current);
        match next {
            SettingsValue::General(g) => {
                assert_eq!(g.launch_page, LaunchPage::Library, "只更新提供的字段");
                assert_eq!(g.language, Language::ZhCn, "未提供的字段保持原值");
            }
            _ => panic!("section 必须一致"),
        }
    }

    #[test]
    fn empty_patch_is_idempotent() {
        let current = SettingsValue::default_for(SettingsSection::Appearance);
        let patch: SettingsPatch = serde_json::from_str(r#"{"section":"appearance"}"#).unwrap();
        assert_eq!(patch.apply_to(&current), current);
    }

    #[test]
    fn appearance_theme_roundtrips_as_an_object_without_changing_the_wire_shape() {
        let theme = test_app_theme();
        let expected_theme = serde_json::to_value(&theme).unwrap();
        let patch_json = serde_json::json!({
            "section": "appearance",
            "customTheme": expected_theme,
        });
        let patch: SettingsPatch = serde_json::from_value(patch_json).unwrap();
        assert_eq!(
            serde_json::to_value(&patch).unwrap()["customTheme"],
            expected_theme
        );

        let current = SettingsValue::default_for(SettingsSection::Appearance);
        let next = patch.apply_to(&current);
        let serialized = serde_json::to_value(&next).unwrap();
        assert_eq!(serialized["section"], "appearance");
        assert_eq!(serialized["customTheme"], expected_theme);
        assert_eq!(
            serde_json::from_value::<SettingsValue>(serialized).unwrap(),
            next
        );

        let mut invalid_theme = expected_theme;
        invalid_theme["unexpected"] = serde_json::json!(true);
        assert!(
            serde_json::from_value::<SettingsPatch>(serde_json::json!({
                "section": "appearance",
                "customTheme": invalid_theme,
            }))
            .is_err(),
            "主题载荷仍须拒绝未知字段"
        );
    }

    #[test]
    fn appearance_patch_distinguishes_missing_nullable_fields_from_explicit_null() {
        let missing: AppearancePatch = serde_json::from_str("{}").unwrap();
        assert_eq!(missing.custom_theme, None);
        assert_eq!(missing.custom_font_asset_id, None);
        assert_eq!(missing.ui_font_preset, None);
        assert_eq!(missing.ui_font_family, None);
        let serialized_missing = serde_json::to_value(&missing).unwrap();
        assert!(serialized_missing.get("customTheme").is_none());
        assert!(serialized_missing.get("customFontAssetId").is_none());
        assert!(serialized_missing.get("uiFontFamily").is_none());

        let clear: SettingsPatch = serde_json::from_str(
            r#"{"section":"appearance","customTheme":null,"customFontAssetId":null,"uiFontFamily":null,"uiFontPreset":"modern_sans"}"#,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&clear).unwrap()["customTheme"],
            serde_json::Value::Null
        );
        let current = SettingsValue::Appearance(AppearanceSettings {
            theme: Theme::Custom,
            custom_theme: Some(Box::new(test_app_theme())),
            custom_font_asset_id: Some(AppearanceAssetId::new()),
            ui_font_preset: UiFontPreset::CustomSystem,
            ui_font_family: Some("Microsoft YaHei UI".to_owned()),
            ..AppearanceSettings::default()
        });

        let unchanged = SettingsPatch::Appearance(missing).apply_to(&current);
        assert_eq!(unchanged, current, "缺失字段必须保留当前自定义值");

        let cleared = clear.apply_to(&current);
        let SettingsValue::Appearance(cleared) = cleared else {
            panic!("appearance patch 必须返回 appearance 设置");
        };
        assert_eq!(cleared.custom_theme, None, "显式 null 必须清除自定义主题");
        assert_eq!(
            cleared.custom_font_asset_id, None,
            "显式 null 必须恢复预设字体"
        );
        assert_eq!(cleared.ui_font_preset, UiFontPreset::ModernSans);
        assert_eq!(cleared.ui_font_family, None, "显式 null 必须清除系统字体名");
    }

    #[test]
    fn ui_font_family_rejects_css_syntax_and_normalizes_a_valid_family_name() {
        let valid: AppearanceSettings = serde_json::from_str(
            r#"{"theme":"system","density":"comfortable","sidebar":"auto","reduceMotion":false,"uiFontPreset":"custom_system","uiFontFamily":"  思源宋体  "}"#,
        )
        .unwrap();
        assert_eq!(valid.ui_font_family.as_deref(), Some("思源宋体"));

        let invalid: Result<AppearanceSettings, _> = serde_json::from_str(
            r#"{"theme":"system","density":"comfortable","sidebar":"auto","reduceMotion":false,"uiFontFamily":"Test; color:red"}"#,
        );
        assert!(invalid.is_err(), "字体名不能带 CSS 声明分隔符");
    }

    /// 壁纸与另外两个可空字段**同形**：缺失 = 不改，显式 null = 清除，有值 = 替换。
    ///
    /// 三者都是「可以真的被清空」的外观字段。壁纸曾经是单层 `Option`，于是
    /// `{"wallpaper": null}` 在这一项上静默变成「不改」，而在相邻两项上变成「清除」——
    /// 同一个 JSON 形状在两个紧挨着的字段上表达两种不同的意思，调用方无从预判。
    #[test]
    fn appearance_patch_treats_wallpaper_null_like_its_siblings() {
        let asset_id = AppearanceAssetId::new();
        let current = SettingsValue::Appearance(AppearanceSettings {
            wallpaper: WallpaperSelection::Static(asset_id),
            ..AppearanceSettings::default()
        });

        let missing: SettingsPatch = serde_json::from_str(r#"{"section":"appearance"}"#).unwrap();
        assert_eq!(missing.apply_to(&current), current, "缺失必须保留当前壁纸");

        let cleared: SettingsPatch =
            serde_json::from_str(r#"{"section":"appearance","wallpaper":null}"#).unwrap();
        let SettingsValue::Appearance(cleared) = cleared.apply_to(&current) else {
            panic!("appearance patch 必须返回 appearance 设置");
        };
        assert_eq!(
            cleared.wallpaper,
            WallpaperSelection::None,
            "显式 null 必须清除壁纸"
        );

        // 显式 `{"kind":"none"}` 是同一件事的另一种写法：两条路都通向「无壁纸」。
        let explicit: SettingsPatch =
            serde_json::from_str(r#"{"section":"appearance","wallpaper":{"kind":"none"}}"#)
                .unwrap();
        let SettingsValue::Appearance(explicit) = explicit.apply_to(&current) else {
            panic!("appearance patch 必须返回 appearance 设置");
        };
        assert_eq!(explicit.wallpaper, WallpaperSelection::None);

        // 有值 = 替换。
        let other = AppearanceAssetId::new();
        let replaced: SettingsPatch = serde_json::from_str(&format!(
            r#"{{"section":"appearance","wallpaper":{{"kind":"dynamic","assetId":"{other}"}}}}"#
        ))
        .unwrap();
        let SettingsValue::Appearance(replaced) = replaced.apply_to(&current) else {
            panic!("appearance patch 必须返回 appearance 设置");
        };
        assert_eq!(replaced.wallpaper, WallpaperSelection::Dynamic(other));
    }

    #[test]
    fn roundtrip_through_json() {
        let value = SettingsValue::General(GeneralSettings {
            launch_page: LaunchPage::LastSession,
            restore_session: true,
            language: Language::EnUs,
            notifications: false,
        });
        let json = serde_json::to_string(&value).unwrap();
        let back: SettingsValue = serde_json::from_str(&json).unwrap();
        assert_eq!(value, back);
        assert!(json.contains("\"section\":\"general\""), "{json}");
        assert!(json.contains("\"launchPage\":\"last_session\""), "{json}");
    }

    #[test]
    fn reading_roundtrip_and_partial_patch() {
        let current = SettingsValue::default_for(SettingsSection::Reading);
        let patch: SettingsPatch = serde_json::from_str(
            r#"{"section":"reading","fontFamily":"kai","fontSize":"large","theme":"dark"}"#,
        )
        .unwrap();
        let next = patch.apply_to(&current);
        assert_eq!(
            next,
            SettingsValue::Reading(ReadingSettings {
                font_family: ReadingFontFamily::Kai,
                custom_font_family: None,
                font_size: ReadingFontSize::Large,
                line_height: ReadingLineHeight::Comfortable,
                content_width: ReadingContentWidth::Medium,
                theme: ReadingTheme::Dark,
                custom_background: None,
                custom_text: None,
                font_weight: ReadingFontWeight::Regular,
                letter_spacing: ReadingLetterSpacing::Normal,
                system_auto: true,
                pagination: ReadingPagination::Scroll,
            })
        );
        let json = serde_json::to_string(&next).unwrap();
        assert!(json.contains("\"section\":\"reading\""));
        let back: SettingsValue = serde_json::from_str(&json).unwrap();
        assert_eq!(back, next);
    }

    #[test]
    fn reading_legacy_rows_fill_new_optional_fields() {
        // b60661e 之前的真实设置行只包含五个基础排版字段；读取旧库时
        // 不应把整行判定为损坏，也不应覆盖用户原有的字体/主题选择。
        let legacy: SettingsValue = serde_json::from_str(
            r#"{"section":"reading","fontFamily":"kai","fontSize":"large","lineHeight":"airy","contentWidth":"wide","theme":"dark"}"#,
        )
        .unwrap();
        assert_eq!(
            legacy,
            SettingsValue::Reading(ReadingSettings {
                font_family: ReadingFontFamily::Kai,
                custom_font_family: None,
                font_size: ReadingFontSize::Large,
                line_height: ReadingLineHeight::Airy,
                content_width: ReadingContentWidth::Wide,
                theme: ReadingTheme::Dark,
                custom_background: None,
                custom_text: None,
                font_weight: ReadingFontWeight::Regular,
                letter_spacing: ReadingLetterSpacing::Normal,
                system_auto: true,
                pagination: ReadingPagination::Scroll,
            })
        );
    }

    #[test]
    fn reading_custom_fields_roundtrip() {
        let current = SettingsValue::default_for(SettingsSection::Reading);
        let patch: SettingsPatch = serde_json::from_str(
            r##"{"section":"reading","fontFamily":"custom","customFontFamily":"MyFont","fontWeight":"bold","letterSpacing":"loose","theme":"custom","customBackground":"#123456","customText":"#abcdef","systemAuto":false}"##,
        )
        .unwrap();
        let next = patch.apply_to(&current);
        match &next {
            SettingsValue::Reading(r) => {
                assert_eq!(r.font_family, ReadingFontFamily::Custom);
                assert_eq!(r.custom_font_family, Some("MyFont".to_owned()));
                assert_eq!(r.font_weight, ReadingFontWeight::Bold);
                assert_eq!(r.letter_spacing, ReadingLetterSpacing::Loose);
                assert_eq!(r.theme, ReadingTheme::Custom);
                assert_eq!(r.custom_background, Some("#123456".to_owned()));
                assert_eq!(r.custom_text, Some("#abcdef".to_owned()));
                assert!(!r.system_auto);
            }
            _ => panic!("wrong section"),
        }
        // empty custom fields should clear
        let clear: SettingsPatch = serde_json::from_str(
            r#"{"section":"reading","customFontFamily":"","customBackground":""}"#,
        )
        .unwrap();
        let cleared = clear.apply_to(&next);
        match cleared {
            SettingsValue::Reading(r) => {
                assert_eq!(r.custom_font_family, None);
                assert_eq!(r.custom_background, None);
            }
            _ => panic!("wrong section"),
        }
    }

    #[test]
    fn comic_roundtrip_and_bounded_values() {
        let current = SettingsValue::default_for(SettingsSection::Comic);
        let patch: SettingsPatch = serde_json::from_str(
            r#"{"section":"comic","viewMode":"double","direction":"ltr","pageGap":"twenty_four","preloadPages":"five"}"#,
        )
        .unwrap();
        let next = patch.apply_to(&current);
        assert_eq!(
            next,
            SettingsValue::Comic(ComicSettings {
                view_mode: ComicViewMode::Double,
                direction: ComicDirection::Ltr,
                page_gap: ComicPageGap::TwentyFour,
                preload_pages: ComicPreloadPages::Five,
            })
        );
        assert_eq!(ComicPageGap::TwentyFour.as_pixels(), 24);
        assert_eq!(ComicPreloadPages::Unlimited.radius(), 12);
        let json = serde_json::to_string(&next).unwrap();
        let back: SettingsValue = serde_json::from_str(&json).unwrap();
        assert_eq!(back, next);
    }

    #[test]
    fn downloads_roundtrip_and_policy_values_are_bounded() {
        let current = SettingsValue::default_for(SettingsSection::Downloads);
        let patch: SettingsPatch = serde_json::from_str(
            r#"{"section":"downloads","concurrentTasks":"five","speedLimit":"mbps2"}"#,
        )
        .unwrap();
        let next = patch.apply_to(&current);
        assert_eq!(
            next,
            SettingsValue::Downloads(DownloadSettings {
                concurrent_tasks: DownloadConcurrency::Five,
                speed_limit: DownloadSpeedLimit::Mbps2,
                auto_continue: true,
            })
        );
        let concurrency = [
            (DownloadConcurrency::One, 1),
            (DownloadConcurrency::Two, 2),
            (DownloadConcurrency::Three, 3),
            (DownloadConcurrency::Five, 5),
        ];
        for (value, expected) in concurrency {
            assert_eq!(value.as_usize(), expected);
        }
        let speed_limits = [
            (DownloadSpeedLimit::Unlimited, None),
            (DownloadSpeedLimit::Kbps512, Some(512 * 1024)),
            (DownloadSpeedLimit::Mbps2, Some(2 * 1024 * 1024)),
            (DownloadSpeedLimit::Mbps5, Some(5 * 1024 * 1024)),
            (DownloadSpeedLimit::Mbps10, Some(10 * 1024 * 1024)),
        ];
        for (value, expected) in speed_limits {
            assert_eq!(value.as_bytes_per_second(), expected);
        }
        let json = serde_json::to_string(&next).unwrap();
        let back: SettingsValue = serde_json::from_str(&json).unwrap();
        assert_eq!(back, next);

        // 旧的 006A 行没有 autoContinue；读取时必须安全补默认值，避免升级后设置整区损坏。
        let legacy: SettingsValue = serde_json::from_str(
            r#"{"section":"downloads","concurrentTasks":"two","speedLimit":"mbps5"}"#,
        )
        .unwrap();
        assert_eq!(
            legacy,
            SettingsValue::Downloads(DownloadSettings {
                concurrent_tasks: DownloadConcurrency::Two,
                speed_limit: DownloadSpeedLimit::Mbps5,
                auto_continue: true,
            })
        );

        let disabled: SettingsPatch =
            serde_json::from_str(r#"{"section":"downloads","autoContinue":false}"#).unwrap();
        assert_eq!(
            disabled.apply_to(&legacy),
            SettingsValue::Downloads(DownloadSettings {
                concurrent_tasks: DownloadConcurrency::Two,
                speed_limit: DownloadSpeedLimit::Mbps5,
                auto_continue: false,
            })
        );

        // 旧 playback 行没有 autoNext；升级时必须继续保持原有连续播放行为。
        let legacy_playback: SettingsValue = serde_json::from_str(
            r#"{"section":"playback","defaultPlaybackRate":"one_point_five","autoResume":false}"#,
        )
        .unwrap();
        assert_eq!(
            legacy_playback,
            SettingsValue::Playback(PlaybackSettings {
                default_playback_rate: PlaybackRate::OnePointFive,
                auto_resume: false,
                auto_next: true,
            })
        );
        let auto_next_disabled: SettingsPatch =
            serde_json::from_str(r#"{"section":"playback","autoNext":false}"#).unwrap();
        assert_eq!(
            auto_next_disabled.apply_to(&legacy_playback),
            SettingsValue::Playback(PlaybackSettings {
                default_playback_rate: PlaybackRate::OnePointFive,
                auto_resume: false,
                auto_next: false,
            })
        );

        // 旧 privacy 行没有 playbackHistory；升级时必须继续保持原有的历史记录行为。
        let legacy_privacy: SettingsValue =
            serde_json::from_str(r#"{"section":"privacy","searchHistory":false}"#).unwrap();
        assert_eq!(
            legacy_privacy,
            SettingsValue::Privacy(PrivacySettings {
                search_history: false,
                playback_history: true,
            })
        );

        let playback_disabled: SettingsPatch =
            serde_json::from_str(r#"{"section":"privacy","playbackHistory":false}"#).unwrap();
        assert_eq!(
            playback_disabled.apply_to(&legacy_privacy),
            SettingsValue::Privacy(PrivacySettings {
                search_history: false,
                playback_history: false,
            })
        );
    }

    /// 旧 appearance 行没有界面字体字段：升级后必须仍能读取，并回落到系统默认字体，
    /// 不能让整区反序列化失败（那会变成“设置数据损坏”）。
    #[test]
    fn appearance_legacy_rows_fill_interface_font_defaults() {
        let legacy: SettingsValue = serde_json::from_str(
            r#"{"section":"appearance","theme":"dark","density":"compact","sidebar":"collapsed","reduceMotion":true}"#,
        )
        .unwrap();
        assert_eq!(
            legacy,
            SettingsValue::Appearance(AppearanceSettings {
                theme: Theme::Dark,
                density: Density::Compact,
                sidebar: SidebarPreference::Collapsed,
                reduce_motion: true,
                interface_font_mode: InterfaceFontMode::System,
                interface_font_family: None,
                interface_font_asset_id: None,
                ..AppearanceSettings::default()
            })
        );

        // 旧行 + 只改模式的 patch：其余字段保持原值，字体选择仍为空。
        let mode_only: SettingsPatch =
            serde_json::from_str(r#"{"section":"appearance","interfaceFontMode":"serif"}"#)
                .unwrap();
        let next = mode_only.apply_to(&legacy);
        match &next {
            SettingsValue::Appearance(v) => {
                assert_eq!(v.interface_font_mode, InterfaceFontMode::Serif);
                assert_eq!(v.theme, Theme::Dark);
                assert_eq!(v.interface_font_family, None);
                assert_eq!(v.interface_font_asset_id, None);
            }
            _ => panic!("section 必须一致"),
        }

        // 非 `custom` 模式下也允许保留选择，渲染端只在 custom 时读取；
        // 空字符串表示清除选择，与 reading 的 custom 字段同一语义。
        let cleared: SettingsPatch = serde_json::from_str(
            r#"{"section":"appearance","interfaceFontFamily":"  ","interfaceFontAssetId":""}"#,
        )
        .unwrap();
        match cleared.apply_to(&next) {
            SettingsValue::Appearance(v) => {
                assert_eq!(v.interface_font_family, None);
                assert_eq!(v.interface_font_asset_id, None);
            }
            _ => panic!("section 必须一致"),
        }

        // 未知模式值必须在反序列化边界拒绝。
        assert!(
            serde_json::from_str::<SettingsPatch>(
                r#"{"section":"appearance","interfaceFontMode":"comic-sans"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn interface_font_selection_roundtrips_through_json() {
        let value = SettingsValue::Appearance(AppearanceSettings {
            theme: Theme::Light,
            density: Density::Comfortable,
            sidebar: SidebarPreference::Auto,
            reduce_motion: false,
            interface_font_mode: InterfaceFontMode::Custom,
            interface_font_family: Some("Microsoft YaHei UI".to_owned()),
            interface_font_asset_id: Some("7f2c1b90-0f4a-4c2f-9a4d-1b2c3d4e5f60".to_owned()),
            ..AppearanceSettings::default()
        });
        let json = serde_json::to_string(&value).unwrap();
        assert!(json.contains(r#""interfaceFontMode":"custom""#));
        assert!(json.contains(r#""interfaceFontFamily":"Microsoft YaHei UI""#));
        let back: SettingsValue = serde_json::from_str(&json).unwrap();
        assert_eq!(back, value);

        // 未选择时字段不落盘，保持旧的 appearance JSON 形状。
        let plain = serde_json::to_string(&SettingsValue::default_for(SettingsSection::Appearance))
            .unwrap();
        assert!(!plain.contains("interfaceFontFamily"));
        assert!(!plain.contains("interfaceFontAssetId"));
    }

    #[test]
    fn interface_font_family_names_are_bounded() {
        assert!(is_safe_interface_font_family("Microsoft YaHei UI"));
        assert!(is_safe_interface_font_family(" 思源黑体 "));
        assert!(is_safe_interface_font_family("Noto Sans CJK SC"));

        assert!(!is_safe_interface_font_family(""));
        assert!(!is_safe_interface_font_family("   "));
        assert!(!is_safe_interface_font_family("Bad\";color:red"));
        assert!(!is_safe_interface_font_family("Bad;color:red"));
        assert!(!is_safe_interface_font_family("Bad{}"));
        assert!(!is_safe_interface_font_family("Bad\\65scape"));
        assert!(!is_safe_interface_font_family("a/*b*/"));
        assert!(!is_safe_interface_font_family("<script>"));
        assert!(!is_safe_interface_font_family("bad\nnewline"));
        assert!(!is_safe_interface_font_family(&"x".repeat(121)));
    }

    #[test]
    fn interface_font_family_names_are_repaired_for_imported_files() {
        // 正常族名原样保留（含中文与内部空格）。
        assert_eq!(
            sanitize_interface_font_family("Microsoft YaHei UI", "导入字体"),
            "Microsoft YaHei UI"
        );
        assert_eq!(
            sanitize_interface_font_family("  思源  黑体  ", "导入字体"),
            "思源 黑体"
        );

        // 第三方字体文件里的危险字符被剥离，结果必然通过安全校验。
        let repaired = sanitize_interface_font_family("Bad\";color:red{}", "导入字体");
        assert_eq!(repaired, "Badcolor:red");
        assert!(is_safe_interface_font_family(&repaired));
        let repaired = sanitize_interface_font_family("a/*b*/c", "导入字体");
        assert!(is_safe_interface_font_family(&repaired));

        // 全是非法字符 → 退化到兜底名，绝不产出空族名。
        assert_eq!(
            sanitize_interface_font_family("\"';{}\\<>", "导入字体"),
            "导入字体"
        );
        assert_eq!(
            sanitize_interface_font_family("   ", "导入字体"),
            "导入字体"
        );

        // 超长族名被截断到上限内。
        let long = sanitize_interface_font_family(&"字".repeat(500), "导入字体");
        assert_eq!(long.chars().count(), 120);
        assert!(is_safe_interface_font_family(&long));

        let spaced = format!("{} X", "x".repeat(MAX_INTERFACE_FONT_FAMILY_LEN - 1));
        let bounded = sanitize_interface_font_family(&spaced, "导入字体");
        assert_eq!(bounded, "x".repeat(MAX_INTERFACE_FONT_FAMILY_LEN - 1));
        assert!(is_safe_interface_font_family(&bounded));
    }
}
