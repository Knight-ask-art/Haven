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

/// appearance 分区设置（Typed DTO；JSON 字段 camelCase，与 wire 规则一致）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AppearanceSettings {
    pub theme: Theme,
    pub density: Density,
    pub sidebar: SidebarPreference,
    pub reduce_motion: bool,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            density: Density::Comfortable,
            sidebar: SidebarPreference::Auto,
            reduce_motion: false,
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
}
