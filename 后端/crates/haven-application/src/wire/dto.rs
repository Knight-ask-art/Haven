//! 首批 Slice Wire DTO（camelCase JSON + ts-rs TypeScript 导出）。
//!
//! 契约来源：FRONTEND_BACKEND_CONTRACT.md §12（核心投影）、§14（Home/Library）、
//! §22（Locator）。ts-rs 通过 serde-compat 读取 serde rename 规则生成同名 TS 字段。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use haven_domain::settings::{
    ComicDirection, ComicPageGap, ComicPatch, ComicPreloadPages, ComicSettings, ComicViewMode,
    PreferenceData, ReadingContentWidth, ReadingFontFamily, ReadingFontSize, ReadingFontWeight,
    ReadingLetterSpacing, ReadingLineHeight, ReadingPagination, ReadingPatch, ReadingSettings,
    ReadingTheme,
};

/// 一级分类（canonical，无 `all`——`all` 只作为查询 sentinel）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ContentCategory {
    Video,
    Book,
    Comic,
    Periodical,
}

/// 查询分类 = `all` | ContentCategory（契约 §12）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum QueryCategory {
    All,
    Video,
    Book,
    Comic,
    Periodical,
}

/// 媒介类型 Wire 值（与 domain MediaType 对齐）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum MediaTypeDto {
    Movie,
    Series,
    Episode,
    Book,
    Document,
    Comic,
    Article,
    Audio,
    Unknown,
}

/// 主操作类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum PrimaryActionKind {
    Playback,
    Reader,
    Comic,
    Article,
    OpenEdition,
}

/// 按钮文案提示（闭合联合，C-04 修复：前端可 exhaustive render，
/// 未知值不得静默落入默认分支）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum LabelHint {
    Start,
    Continue,
    Open,
}

/// 主操作。由后端基于进度、偏好和可用 Resource 决定（契约 §12 投影规则）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct PrimaryActionDto {
    pub kind: PrimaryActionKind,
    pub label_hint: LabelHint,
    pub edition_id: String,
    pub media_item_id: Option<String>,
    pub locator: Option<LocatorDto>,
}

/// 作品卡片投影（Library 列表 / 搜索结果共用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct WorkCardDto {
    pub work_id: String,
    pub title: String,
    pub original_title: Option<String>,
    pub description: Option<String>,
    pub categories: Vec<ContentCategory>,
    pub available_media_types: Vec<MediaTypeDto>,
    pub poster_uri: Option<String>,
    pub backdrop_uri: Option<String>,
    pub release_year: Option<i32>,
    pub rating_value: Option<f64>,
    pub rating_scale: Option<f64>,
    pub favorite: bool,
    pub progress: Option<ProgressSummaryDto>,
    pub primary_action: Option<PrimaryActionDto>,
    /// 元数据去重键投影（契约 §36.1）；无已知映射返回 []。
    pub external_ids: Vec<ExternalIdDto>,
}

/// 资源内设目标作用域（ADR-RESOURCE-PREF-001）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum PreferenceTargetDto {
    Edition,
    MediaItem,
}

/// 资源内设的阅读字体。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum PreferenceReadingFontFamilyDto {
    Sans,
    Serif,
    Kai,
    Heiti,
    Fangsong,
    Mianfei,
    Custom,
}

impl From<ReadingFontFamily> for PreferenceReadingFontFamilyDto {
    fn from(value: ReadingFontFamily) -> Self {
        match value {
            ReadingFontFamily::Sans => Self::Sans,
            ReadingFontFamily::Serif => Self::Serif,
            ReadingFontFamily::Kai => Self::Kai,
            ReadingFontFamily::Heiti => Self::Heiti,
            ReadingFontFamily::Fangsong => Self::Fangsong,
            ReadingFontFamily::Mianfei => Self::Mianfei,
            ReadingFontFamily::Custom => Self::Custom,
        }
    }
}

impl From<PreferenceReadingFontFamilyDto> for ReadingFontFamily {
    fn from(value: PreferenceReadingFontFamilyDto) -> Self {
        match value {
            PreferenceReadingFontFamilyDto::Sans => Self::Sans,
            PreferenceReadingFontFamilyDto::Serif => Self::Serif,
            PreferenceReadingFontFamilyDto::Kai => Self::Kai,
            PreferenceReadingFontFamilyDto::Heiti => Self::Heiti,
            PreferenceReadingFontFamilyDto::Fangsong => Self::Fangsong,
            PreferenceReadingFontFamilyDto::Mianfei => Self::Mianfei,
            PreferenceReadingFontFamilyDto::Custom => Self::Custom,
        }
    }
}

macro_rules! preference_enum_conversion {
    ($wire:ident, $domain:ident, { $( $variant:ident ),+ $(,)? }) => {
        impl From<$domain> for $wire {
            fn from(value: $domain) -> Self {
                match value {
                    $( $domain::$variant => Self::$variant, )+
                }
            }
        }

        impl From<$wire> for $domain {
            fn from(value: $wire) -> Self {
                match value {
                    $( $wire::$variant => Self::$variant, )+
                }
            }
        }
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum PreferenceReadingFontSizeDto {
    Small,
    Medium,
    Large,
}
preference_enum_conversion!(PreferenceReadingFontSizeDto, ReadingFontSize, { Small, Medium, Large });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum PreferenceReadingLineHeightDto {
    Compact,
    Comfortable,
    Airy,
}
preference_enum_conversion!(PreferenceReadingLineHeightDto, ReadingLineHeight, { Compact, Comfortable, Airy });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum PreferenceReadingContentWidthDto {
    Narrow,
    Medium,
    Wide,
}
preference_enum_conversion!(PreferenceReadingContentWidthDto, ReadingContentWidth, { Narrow, Medium, Wide });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum PreferenceReadingFontWeightDto {
    Light,
    Regular,
    Medium,
    Semibold,
    Bold,
}
preference_enum_conversion!(PreferenceReadingFontWeightDto, ReadingFontWeight, { Light, Regular, Medium, Semibold, Bold });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum PreferenceReadingLetterSpacingDto {
    Tight,
    Normal,
    Relaxed,
    Loose,
}
preference_enum_conversion!(PreferenceReadingLetterSpacingDto, ReadingLetterSpacing, { Tight, Normal, Relaxed, Loose });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum PreferenceReadingThemeDto {
    System,
    Paper,
    Warm,
    Slate,
    Dark,
    Sepia,
    #[serde(rename = "eyeCare")]
    EyeCare,
    Custom,
}
preference_enum_conversion!(PreferenceReadingThemeDto, ReadingTheme, { System, Paper, Warm, Slate, Dark, Sepia, EyeCare, Custom });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum PreferenceReadingPaginationDto {
    Scroll,
    Paginated,
    Double,
}
preference_enum_conversion!(PreferenceReadingPaginationDto, ReadingPagination, { Scroll, Paginated, Double });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum PreferenceComicViewModeDto {
    Single,
    Double,
    Strip,
}
preference_enum_conversion!(PreferenceComicViewModeDto, ComicViewMode, { Single, Double, Strip });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum PreferenceComicDirectionDto {
    Rtl,
    Ltr,
}
preference_enum_conversion!(PreferenceComicDirectionDto, ComicDirection, { Rtl, Ltr });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum PreferenceComicPageGapDto {
    Zero,
    Twelve,
    TwentyFour,
}
preference_enum_conversion!(PreferenceComicPageGapDto, ComicPageGap, { Zero, Twelve, TwentyFour });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum PreferenceComicPreloadPagesDto {
    One,
    Three,
    Five,
    Unlimited,
}
preference_enum_conversion!(PreferenceComicPreloadPagesDto, ComicPreloadPages, { One, Three, Five, Unlimited });

/// 资源内设阅读部分更新。字段缺失表示不覆盖该字段；null 只用于清除整个 section。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct PreferenceReadingPatchDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<PreferenceReadingFontFamilyDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_font_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<PreferenceReadingFontSizeDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height: Option<PreferenceReadingLineHeightDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_width: Option<PreferenceReadingContentWidthDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<PreferenceReadingThemeDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_background: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_weight: Option<PreferenceReadingFontWeightDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub letter_spacing: Option<PreferenceReadingLetterSpacingDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_auto: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pagination: Option<PreferenceReadingPaginationDto>,
}

impl From<ReadingPatch> for PreferenceReadingPatchDto {
    fn from(value: ReadingPatch) -> Self {
        Self {
            font_family: value.font_family.map(Into::into),
            custom_font_family: value.custom_font_family,
            font_size: value.font_size.map(Into::into),
            line_height: value.line_height.map(Into::into),
            content_width: value.content_width.map(Into::into),
            theme: value.theme.map(Into::into),
            custom_background: value.custom_background,
            custom_text: value.custom_text,
            font_weight: value.font_weight.map(Into::into),
            letter_spacing: value.letter_spacing.map(Into::into),
            system_auto: value.system_auto,
            pagination: value.pagination.map(Into::into),
        }
    }
}

impl From<PreferenceReadingPatchDto> for ReadingPatch {
    fn from(value: PreferenceReadingPatchDto) -> Self {
        Self {
            font_family: value.font_family.map(Into::into),
            custom_font_family: value.custom_font_family,
            font_size: value.font_size.map(Into::into),
            line_height: value.line_height.map(Into::into),
            content_width: value.content_width.map(Into::into),
            theme: value.theme.map(Into::into),
            custom_background: value.custom_background,
            custom_text: value.custom_text,
            font_weight: value.font_weight.map(Into::into),
            letter_spacing: value.letter_spacing.map(Into::into),
            system_auto: value.system_auto,
            pagination: value.pagination.map(Into::into),
        }
    }
}

/// 资源内设漫画部分更新。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct PreferenceComicPatchDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_mode: Option<PreferenceComicViewModeDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<PreferenceComicDirectionDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_gap: Option<PreferenceComicPageGapDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preload_pages: Option<PreferenceComicPreloadPagesDto>,
}

impl From<ComicPatch> for PreferenceComicPatchDto {
    fn from(value: ComicPatch) -> Self {
        Self {
            view_mode: value.view_mode.map(Into::into),
            direction: value.direction.map(Into::into),
            page_gap: value.page_gap.map(Into::into),
            preload_pages: value.preload_pages.map(Into::into),
        }
    }
}

impl From<PreferenceComicPatchDto> for ComicPatch {
    fn from(value: PreferenceComicPatchDto) -> Self {
        Self {
            view_mode: value.view_mode.map(Into::into),
            direction: value.direction.map(Into::into),
            page_gap: value.page_gap.map(Into::into),
            preload_pages: value.preload_pages.map(Into::into),
        }
    }
}

/// 有效阅读设置投影。`section` 是 Wire discriminator，Domain 设置本身不携带该字段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct PreferenceReadingSettingsDto {
    #[ts(type = "\"reading\"")]
    pub section: String,
    pub font_family: PreferenceReadingFontFamilyDto,
    pub custom_font_family: Option<String>,
    pub font_size: PreferenceReadingFontSizeDto,
    pub line_height: PreferenceReadingLineHeightDto,
    pub content_width: PreferenceReadingContentWidthDto,
    pub theme: PreferenceReadingThemeDto,
    pub custom_background: Option<String>,
    pub custom_text: Option<String>,
    pub font_weight: PreferenceReadingFontWeightDto,
    pub letter_spacing: PreferenceReadingLetterSpacingDto,
    pub system_auto: bool,
    pub pagination: PreferenceReadingPaginationDto,
}

impl From<ReadingSettings> for PreferenceReadingSettingsDto {
    fn from(value: ReadingSettings) -> Self {
        Self {
            section: "reading".to_owned(),
            font_family: value.font_family.into(),
            custom_font_family: value.custom_font_family,
            font_size: value.font_size.into(),
            line_height: value.line_height.into(),
            content_width: value.content_width.into(),
            theme: value.theme.into(),
            custom_background: value.custom_background,
            custom_text: value.custom_text,
            font_weight: value.font_weight.into(),
            letter_spacing: value.letter_spacing.into(),
            system_auto: value.system_auto,
            pagination: value.pagination.into(),
        }
    }
}

/// 有效漫画设置投影。`section` 是 Wire discriminator，Domain 设置本身不携带该字段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct PreferenceComicSettingsDto {
    #[ts(type = "\"comic\"")]
    pub section: String,
    pub view_mode: PreferenceComicViewModeDto,
    pub direction: PreferenceComicDirectionDto,
    pub page_gap: PreferenceComicPageGapDto,
    pub preload_pages: PreferenceComicPreloadPagesDto,
}

impl From<ComicSettings> for PreferenceComicSettingsDto {
    fn from(value: ComicSettings) -> Self {
        Self {
            section: "comic".to_owned(),
            view_mode: value.view_mode.into(),
            direction: value.direction.into(),
            page_gap: value.page_gap.into(),
            preload_pages: value.preload_pages.into(),
        }
    }
}

/// 读取资源内设的请求。`mediaItemId` 与 `editionId` 都必须由服务端验证归属。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct PreferenceGetRequest {
    pub media_item_id: String,
    pub edition_id: String,
}

/// 更新资源内设的请求；空 Patch（null）表示清除对应作用域的覆盖。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct PreferenceUpdateRequest {
    pub media_item_id: String,
    pub edition_id: String,
    pub target: PreferenceTargetDto,
    pub reading_patch: Option<PreferenceReadingPatchDto>,
    pub comic_patch: Option<PreferenceComicPatchDto>,
    pub expected_revision: Option<String>,
}

impl PreferenceUpdateRequest {
    pub fn data(&self) -> PreferenceData {
        PreferenceData {
            reading: self.reading_patch.clone().map(Into::into),
            comic: self.comic_patch.clone().map(Into::into),
        }
    }
}

/// 资源内设读模型：同时返回原始覆盖和 effective 值，避免前端自行合并 global 设置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct PreferenceGetResult {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub media_item_id: String,
    pub edition_id: String,
    pub reading_patch: Option<PreferenceReadingPatchDto>,
    pub comic_patch: Option<PreferenceComicPatchDto>,
    pub edition_reading_patch: Option<PreferenceReadingPatchDto>,
    pub edition_comic_patch: Option<PreferenceComicPatchDto>,
    pub media_item_reading_patch: Option<PreferenceReadingPatchDto>,
    pub media_item_comic_patch: Option<PreferenceComicPatchDto>,
    pub effective_reading: PreferenceReadingSettingsDto,
    pub effective_comic: PreferenceComicSettingsDto,
    pub media_item_revision: Option<String>,
    pub edition_revision: Option<String>,
}

/// 更新结果，携带更新后的资源内设读模型和目标作用域 revision。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct PreferenceUpdateResult {
    pub result: PreferenceGetResult,
    pub target: PreferenceTargetDto,
    pub revision: Option<String>,
    pub changed: bool,
}

/// 完成状态（闭合集合；NOTE-2：请求与响应共用，禁止裸 String）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum CompletionWire {
    NotStarted,
    InProgress,
    Completed,
    Abandoned,
}

impl From<CompletionWire> for haven_domain::enums::CompletionState {
    fn from(value: CompletionWire) -> Self {
        match value {
            CompletionWire::NotStarted => haven_domain::enums::CompletionState::NotStarted,
            CompletionWire::InProgress => haven_domain::enums::CompletionState::InProgress,
            CompletionWire::Completed => haven_domain::enums::CompletionState::Completed,
            CompletionWire::Abandoned => haven_domain::enums::CompletionState::Abandoned,
        }
    }
}

impl From<haven_domain::enums::CompletionState> for CompletionWire {
    fn from(value: haven_domain::enums::CompletionState) -> Self {
        match value {
            haven_domain::enums::CompletionState::NotStarted => CompletionWire::NotStarted,
            haven_domain::enums::CompletionState::InProgress => CompletionWire::InProgress,
            haven_domain::enums::CompletionState::Completed => CompletionWire::Completed,
            haven_domain::enums::CompletionState::Abandoned => CompletionWire::Abandoned,
        }
    }
}

/// 进度摘要（progressRatio 是派生值，事实来源是 locator）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ProgressSummaryDto {
    pub media_item_id: String,
    pub completion: CompletionWire,
    pub progress_ratio: Option<f64>,
    /// 进度写入的 opaque CAS token。与 `ProgressSaveResult.revision` 同源；
    /// `updatedAt` 仅用于展示，不能被前端当作并发写入版本。
    pub revision: String,
    pub updated_at: String,
    pub locator: LocatorDto,
    /// 关键帧（退出时截取的当前帧，data URI 或 haven-resource 链接），缺失回退海报。
    #[serde(default)]
    #[ts(optional)]
    pub keyframe_uri: Option<String>,
}

/// 统一 Locator Wire 形式：`{ version: 1; kind: "..."; data: ... }`（契约 §22）。
/// serde 手写（tag+content 无法在顶层注入常量 version）；TS 类型覆盖为契约联合形状。
#[derive(Debug, Clone, PartialEq, TS)]
#[ts(
    type = "{ version: 1, kind: \"video\", data: VideoLocatorDto } | { version: 1, kind: \"book\", data: BookLocatorDto } | { version: 1, kind: \"pdf\", data: PdfLocatorDto } | { version: 1, kind: \"comic\", data: ComicLocatorDto } | { version: 1, kind: \"article\", data: ArticleLocatorDto }"
)]
pub enum LocatorDto {
    Video(VideoLocatorDto),
    Book(BookLocatorDto),
    Pdf(PdfLocatorDto),
    Comic(ComicLocatorDto),
    Article(ArticleLocatorDto),
}

const LOCATOR_WIRE_VERSION: u32 = 1;

impl Serialize for LocatorDto {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let (kind, data) = match self {
            LocatorDto::Video(v) => ("video", serde_json::to_value(v)),
            LocatorDto::Book(v) => ("book", serde_json::to_value(v)),
            LocatorDto::Pdf(v) => ("pdf", serde_json::to_value(v)),
            LocatorDto::Comic(v) => ("comic", serde_json::to_value(v)),
            LocatorDto::Article(v) => ("article", serde_json::to_value(v)),
        };
        let data = data.map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("LocatorDto", 3)?;
        state.serialize_field("version", &LOCATOR_WIRE_VERSION)?;
        state.serialize_field("kind", kind)?;
        state.serialize_field("data", &data)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for LocatorDto {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            version: u32,
            kind: String,
            data: serde_json::Value,
        }
        let raw = Raw::deserialize(deserializer)?;
        if raw.version != LOCATOR_WIRE_VERSION {
            return Err(serde::de::Error::custom(format!(
                "unsupported locator wire version: {}",
                raw.version
            )));
        }
        match raw.kind.as_str() {
            "video" => serde_json::from_value(raw.data)
                .map(LocatorDto::Video)
                .map_err(serde::de::Error::custom),
            "book" => serde_json::from_value(raw.data)
                .map(LocatorDto::Book)
                .map_err(serde::de::Error::custom),
            "pdf" => serde_json::from_value(raw.data)
                .map(LocatorDto::Pdf)
                .map_err(serde::de::Error::custom),
            "comic" => serde_json::from_value(raw.data)
                .map(LocatorDto::Comic)
                .map_err(serde::de::Error::custom),
            "article" => serde_json::from_value(raw.data)
                .map(LocatorDto::Article)
                .map_err(serde::de::Error::custom),
            other => Err(serde::de::Error::custom(format!(
                "unknown locator kind: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct VideoLocatorDto {
    /// 毫秒整数；wire 使用 number（契约 §11.2，视频时长远小于 2^53）。
    #[ts(type = "number")]
    pub position_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct BookLocatorDto {
    pub publication_resource: String,
    pub progression: Option<f64>,
    pub text_anchor: Option<TextAnchorDto>,
    pub format_locator: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct PdfLocatorDto {
    pub page_index: u32,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub zoom: Option<f64>,
    pub text_anchor: Option<TextAnchorDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicLocatorDto {
    pub chapter_item_id: String,
    pub page_index: u32,
    pub page_progression: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ArticleLocatorDto {
    pub block_id: Option<String>,
    pub progression: Option<f64>,
    pub text_anchor: Option<TextAnchorDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct TextAnchorDto {
    pub exact: Option<String>,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
}

/// 分页（契约 §11.3）。cursor 是 opaque string，前端不得解析。
/// NOTE-1：空值统一 `T | null`（字段始终存在）；`schemaVersion: 1` 契约要求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct PageDto<T: TS> {
    /// 契约字面量 `1`（C-04 修复：生成物为 `schemaVersion: 1`，禁止任意 number）。
    #[ts(type = "1")]
    pub schema_version: u32,
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    #[ts(type = "number | null")]
    pub total: Option<u64>,
    pub revision: Option<String>,
}

/// Library 列表排序。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum LibraryListSort {
    RecentlyAdded,
    Title,
    LastActive,
    ReleaseDate,
    Rating,
}

/// `library_list` 请求（契约 §14.3）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct LibraryListRequest {
    pub category: QueryCategory,
    pub media_types: Option<Vec<MediaTypeDto>>,
    pub query: Option<String>,
    pub sort: LibraryListSort,
    pub cursor: Option<String>,
    pub limit: u32,
}

/// `session_open` 的执行引擎。Wire 值是稳定的 opaque 枚举，不暴露实现类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum SessionEngineDto {
    Playback,
    Reader,
    Comic,
    Article,
}

/// 打开消费 Session 的请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SessionOpenRequest {
    pub media_item_id: String,
    pub engine: SessionEngineDto,
}

/// 字幕格式是受控的解析提示，不是文件名或路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum SubtitleFormatDto {
    Srt,
    Vtt,
    Sbv,
    Ass,
    Ssa,
    Ttml,
    Dfxp,
    Sub,
    Lrc,
    Unknown,
}

/// Session 内可消费的字幕轨道。路径、来源 URL 和凭据永不进入 Wire。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SubtitleTrackDto {
    pub track_id: String,
    pub label: String,
    pub language: Option<String>,
    pub format: SubtitleFormatDto,
    pub content_uri: String,
}

/// 打开消费 Session 的安全响应。受控内容 URI 由 Tauri registry 签发。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SessionOpenResultDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub session_id: String,
    /// Comic 会话必须返回 null，防止整本 CBZ 绕过逐页授权；其他引擎返回受控 URI。
    pub content_uri: Option<String>,
    pub work_id: String,
    pub edition_id: String,
    pub media_item_id: String,
    pub engine: SessionEngineDto,
    pub progress: Option<ProgressSummaryDto>,
    /// 受控播放流的安全类型提示。旧客户端可省略；`hls` 由播放器交给
    /// hls.js，`direct` 使用浏览器原生 `<video>`，不暴露上游 MIME/URL。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub stream_kind: Option<StreamKindDto>,
    /// 与本地视频同目录且仍在存储根内的受控外挂字幕；旧客户端可省略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub subtitle_tracks: Option<Vec<SubtitleTrackDto>>,
}

/// 关闭消费 Session 的请求（幂等撤销；sessionId 为不透明 UUID token）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SessionCloseRequest {
    pub session_id: String,
}

/// 关闭消费 Session 的结果（未知/重复 token 也返回成功）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SessionCloseResultDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub closed: bool,
}

/// `comic_page_manifest_get` 的只读请求。MediaItem/Resource/Engine 均从 Session 推导。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicPageManifestGetRequest {
    pub session_id: String,
}

/// 当前 Session 内某一页的可用性。不可用页保留 pageIndex，不让后续页重排。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ComicPageAvailabilityDto {
    Ready,
    Unavailable,
}

/// 漫画页公开投影。pageId 是逻辑身份，contentUri 是独立的读取 capability。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicPageDto {
    pub page_id: String,
    pub page_index: u32,
    pub availability: ComicPageAvailabilityDto,
    pub content_uri: Option<String>,
}

/// 当前 Comic Session 的稳定页面清单。pageIndex 严格连续且从 0 开始。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicPageManifestDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub session_id: String,
    pub media_item_id: String,
    pub page_count: u32,
    pub pages: Vec<ComicPageDto>,
}

/// `reader_toc_get` 的只读请求。MediaItem/Resource/Engine 均从 Session 推导。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ReaderTocGetRequest {
    pub session_id: String,
}

/// 目录条目（扁平列表，depth 决定层级；前端按 depth 缩进渲染）。
/// id 为稳定 `FNV-1a64(href \0 title \0 occurrence)` 的 16 位 hex，
/// 同文件跨会话一致，可用于书签/批注的章节归属比对。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct TocItemDto {
    pub id: String,
    pub title: String,
    pub depth: u32,
    /// 可选的 EPUB 文档内锚点。解析失败或条目指向章节起点时为 null。
    #[serde(default)]
    #[ts(optional)]
    pub fragment: Option<String>,
    /// 0..1；条目所在 spine 文档起点在全书的估算进度（spine 序号 / spine 长度）。
    pub progression: f64,
}

/// `reader_toc_get` 响应。非 EPUB 图书（TXT/Markdown）返回 FORMAT_UNSUPPORTED，
/// EPUB 无显式目录时按 spine 顺序生成扁平兜底目录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ReaderTocResultDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub session_id: String,
    pub items: Vec<TocItemDto>,
}

/// `reader_search` 请求。query 1..128 字符（归一化后），空或超长返回空结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ReaderSearchRequest {
    pub session_id: String,
    pub query: String,
}

/// 检索命中（`book-search.ts` 前端语义的 Rust 等价物；按 TF-IDF 分数降序）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ReaderSearchHitDto {
    pub chapter_id: String,
    pub chapter_title: String,
    pub chapter_index: u32,
    pub paragraph_index: u32,
    pub progression_in_chapter: f64,
    pub text_anchor: TextAnchorDto,
    pub score: f64,
}

/// `reader_search` 响应。命中为原文片段（exact 12..240，prefix/suffix 各 30）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ReaderSearchResultDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub session_id: String,
    pub hits: Vec<ReaderSearchHitDto>,
}

/// `reader_search` Channel 事件种类（`ReaderSearch` 单书检索，terminal 唯一）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ReaderSearchEventKind {
    Started,
    Progress,
    Result,
    Completed,
    Cancelled,
    Failed,
}

/// `reader_search` Channel 事件负载。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ReaderSearchEventData {
    pub hits: Vec<ReaderSearchHitDto>,
    pub scanned_chapters: Option<u32>,
    pub total_chapters: Option<u32>,
    pub code: Option<String>,
    pub message: Option<String>,
}

/// `reader_search` Channel 事件（与 `search.source` 同 envelope 语义）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ReaderSearchEvent {
    pub operation_id: String,
    pub sequence: u32,
    pub at: String,
    pub kind: ReaderSearchEventKind,
    pub data: ReaderSearchEventData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ReaderSearchCancelRequest {
    pub operation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ReaderSearchCancelResultDto {
    pub operation_id: String,
    pub already_terminal: bool,
}

/// `favorite_set` 请求（Work 收藏目标；幂等）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct FavoriteSetRequest {
    pub work_id: String,
    pub favorite: bool,
}

/// `favorite_set` 成功响应（R-C01 冻结；名称与形状固定）。
/// revision 语义（R-FAV-001，状态版本）：
/// - 状态变化（未收藏↔收藏）→ 生成新 revision 并持久化（work_favorite_versions）。
/// - 重复设置相同状态 → 返回当前 revision，**不制造新 token**（幂等收敛）。
/// - 从未变更过状态（无版本历史）→ `revision: null`（favorite 字段为权威状态；
///   幂等路径不发 Event，因此 Event 的 revision 恒非空 string）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct FavoriteSetResult {
    pub work_id: String,
    pub favorite: bool,
    #[ts(type = "string | null")]
    pub revision: Option<String>,
}

/// `progress_save` 请求（契约 §22.6）。
/// workId/editionId 由后端从 MediaItem 推导，前端禁止提交（防 ID 矛盾）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ProgressSaveRequest {
    pub media_item_id: String,
    pub locator: LocatorDto,
    pub completion: Option<CompletionWire>,
    pub expected_revision: Option<String>,
    /// 关键帧 JPEG data URL（data:image/jpeg;base64,...），可选，超大则忽略。
    #[serde(default)]
    #[ts(optional)]
    pub keyframe: Option<String>,
}

/// `progress_mark_completed` 请求。`initial_locator` 只用于尚无 Progress
/// 时创建安全起点；已有记录时后端原子保留其 Locator 与 percentage。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ProgressMarkCompletedRequest {
    pub media_item_id: String,
    pub initial_locator: LocatorDto,
}

/// `progress_save` 结果：新 Revision（opaque；语义由 BE-REVISION-001 正式化）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ProgressSaveResult {
    pub revision: String,
}

/// 历史条目投影（契约 §23：列表 Item 带目标类型与明确 ID）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct HistoryEntryDto {
    pub history_entry_id: String,
    pub media_item_id: String,
    pub work_id: String,
    pub edition_id: String,
    pub started_at: String,
    pub last_active_at: String,
    pub completed_at: Option<String>,
    /// 精确指向此历史条目的消费目标；绝不能从同一 Work 的首集或当前卡片投影推断。
    pub primary_action: Option<PrimaryActionDto>,
    /// 此历史条目 MediaItem 的当前进度（如有）；展示时不得以另一集的进度替代。
    pub progress: Option<ProgressSummaryDto>,
}

/// `history_list` 请求（契约 §23.1）：最近活跃历史。
/// `limit` 为 None 时由后端取默认上限（MAX_LIMIT）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct HistoryListRequest {
    pub limit: Option<u32>,
}

/// 搜索历史条目（V02-SETTINGS-PRIVACY-DATA-007）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SearchHistoryEntryDto {
    pub term: String,
    pub last_used_at: String,
}

/// `search_history_list` 请求；None 使用应用层默认上限。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SearchHistoryListRequest {
    pub limit: Option<u32>,
}

/// `search_history_record` 请求。显式写命令避免 Query 隐式写数据库。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SearchHistoryRecordRequest {
    pub term: String,
}

/// `search_history_remove` 请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SearchHistoryRemoveRequest {
    pub term: String,
}

/// `progress_recent` 请求（契约 §23.1）：最近活跃进度（首页 Continue 数据源）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ProgressRecentRequest {
    pub limit: Option<u32>,
}

/// `progress_reset` 请求（契约 §23.2）：业务操作，只清进度状态不删除实体。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ProgressResetRequest {
    pub media_item_id: String,
}

/// `marker_list` 请求（契约 §23.1）：列出某 MediaItem 下未软删除的标记。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct MarkerListRequest {
    pub media_item_id: String,
}

/// `marker_list_all` 请求（契约 §23.1 足迹聚合 Query）：列出所有未软删除标记。
/// `limit` 为 None 时由后端取默认上限。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct MarkerListAllRequest {
    pub limit: Option<u32>,
}

/// `marker_delete` 请求（契约 §23.2）：软删除（墓碑语义）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct MarkerDeleteRequest {
    pub marker_id: String,
}

/// 标记类型（UI 统一叫"标记"，底层区分；契约 §42）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum MarkerTypeDto {
    Bookmark,
    Highlight,
    Note,
    Scene,
    Quote,
    Image,
}

impl From<MarkerTypeDto> for haven_domain::enums::MarkerType {
    fn from(value: MarkerTypeDto) -> Self {
        match value {
            MarkerTypeDto::Bookmark => haven_domain::enums::MarkerType::Bookmark,
            MarkerTypeDto::Highlight => haven_domain::enums::MarkerType::Highlight,
            MarkerTypeDto::Note => haven_domain::enums::MarkerType::Note,
            MarkerTypeDto::Scene => haven_domain::enums::MarkerType::Scene,
            MarkerTypeDto::Quote => haven_domain::enums::MarkerType::Quote,
            MarkerTypeDto::Image => haven_domain::enums::MarkerType::Image,
        }
    }
}

impl From<haven_domain::enums::MarkerType> for MarkerTypeDto {
    fn from(value: haven_domain::enums::MarkerType) -> Self {
        match value {
            haven_domain::enums::MarkerType::Bookmark => MarkerTypeDto::Bookmark,
            haven_domain::enums::MarkerType::Highlight => MarkerTypeDto::Highlight,
            haven_domain::enums::MarkerType::Note => MarkerTypeDto::Note,
            haven_domain::enums::MarkerType::Scene => MarkerTypeDto::Scene,
            haven_domain::enums::MarkerType::Quote => MarkerTypeDto::Quote,
            haven_domain::enums::MarkerType::Image => MarkerTypeDto::Image,
        }
    }
}

/// `marker_create` 请求（契约 §23：标记中心）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct MarkerCreateRequest {
    pub media_item_id: String,
    pub locator: LocatorDto,
    pub marker_type: MarkerTypeDto,
    pub title: Option<String>,
    pub excerpt: Option<String>,
    pub note: Option<String>,
}

/// 标记投影。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct MarkerDto {
    pub marker_id: String,
    pub media_item_id: String,
    pub work_id: String,
    pub edition_id: String,
    pub locator: LocatorDto,
    pub marker_type: MarkerTypeDto,
    pub title: Option<String>,
    pub excerpt: Option<String>,
    pub note: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// `library_scan_start` 请求（C-06 冻结裁决）。
/// 生产协议**只提交已注册的 `storageLocationId`**，不暴露裸 rootPath：
/// 任意路径能力与"禁止向高权限 WebView 暴露本地路径"的安全边界冲突。
/// 存储提供方 Wire 枚举（与 domain 值对齐，但 Domain Entity 不出 IPC）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum StorageProviderTypeDto {
    Local,
    WebDav,
    OneDrive,
    GoogleDrive,
}

impl From<haven_domain::enums::StorageProviderType> for StorageProviderTypeDto {
    fn from(value: haven_domain::enums::StorageProviderType) -> Self {
        match value {
            haven_domain::enums::StorageProviderType::Local => Self::Local,
            haven_domain::enums::StorageProviderType::WebDav => Self::WebDav,
            haven_domain::enums::StorageProviderType::OneDrive => Self::OneDrive,
            haven_domain::enums::StorageProviderType::GoogleDrive => Self::GoogleDrive,
        }
    }
}

/// 存储位置状态 Wire 枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum StorageStatusDto {
    Connected,
    Disconnected,
    AuthExpired,
    Unavailable,
    ReadOnly,
    Error,
    Disabled,
    Missing,
}

impl From<haven_domain::enums::StorageStatus> for StorageStatusDto {
    fn from(value: haven_domain::enums::StorageStatus) -> Self {
        match value {
            haven_domain::enums::StorageStatus::Connected => Self::Connected,
            haven_domain::enums::StorageStatus::Disconnected => Self::Disconnected,
            haven_domain::enums::StorageStatus::AuthExpired => Self::AuthExpired,
            haven_domain::enums::StorageStatus::Unavailable => Self::Unavailable,
            haven_domain::enums::StorageStatus::ReadOnly => Self::ReadOnly,
            haven_domain::enums::StorageStatus::Error => Self::Error,
            haven_domain::enums::StorageStatus::Disabled => Self::Disabled,
            haven_domain::enums::StorageStatus::Missing => Self::Missing,
        }
    }
}

/// `storage_location_list` 的安全行投影。
///
/// 只允许传输位置的稳定标识、显示名、provider 和状态；不得把 rootRef/rootPath、
/// credentialRef、绝对路径、UNC 路径或其他内部路径带出特权后端边界。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct StorageLocationDto {
    pub location_id: String,
    pub display_name: String,
    pub provider_type: StorageProviderTypeDto,
    pub status: StorageStatusDto,
}

impl From<haven_domain::entities::StorageLocation> for StorageLocationDto {
    fn from(value: haven_domain::entities::StorageLocation) -> Self {
        Self {
            location_id: value.id.to_string(),
            display_name: value.display_name,
            provider_type: value.provider_type.into(),
            status: value.status.into(),
        }
    }
}

/// 目录选择若在后续版本需要，必须走受控注册流程签发的 opaque scope token。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct LibraryScanStartRequest {
    pub storage_location_id: String,
}

/// `library_scan_start` 成功响应（R-C02 冻结裁决：**幂等**——同一 storageLocationId
/// 已有运行任务时，返回既有任务而非冲突错误；前端按 taskId 合并订阅）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ScanStartResult {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub operation_id: String,
    pub task_id: String,
    /// true = 该任务已在运行，本次启动幂等合并到既有任务。
    pub already_running: bool,
}

/// `scan_cancel` 幂等结果。运行中任务返回 cancelled 受理；
/// 已终态任务返回真实终态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ScanCancelResultDto {
    pub task_id: String,
    pub already_terminal: bool,
    pub phase: ScanPhase,
}

/// 扫描阶段（契约 §14.4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ScanPhase {
    Started,
    Enumerating,
    Detecting,
    Fingerprinting,
    Indexing,
    ItemIndexed,
    Warning,
    Completed,
    Cancelled,
    Failed,
}

/// 扫描 Channel 事件负载（稳定 taskId、阶段、计数、当前项与终态）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ScanEventData {
    pub task_id: String,
    #[ts(type = "number")]
    pub files_seen: u64,
    #[ts(type = "number")]
    pub recognized: u64,
    #[ts(type = "number")]
    pub new: u64,
    #[ts(type = "number")]
    pub updated: u64,
    #[ts(type = "number")]
    pub skipped: u64,
    #[ts(type = "number")]
    pub errors: u64,
    pub current_item: Option<String>,
    pub message: Option<String>,
}

/// 扫描 Channel 事件（envelope：operationId + 递增 sequence + RFC3339 at + kind + data）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct LibraryScanEvent {
    pub operation_id: String,
    pub sequence: u32,
    pub at: String,
    pub kind: ScanPhase,
    pub data: ScanEventData,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_fields_serialize_camel_case() {
        let card = WorkCardDto {
            work_id: "w-1".into(),
            title: "三体".into(),
            original_title: None,
            description: None,
            categories: vec![ContentCategory::Book],
            available_media_types: vec![MediaTypeDto::Book],
            poster_uri: Some("haven://artwork/1".into()),
            backdrop_uri: None,
            release_year: Some(2008),
            rating_value: None,
            rating_scale: None,
            favorite: false,
            progress: None,
            primary_action: None,
            external_ids: Vec::new(),
        };
        let json = serde_json::to_string(&card).unwrap();
        assert!(
            json.contains("\"workId\":\"w-1\""),
            "字段必须 camelCase: {json}"
        );
        assert!(json.contains("\"posterUri\":\"haven://artwork/1\""));
        assert!(json.contains("\"categories\":[\"book\"]"));
        assert!(json.contains("\"availableMediaTypes\":[\"book\"]"));
        assert!(json.contains("\"externalIds\":[]"), "{json}");
    }

    #[test]
    fn scan_event_serializes_snake_case_kind() {
        let event = LibraryScanEvent {
            operation_id: "op-1".into(),
            sequence: 3,
            at: "2026-08-13T08:30:00Z".into(),
            kind: ScanPhase::ItemIndexed,
            data: ScanEventData {
                task_id: "t-1".into(),
                files_seen: 10,
                recognized: 8,
                new: 2,
                updated: 0,
                skipped: 2,
                errors: 0,
                current_item: Some("movie.mkv".into()),
                message: None,
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"operationId\":\"op-1\""), "{json}");
        assert!(json.contains("\"kind\":\"item_indexed\""), "{json}");
        assert!(json.contains("\"currentItem\":\"movie.mkv\""), "{json}");
    }

    #[test]
    fn page_dto_roundtrip() {
        let page = PageDto::<WorkCardDto> {
            schema_version: 1,
            items: vec![],
            next_cursor: None,
            total: Some(0),
            revision: Some("r1".into()),
        };
        let json = serde_json::to_string(&page).unwrap();
        assert!(json.contains("\"schemaVersion\":1"), "{json}");
        assert!(json.contains("\"nextCursor\":null"), "{json}");
        let back: PageDto<WorkCardDto> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.total, Some(0));
    }

    #[test]
    fn locator_dto_uses_kind_and_data() {
        let loc = LocatorDto::Video(VideoLocatorDto {
            position_ms: 120_000,
        });
        let json = serde_json::to_string(&loc).unwrap();
        assert!(json.contains("\"kind\":\"video\""), "{json}");
        assert!(json.contains("\"data\":{\"positionMs\":120000}"), "{json}");
    }

    #[test]
    fn library_list_request_defaults() {
        let req = LibraryListRequest {
            category: QueryCategory::All,
            media_types: None,
            query: None,
            sort: LibraryListSort::RecentlyAdded,
            cursor: None,
            limit: 50,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"sort\":\"recently_added\""), "{json}");
    }

    #[test]
    fn comic_work_catalog_wire_preserves_backend_order_and_receipts() {
        let value = serde_json::to_value(ComicWorkChapterCatalogDto {
            schema_version: 1,
            work_id: "work-1".into(),
            current_media_item_id: Some("media-1".into()),
            editions: vec![ComicWorkEditionDto {
                edition_id: "edition-1".into(),
                profile: ComicEditionProfileDto {
                    language: Some("zh-Hans".into()),
                    language_kind: ComicEditionFacetKindDto::Known,
                    translation_line: None,
                    translation_line_kind: ComicEditionFacetKindDto::Unknown,
                    scan_group: Some("group-a".into()),
                    scan_group_kind: ComicScanGroupKindDto::ContentLine,
                    color_mode: ComicColorModeDto::FullColor,
                },
                display_label: "zh-Hans / group-a / color".into(),
                chapter_count: 1,
            }],
            chapters: vec![ComicWorkChapterDto {
                media_item_id: "media-1".into(),
                edition_id: "edition-1".into(),
                subject_id: None,
                chapter_number: Some(1.0),
                volume_number: None,
                title: Some("第一话".into()),
                published_at: None,
                page_count: Some(20),
                status: ComicChapterAggregateStatusDto::Available,
                can_open: true,
                sources: vec![],
                match_result: None,
                progress: None,
                previous_media_item_id: None,
                next_media_item_id: Some("media-2".into()),
                backend_order: 0,
            }],
            refresh_status: ComicWorkCatalogStatusDto::Synced,
            last_observed_at: Some("2026-09-12T00:00:00Z".into()),
            truncated: false,
            refresh_receipts: vec![ComicCatalogRefreshReceiptDto {
                refresh_id: "refresh-1".into(),
                work_id: "work-1".into(),
                source_id: "mangadex".into(),
                remote_work_id: "remote-work-1".into(),
                status: ComicCatalogRefreshOutcomeStatusDto::Succeeded,
                generation_before: 0,
                generation_after: Some(1),
                observed_from: Some("chapter-1".into()),
                observed_to: Some("chapter-1".into()),
                truncated: false,
                retained_previous_catalog: false,
                error_code: None,
                observed_at: "2026-09-12T00:00:00Z".into(),
            }],
        })
        .unwrap();

        assert_eq!(value["schemaVersion"], 1);
        assert_eq!(value["chapters"][0]["mediaItemId"], "media-1");
        assert_eq!(value["chapters"][0]["nextMediaItemId"], "media-2");
        assert_eq!(value["refreshReceipts"][0]["status"], "succeeded");
        assert!(value["chapters"][0].get("providerUrl").is_none());
    }

    #[test]
    fn comic_work_catalog_request_rejects_unknown_fields_and_preserves_local_ids() {
        let both = serde_json::json!({
            "workId": "work-1",
            "mediaItemId": "media-1"
        });
        let decoded = serde_json::from_value::<ComicWorkChapterCatalogRequestDto>(both).unwrap();
        assert_eq!(decoded.work_id.as_deref(), Some("work-1"));
        assert_eq!(decoded.media_item_id.as_deref(), Some("media-1"));

        let extra = serde_json::json!({
            "workId": "work-1",
            "pageUrl": "https://example.invalid/page"
        });
        assert!(serde_json::from_value::<ComicWorkChapterCatalogRequestDto>(extra).is_err());
    }

    #[test]
    fn comic_progress_migration_receipt_wire_contains_explainable_snapshots() {
        let receipt = ComicProgressMigrationReceiptDto {
            migration_id: "migration-1".into(),
            source_media_item_id: "source-1".into(),
            target_media_item_id: "target-1".into(),
            strategy: ComicPageMappingStrategyDto::ContentFingerprint,
            confidence: ComicPageMappingConfidenceDto::Medium,
            evidence: vec![ComicChapterEvidenceDto {
                kind: ComicChapterEvidenceKindDto::MatchingChapterMetadata,
                matched: None,
            }],
            source_progress_snapshot: Some(ComicProgressSnapshotDto {
                media_item_id: "source-1".into(),
                page_index: Some(2),
                page_progression: Some(0.5),
                completion: CompletionWire::InProgress,
                percentage: Some(0.5),
                revision: Some("revision-1".into()),
                updated_at: Some("2026-09-12T00:00:00Z".into()),
            }),
            target_progress_before: None,
            target_progress_after: None,
            page_mapping: ComicPageMigrationDto {
                target_page_index: Some(3),
                confidence: ComicPageMappingConfidenceDto::Medium,
                strategy: ComicPageMappingStrategyDto::ContentFingerprint,
                reversible: true,
            },
            algorithm_version: "comic-progress/v2".into(),
            created_at: "2026-09-12T00:00:00Z".into(),
            undoable: false,
            applied_revision: None,
        };
        let value = serde_json::to_value(&receipt).unwrap();
        for field in [
            "migrationId",
            "sourceMediaItemId",
            "targetMediaItemId",
            "strategy",
            "confidence",
            "evidence",
            "sourceProgressSnapshot",
            "targetProgressBefore",
            "targetProgressAfter",
            "pageMapping",
            "algorithmVersion",
            "createdAt",
            "undoable",
            "appliedRevision",
        ] {
            assert!(value.get(field).is_some(), "receipt 缺少字段 {field}");
        }
        assert!(value.get("locator").is_none());
        assert!(value.get("pageId").is_none());
        assert!(value.get("grant").is_none());
        assert!(value.get("url").is_none());
    }

    #[test]
    fn source_work_import_target_is_backward_compatible_and_local_only() {
        let request: SourceWorkImportRequest = serde_json::from_value(serde_json::json!({
            "operationId": "operation-1",
            "index": 0
        }))
        .unwrap();
        assert_eq!(request.target_work_id, None);

        let request_with_target: SourceWorkImportRequest =
            serde_json::from_value(serde_json::json!({
                "operationId": "operation-1",
                "index": 0,
                "targetWorkId": "work-1"
            }))
            .unwrap();
        assert_eq!(
            request_with_target.target_work_id.as_deref(),
            Some("work-1")
        );
    }
}

/// Continue 条目（首页 `home_get`；NOTE-7 契约定义）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ContinueItemDto {
    pub work_id: String,
    pub media_item_id: String,
    pub progress: ProgressSummaryDto,
    pub primary_action: PrimaryActionDto,
}

/// 内容架（`home_get` / `library_shelves`；NOTE-7 契约定义）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ShelfDto {
    pub shelf_id: String,
    pub title_key: String,
    pub preview: Vec<WorkCardDto>,
    pub view_more: Option<LibraryListRequest>,
}

/// 首页投影（`home_get`；NOTE-7 契约定义）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct HomeDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub continue_items: Vec<ContinueItemDto>,
    pub recently_added: Vec<WorkCardDto>,
    pub shelves: Vec<ShelfDto>,
}

/// `library_shelves` 顶层响应（C-05 冻结裁决：版本在顶层，不散落到每个 Shelf）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct LibraryShelvesDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    /// 空列表固定返回 `[]`，不返回 `null`。
    pub shelves: Vec<ShelfDto>,
}

/// Wire 错误（C-02 冻结：ErrorDto 进入单一生成源；前端依据 code 决定
/// 回滚/刷新/重试/导航；retryable 由后端错误分类）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ErrorDto {
    /// 稳定错误码（v1 Error Catalog，见契约 §13）。
    pub code: String,
    /// 可展示给用户的文案（前端不得拼后端内部消息）。
    pub user_message: String,
    /// true 时允许用户重试（平台暂时性错误），false 时重试无意义。
    pub retryable: bool,
}

/// `library.changed` 事件负载（C-03 冻结）。
/// 语义：只携带失效信号，不复制页面状态；完整数据由 Query 拉取。
/// 乱序/重复：sequence 单调递增（同一 operation 内），消费者按 sequence
/// 丢弃旧事件；重复投递幂等（revision 与本地比对，旧则忽略）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct LibraryChangedDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    /// RFC3339 投递时间。
    pub at: String,
    pub operation_id: String,
    /// 事件源内递增序号（乱序检测）。
    pub sequence: u32,
    /// 列表缓存失效用 revision（与 PageDto.revision 同源；`null` 表示全量刷新）。
    pub revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct WorkDetailCountsDto {
    pub editions: u32,
    pub relations: u32,
    pub available_resources: u32,
    pub active_downloads: u32,
    pub markers: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct WorkDetailHeaderDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub work_id: String,
    pub title: String,
    pub original_title: Option<String>,
    pub description: Option<String>,
    pub poster_uri: Option<String>,
    pub backdrop_uri: Option<String>,
    pub release_year: Option<i32>,
    #[serde(default)]
    #[ts(optional)]
    pub director: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub actor: Option<String>,
    pub categories: Vec<ContentCategory>,
    pub available_media_types: Vec<MediaTypeDto>,
    pub favorite: bool,
    pub primary_action: Option<PrimaryActionDto>,
    pub progress: Option<ProgressSummaryDto>,
    /// 元数据去重键投影（契约 §36.1）；无已知映射返回 []。
    pub external_ids: Vec<ExternalIdDto>,
    pub counts: WorkDetailCountsDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct WorkGetRequest {
    pub work_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct EditionListByWorkRequest {
    pub work_id: String,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct EditionGetRequest {
    pub edition_id: String,
}

/// 漫画章节目录的来源可用性。不可用章节也会进入只读目录投影，供刷新层
/// 区分暂时不可用、外部跳转和字段不完整，而不是被前端误认为已删除。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ComicChapterAvailabilityDto {
    Available,
    TemporarilyUnavailable,
    ExternalOnly,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ComicEditionFacetKindDto {
    Unknown,
    Known,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ComicScanGroupKindDto {
    Unknown,
    ContentLine,
    MirrorLabel,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ComicColorModeDto {
    Unknown,
    FullColor,
    Grayscale,
    Mixed,
}

/// 章节目录中用于展示 Edition 区分依据的安全画像。
///
/// 只投影已清洗的标签和语义 kind；不投影 URL、pageId、grant、请求头或
/// provider 内部页面定位。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicEditionProfileDto {
    pub language: Option<String>,
    pub language_kind: ComicEditionFacetKindDto,
    pub translation_line: Option<String>,
    pub translation_line_kind: ComicEditionFacetKindDto,
    pub scan_group: Option<String>,
    pub scan_group_kind: ComicScanGroupKindDto,
    pub color_mode: ComicColorModeDto,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicChapterCatalogItemDto {
    /// Provider 章节 ID 作为 opaque identity 返回；所有消费命令仍由后端复核。
    pub remote_chapter_id: String,
    pub chapter_number: Option<f64>,
    pub volume_number: Option<f64>,
    pub title: Option<String>,
    pub page_count: Option<u32>,
    pub published_at: Option<String>,
    pub updated_at: Option<String>,
    pub availability: ComicChapterAvailabilityDto,
    pub edition_profile: ComicEditionProfileDto,
}

/// 获取一个来源作品当前章节目录的只读请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicChapterCatalogGetRequest {
    pub source_id: String,
    pub remote_work_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicChapterCatalogDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub source_id: String,
    pub remote_work_id: String,
    pub fetched_at: String,
    pub total: Option<u32>,
    pub truncated: bool,
    pub chapters: Vec<ComicChapterCatalogItemDto>,
}

/// 已登记章节的来源状态。`Missing` 只表示最近一次完整刷新没有再次看到
/// 该章节；MediaItem、Progress、Marker、History 仍然保留。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ComicChapterSourceStatusDto {
    Available,
    TemporarilyUnavailable,
    ExternalOnly,
    Unknown,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicChapterCatalogRefreshStateDto {
    #[ts(type = "number")]
    pub generation: u64,
    pub fetched_at: String,
    pub total: Option<u32>,
    pub truncated: bool,
}

/// SQLite 中已经登记的章节安全投影。
///
/// 与 Provider 观察目录分开建模：这里返回 Haven `mediaItemId` 和已持久化的
/// 来源状态，供章节列表、换源和进度迁移使用；内部 Resource locator、URL、
/// pageId、grant、请求头和 authoritative content key 永不进入 Wire。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicRegisteredChapterCatalogItemDto {
    pub media_item_id: String,
    pub source_id: String,
    pub remote_work_id: String,
    pub remote_chapter_id: String,
    pub chapter_number: Option<f64>,
    pub volume_number: Option<f64>,
    pub title: Option<String>,
    pub page_count: Option<u32>,
    pub source_order: u32,
    pub availability: ComicChapterSourceStatusDto,
    pub published_at: Option<String>,
    pub source_updated_at: Option<String>,
    #[ts(type = "number | null")]
    pub last_seen_generation: Option<u64>,
    pub edition_profile: ComicEditionProfileDto,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicRegisteredChapterCatalogDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub source_id: String,
    pub remote_work_id: String,
    pub refresh_state: Option<ComicChapterCatalogRefreshStateDto>,
    pub chapters: Vec<ComicRegisteredChapterCatalogItemDto>,
}

/// Work 级漫画目录请求。这里只携带本地 Work/MediaItem 身份；二选一的业务
/// 约束由命令/Application 核心校验，serde 层只负责字段闭合和兼容反序列化。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicWorkChapterCatalogRequestDto {
    pub work_id: Option<String>,
    pub media_item_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ComicChapterAggregateStatusDto {
    Available,
    TemporarilyUnavailable,
    ExternalOnly,
    Unknown,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicWorkEditionDto {
    pub edition_id: String,
    pub profile: ComicEditionProfileDto,
    pub display_label: String,
    pub chapter_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicChapterSourceSummaryDto {
    pub source: ComicChapterSourceIdentityDto,
    pub status: ComicChapterSourceStatusDto,
    pub mirror_label: Option<String>,
    pub observed_at: Option<String>,
    pub source_order: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicWorkChapterDto {
    pub media_item_id: String,
    pub edition_id: String,
    pub subject_id: Option<String>,
    pub chapter_number: Option<f64>,
    pub volume_number: Option<f64>,
    pub title: Option<String>,
    pub published_at: Option<String>,
    pub page_count: Option<u32>,
    pub status: ComicChapterAggregateStatusDto,
    pub can_open: bool,
    pub sources: Vec<ComicChapterSourceSummaryDto>,
    pub match_result: Option<ComicChapterMatchDto>,
    pub progress: Option<ProgressSummaryDto>,
    pub previous_media_item_id: Option<String>,
    pub next_media_item_id: Option<String>,
    pub backend_order: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ComicCatalogRefreshOutcomeStatusDto {
    NeverSynced,
    Succeeded,
    TemporarilyUnavailable,
    ExternalOnly,
    Unknown,
    RefreshFailed,
    Truncated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicCatalogRefreshReceiptDto {
    pub refresh_id: String,
    pub work_id: String,
    pub source_id: String,
    pub remote_work_id: String,
    pub status: ComicCatalogRefreshOutcomeStatusDto,
    #[ts(type = "number")]
    pub generation_before: u64,
    #[ts(type = "number | null")]
    pub generation_after: Option<u64>,
    pub observed_from: Option<String>,
    pub observed_to: Option<String>,
    pub truncated: bool,
    pub retained_previous_catalog: bool,
    pub error_code: Option<String>,
    pub observed_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ComicWorkCatalogStatusDto {
    NeverSynced,
    Synced,
    RefreshFailed,
    Truncated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicWorkChapterCatalogDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub work_id: String,
    pub current_media_item_id: Option<String>,
    pub editions: Vec<ComicWorkEditionDto>,
    pub chapters: Vec<ComicWorkChapterDto>,
    pub refresh_status: ComicWorkCatalogStatusDto,
    pub last_observed_at: Option<String>,
    pub truncated: bool,
    pub refresh_receipts: Vec<ComicCatalogRefreshReceiptDto>,
}

/// 获取当前章节已经登记的其他来源候选。请求中的来源身份必须先在
/// SQLite 中解析，前端不能用它直接构造 Provider URL 或资源定位器。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicChapterSourceCandidatesGetRequestDto {
    pub source: ComicChapterSourceIdentityDto,
}

/// 一个来源章节候选的安全展示投影。`matchResult` 是后端比较出的证据，
/// 不允许前端自行以标题、章节号或页数写回数据库。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicChapterSourceCandidateDto {
    pub source: ComicChapterSourceIdentityDto,
    pub media_item_id: String,
    pub chapter_number: Option<f64>,
    pub volume_number: Option<f64>,
    pub title: Option<String>,
    pub page_count: Option<u32>,
    pub source_order: u32,
    pub availability: ComicChapterSourceStatusDto,
    pub published_at: Option<String>,
    pub source_updated_at: Option<String>,
    #[ts(type = "number | null")]
    pub last_seen_generation: Option<u64>,
    pub edition_profile: ComicEditionProfileDto,
    pub match_result: ComicChapterMatchDto,
}

/// 当前来源章节的候选集合。候选按后端证据排序，`truncated=true` 表示
/// Work 下的来源引用超过本次安全返回上限；它不是“没有更多章节”的判断。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicChapterSourceCandidatesDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub source: ComicChapterSourceIdentityDto,
    pub current_media_item_id: String,
    pub candidates: Vec<ComicChapterSourceCandidateDto>,
    pub truncated: bool,
}

/// 一个章节在某个来源上的 opaque 身份。来源/作品/章节 ID 由后端重新校验，
/// 不表示 URL、资源路径或运行时 pageId。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicChapterSourceIdentityDto {
    pub source_id: String,
    pub remote_work_id: String,
    pub remote_chapter_id: String,
}

/// 跨来源章节迁移请求。低置信度 `Suggested` 匹配只有在用户或应用策略明确
/// 允许最佳努力迁移时才会写入；目标已有进度默认保留，只有用户明确选择覆盖
/// 时才允许 `allowTargetOverwrite=true`；源/目标 revision 仍由后端在原子事务中校验。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicProgressMigrationRequestDto {
    pub source: ComicChapterSourceIdentityDto,
    pub target: ComicChapterSourceIdentityDto,
    /// 允许低置信度元数据匹配执行可撤销的最佳努力迁移。
    pub allow_best_effort: bool,
    /// 只有用户明确选择覆盖目标现有进度时才设为 true。
    pub allow_target_overwrite: bool,
}

/// 重新检查 owner-bound Comic Session 的页面序列并迁移进度。
///
/// 页面身份必须由后端重新 inspect 生成；前端不能提交旧/新页面身份，
/// 也不能把运行时 pageId、grant、URL 或路径伪装成迁移证据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicPageProgressRemapRequestDto {
    pub session_id: String,
    pub expected_revision: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ComicProgressMigrationStatusDto {
    Unchanged,
    NotApplicable,
    Applied,
    SharedContent,
    Suggested,
    NoSourceProgress,
    TargetProgressPreserved,
    NoTargetPage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ComicChapterMatchKindDto {
    SameRemoteChapter,
    SameContent,
    SameLogicalChapterVariant,
    Candidate,
    Unrelated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ComicMatchConfidenceDto {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ComicProgressMigrationModeDto {
    Shared,
    OneTime,
    Suggested,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ComicChapterEvidenceKindDto {
    SameRemoteIdentity,
    AuthoritativeContentKey,
    ConflictingAuthoritativeContentKey,
    EditionCompatible,
    EditionConflict,
    ExactPageIdentity,
    PartialPageIdentity,
    MatchingChapterMetadata,
    WeakChapterMetadata,
}

/// 证据保持为闭合 kind + 可选计数，避免把 Domain enum 的内部表示直接暴露给 IPC。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicChapterEvidenceDto {
    pub kind: ComicChapterEvidenceKindDto,
    pub matched: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicChapterMatchDto {
    pub kind: ComicChapterMatchKindDto,
    pub confidence: ComicMatchConfidenceDto,
    pub progress_migration: ComicProgressMigrationModeDto,
    pub evidence: Vec<ComicChapterEvidenceDto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ComicPageMappingConfidenceDto {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ComicPageMappingStrategyDto {
    StableKey,
    ContentFingerprint,
    ReorderedAnchor,
    NearestSurvivingPage,
    ProportionalFallback,
    NoTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicPageMigrationDto {
    pub target_page_index: Option<u32>,
    pub confidence: ComicPageMappingConfidenceDto,
    pub strategy: ComicPageMappingStrategyDto,
    pub reversible: bool,
}

/// 章节换源/页面重定位的统一结果。低置信度自动迁移也必须带 snapshotId，
/// 以便用户在目标进度未被后续写入时撤销。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicProgressMigrationResultDto {
    pub status: ComicProgressMigrationStatusDto,
    pub match_result: Option<ComicChapterMatchDto>,
    pub page_migration: ComicPageMigrationDto,
    pub snapshot_id: Option<String>,
    pub applied_revision: Option<String>,
    pub receipt: ComicProgressMigrationReceiptDto,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicProgressSnapshotDto {
    pub media_item_id: String,
    pub page_index: Option<u32>,
    pub page_progression: Option<f64>,
    pub completion: CompletionWire,
    pub percentage: Option<f64>,
    pub revision: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicProgressMigrationReceiptDto {
    pub migration_id: String,
    pub source_media_item_id: String,
    pub target_media_item_id: String,
    pub strategy: ComicPageMappingStrategyDto,
    pub confidence: ComicPageMappingConfidenceDto,
    pub evidence: Vec<ComicChapterEvidenceDto>,
    pub source_progress_snapshot: Option<ComicProgressSnapshotDto>,
    pub target_progress_before: Option<ComicProgressSnapshotDto>,
    pub target_progress_after: Option<ComicProgressSnapshotDto>,
    pub page_mapping: ComicPageMigrationDto,
    pub algorithm_version: String,
    pub created_at: String,
    pub undoable: bool,
    pub applied_revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicProgressMigrationRevertRequestDto {
    pub migration_id: String,
    pub expected_applied_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ComicProgressMigrationRevertResultDto {
    pub reverted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct EditionAvailabilityDto {
    pub available: u32,
    pub offline_available: u32,
    pub unavailable: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct EditionSummaryDto {
    pub edition_id: String,
    pub work_id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub media_type: MediaTypeDto,
    pub release_date: Option<String>,
    pub language: Option<String>,
    pub region: Option<String>,
    pub media_item_count: u32,
    pub availability: EditionAvailabilityDto,
    pub progress: Option<ProgressSummaryDto>,
    pub primary_action: Option<PrimaryActionDto>,
    pub download: Option<String>,
}

/// Edition 详情页的真实消费单元投影。内部路径、Resource 和数据库 Row 不进入 Wire。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct EditionDetailDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub edition_id: String,
    pub work_id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub media_type: MediaTypeDto,
    pub release_date: Option<String>,
    pub language: Option<String>,
    pub region: Option<String>,
    pub publisher_or_studio: Option<String>,
    pub description: Option<String>,
    pub items: Vec<MediaItemSummaryDto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum MediaItemStatusDto {
    Available,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct MediaItemSummaryDto {
    pub media_item_id: String,
    pub edition_id: String,
    pub title: String,
    pub media_type: MediaTypeDto,
    pub index_label: String,
    #[ts(type = "number | null")]
    pub duration_ms: Option<u64>,
    pub page_count: Option<u32>,
    pub chapter_count: Option<u32>,
    pub published_at: Option<String>,
    pub status: MediaItemStatusDto,
    pub available_resource_count: u32,
    /// 季/集号投影（契约 §36.6）；无季集语义时为 null。后端从解析规则推导。
    pub season_number: Option<i32>,
    pub episode_number: Option<i32>,
    pub progress: Option<ProgressSummaryDto>,
    pub primary_action: Option<PrimaryActionDto>,
}

/// Resource 类型的安全 Wire 值。定位内容（本地路径、远程 URL、凭据）永不出现在 DTO。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ResourceTypeDto {
    LocalFile,
    CloudFile,
    HttpFile,
    VideoStream,
    HlsStream,
    DashStream,
    PublicationFile,
    ComicArchive,
    ImageSequence,
    ArticleSnapshot,
    RemoteChapter,
    RemotePageSet,
    /// 在线流（契约 §36.4）；原始 URL 不进 IPC，播放走 session 受控代理 URI。
    RemoteStream,
}

/// Resource 可用性摘要；后端是唯一事实来源，前端不得自行推断。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum AvailabilityDto {
    Available,
    OfflineAvailable,
    TemporarilyUnavailable,
    SourceUnavailable,
    StorageUnavailable,
    Missing,
    Unknown,
}

/// `resource_list_by_media_item` 的安全摘要。
/// 该类型明确不包含 locator、root path、下载 URL、签名 URL 或 credential ref。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ResourceSummaryDto {
    pub resource_id: String,
    pub resource_type: ResourceTypeDto,
    pub availability: AvailabilityDto,
    pub mime_type: Option<String>,
    #[ts(type = "number | null")]
    pub size: Option<u64>,
    pub storage_display_name: Option<String>,
    pub source_display_name: Option<String>,
    pub is_offline: bool,
    pub is_local: bool,
    pub requires_reauthorization: bool,
    /// 后端根据资源定位、来源能力和存储状态计算的下载能力。
    /// 前端不得通过 `isLocal` 或资源类型自行推断。
    pub can_download: bool,
    /// 后端根据资源定位、来源能力和可用性计算的在线打开能力。
    /// 该字段不暴露远端 URL、SourceObject 或路径。
    pub can_online_read: bool,
    /// 在线流种类（契约 §36.4）；仅 remote_stream 资源非 null，其余恒 null。
    pub stream_kind: Option<StreamKindDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ResourceListByMediaItemRequest {
    pub media_item_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ResourceListDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub items: Vec<ResourceSummaryDto>,
}

/// 下载任务状态（与 Domain DownloadState 一一对应；Finalizing 等未纳入 v0.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum DownloadStateDto {
    Queued,
    Resolving,
    Downloading,
    Paused,
    Verifying,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct DownloadTaskDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub task_id: String,
    pub work_id: Option<String>,
    pub edition_id: Option<String>,
    pub media_item_id: Option<String>,
    pub source_resource_id: String,
    pub target_storage_id: String,
    pub offline_resource_id: Option<String>,
    pub title: String,
    pub media_type: MediaTypeDto,
    pub category: ContentCategory,
    pub poster_uri: Option<String>,
    pub state: DownloadStateDto,
    #[ts(type = "number | null")]
    pub bytes_total: Option<u64>,
    #[ts(type = "number")]
    pub bytes_downloaded: u64,
    #[ts(type = "number | null")]
    pub speed_bps: Option<u64>,
    #[ts(type = "number | null")]
    pub eta_seconds: Option<u64>,
    #[ts(type = "number | null")]
    pub progress_ratio: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct DownloadCreateRequest {
    pub source_resource_id: String,
    pub target_storage_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct DownloadListRequest {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct DownloadTaskActionRequest {
    pub task_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct DownloadMutationResultDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub task_id: String,
    pub record_removed: bool,
    pub offline_resource_removed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct DownloadRevealResultDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub task_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum DownloadEventKind {
    Updated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct DownloadEventData {
    pub task_id: String,
    pub state: DownloadStateDto,
    pub offline_resource_id: Option<String>,
    #[ts(type = "number | null")]
    pub bytes_total: Option<u64>,
    #[ts(type = "number")]
    pub bytes_downloaded: u64,
    #[ts(type = "number | null")]
    pub speed_bps: Option<u64>,
    #[ts(type = "number | null")]
    pub eta_seconds: Option<u64>,
    /// 终态失败/中断的稳定错误码；普通进度事件为 null。
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct DownloadEvent {
    pub operation_id: String,
    pub sequence: u32,
    pub at: String,
    pub kind: DownloadEventKind,
    pub data: DownloadEventData,
}

/// `favorite.changed` 事件负载（C-03 / R-FAV-001 冻结）。
/// revision 与 `FavoriteSetResult.revision` 同源（状态版本）；重复设置相同状态
/// 不发布本事件，因此事件携带的 revision 恒非空。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct FavoriteChangedDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub at: String,
    pub operation_id: String,
    pub sequence: u32,
    pub work_id: String,
    pub favorite: bool,
    pub revision: String,
}

// ---------- v0.2 契约冻结（契约 §36；CONTRACT-V02-*，2026-08-22） ----------

/// 第三方元数据 Provider（契约 §36.1 闭合枚举；扩展需契约修订）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ExternalIdProviderDto {
    Tmdb,
    Bangumi,
    Anilist,
    Tvmaze,
    Gutenberg,
    Openlibrary,
}

/// 元数据去重键投影（契约 §36.1）。`(provider, externalId)` 在同一 Work 内唯一；
/// Enrichment 匹配成功才写入，失败不产生占位条目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ExternalIdDto {
    pub provider: ExternalIdProviderDto,
    pub external_id: String,
}

/// 来源能力种类（capability 投影；契约 §36.2）。
///
/// 这些值描述用户可以在栖阅中执行的动作，而不是 Provider 的内部实现。
/// `search` 表示可以返回真实搜索结果，`online_read` 表示可以创建受控的
/// 在线 Session，`offline_download` 表示用户明确点击下载后可以生成离线资源。
/// 导入不会因为 `offline_download` 而隐式落盘。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum SourceKindDto {
    Search,
    OnlineRead,
    OfflineDownload,
}

/// 来源面向用户的内容分类。该分类只用于设置页分组和来源筛选，
/// 不替代搜索/媒体领域中的 `ContentCategory`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum SourceCategoryDto {
    Video,
    Book,
    Comic,
    Periodical,
}

/// 来源配置边界：一个入口可聚合多个上游，或一个入口对应一个 Provider/目录。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum SourceModeDto {
    Single,
    Collection,
}

/// 来源健康（后端探测事实；出厂 unknown，契约 §36.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum SourceHealthDto {
    Unknown,
    Ok,
    Degraded,
    Down,
}

/// 来源描述符。端点 URL、凭据与端点内容禁止进入本投影（契约 §36.2）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SourceDescriptorDto {
    /// 稳定来源 ID（内置静态目录定义，前端不得自造）。
    pub source_id: String,
    /// 安全显示名。
    pub display_name: String,
    pub kinds: Vec<SourceKindDto>,
    /// 面向用户的内容分类（影视、图书、漫画、报刊文章）。
    pub categories: Vec<SourceCategoryDto>,
    /// 单一来源或聚合来源。
    pub mode: SourceModeDto,
    /// 由内置来源清单维护的说明；自定义来源使用固定安全文案。
    pub notes: String,
    pub enabled: bool,
    pub health: SourceHealthDto,
    /// 用户自填端点（CMS10/M3U）是否已配置；端点本身不出 IPC。
    pub endpoint_configured: bool,
    /// 健康探测：最后检测时间（RFC3339），无探测为 null。
    pub last_checked: Option<String>,
    /// 最后一次探测延迟（毫秒），无探测为 null。
    #[ts(type = "number | null")]
    pub latency_ms: Option<u64>,
    /// 滚动成功率（0.0-1.0），无数据为 null。
    #[ts(type = "number | null")]
    pub success_rate: Option<f64>,
}

/// `source_registry_list` 顶层响应（schemaVersion 2，契约 §36.2）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SourceRegistryDto {
    #[ts(type = "2")]
    pub schema_version: u32,
    /// 内置目录固定非空集合；空目录返回 []。
    pub sources: Vec<SourceDescriptorDto>,
}

/// `source_registry_set` 请求（幂等；未知 sourceId → INVALID_ARGUMENT）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SourceRegistrySetRequest {
    pub source_id: String,
    pub enabled: bool,
}

/// `source_registry_set` 结果（重复设置同值返回同结果）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SourceRegistrySetResult {
    pub source_id: String,
    pub enabled: bool,
}

/// `source_registry_set_endpoint` 请求（V2-B 实战批次增量；契约 §36.2 演进）。
/// 端点只写后端持久化（settings KV），永不回传 IPC。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SourceEndpointSetRequest {
    pub source_id: String,
    /// 仅接受 http/https 绝对 URL；写入前校验，响应不含端点本身。
    pub endpoint: String,
}

/// `source_registry_set_endpoint` 结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SourceEndpointSetResult {
    pub source_id: String,
    pub endpoint_configured: bool,
}

/// `source_add`：新增自定义 OPDS 书源（V2-H 收尾批次）。
/// 凭据不随本请求提交；先 add 再 `source_set_credential`（secret 单独走 keyring）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SourceAddRequest {
    pub display_name: String,
    /// OPDS 根目录 URL（http/https 绝对地址）；响应不含端点本身。
    pub endpoint: String,
}

/// `source_add` 结果：返回稳定 sourceId（`custom_` 前缀），端点不出 IPC。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SourceAddResult {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub source_id: String,
}

/// `source_update`：修改自定义源显示名与/或端点。null = 不变。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SourceUpdateRequest {
    pub source_id: String,
    pub display_name: Option<String>,
    /// null = 不变；空串非法（用 `source_registry_set_endpoint` 语义清空不适用，此处必须给出有效端点）。
    pub endpoint: Option<String>,
}

/// `source_update` 幂等结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SourceUpdateResult {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub source_id: String,
}

/// `source_remove`：删除自定义源（ADR-001 删除顺序：先删系统凭据再清持久化引用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SourceRemoveRequest {
    pub source_id: String,
}

/// `source_remove` 幂等结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SourceRemoveResult {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub source_id: String,
    /// true = 系统凭据实际删除；false = 本无凭据或已不存在。
    pub credential_deleted: bool,
}

/// `source_set_credential`：写入/清除自定义源凭据。secret 只进系统 keyring；
/// 持久化仅存 credential_ref；secret 与 target 禁止出 IPC、日志。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SourceSetCredentialRequest {
    pub source_id: String,
    /// null = 清除凭据；Some(空串) 非法 → INVALID_ARGUMENT。
    pub secret: Option<String>,
}

/// `source_work_import` 请求：导入搜索候选（operationId + 序号定位服务端缓存）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SourceWorkImportRequest {
    pub operation_id: String,
    pub index: u32,
    /// null/缺省保持“导入为新 Work”；非 null 只能是本地 WorkId。
    #[serde(default)]
    #[ts(optional)]
    pub target_work_id: Option<String>,
}

/// `source_work_import` 结果：入库后的真实身份（可路由播放）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SourceWorkImportResult {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub work_id: String,
    /// 首个可消费媒体条目（直接可用于 session/stream open）。
    pub media_item_id: String,
}

/// 渐进式来源搜索请求（契约 §36.3）。query 去空白后为空 → INVALID_ARGUMENT。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SearchSourceStartRequest {
    pub query: String,
    /// null = 全部分类。
    pub category: Option<QueryCategory>,
    /// null = 后端默认上限；>50 → INVALID_ARGUMENT。
    pub limit_per_source: Option<u32>,
}

/// `search_source_start` 成功响应（R-C02 同款幂等语义：alreadyRunning 合并既有任务）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SearchStartResultDto {
    pub operation_id: String,
    pub task_id: String,
    pub already_running: bool,
}

/// 搜索 Channel 事件种类（契约 §36.3）。Terminal 只能出现一次：
/// completed | cancelled | failed。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum SearchSourceEventKind {
    Started,
    SourceResult,
    Warning,
    Completed,
    Cancelled,
    Failed,
}

/// 搜索 Channel 事件负载。sourceId 仅 source_result/warning 非空；
/// works 仅 source_result 非空；code 仅 warning/failed 非空。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SearchSourceEventData {
    pub source_id: Option<String>,
    pub works: Vec<WorkCardDto>,
    /// warning/failed 携带稳定错误码（v1 Error Catalog），其余 null。
    pub code: Option<String>,
    /// warning 携带安全用户文案（不含完整 URL/路径/Provider Body），其余 null
    /// （V2-H 收尾批次；来源健康度明细）。
    #[serde(default)]
    pub message: Option<String>,
}

/// `search.source` Channel 事件（envelope 与 §10.3 一致；sequence 从 1 严格递增）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SearchSourceEvent {
    pub operation_id: String,
    pub sequence: u32,
    pub at: String,
    pub kind: SearchSourceEventKind,
    pub data: SearchSourceEventData,
}

/// `search_source_cancel` 请求（幂等；未知 operationId → RESOURCE_NOT_FOUND）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SearchSourceCancelRequest {
    pub operation_id: String,
}

/// `search_source_cancel` 幂等结果；已终态任务返回真实状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct SearchSourceCancelResultDto {
    pub operation_id: String,
    pub already_terminal: bool,
}

/// 在线流种类（契约 §36.4）；仅 remote_stream 资源非 null。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum StreamKindDto {
    Hls,
    Direct,
}

/// 凭据 Provider（契约 §36.5 闭合枚举）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum CredentialProviderDto {
    Webdav,
    /// OPDS 自定义书源凭据（V2-H 收尾批次；profile 段为 sourceId）。
    Opds,
    /// AI Provider Profile 凭据（`haven:ai:<profile-id>`）。只用于受控的
    /// status/set/delete；任何响应都不回显 secret。
    Ai,
}

impl CredentialProviderDto {
    /// CredentialStore scoped target 的 provider 段（ADR-001 校验规则内）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Webdav => "webdav",
            Self::Opds => "opds",
            Self::Ai => "ai",
        }
    }
}

/// `credential_status` 请求；profileId=null 表示默认 profile（"default"）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CredentialStatusRequest {
    pub provider: CredentialProviderDto,
    pub profile_id: Option<String>,
}

/// 凭据状态投影（契约 §36.5）。secret/credentialRef/target 名禁止出 IPC；
/// 凭据存储不提供写入时间时 updatedAt 为 null。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CredentialStatusDto {
    pub configured: bool,
    pub updated_at: Option<String>,
}

/// `credential_set` 请求。Secret 单向写入 Windows Credential Store，
/// 幂等覆盖；任何响应/事件/日志不得回显。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CredentialSetRequest {
    pub provider: CredentialProviderDto,
    pub profile_id: Option<String>,
    pub secret: String,
}

/// `credential_delete` 请求。幂等；不存在视为成功。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CredentialDeleteRequest {
    pub provider: CredentialProviderDto,
    pub profile_id: Option<String>,
}

/// Locator 类型投影（契约 §36.7；不含 Locator 明细）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum LocatorKindDto {
    Video,
    Book,
    Pdf,
    Comic,
    Article,
}

impl LocatorKindDto {
    /// Domain Locator → Wire 类型投影。Generic 是内部回退种类，不穿 IPC；
    /// 聚合时跳过该类进度记录。
    pub fn try_from_domain(value: &haven_domain::locator::Locator) -> Option<Self> {
        match value {
            haven_domain::locator::Locator::Video(_) => Some(Self::Video),
            haven_domain::locator::Locator::Book(_) => Some(Self::Book),
            haven_domain::locator::Locator::Pdf(_) => Some(Self::Pdf),
            haven_domain::locator::Locator::Comic(_) => Some(Self::Comic),
            haven_domain::locator::Locator::Article(_) => Some(Self::Article),
            haven_domain::locator::Locator::Generic(_) => None,
        }
    }
}

/// `media_state_get` 请求（契约 §36.7）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct MediaStateGetRequest {
    pub work_id: String,
}

/// Work 级进度摘要投影。locatorKind 是类型投影，明细由 progress_recent 提供。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct MediaStateProgressDto {
    pub edition_id: String,
    pub media_item_id: String,
    pub locator_kind: LocatorKindDto,
    pub updated_at: String,
}

/// 历史聚合摘要（契约 §36.7）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct HistorySummaryDto {
    pub last_opened_at: String,
    #[ts(type = "number")]
    pub open_count: u64,
}

/// Work 级用户状态聚合投影（schemaVersion 2，契约 §36.7）。
/// Application 层一次拼装 favorite/progress/history/marker 四表。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct MediaStateDto {
    #[ts(type = "2")]
    pub schema_version: u32,
    pub work_id: String,
    pub favorite: bool,
    pub progress: Option<MediaStateProgressDto>,
    pub history_summary: Option<HistorySummaryDto>,
    #[ts(type = "number")]
    pub marker_count: u64,
    /// v0.2 预留位（§36.9）：恒为 null；未来豆瓣式五星时以契约修订落地。
    #[ts(type = "null")]
    pub rating: (),
}

/// Enrichment 状态（契约 §36.8 闭合枚举）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum EnrichmentStatusWire {
    Pending,
    Enriched,
    Failed,
}

/// `enrichment_status` 请求；workId=null 返回全部记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct EnrichmentStatusRequest {
    pub work_id: Option<String>,
}

/// 单条 enrichment 记录投影。error 为安全文案，不含内部路径/远端响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct EnrichmentStateDto {
    pub work_id: String,
    pub status: EnrichmentStatusWire,
    pub source_id: Option<String>,
    pub error: Option<String>,
}

/// `metadata.changed` 事件负载（复用既有事件名；契约 §36.8）。
/// 只携带失效信号，完整投影由 Query 拉取。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct MetadataChangedDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub at: String,
    pub operation_id: String,
    pub sequence: u32,
    pub work_id: String,
    pub status: EnrichmentStatusWire,
    pub source_id: Option<String>,
    pub error: Option<String>,
}

// ---------- v0.2 热榜与投屏契约（2026-08-23; T3 + v02-cast-001） ----------

/// 热榜单项投影。生产海报只能走受控 `haven://artwork/*` 代理；缺失时为 null。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct TrendingItemDto {
    pub title: String,
    pub subtitle: String,
    pub description: String,
    /// 受控海报 URI（`haven://artwork/<id>`）；为 null 时前端用本地占位。
    pub poster_uri: Option<String>,
    pub status_badge: Option<String>,
}

/// 热榜看板投影（4 榜：动漫/国产剧/综艺/英美剧）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct TrendingBoardDto {
    pub board_id: String,
    pub title: String,
    pub subtitle: String,
    pub items: Vec<TrendingItemDto>,
}

/// `trending_boards_get` 响应（schemaVersion 1，空榜返回 []）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct TrendingBoardsDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub boards: Vec<TrendingBoardDto>,
}

// ---------- v0.2 About / Diagnostics（V02-SETTINGS-ABOUT-DIAGNOSTICS-008） ----------

/// 受控应用目录。前端只能选择这三个固定范围，不能提交任意路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum AppDirectoryKindDto {
    Data,
    Logs,
    Cache,
}

/// About 页面显示的目录投影。`displayPath` 只允许脱敏的逻辑路径，不能包含用户目录名。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AppDirectoryDto {
    pub kind: AppDirectoryKindDto,
    pub display_name: String,
    pub display_path: String,
    pub exists: bool,
    pub can_open: bool,
}

/// Third-party notice 的最小可读投影；完整正文仍由构建时登记的仓库清单拥有。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ThirdPartyNoticeDto {
    pub name: String,
    pub license: String,
}

/// About/Diagnostics 查询。只返回构建和脱敏运行时信息，不返回 Secret、完整路径或用户内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AppInfoDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub app_version: String,
    pub build_channel: String,
    pub source_pack_version: Option<String>,
    pub protocol_version: String,
    pub database_version: String,
    pub app_license: Option<String>,
    pub third_party_notices: Vec<ThirdPartyNoticeDto>,
    pub directories: Vec<AppDirectoryDto>,
}

// ---------- v0.2 安全错误诊断报告（V02-OPEN-SOURCE-DIAGNOSTICS-001） ----------

/// 诊断报告收集等级。等级只扩大脱敏后的运行时摘要，不会包含用户内容、凭据
/// 或完整路径；详细日志必须由后端先经过脱敏检查。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ErrorReportLevelDto {
    Basic,
    Standard,
    Detailed,
}

/// 脱敏检查的明确结果。导出和打开 Issue 前必须为 Passed。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ErrorReportRedactionStatusDto {
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ErrorReportRedactionDto {
    pub status: ErrorReportRedactionStatusDto,
    /// 只列出被移除的敏感类别，不列出原文或路径。
    pub removed_fields: Vec<String>,
    pub contains_sensitive_data: bool,
}

/// 标准/详细等级额外显示的安全运行上下文。diagnosticLines 只允许稳定错误摘要，
/// 不承载日志正文、URL 或本地路径。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ErrorReportDetailsDto {
    pub protocol_version: Option<String>,
    pub database_version: Option<String>,
    pub source_pack_version: Option<String>,
    pub diagnostic_lines: Vec<String>,
}

/// 预览与导出共用的脱敏报告投影。该 DTO 可安全展示在设置页，不能反向推导
/// 数据库路径、媒体内容、搜索词、Cookie 或任何 Secret。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ErrorReportPreviewDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub report_id: String,
    pub level: ErrorReportLevelDto,
    pub created_at: String,
    pub app_version: String,
    pub operating_system: String,
    pub runtime_mode: String,
    pub stable_error_codes: Vec<String>,
    pub error_summary: String,
    pub redaction: ErrorReportRedactionDto,
    pub details: Option<ErrorReportDetailsDto>,
    pub requires_confirmation: bool,
}

/// 预览请求只允许选择等级并带入有限的稳定错误码；错误码由 Application
/// 严格校验为大写标识符，前端不能传递消息正文、URL、路径或 Headers。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export, rename_all = "camelCase")]
pub struct ErrorReportPreviewRequest {
    pub level: ErrorReportLevelDto,
    pub stable_error_codes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export, rename_all = "camelCase")]
pub struct ErrorReportConfirmRequest {
    pub report_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ErrorReportConfirmResultDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub report_id: String,
    pub confirmed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum ErrorReportActionStatusDto {
    Exported,
    Opened,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct ErrorReportActionResultDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub report_id: String,
    pub status: ErrorReportActionStatusDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export, rename_all = "camelCase")]
pub struct ErrorReportActionRequest {
    pub report_id: String,
}

/// 可清理的技术缓存范围；业务事实、离线资源和原始媒体不在此枚举内。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export, rename_all = "kebab-case")]
pub enum CacheScopeDto {
    Artwork,
    Thumbnails,
    ProviderResponseCache,
}

/// `cache_clear` 结果；仅返回清理范围和条目数量，不返回路径。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CacheClearResultDto {
    pub scope: CacheScopeDto,
    pub removed_entries: u64,
}

// ---------- v0.2 受控视频截图（V02-PLAYBACK-HARDWARE-SCREENSHOT-001） ----------

/// `video_screenshot_begin` 返回的有界上传能力；不包含路径或图片数据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct VideoScreenshotBeginResultDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub upload_id: String,
    #[ts(type = "number")]
    pub max_chunk_bytes: u32,
    #[ts(type = "number")]
    pub max_total_bytes: u64,
}

/// 单个截图分块。Command/Application 会再次校验大小、归属和 sequence。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct VideoScreenshotChunkRequest {
    pub upload_id: String,
    #[ts(type = "number")]
    pub sequence: u32,
    pub bytes: Vec<u8>,
}

/// 用户关闭保存对话框时以 `cancelled` 结果返回，不作为错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum VideoScreenshotStatusDto {
    Saved,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct VideoScreenshotResultDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub status: VideoScreenshotStatusDto,
}

/// 投屏协议（双栈同批交付）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum CastProtocolDto {
    Dlna,
    Chromecast,
}

/// 投屏设备投影（仅含发现与控制所需最小信息；控制 URL 不出 IPC）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CastDeviceDto {
    pub device_id: String,
    pub friendly_name: String,
    pub ip: String,
    pub protocol: CastProtocolDto,
    pub model_name: Option<String>,
}

/// `cast_discover` 请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CastDiscoverRequest {
    /// 超时毫秒（null → 默认 5000；>10000 → INVALID_ARGUMENT）。
    pub timeout_ms: Option<u32>,
}

/// `cast_discover` 结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CastDiscoverResult {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub devices: Vec<CastDeviceDto>,
}

/// `cast_play` 请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CastPlayRequest {
    pub media_item_id: String,
    pub device_id: String,
    pub engine: SessionEngineDto,
}

/// `cast_play` 结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CastPlayResult {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub cast_session_id: String,
    pub lan_url: String,
    pub device_name: String,
}

/// 投屏传输状态（与 AVTransport / CastV2 映射）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum CastTransportStateDto {
    Playing,
    Paused,
    Stopped,
    NoMedia,
    Transitioning,
    Unknown,
}

/// `cast_status` 请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CastStatusRequest {
    pub cast_session_id: String,
}

/// `cast_status` 结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CastStatusDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub transport_state: CastTransportStateDto,
    #[ts(type = "number | null")]
    pub position_ms: Option<u64>,
    #[ts(type = "number | null")]
    pub duration_ms: Option<u64>,
}

/// `cast_stop` 请求（幂等）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CastStopRequest {
    pub cast_session_id: String,
}

/// `cast_stop` 结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct CastStopResult {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub stopped: bool,
}

// ── 报刊层级（`ContentCategory::Periodical`）─────────────────────────────────
//
// 只读查询：期刊 → 卷 → 期 → 文章。字段只包含稳定的身份与展示/归属事实：
// - 身份：Haven ID（字符串 UUID）与 ISSN（print / electronic 是两个独立身份）；
// - 展示/归属：标题、出版方、卷/期标签与序号、发行日期、DOI、页码、来源 opaque 身份。
//
// **绝不输出** URL、Cookie、请求头、本地路径、Provider 原始响应或签名资源地址。
// 文章正文与资源地址只能经既有 Resource / Session 链路获取。
//
// 列表顺序 = Repository 返回顺序（卷、期按 ordinal，文章按 ordinal）；
// 前端不得自行重排或推断层级归属。
//
// 反序列化是闭合的：这一组 DTO（含请求）都带 `deny_unknown_fields`，
// 多一个字段即整体失败而不是被静默忽略。字段重命名是精确匹配，
// snake_case 与大小写变体不构成同一个字段。
// 说明只写在这里：ts-rs 会把 `///` 文档注释复制进生成物 wire.ts，
// 在 `//` 注释里说明才不会造成生成绑定漂移。

/// `periodical_tree_get` 请求：按 Work 读取该作品的完整期刊层级。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct PeriodicalTreeGetRequest {
    pub work_id: String,
}

/// 页码区间。报刊页码允许非连续与不规则形态（`e12345`、`S1-S5`），
/// 因此保留来源文本而不是解析成整数；单页时 `end` 为 null。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct PageRangeDto {
    pub start: String,
    pub end: Option<String>,
}

/// 期刊身份与展示事实。`workId` 是期刊在 Haven 中的 Work 归属。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct PeriodicalDto {
    pub id: String,
    pub work_id: String,
    pub title: String,
    /// ISSN（印刷版）；来源未给出时为 null，不与 electronic 互相推断。
    pub issn_print: Option<String>,
    /// ISSN（电子版）。
    pub issn_electronic: Option<String>,
    pub publisher: Option<String>,
}

/// 文章正文可用性：Provider 对「这篇正文是否可读」的观察（闭合三态）。
///
/// 这不是本地阅读能力：`fullText` 只说明来源声明有正文，`metadataOnly` 只说明
/// 来源只有元数据，`unknown` 说明没有观察到。页面据此区分三种情况，但**不得**
/// 用它阻止用户打开自己已经拥有的在线或离线内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum PeriodicalArticleAvailabilityDto {
    FullText,
    MetadataOnly,
    Unknown,
}

/// 期刊文章。`mediaItemId` 绑定既有 MediaItem，阅读/进度/资源链路由既有契约承担。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct PeriodicalArticleDto {
    pub id: String,
    pub issue_id: String,
    pub media_item_id: String,
    /// 期内文章序号；来源未给出时为 null，后端不猜测。
    pub ordinal: Option<u32>,
    pub title: String,
    /// DOI 文本（不是可解析 URL）。
    pub doi: Option<String>,
    pub page_range: Option<PageRangeDto>,
    /// 来源 key（如 `europepmc`）与来源侧 opaque 文章 ID：只用于幂等与归属核对。
    pub source_key: String,
    pub remote_article_id: String,
    /// Provider 的正文观察。页面用它区分「仅有元数据」与「没有观察到」；
    /// 真实可读路径由既有 Resource/Session 能力决定，不受此字段限制。
    pub availability: PeriodicalArticleAvailabilityDto,
}

/// 期刊期号；`label` 保留不规则期号原文（`3-4`、`Suppl 2`、`Spring`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct PeriodicalIssueDto {
    pub id: String,
    pub volume_id: String,
    pub label: Option<String>,
    pub number: Option<f64>,
    /// 发行日期（来源精度：`YYYY` / `YYYY-MM` / `YYYY-MM-DD`）。
    pub publication_date: Option<String>,
    pub ordinal: u32,
    pub articles: Vec<PeriodicalArticleDto>,
}

/// 期刊卷；`label` 保留来源原文（例如 `Suppl 1`），`number` 仅在可解析时存在。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct PeriodicalVolumeDto {
    pub id: String,
    pub periodical_id: String,
    pub label: Option<String>,
    pub number: Option<f64>,
    pub year: Option<i32>,
    pub ordinal: u32,
    pub issues: Vec<PeriodicalIssueDto>,
}

/// `periodical_tree_get` 响应：一个 Work 的完整期刊层级。
///
/// 层级由后端按 Repository 顺序组装；前端只做渲染，不重排、不推断归属。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct PeriodicalTreeDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub work_id: String,
    pub periodical: PeriodicalDto,
    pub volumes: Vec<PeriodicalVolumeDto>,
}

// ========== Agent 全局设置 Typed IPC（A1「智能配置推荐」垂直切片） ==========
//
// 本组 DTO 是设置页栖伴读写的**唯一**契约入口：读的是脱敏后的全局阅读设置
// 快照，写的是严格 Typed 的 `PreferenceReadingPatchDto` 提案。
// 它们共同保证：
// - 没有任意 JSON / SQL / 路径 / 自由 command 名的入口；
// - canonical digest 只由 Rust 生成，UI 只回传它显示的那一份；
// - 一次性 Approval Token 不出现在任何字段里（它只在 Rust 内部批准路径存在）。

/// 全局设置上下文覆盖的 Section（闭合集合；第一版只有 `reading`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum AgentSettingsSectionDto {
    Reading,
}

impl From<haven_domain::agent::AgentSettingsSection> for AgentSettingsSectionDto {
    fn from(value: haven_domain::agent::AgentSettingsSection) -> Self {
        match value {
            haven_domain::agent::AgentSettingsSection::Reading => Self::Reading,
        }
    }
}

impl From<AgentSettingsSectionDto> for haven_domain::agent::AgentSettingsSection {
    fn from(value: AgentSettingsSectionDto) -> Self {
        match value {
            AgentSettingsSectionDto::Reading => Self::Reading,
        }
    }
}

/// 被服务端脱敏（清空）的字段名。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum AgentSettingsRedactedFieldDto {
    CustomFontFamily,
    CustomBackground,
    CustomText,
}

impl From<haven_domain::agent::AgentSettingsRedactedField> for AgentSettingsRedactedFieldDto {
    fn from(value: haven_domain::agent::AgentSettingsRedactedField) -> Self {
        use haven_domain::agent::AgentSettingsRedactedField as Field;
        match value {
            Field::CustomFontFamily => Self::CustomFontFamily,
            Field::CustomBackground => Self::CustomBackground,
            Field::CustomText => Self::CustomText,
        }
    }
}

/// Agent 能力清单投影（与领域清单同形；`capabilities` 缺省即全关）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentCapabilityManifestDto {
    pub agent_api_version: u32,
    pub capabilities: AgentCapabilitySetDto,
}

/// 能力开关集合。这里是**服务端固定清单**的投影，不是调用方可配置的授权开关。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentCapabilitySetDto {
    pub settings_read: bool,
    pub settings_proposal: bool,
    pub library_summary_read: bool,
    pub setting_sources_read: bool,
    pub resource_preference_read: bool,
    pub resource_preference_proposal: bool,
    pub media_capabilities_read: bool,
    pub onboarding_read: bool,
    pub metadata_proposal: bool,
    pub rename_proposal: bool,
    pub secret_read: bool,
    pub filesystem_write: bool,
}

impl From<haven_domain::agent::AgentCapabilityManifest> for AgentCapabilityManifestDto {
    fn from(value: haven_domain::agent::AgentCapabilityManifest) -> Self {
        Self {
            agent_api_version: value.agent_api_version,
            capabilities: AgentCapabilitySetDto {
                settings_read: value.capabilities.settings_read,
                settings_proposal: value.capabilities.settings_proposal,
                library_summary_read: value.capabilities.library_summary_read,
                setting_sources_read: value.capabilities.setting_sources_read,
                resource_preference_read: value.capabilities.resource_preference_read,
                resource_preference_proposal: value.capabilities.resource_preference_proposal,
                media_capabilities_read: value.capabilities.media_capabilities_read,
                onboarding_read: value.capabilities.onboarding_read,
                metadata_proposal: value.capabilities.metadata_proposal,
                rename_proposal: value.capabilities.rename_proposal,
                secret_read: value.capabilities.secret_read,
                filesystem_write: value.capabilities.filesystem_write,
            },
        }
    }
}

/// 脱敏后的全局阅读设置快照。
///
/// 字段名与 `PreferenceReadingSettingsDto` 一致，便于页面直接渲染当前档位；
/// `redactedFields` 里的字段在服务端已被清空（null），页面必须显示为「已脱敏」，
/// 不得当作「未设置」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentSettingsReadingSnapshotDto {
    #[ts(type = "\"reading\"")]
    pub section: String,
    pub font_family: PreferenceReadingFontFamilyDto,
    pub custom_font_family: Option<String>,
    pub font_size: PreferenceReadingFontSizeDto,
    pub line_height: PreferenceReadingLineHeightDto,
    pub content_width: PreferenceReadingContentWidthDto,
    pub theme: PreferenceReadingThemeDto,
    pub custom_background: Option<String>,
    pub custom_text: Option<String>,
    pub font_weight: PreferenceReadingFontWeightDto,
    pub letter_spacing: PreferenceReadingLetterSpacingDto,
    pub system_auto: bool,
    pub pagination: PreferenceReadingPaginationDto,
    /// 服务端脱敏过的字段名（空数组表示没有被裁剪）。
    pub redacted_fields: Vec<AgentSettingsRedactedFieldDto>,
}

/// `agent_settings_context_get` 响应：读取边界（只读，无 secret/路径/正文）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentSettingsContextDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    /// 服务端派生的设置上下文身份（64 位小写十六进制 context hash 的派生 UUID）。
    pub context_id: String,
    /// canonical 设置上下文载荷的 SHA-256；提案创建必须回传它。
    pub context_hash: String,
    pub subject: AgentSettingsSubjectDto,
    /// authoritative 设置 revision；`null` 表示该分区从未保存过。
    pub revision: Option<String>,
    pub reading: AgentSettingsReadingSnapshotDto,
    pub capabilities: AgentCapabilityManifestDto,
}

/// 全局设置范围（不含任何作品身份，也不含伪造的 Work ID）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentSettingsSubjectDto {
    pub section: AgentSettingsSectionDto,
}

impl From<haven_domain::agent::AgentSettingsSubject> for AgentSettingsSubjectDto {
    fn from(value: haven_domain::agent::AgentSettingsSubject) -> Self {
        Self {
            section: value.section.into(),
        }
    }
}

/// 设置来源层（Agent 只读投影）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum AgentSettingSourceLayerDto {
    Default,
    Global,
    Edition,
    MediaItem,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentSettingSourceDto {
    pub layer: AgentSettingSourceLayerDto,
    pub present: bool,
    pub revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentSettingSourcesDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub section: AgentSettingsSectionDto,
    pub revision: Option<String>,
    pub layers: Vec<AgentSettingSourceDto>,
}

/// 资源偏好 Agent 作用域。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum AgentResourcePreferenceScopeDto {
    Edition,
    MediaItem,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentResourcePreferenceSnapshotDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub context_id: String,
    pub context_hash: String,
    pub target_scope: AgentResourcePreferenceScopeDto,
    pub edition_id: String,
    pub media_item_id: Option<String>,
    pub revision: Option<String>,
    pub reading: Option<PreferenceReadingPatchDto>,
    pub comic: Option<PreferenceComicPatchDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentResourcePreferencePatchDto {
    pub reading: Option<PreferenceReadingPatchDto>,
    pub comic: Option<PreferenceComicPatchDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentResourcePreferenceProposalCreateRequest {
    pub session_id: String,
    pub request_id: String,
    pub target_scope: AgentResourcePreferenceScopeDto,
    pub edition_id: String,
    pub media_item_id: Option<String>,
    pub context_id: String,
    pub context_hash: String,
    pub base_revision: Option<String>,
    pub patch: AgentResourcePreferencePatchDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentResourcePreferenceProposalDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub proposal_id: String,
    pub status: AgentSettingsProposalStatusDto,
    pub target_scope: AgentResourcePreferenceScopeDto,
    pub target_label: String,
    pub edition_id: String,
    pub media_item_id: Option<String>,
    pub base_revision: Option<String>,
    pub digest: String,
    pub created_at: String,
    pub expires_at: String,
    pub changes: Vec<AgentSettingChangeDto>,
}

/// 按提案 ID 回读资源级 Agent 提案；供未来 UI 从外部 Agent 返回的 proposalId 打开 Diff。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentResourcePreferenceProposalGetRequest {
    pub proposal_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentResourcePreferenceProposalGetResultDto {
    pub proposal: AgentResourcePreferenceProposalDto,
    pub receipt: Option<AgentSettingChangeReceiptDto>,
}

/// 用户批准资源级 Agent 提案；目标作用域从持久化 Proposal 重建，不由调用方自带。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentResourcePreferenceProposalApproveRequest {
    pub proposal_id: String,
    pub expected_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentResourcePreferenceProposalApproveResultDto {
    pub proposal: AgentResourcePreferenceProposalDto,
    pub receipt: AgentSettingChangeReceiptDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentLibrarySummaryCountsDto {
    pub works: u64,
    pub editions: u64,
    pub media_items: u64,
    pub favorites: u64,
    pub in_progress: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentLibraryCategoryCountDto {
    pub category: ContentCategory,
    pub work_count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentLibraryRecentItemDto {
    pub work_id: String,
    pub title: String,
    // wire 的 `ContentCategory` 没有 `all`：无法按媒体类型归类的作品投影成 null，
    // 页面必须显示为「未分类」而不是猜一个具体分类。
    pub category: Option<ContentCategory>,
    pub progress_ratio: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentLibrarySummaryDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub counts: AgentLibrarySummaryCountsDto,
    pub categories: Vec<AgentLibraryCategoryCountDto>,
    pub recent: Vec<AgentLibraryRecentItemDto>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum AgentMediaAvailabilityDto {
    Available,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum AgentMediaCapabilityFlagDto {
    OpenSession,
    ExtractText,
    RenderPages,
    Stream,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentMediaCapabilityDto {
    pub media_item_id: String,
    pub media_type: MediaTypeDto,
    pub availability: AgentMediaAvailabilityDto,
    pub can_open_session: bool,
    pub can_extract_text: bool,
    pub can_render_pages: bool,
    pub declared_capabilities: Vec<AgentMediaCapabilityFlagDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentMediaCapabilitiesDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub items: Vec<AgentMediaCapabilityDto>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentOnboardingStateDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub completed_steps: Vec<String>,
    pub next_step: Option<String>,
    pub has_storage_location: bool,
    pub has_library_content: bool,
    pub has_ai_provider: bool,
}

/// 创建设置提案的请求（闭合 typed DTO）。
///
/// `contextId` / `contextHash` / `baseRevision` 都必须来自最近一次
/// `agent_settings_context_get`：服务端会重新读取 authoritative 设置、重建上下文并
/// 逐项比对，任一项对不上都 fail-closed（零写入）。这里**没有**任何绕过字段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentSettingsProposalCreateRequest {
    pub session_id: String,
    pub request_id: String,
    pub context_id: String,
    pub context_hash: String,
    pub base_revision: Option<String>,
    pub patch: PreferenceReadingPatchDto,
}

/// 提案状态（闭合集合）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum AgentSettingsProposalStatusDto {
    Pending,
    Applied,
    Rejected,
    Expired,
}

impl From<haven_domain::setting_proposal::SettingProposalStatus>
    for AgentSettingsProposalStatusDto
{
    fn from(value: haven_domain::setting_proposal::SettingProposalStatus) -> Self {
        use haven_domain::setting_proposal::SettingProposalStatus as Status;
        match value {
            Status::Pending => Self::Pending,
            Status::Applied => Self::Applied,
            Status::Rejected => Self::Rejected,
            Status::Expired => Self::Expired,
        }
    }
}

/// 一条提案内的单项改动（从提案载荷与 authoritative 当前值推导，不由前端计算）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentSettingChangeDto {
    /// 稳定设置项 id（例如 `reading.fontSize`）。
    pub key: String,
    /// 改动前的档位（Wire 枚举值字符串）。
    pub before: String,
    /// 改动后的档位（Wire 枚举值字符串）。
    pub after: String,
}

/// Agent 设置提案投影。
///
/// `digest` 是 Rust 生成的 canonical digest（64 位小写十六进制）；UI 必须原样
/// 回传它，不得自行计算领域 digest。这里**没有** Approval Token 字段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentSettingsProposalDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub proposal_id: String,
    pub status: AgentSettingsProposalStatusDto,
    pub subject: AgentSettingsSubjectDto,
    pub target_label: String,
    pub base_revision: Option<String>,
    pub digest: String,
    pub created_at: String,
    pub expires_at: String,
    pub changes: Vec<AgentSettingChangeDto>,
}

/// 回执投影：只暴露「改了什么、结果如何」，不暴露 token 或任意 JSON 结构。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentSettingChangeReceiptDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub receipt_id: String,
    pub proposal_id: String,
    /// 与提案 digest 同源；UI 用它核对回执确实对应自己批准的那一份。
    pub proposal_digest: String,
    /// `applied` / `rejected` / `expired` / `pending`（未应用时为 pending）。
    pub status: AgentSettingsProposalStatusDto,
    /// 应用后的 authoritative revision；`null` 表示没有写出新版本。
    pub applied_revision: Option<String>,
    pub changed: bool,
    pub changes: Vec<AgentSettingChangeDto>,
    pub applied_at: String,
}

/// 批准请求：只有提案 ID 与**UI 显示的那份 canonical digest**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentSettingsProposalApproveRequest {
    pub proposal_id: String,
    pub expected_digest: String,
}

/// 拒绝请求：同样只提交提案 ID 与 UI 显示的那份 canonical digest。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentSettingsProposalRejectRequest {
    pub proposal_id: String,
    pub expected_digest: String,
}

/// 提案读取请求（UI 刷新/重开后回读）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentSettingsProposalGetRequest {
    pub proposal_id: String,
}

/// 回执读取请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentSettingChangeReceiptGetRequest {
    pub proposal_id: String,
}

/// 批准结果：应用后的提案状态 + 回执。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentSettingsProposalApproveResultDto {
    pub proposal: AgentSettingsProposalDto,
    pub receipt: AgentSettingChangeReceiptDto,
}

/// 拒绝结果：拒绝后的提案投影（零写入）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentSettingsProposalRejectResultDto {
    pub proposal: AgentSettingsProposalDto,
}

/// 读取结果：提案 + 可能存在的回执（未应用时 `receipt` 为 null）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentSettingsProposalGetResultDto {
    pub proposal: AgentSettingsProposalDto,
    pub receipt: Option<AgentSettingChangeReceiptDto>,
}

// ---------- 外部 Agent Broker（A5；默认关闭） ----------

/// Broker 当前生命周期状态。它是运行时状态，不是用户授权开关；授权边界仍由
/// Rust Broker 的闭合请求集合与 Proposal 审批链决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum AgentBrokerStatusDto {
    Disabled,
    Listening,
    Busy,
    Unavailable,
}

/// 外部 Agent Broker 状态投影。端点只在 listening 时出现；reason 只携带稳定、可展示的
/// fail-closed 文案，不携带路径、堆栈、凭据或原始系统错误。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentBrokerStatusResultDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub status: AgentBrokerStatusDto,
    pub endpoint: Option<String>,
    pub reason: Option<String>,
}

// ---------- AI Provider Profile（A2 基础切片；docs/architecture/AI_SYSTEM.md） ----------
//
// 这里的 DTO 全部是**非敏感**投影：profile 行本身不含 secret，模型目录来自
// Provider 的 /models。API key 只存在于 CredentialStore，任何响应只投影
// `credentialConfigured` 布尔事实，不投影 target 名或 secret。

/// AI Provider 种类（闭合枚举；第一版只有 OpenAI 兼容协议）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum AiProviderKindDto {
    // 显式 rename：`rename_all = "snake_case"` 会把 `OpenAiCompatible` 拆成
    // `open_ai_compatible`，与领域/数据库使用的稳定值 `openai_compatible` 不一致。
    #[serde(rename = "openai_compatible")]
    #[ts(rename = "openai_compatible")]
    OpenAiCompatible,
}

impl From<haven_domain::ai_provider::AiProviderKind> for AiProviderKindDto {
    fn from(value: haven_domain::ai_provider::AiProviderKind) -> Self {
        match value {
            haven_domain::ai_provider::AiProviderKind::OpenAiCompatible => Self::OpenAiCompatible,
        }
    }
}

impl From<AiProviderKindDto> for haven_domain::ai_provider::AiProviderKind {
    fn from(value: AiProviderKindDto) -> Self {
        match value {
            AiProviderKindDto::OpenAiCompatible => {
                haven_domain::ai_provider::AiProviderKind::OpenAiCompatible
            }
        }
    }
}

/// 模型能力三态。`unknown` 表示 Provider 没有声明该能力，
/// **不**等价于「不支持」，也**不**从模型名推断。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum AiModelCapabilityDto {
    Supported,
    Unsupported,
    Unknown,
}

impl From<haven_domain::ai_provider::AiModelCapability> for AiModelCapabilityDto {
    fn from(value: haven_domain::ai_provider::AiModelCapability) -> Self {
        use haven_domain::ai_provider::AiModelCapability as Capability;
        match value {
            Capability::Supported => Self::Supported,
            Capability::Unsupported => Self::Unsupported,
            Capability::Unknown => Self::Unknown,
        }
    }
}

/// AI Provider Profile 投影。**不含** API key、credentialRef 或凭据 target 名。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AiProviderProfileDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub profile_id: String,
    pub display_name: String,
    pub kind: AiProviderKindDto,
    /// 用户显式配置的 API 根地址（已通过共享 HTTP URL 策略）。
    pub endpoint: String,
    pub enabled: bool,
    /// 用户选择的模型 id；从未选择时为 null。
    pub selected_model_id: Option<String>,
    /// 该 profile 的凭据是否已配置（凭据存储只提供存在性，不提供写入时间）。
    pub credential_configured: bool,
    /// CAS 版本号；更新/删除必须原样回传。
    pub revision: String,
    pub created_at: String,
    pub updated_at: String,
}

/// `ai_provider_profile_list` 响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AiProviderProfileListResultDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub profiles: Vec<AiProviderProfileDto>,
}

/// `ai_provider_profile_get` 请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AiProviderProfileGetRequest {
    pub profile_id: String,
}

/// `ai_provider_profile_upsert` 请求。
///
/// `expectedRevision` 为 null 表示「期望该 profile 尚不存在」；
/// 与当前行不一致时返回冲突且零写入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AiProviderProfileUpsertRequest {
    pub profile_id: String,
    pub display_name: String,
    pub kind: AiProviderKindDto,
    pub endpoint: String,
    pub enabled: bool,
    pub selected_model_id: Option<String>,
    pub expected_revision: Option<String>,
}

/// `ai_provider_profile_delete` 请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AiProviderProfileDeleteRequest {
    pub profile_id: String,
    pub expected_revision: Option<String>,
}

/// 删除结果。凭据清理先于行删除；`credentialDeleted` 只报告事实，不含任何凭据材料。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AiProviderProfileDeleteResultDto {
    pub profile_id: String,
    pub credential_deleted: bool,
}

/// `ai_provider_models_list` 请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AiProviderModelsListRequest {
    pub profile_id: String,
}

/// 模型目录状态。`no_credential` / `disabled` / `empty` 都是**诚实空态**而不是错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum AiProviderModelsCatalogStateDto {
    /// 目录已取到且至少有一条模型。
    Ready,
    /// profile 被禁用：不发请求。
    Disabled,
    /// profile 未配置凭据：不发请求。
    NoCredential,
    /// 请求成功但 Provider 没有返回任何模型。
    Empty,
}

/// 一条模型目录记录。能力字段缺失一律 `unknown`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AiProviderModelDto {
    pub model_id: String,
    pub display_name: Option<String>,
    /// Provider 自报的创建时间（Unix 秒）；语义由 Provider 定义。
    #[ts(type = "number | null")]
    pub created: Option<i64>,
    pub owned_by: Option<String>,
    pub chat: AiModelCapabilityDto,
    pub vision: AiModelCapabilityDto,
    pub embedding: AiModelCapabilityDto,
}

/// `ai_provider_models_list` 响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AiProviderModelsCatalogDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub profile_id: String,
    pub state: AiProviderModelsCatalogStateDto,
    pub models: Vec<AiProviderModelDto>,
}

/// 生成智能阅读设置推荐的请求。
///
/// Provider 只负责返回结构化建议；`contextId`、`contextHash` 与
/// `baseRevision` 必须来自最近一次 authoritative 上下文读取，最终仍停在
/// `pending` Proposal，不包含任何 Apply / Approval / Token 字段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AiSettingsRecommendationGenerateRequest {
    pub profile_id: String,
    pub session_id: String,
    pub request_id: String,
    pub context_id: String,
    pub context_hash: String,
    pub base_revision: Option<String>,
}

/// Provider 生成的结构化推荐及其待批准 Proposal。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AiSettingsRecommendationDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub profile_id: String,
    pub model_id: String,
    pub explanation: Option<String>,
    pub recommended_patch: PreferenceReadingPatchDto,
    pub proposal: AgentSettingsProposalDto,
}

/// Agent 轨迹事件类型（闭合集合）。事件只描述步骤，不携带模型原文、路径、SQL 或 secret。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, rename_all = "snake_case")]
pub enum AgentTraceEventKindDto {
    RequestStarted,
    ContextLoaded,
    ProviderRequest,
    ProviderResponse,
    StructuredOutputValidated,
    ProposalCreated,
    WaitingForApproval,
    ApprovalRejected,
    CasStarted,
    Applied,
    ReceiptCreated,
    Cancelled,
    Retrying,
    Failed,
}

/// 读取指定 Agent 会话轨迹的请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentTraceGetRequest {
    pub session_id: String,
}

/// 脱敏轨迹事件。context 只保留可重新核对的 id/hash。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentTraceEventDto {
    pub session_id: String,
    pub request_id: String,
    pub context_id: String,
    pub context_hash: String,
    #[ts(type = "number")]
    pub sequence: u64,
    pub kind: AgentTraceEventKindDto,
    #[ts(type = "number | null")]
    pub duration_ms: Option<u64>,
    pub occurred_at: String,
}

/// Agent 轨迹读取结果。事件上限由 Application collector 固定保证。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[ts(export, rename_all = "camelCase")]
pub struct AgentTraceGetResultDto {
    #[ts(type = "1")]
    pub schema_version: u32,
    pub session_id: String,
    pub events: Vec<AgentTraceEventDto>,
}

impl AiProviderModelDto {
    /// 领域模型记录 → wire 投影。
    pub fn from_domain(value: &haven_domain::ai_provider::AiModelDescriptor) -> Self {
        Self {
            model_id: value.model_id.clone(),
            display_name: value.display_name.clone(),
            created: value.created,
            owned_by: value.owned_by.clone(),
            chat: value.chat.into(),
            vision: value.vision.into(),
            embedding: value.embedding.into(),
        }
    }
}
