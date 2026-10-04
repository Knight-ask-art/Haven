//! 应用外观（Appearance）领域切片 —— Stage 1A Foundation。
//!
//! 本模块只定义外观能力的**领域事实**，不实现 UI、注册表、IPC、运行时或文件存储。
//!
//! 覆盖的八项应用外观能力与类型的对应关系：
//! 1. 明/暗两套自定义主题调色板 → [`AppTheme::light`] / [`AppTheme::dark`]；
//! 2. 与调色板分离的独立强调色 → [`AppTheme::accent_color`]；
//! 3. 壁纸选择（无 / 静态 / 动态）→ [`WallpaperSelection`]；
//! 4. 自定义字体资产 → [`AppearanceAssetKind::Font`]；
//! 5. 资产校验状态与体积上限 → [`AppearanceAssetMetadata`]；
//! 6. 首页模块的闭合模块集合与档位 → [`HomeModuleId`] / [`HomeModuleSize`]；
//! 7. 首页模块布局与自定义校验 → [`HomeLayout`] / [`HomeModulePlacement`]；
//! 8. 设置页总览的模块布局 → [`OverviewLayout`] / [`OverviewModulePlacement`]。
//!
//! 第 6/7 与第 8 是**两套独立事实**：首页是四列网格、总览是三列网格，闭合模块集合也
//! 完全不同，因此各有自己的类型、错误代码与存储表（045/046 与 048）。
//!
//! 设计原则：
//! - 所有外观配置都是**闭合、强类型**的领域值：未知字段、未知枚举与越界值在反序列化
//!   边界拒绝，不存在「任意 JSON / localStorage Map」这条写入路径。
//! - 资产身份是**不透明 UUID**：模型里没有任何字段可以承载路径或 URL，壁纸与字体
//!   只引用 [`AppearanceAssetId`]；真实字节由后续 Foundation 的资产存储负责。
//! - 展示名必须是**真能显示出来的文本**：C0/DEL 与会伪造视觉顺序或隐藏可见文本的
//!   格式/双向控制字符一律拒绝，避免两个不同资产在界面上渲染成同一串字。
//! - 体积与坐标**有界**：每种资产有自己的上限常量，首页模块坐标受固定网格约束，
//!   且模块**实际占用的格子**（含档位跨度）互不重叠，避免损坏或恶意配置把任意数值
//!   变成领域事实。
//! - 「没有布局行」等于「从未自定义」：读取侧回落到 [`HomeLayout::default`] 的
//!   当前真实三个首页模块，保证升级前后观感一致（不伪造用户没做过的自定义）。
//!
//! 本模块是纯领域判断，不访问 SQLite、网络、文件系统或 Tauri。

use std::fmt;
use std::str::FromStr;

use haven_common::{AppError, ErrorKind};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::comic_identity::has_opaque_control_character;

// ---------- 稳定错误代码 ----------

/// 颜色值非法（不是规范 `#rrggbb` 或 `#rrggbbaa`）。
pub const APPEARANCE_INVALID_COLOR: &str = "APPEARANCE_INVALID_COLOR";
/// 外观资产元数据非法（种类/状态/体积/展示名不满足约束）。
pub const APPEARANCE_INVALID_ASSET: &str = "APPEARANCE_INVALID_ASSET";
/// 首页布局非法（模块集合/坐标/schema 版本不满足约束）。
pub const APPEARANCE_INVALID_HOME_LAYOUT: &str = "APPEARANCE_INVALID_HOME_LAYOUT";
/// 总览布局非法（模块集合/坐标/schema 版本不满足约束）。
///
/// 与首页布局分开一个代码：两者是两个独立事实，存储表与 revision 也各自独立，
/// 合成一个代码会让「哪一侧的布局被拒绝」变得无法从错误里读出来。
pub const APPEARANCE_INVALID_OVERVIEW_LAYOUT: &str = "APPEARANCE_INVALID_OVERVIEW_LAYOUT";
/// 资产仍被持久化外观设置引用（删除被拒绝，零写入）。
pub const APPEARANCE_ASSET_IN_USE: &str = "APPEARANCE_ASSET_IN_USE";

fn invalid_color(message: impl Into<String>) -> AppError {
    AppError::new(
        APPEARANCE_INVALID_COLOR,
        ErrorKind::Validation,
        message,
        false,
    )
}

fn invalid_asset(message: impl Into<String>) -> AppError {
    AppError::new(
        APPEARANCE_INVALID_ASSET,
        ErrorKind::Validation,
        message,
        false,
    )
}

fn invalid_home_layout(message: impl Into<String>) -> AppError {
    AppError::new(
        APPEARANCE_INVALID_HOME_LAYOUT,
        ErrorKind::Validation,
        message,
        false,
    )
}

fn invalid_overview_layout(message: impl Into<String>) -> AppError {
    AppError::new(
        APPEARANCE_INVALID_OVERVIEW_LAYOUT,
        ErrorKind::Validation,
        message,
        false,
    )
}

// ---------- 颜色与调色板 ----------

/// 十六进制颜色（严格 `#rrggbb` 或 `#rrggbbaa`）。
///
/// - 恰好 7 或 9 个 ASCII 字符：`#` + 6 位十六进制（+ 2 位十六进制 alpha）；
/// - 大写十六进制被接受，但**归一化为小写**后存储，避免同一颜色出现两种文本身份；
/// - 8 位写法不是「顺手多收一种」，而是现有前端 token 的真实形状：`--border` 落在
///   `rgba(60, 60, 67, 0.16)` 上（等价 `#3c3c4329`）。只收 6 位会把用户自定义的
///   边框色静默变成不透明，观感与既定设计不符；
/// - 仍然拒绝 3/4 位简写、命名颜色，以及 `rgba(...)` / `color-mix(...)` 之类的 CSS
///   文本：本类型表达的是**规范十六进制**，不是一个 CSS 颜色解析器。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HexColor(String);

impl HexColor {
    /// 解析并归一化（非法值返回 `None`）。
    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.to_ascii_lowercase();
        let bytes = normalized.as_bytes();
        if !matches!(bytes.len(), 7 | 9) || bytes[0] != b'#' {
            return None;
        }
        if !bytes[1..].iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        Some(Self(normalized))
    }

    /// 归一化后的 `#rrggbb` / `#rrggbbaa` 文本（小写）。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HexColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for HexColor {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or_else(|| invalid_color("颜色必须是 #rrggbb 或 #rrggbbaa"))
    }
}

impl Serialize for HexColor {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for HexColor {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).ok_or_else(|| serde::de::Error::custom("颜色必须是 #rrggbb 或 #rrggbbaa"))
    }
}

/// 自定义主题调色板：与前端 CSS token 集合一一对应的闭合集合。
///
/// 字段集合是**精确的** 19 个既有 token；`deny_unknown_fields` 保证多写一个 token
/// 会在反序列化边界失败，而不是被静默丢弃（详见 `前端/app/src/index.css`）。
/// `border` / `input` 这两个既有 token 实际带 alpha（前端是
/// `rgba(60, 60, 67, 0.16)`），所以它们的取值是 [`HexColor`] 的 8 位形态
/// （`#3c3c4329`），而不是被压成不透明色。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AppThemePalette {
    pub background: HexColor,
    pub foreground: HexColor,
    pub card: HexColor,
    pub card_foreground: HexColor,
    pub popover: HexColor,
    pub popover_foreground: HexColor,
    pub primary: HexColor,
    pub primary_foreground: HexColor,
    pub secondary: HexColor,
    pub secondary_foreground: HexColor,
    pub muted: HexColor,
    pub muted_foreground: HexColor,
    pub accent: HexColor,
    pub accent_foreground: HexColor,
    pub destructive: HexColor,
    pub destructive_foreground: HexColor,
    pub border: HexColor,
    pub input: HexColor,
    pub ring: HexColor,
}

/// 自定义主题：明/暗两套调色板 + 独立强调色。
///
/// `accent_color` 与调色板里的 `accent` 是两件事：前者是用户单独选定的品牌强调色，
/// 后者是调色板中的一个 token，因此必须各自校验、各自保存。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AppTheme {
    pub light: AppThemePalette,
    pub dark: AppThemePalette,
    pub accent_color: HexColor,
}

// ---------- 资产身份 ----------

/// 外观资产的不透明 ID（UUID）。
///
/// 内层私有：唯一构造路径是 [`AppearanceAssetId::new`]、[`AppearanceAssetId::from_uuid`]
/// 与 [`AppearanceAssetId::parse`]（规范小写连字符文本）。模型里没有承载路径或 URL 的
/// 字段，资产身份与资产字节的存储位置因此不可能互相伪造。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AppearanceAssetId(Uuid);

impl AppearanceAssetId {
    pub fn new() -> Self {
        Self(crate::ids::new_v7())
    }

    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }

    /// 解析规范小写连字符 UUID 文本；大写、无连字符、花括号与 URN 形态一律拒绝，
    /// 保证同一个资产只有一种文本身份（与 045 迁移的 `appearance_assets.id` 约束一致）。
    pub fn parse(value: &str) -> Option<Self> {
        let uuid = Uuid::parse_str(value).ok()?;
        let canonical = uuid.hyphenated().to_string();
        (value == canonical.as_str()).then_some(Self(uuid))
    }
}

impl Default for AppearanceAssetId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for AppearanceAssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.hyphenated().to_string())
    }
}

impl FromStr for AppearanceAssetId {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or_else(|| invalid_asset("外观资产 ID 必须是规范 UUID 文本"))
    }
}

impl Serialize for AppearanceAssetId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.hyphenated().to_string())
    }
}

impl<'de> Deserialize<'de> for AppearanceAssetId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw)
            .ok_or_else(|| serde::de::Error::custom("外观资产 ID 必须是规范 UUID 文本"))
    }
}

// ---------- 壁纸 ----------

/// 壁纸选择（闭合枚举：无 / 静态 / 动态）。
///
/// 静态与动态壁纸**只持有资产 ID**，不持有路径或 URL：真实字节由资产存储按 ID 解析，
/// 领域模型里不存在可以被当作文件系统或网络地址使用的字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WallpaperSelection {
    None,
    Static(AppearanceAssetId),
    Dynamic(AppearanceAssetId),
}

impl Default for WallpaperSelection {
    fn default() -> Self {
        Self::None
    }
}

impl WallpaperSelection {
    /// 壁纸种类的稳定文本（与 wire 的 `kind` 字段一致）。
    pub fn kind_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Static(_) => "static",
            Self::Dynamic(_) => "dynamic",
        }
    }

    /// 引用的资产（`none` 没有资产）。
    pub fn asset_id(self) -> Option<AppearanceAssetId> {
        match self {
            Self::None => None,
            Self::Static(id) | Self::Dynamic(id) => Some(id),
        }
    }
}

/// 壁纸种类判别标签（闭合集合）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WallpaperKind {
    None,
    Static,
    Dynamic,
}

impl<'de> Deserialize<'de> for WallpaperKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        match raw.as_str() {
            "none" => Ok(Self::None),
            "static" => Ok(Self::Static),
            "dynamic" => Ok(Self::Dynamic),
            _ => Err(serde::de::Error::custom(
                "壁纸种类必须是 none / static / dynamic",
            )),
        }
    }
}

/// 壁纸 wire 形状：`{"kind":"static","assetId":"<uuid>"}`。
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct WallpaperSelectionWire {
    kind: WallpaperKind,
    #[serde(default)]
    asset_id: Option<AppearanceAssetId>,
}

impl<'de> Deserialize<'de> for WallpaperSelection {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = WallpaperSelectionWire::deserialize(deserializer)?;
        match (wire.kind, wire.asset_id) {
            (WallpaperKind::None, None) => Ok(Self::None),
            (WallpaperKind::None, Some(_)) => {
                Err(serde::de::Error::custom("kind=none 不得携带 assetId"))
            }
            (WallpaperKind::Static, Some(id)) => Ok(Self::Static(id)),
            (WallpaperKind::Dynamic, Some(id)) => Ok(Self::Dynamic(id)),
            (WallpaperKind::Static, None) => {
                Err(serde::de::Error::custom("静态壁纸必须携带 assetId"))
            }
            (WallpaperKind::Dynamic, None) => {
                Err(serde::de::Error::custom("动态壁纸必须携带 assetId"))
            }
        }
    }
}

impl Serialize for WallpaperSelection {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;

        let asset_id = self.asset_id();
        let field_count = if asset_id.is_some() { 2 } else { 1 };
        let mut state = serializer.serialize_struct("WallpaperSelection", field_count)?;
        state.serialize_field("kind", self.kind_str())?;
        if let Some(id) = asset_id {
            state.serialize_field("assetId", &id)?;
        }
        state.end()
    }
}

// ---------- 资产元数据 ----------

/// 字体资产的最大字节数（32 MiB）。
pub const MAX_FONT_BYTES: u64 = 32 * 1024 * 1024;
/// 静态壁纸资产的最大字节数（32 MiB）。
pub const MAX_STATIC_WALLPAPER_BYTES: u64 = 32 * 1024 * 1024;
/// 动态壁纸资产的最大字节数（256 MiB）。
pub const MAX_DYNAMIC_WALLPAPER_BYTES: u64 = 256 * 1024 * 1024;
/// 资产展示名的最大字符数。
pub const MAX_ASSET_DISPLAY_NAME_CHARS: usize = 120;

/// 外观资产种类（闭合集合）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppearanceAssetKind {
    Font,
    StaticWallpaper,
    DynamicWallpaper,
}

impl AppearanceAssetKind {
    pub const ALL: [Self; 3] = [Self::Font, Self::StaticWallpaper, Self::DynamicWallpaper];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Font => "font",
            Self::StaticWallpaper => "static_wallpaper",
            Self::DynamicWallpaper => "dynamic_wallpaper",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "font" => Some(Self::Font),
            "static_wallpaper" => Some(Self::StaticWallpaper),
            "dynamic_wallpaper" => Some(Self::DynamicWallpaper),
            _ => None,
        }
    }

    /// 该种类的字节数上限（与 045 迁移的 CHECK 约束一致）。
    pub const fn max_bytes(self) -> u64 {
        match self {
            Self::Font => MAX_FONT_BYTES,
            Self::StaticWallpaper => MAX_STATIC_WALLPAPER_BYTES,
            Self::DynamicWallpaper => MAX_DYNAMIC_WALLPAPER_BYTES,
        }
    }

    /// 用户可见的种类名。
    ///
    /// 错误文案与 Native 选择器的过滤器名共用它：同一件事在界面上只该有一个说法，
    /// 两条路径各写一份中文迟早会漂移。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Font => "字体",
            Self::StaticWallpaper => "静态壁纸",
            Self::DynamicWallpaper => "动态壁纸",
        }
    }

    /// Native 文件选择器按种类过滤时使用的扩展名。
    ///
    /// 这份清单是**选择器提示**，不是校验：真正的判定在
    /// `AppearanceAssetStorage::import_file` 里按字节签名做（扩展名随手就能改）。
    /// 两者刻意同集合——选择器只列出这种资产真能接受的格式，用户因此不会先选中一个
    /// 必然被拒的文件再读一遍错误信息；但过滤本身不构成任何安全边界，
    /// 所以绕过选择器提交的文件照样要走签名校验。
    pub const fn picker_extensions(self) -> &'static [&'static str] {
        match self {
            Self::Font => &["ttf", "otf", "woff", "woff2"],
            Self::StaticWallpaper => &["png", "jpg", "jpeg", "webp"],
            Self::DynamicWallpaper => &["mp4", "webm"],
        }
    }
}

/// 资产校验状态（闭合集合）。
///
/// 只有 `Validated` 才代表资产字节已被真实校验且可以使用；`Pending` 是刚登记但尚未
/// 校验的状态，`Rejected` 是校验失败的终态。状态是**必填**字段：缺失的状态既不是
/// 「已校验」也不是「待校验」，本阶段类型严格闭合，不用默认值把缺失补成 `Pending`
/// （045 的 `validation_state` 同样是 NOT NULL，缺失在读路径上本来就不可能出现）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetValidationState {
    Pending,
    Validated,
    Rejected,
}

impl AssetValidationState {
    pub const ALL: [Self; 3] = [Self::Pending, Self::Validated, Self::Rejected];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Validated => "validated",
            Self::Rejected => "rejected",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "validated" => Some(Self::Validated),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }

    /// 只有已校验的资产可以被消费（字体/壁纸渲染）。
    pub const fn is_usable(self) -> bool {
        matches!(self, Self::Validated)
    }
}

/// 外观资产元数据（字体 / 静态壁纸 / 动态壁纸）。
///
/// 不变量：
/// - `id` 是不透明 UUID，模型里没有路径或 URL；
/// - `kind` / `state` 必填（缺失状态不得被静默补成 `Pending`，见 [`AssetValidationState`]）；
/// - `byte_size` 必须为正且不超过该种类的上限；
/// - `display_name` 可选、有界、去掉首尾空白后非空（空串等价于「没有展示名」），
///   且不含控制字符或格式/双向控制字符（见 `bounded_display_name`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "AppearanceAssetMetadataWire")]
pub struct AppearanceAssetMetadata {
    pub id: AppearanceAssetId,
    pub kind: AppearanceAssetKind,
    pub state: AssetValidationState,
    pub byte_size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

impl AppearanceAssetMetadata {
    /// 校验构造器：唯一的合法入口（反序列化同样走这里）。
    pub fn new(
        id: AppearanceAssetId,
        kind: AppearanceAssetKind,
        state: AssetValidationState,
        byte_size: u64,
        display_name: Option<String>,
    ) -> Result<Self, AppError> {
        let metadata = Self {
            id,
            kind,
            state,
            byte_size,
            display_name: bounded_display_name(display_name)?,
        };
        metadata.validate()?;
        Ok(metadata)
    }

    /// 重新校验既有值（例如从存储恢复时）。
    pub fn validate(&self) -> Result<(), AppError> {
        if self.byte_size == 0 {
            return Err(invalid_asset("资产体积必须为正"));
        }
        if self.byte_size > self.kind.max_bytes() {
            return Err(invalid_asset(format!(
                "资产体积超出 {:?} 上限 {} 字节",
                self.kind,
                self.kind.max_bytes()
            )));
        }
        if let Some(display_name) = &self.display_name {
            if bounded_display_name(Some(display_name.clone()))?.as_deref()
                != Some(display_name.as_str())
            {
                return Err(invalid_asset("资产展示名必须是去空白后的非空有界文本"));
            }
        }
        Ok(())
    }
}

/// 展示名里不得出现的格式/双向控制字符。
///
/// 这些码位不打印任何可见字形，却能**伪造视觉顺序或隐藏可见文本**：
/// U+200B..U+200D 零宽字符可以让两个不同的展示名渲染成同一串字；U+200E/U+200F 与
/// U+202A..U+202E、U+2066..U+2069 是双向文本控制（Trojan Source 那一类，能把
/// 「字体 A」显示成「字体 B」）；U+2028/U+2029 是行/段分隔符；U+2060 与 U+FEFF 是
/// 无宽不换行与 BOM。
///
/// 清单与 045 迁移 `display_name` CHECK 里的 `NOT GLOB` 字符类**逐项一致**（18 个码位）。
/// C0/DEL 仍归共享的 [`has_opaque_control_character`] 管：本函数不改变它的集合，
/// 也不接管它的语义，两个集合各自独立、互不重叠。
fn has_display_name_format_control(character: char) -> bool {
    matches!(
        character,
        '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{2029}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}'
    )
}

/// 展示名的**公开**校验入口：返回修剪后的规范形态。
///
/// [`AppearanceAssetMetadata::new`] 走的是同一条规则，这里只是把它单独暴露出来，
/// 让应用层能在**触碰文件系统之前**先拒绝非法展示名——否则导入路径会先把字节复制
/// 进受控存储，再因为展示名非法而回滚，白写一次磁盘。规则本身只有一份
/// （[`bounded_display_name`]），这里不做任何放宽。
pub fn normalize_asset_display_name(value: Option<String>) -> Result<Option<String>, AppError> {
    bounded_display_name(value)
}

/// 选中文件的**文件名词干** → 安全的展示名；归一化之后什么都不剩时返回 `None`。
///
/// 导入时后端 Native 选择器只给出一个路径，文件名是用户唯一看得懂的线索——不取用它，
/// 界面上每个资产都只能叫「资产 0196f0d2」。但文件名是**外部输入**：它可能超长、可能
/// 含控制字符或伪顺序格式字符，也可能整段都是这些字符。所以这里的目标不是「校验并报错」
/// 而是「归一化到一个安全形态」，并且把「归一化之后什么都不剩」如实当成「没有展示名」：
/// 名字不好看可以接受，一次导入因为文件名而失败、或者库里落进一个渲染不出真实顺序的
/// 名字都不可以。
///
/// 每一步都在 [`bounded_display_name`] 的既有规则之内——去掉控制与格式/双向控制字符、
/// 把连续空白并成一个空格、修剪首尾、按字符数截断——最后再过一遍
/// [`normalize_asset_display_name`]，返回值因此一定是落库形态的不动点。万一将来规则收紧
/// 把结果判成非法，这里回退到 `None`，而不是把一次「名字不好」升级成导入失败。
pub fn appearance_display_name_from_file_stem(stem: &str) -> Option<String> {
    let mut cleaned = String::with_capacity(stem.len());
    let mut separated = false;
    for character in stem.chars() {
        let mut buffer = [0u8; 4];
        let encoded = character.encode_utf8(&mut buffer);
        if character.is_whitespace() || has_opaque_control_character(encoded) {
            // 连续的空白/控制字符只留一个分隔符；开头的那些整个丢掉。
            separated = !cleaned.is_empty();
            continue;
        }
        if has_display_name_format_control(character) {
            continue;
        }
        if separated {
            cleaned.push(' ');
            separated = false;
        }
        cleaned.push(character);
    }
    let truncated: String = cleaned.chars().take(MAX_ASSET_DISPLAY_NAME_CHARS).collect();
    normalize_asset_display_name(Some(truncated)).ok().flatten()
}

/// 展示名边界：修剪首尾空白；修剪后为空等价于「没有展示名」；
/// 超长、含控制字符或含格式/双向控制字符一律拒绝。
///
/// 这里的「首尾空白」是 `str::trim` 的真实集合（Unicode `White_Space` 全集，不是
/// 半角空格）：U+0009..U+000D、U+0020、U+0085、U+00A0、U+1680、U+2000..U+200A、
/// U+2028、U+2029、U+202F、U+205F、U+3000（`display_name_trims_exactly_unicode_white_space`
/// 钉住这 25 个码位，与 045 迁移里 `trim(display_name, char(...))` 的列表逐项对应）。
/// 「控制字符」是 [`has_opaque_control_character`] 的集合（C0 `0x00..=0x1f` 与
/// DEL `0x7f`），「格式/双向控制字符」是 [`has_display_name_format_control`] 的 18 个
/// 码位。045 迁移的 `display_name` CHECK 用**同一组码位**校验落库形态
/// （`trim(display_name, char(...))` 与两条 `instr`/`NOT GLOB`），
/// 两端集合一旦漂移，库里就会出现领域读不回来的展示名。
///
/// 判断顺序是「先修剪、再查字符」：U+2028/U+2029 同时属于 `White_Space` 与格式字符，
/// 因此它们在首尾只是被修剪掉（`"\u{2028}名"` → `"名"`），只有中间位置才以格式字符
/// 被拒绝。落库的是修剪后的形态，SQL 又只校验落库形态，两端仍然一致。
///
/// 落库形态是这里的**不动点**：修剪过的值再进一次 `bounded_display_name` 必须原样返回，
/// 因此 [`AppearanceAssetMetadata::validate`] 能用同一条规则复检存储读回的值。
fn bounded_display_name(value: Option<String>) -> Result<Option<String>, AppError> {
    let Some(raw) = value else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > MAX_ASSET_DISPLAY_NAME_CHARS {
        return Err(invalid_asset(format!(
            "资产展示名不得超过 {MAX_ASSET_DISPLAY_NAME_CHARS} 个字符"
        )));
    }
    if has_opaque_control_character(trimmed) {
        return Err(invalid_asset("资产展示名不得包含控制字符"));
    }
    if trimmed.chars().any(has_display_name_format_control) {
        return Err(invalid_asset(
            "资产展示名不得包含会伪造视觉顺序或隐藏文本的格式字符",
        ));
    }
    Ok(Some(trimmed.to_owned()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AppearanceAssetMetadataWire {
    id: AppearanceAssetId,
    kind: AppearanceAssetKind,
    /// 必填：缺失状态必须拒绝，不得静默补 `Pending`。
    state: AssetValidationState,
    byte_size: u64,
    /// 可选：`displayName` 确实可以不存在（对应 045 的 NULLABLE 列）。
    #[serde(default)]
    display_name: Option<String>,
}

impl TryFrom<AppearanceAssetMetadataWire> for AppearanceAssetMetadata {
    type Error = AppError;

    fn try_from(wire: AppearanceAssetMetadataWire) -> Result<Self, Self::Error> {
        Self::new(
            wire.id,
            wire.kind,
            wire.state,
            wire.byte_size,
            wire.display_name,
        )
    }
}

// ---------- 首页布局 ----------

/// 首页布局 schema 版本（当前唯一受支持的版本）。
pub const HOME_LAYOUT_SCHEMA_VERSION: u32 = 1;
/// 首页布局允许的最大模块数。
///
/// 排序号必须唯一且小于该值，因此模块数天然也不会超过它；长度检查仍然显式保留，
/// 让「模块过多」这件事有自己的错误信息，而不是靠排序号唯一性间接拒绝。
pub const HOME_LAYOUT_MAX_MODULES: usize = 16;
/// 首页网格的列数。
pub const HOME_LAYOUT_GRID_COLUMNS: usize = 4;
/// 首页网格的行数。
pub const HOME_LAYOUT_GRID_ROWS: usize = 12;

/// 首页模块 ID（闭合集合：只有当前真实存在的三个首页模块）。
///
/// wire 值由 [`HomeModuleId::as_str`] / [`HomeModuleId::parse`] 唯一决定：这里手写
/// `Serialize`/`Deserialize`，而不是给每个变体挂 `#[serde(rename = "...")]`——属性里的
/// 字面量与 `as_str` 是两份可以各自漂移的事实，而 `shelf-favorites` 的连字符尤其容易
/// 被 `rename_all = "snake_case"` 悄悄改成下划线（那会变成另一个模块 ID）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HomeModuleId {
    Continue,
    RecentlyAdded,
    ShelfFavorites,
}

impl HomeModuleId {
    /// 当前真实的全部首页模块（默认布局与校验都用它，不引入只服务于测试的 ID）。
    pub const ALL: [Self; 3] = [Self::Continue, Self::RecentlyAdded, Self::ShelfFavorites];

    /// 稳定文本：`shelf-favorites` 与 HomeService 的 `shelf_id` 完全一致（连字符，非下划线）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::RecentlyAdded => "recently_added",
            Self::ShelfFavorites => "shelf-favorites",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "continue" => Some(Self::Continue),
            "recently_added" => Some(Self::RecentlyAdded),
            "shelf-favorites" => Some(Self::ShelfFavorites),
            _ => None,
        }
    }
}

impl Serialize for HomeModuleId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for HomeModuleId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).ok_or_else(|| {
            serde::de::Error::custom(
                "首页模块 ID 必须是 continue / recently_added / shelf-favorites",
            )
        })
    }
}

/// 首页模块档位（闭合集合）。档位决定模块在网格里占几列。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeModuleSize {
    Small,
    Medium,
    Large,
}

impl HomeModuleSize {
    pub const ALL: [Self; 3] = [Self::Small, Self::Medium, Self::Large];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "small" => Some(Self::Small),
            "medium" => Some(Self::Medium),
            "large" => Some(Self::Large),
            _ => None,
        }
    }

    /// 该档位占用的列数（保守网格：Large 占满整行）。
    pub const fn column_span(self) -> u16 {
        match self {
            Self::Small => 1,
            Self::Medium => 2,
            Self::Large => HOME_LAYOUT_GRID_COLUMNS as u16,
        }
    }

    /// 该档位占用的行数。
    ///
    /// 当前所有档位都只占一行：档位只决定列跨度（见 [`Self::column_span`]）。
    /// 引入两行高的档位必须先有新迁移重建表约束，不能在领域里单方面放开。
    pub const fn row_span(self) -> u16 {
        1
    }
}

/// 首页模块放置：模块、档位与网格坐标。
///
/// `row` / `column` 是**起始格**：模块实际占用从该列开始的整段格子
/// （`column..column + 档位列跨度`，所有档位都只占一行，见 [`Self::occupied_cells`]）。
/// `order` 是布局内的稳定排序号：它在同一份布局里**唯一**（见 [`HomeLayout`]），
/// 因此读取侧的顺序不依赖并列排序号的偶然排列。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "HomeModulePlacementWire")]
pub struct HomeModulePlacement {
    pub module: HomeModuleId,
    pub size: HomeModuleSize,
    pub row: u16,
    pub column: u16,
    pub order: u16,
}

impl HomeModulePlacement {
    /// 校验构造器：行/列必须落在固定网格内，且档位跨度不得越出右侧边界。
    pub fn new(
        module: HomeModuleId,
        size: HomeModuleSize,
        row: u16,
        column: u16,
        order: u16,
    ) -> Result<Self, AppError> {
        if usize::from(row) >= HOME_LAYOUT_GRID_ROWS {
            return Err(invalid_home_layout(format!(
                "首页模块行坐标必须小于 {HOME_LAYOUT_GRID_ROWS}"
            )));
        }
        if usize::from(column) + usize::from(size.column_span()) > HOME_LAYOUT_GRID_COLUMNS {
            return Err(invalid_home_layout(format!(
                "首页模块列坐标加档位跨度不得超出 {HOME_LAYOUT_GRID_COLUMNS} 列"
            )));
        }
        if usize::from(order) >= HOME_LAYOUT_MAX_MODULES {
            return Err(invalid_home_layout(format!(
                "首页模块排序必须小于 {HOME_LAYOUT_MAX_MODULES}"
            )));
        }
        Ok(Self {
            module,
            size,
            row,
            column,
            order,
        })
    }

    /// 起始格 `(row, column)`。
    ///
    /// 布局的不变量是**占用格子**不重叠（见 [`Self::occupied_cells`] 与
    /// [`Self::overlaps`]）；起始格不重复只是它的推论，光看起始格不够——`small`
    /// 从第 1 列开始、`medium` 从第 2 列开始时起始格不同，第 2 列却同时属于两者。
    pub const fn position(&self) -> (u16, u16) {
        (self.row, self.column)
    }

    /// 该放置**实际占用**的全部格子（含档位跨度），从左到右。
    pub fn occupied_cells(&self) -> Vec<(u16, u16)> {
        let span = self.size.column_span();
        (self.column..self.column + span)
            .map(|column| (self.row, column))
            .collect()
    }

    /// 两个放置是否占用了同一个格子。
    ///
    /// 行跨度与列跨度都要比较；当前所有档位的行跨度都是 1，比较行范围与直接比较
    /// `row` 等价，但写成范围比较可以让「未来出现跨行档位」时不用重写规则。
    pub fn overlaps(&self, other: &Self) -> bool {
        let rows_overlap = self.row < other.row + other.size.row_span()
            && other.row < self.row + self.size.row_span();
        let columns_overlap = self.column < other.column + other.size.column_span()
            && other.column < self.column + self.size.column_span();
        rows_overlap && columns_overlap
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct HomeModulePlacementWire {
    module: HomeModuleId,
    /// 必填：缺失档位必须拒绝，不得静默补 `Medium`。
    size: HomeModuleSize,
    row: u16,
    column: u16,
    order: u16,
}

impl TryFrom<HomeModulePlacementWire> for HomeModulePlacement {
    type Error = AppError;

    fn try_from(wire: HomeModulePlacementWire) -> Result<Self, Self::Error> {
        Self::new(wire.module, wire.size, wire.row, wire.column, wire.order)
    }
}

/// 首页布局：schema 版本 + 有界模块列表。
///
/// 自定义校验拒绝：非法 schema 版本、模块过多、未知模块 ID（由闭合枚举在
/// 反序列化边界拒绝）、重复模块、**占用格子重叠**（按档位跨度展开，而不只是起始格
/// 重复）与越界坐标。
/// 排序号唯一是稳定顺序的**事实**而不是约定：没有它，`order` 相同的两个模块会留下
/// 一个由存储返回顺序决定的偶然排列（045 的 `appearance_home_modules` 同样用
/// `UNIQUE (sort_order)` 兜住这一点）。
/// 空模块列表是合法的（用户隐藏全部模块），但**没有布局行**不等于空布局：
/// 读取侧把缺失回落到 [`HomeLayout::default`]，不伪造用户没做过的自定义。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "HomeLayoutWire")]
pub struct HomeLayout {
    pub schema_version: u32,
    pub modules: Vec<HomeModulePlacement>,
}

impl HomeLayout {
    /// 校验构造器：唯一的合法入口（反序列化同样走这里）。
    pub fn new(schema_version: u32, modules: Vec<HomeModulePlacement>) -> Result<Self, AppError> {
        Self::validate_parts(schema_version, &modules)?;
        Ok(Self {
            schema_version,
            modules,
        })
    }

    /// 重新校验全局不变量（例如从存储恢复时）。
    pub fn validate(&self) -> Result<(), AppError> {
        Self::validate_parts(self.schema_version, &self.modules)
    }

    /// 与 [`Self::new`] 共享的判断主体，避免「构造与复检」两套规则漂移。
    fn validate_parts(
        schema_version: u32,
        modules: &[HomeModulePlacement],
    ) -> Result<(), AppError> {
        if schema_version != HOME_LAYOUT_SCHEMA_VERSION {
            return Err(invalid_home_layout(format!(
                "首页布局 schema 版本必须为 {HOME_LAYOUT_SCHEMA_VERSION}"
            )));
        }
        if modules.len() > HOME_LAYOUT_MAX_MODULES {
            return Err(invalid_home_layout(format!(
                "首页模块不得超过 {HOME_LAYOUT_MAX_MODULES} 个"
            )));
        }
        for (index, placement) in modules.iter().enumerate() {
            let rebuilt = HomeModulePlacement::new(
                placement.module,
                placement.size,
                placement.row,
                placement.column,
                placement.order,
            )?;
            if rebuilt != *placement {
                return Err(invalid_home_layout("首页模块放置必须保持规范形态"));
            }
            if modules[..index]
                .iter()
                .any(|other| other.module == placement.module)
            {
                return Err(invalid_home_layout("首页模块不得重复"));
            }
            // 占用格子不得重叠：起始格不同也可能压在同一个格子上（medium 的
            // 第 2 列就是 small 从第 2 列开始时的起始格），所以按档位跨度展开比较。
            // 045 用触发器表达同一条不变量。
            if modules[..index]
                .iter()
                .any(|other| other.overlaps(placement))
            {
                return Err(invalid_home_layout("首页模块占用的网格格子不得重叠"));
            }
            // 排序号唯一：order 相同的两个模块无法给出稳定顺序，读取侧只能得到
            // 存储返回顺序这个偶然事实（045 有对应的 UNIQUE (sort_order)）。
            if modules[..index]
                .iter()
                .any(|other| other.order == placement.order)
            {
                return Err(invalid_home_layout("首页模块的排序号不得重复"));
            }
        }
        Ok(())
    }

    /// 规范形态：模块按 `order` 升序排列。
    ///
    /// 这是「写入形态 == 读回形态」的**不动点**：存储读取侧本来就按 `sort_order`
    /// 返回模块（045 的 `UNIQUE (sort_order)` 让这个顺序是全序），所以一份布局
    /// 无论以什么 `Vec` 顺序提交，落库后都只会读回它的规范形态。写入路径先规范化，
    /// 幂等比较才成立——否则「同一份布局、不同的数组顺序」会在每次保存时都被判为
    /// 变化并写出新 revision，`changed` 变成噪声。
    ///
    /// 排序不改变任何被校验的不变量（模块唯一性、占格不重叠、排序号唯一都与顺序
    /// 无关），因此规范形态一定仍然合法。
    pub fn canonicalized(mut self) -> Self {
        self.modules.sort_by_key(|placement| placement.order);
        self
    }

    /// 按 `order` 稳定排序后的模块（读取侧消费顺序）。
    ///
    /// `order` 在合法布局里唯一，因此这里的顺序是**全序**，不依赖排序算法是否稳定。
    pub fn ordered_modules(&self) -> Vec<&HomeModulePlacement> {
        let mut modules: Vec<&HomeModulePlacement> = self.modules.iter().collect();
        modules.sort_by_key(|placement| placement.order);
        modules
    }
}

impl Default for HomeLayout {
    /// 「从未自定义」的布局：当前真实的三个首页模块，位置稳定不变。
    fn default() -> Self {
        let modules = vec![
            HomeModulePlacement::new(HomeModuleId::Continue, HomeModuleSize::Medium, 0, 0, 0),
            HomeModulePlacement::new(HomeModuleId::RecentlyAdded, HomeModuleSize::Medium, 0, 2, 1),
            HomeModulePlacement::new(
                HomeModuleId::ShelfFavorites,
                HomeModuleSize::Medium,
                1,
                0,
                2,
            ),
        ];
        Self::new(
            HOME_LAYOUT_SCHEMA_VERSION,
            modules
                .into_iter()
                .map(|placement| placement.expect("内置默认首页模块必须合法"))
                .collect(),
        )
        .expect("内置默认首页布局必须合法")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct HomeLayoutWire {
    schema_version: u32,
    modules: Vec<HomeModulePlacement>,
}

impl TryFrom<HomeLayoutWire> for HomeLayout {
    type Error = AppError;

    fn try_from(wire: HomeLayoutWire) -> Result<Self, Self::Error> {
        Self::new(wire.schema_version, wire.modules)
    }
}

// ---------- 总览布局 ----------
//
// 设置页「总览」的模块布局。它与首页布局是**两个独立事实**：网格列数、闭合模块集合与
// 档位跨度都不同，存储表与 revision 也各自独立（见 048 迁移）。这里刻意不复用
// `HomeLayout`，因为「首页的四列网格」与「总览的三列网格」一旦共用一个类型，任何一次
// 为其中一侧放宽跨度或换列数的改动都会静默改掉另一侧的合法输入集合。
//
// 列数为什么是 3：总览现有的真实版式是「两行整宽 + 一行 2:1 图表 + 一行整宽热力图」。
// 3 是能表达 2:1 的最小列数（medium 占 2 列 + small 占 1 列），因此默认布局可以**逐屏**
// 复现升级前的观感；四列网格只能给出 2:2 的均分，默认版式会肉眼可见地变化。

/// 总览布局 schema 版本（当前唯一受支持的版本）。
pub const OVERVIEW_LAYOUT_SCHEMA_VERSION: u32 = 1;
/// 总览布局允许的最大模块数。
///
/// 与首页布局同理：排序号必须唯一且小于该值，模块数因此天然不会超过它；长度检查仍然
/// 显式保留，让「模块过多」有自己的错误信息。
pub const OVERVIEW_LAYOUT_MAX_MODULES: usize = 16;
/// 总览网格的列数。
pub const OVERVIEW_LAYOUT_GRID_COLUMNS: usize = 3;
/// 总览网格的行数。
pub const OVERVIEW_LAYOUT_GRID_ROWS: usize = 12;

/// 总览模块 ID（闭合集合：只有 `SettingsOverview` 真实渲染的五个内容块）。
///
/// 这里**没有**页头（标题与统计范围）与阅读统计错误横幅：它们不是可管理的模块，而是
/// 页面的身份与失败事实——把它们做成可隐藏的模块，等于允许用户把一次读取失败藏起来。
///
/// wire 值由 [`OverviewModuleId::as_str`] / [`OverviewModuleId::parse`] 唯一决定：手写
/// `Serialize`/`Deserialize` 而不是挂 `#[serde(rename)]`，理由与 [`HomeModuleId`] 相同
/// （属性字面量与 `as_str` 是两份可以各自漂移的事实，连字符尤其容易被
/// `rename_all = "snake_case"` 悄悄换成下划线，那会变成另一个模块 ID）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OverviewModuleId {
    /// 当前偏好（主题 / 字体 / 启动页 / 默认倍速四张卡）。
    Preferences,
    /// 阅读指标（首选类型 / 最长连续 / 日均时长 / 本周阅读）。
    Metrics,
    /// 每日阅读时长柱状图。
    ReadingMinutes,
    /// 作品类型分布圆环图。
    TypeShare,
    /// 阅读时段热力图。
    ReadingHeatmap,
}

impl OverviewModuleId {
    /// 当前真实的全部总览模块（默认布局与校验都用它，不引入只服务于测试的 ID）。
    pub const ALL: [Self; 5] = [
        Self::Preferences,
        Self::Metrics,
        Self::ReadingMinutes,
        Self::TypeShare,
        Self::ReadingHeatmap,
    ];

    /// 稳定文本（连字符，非下划线）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Preferences => "preferences",
            Self::Metrics => "metrics",
            Self::ReadingMinutes => "reading-minutes",
            Self::TypeShare => "type-share",
            Self::ReadingHeatmap => "reading-heatmap",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "preferences" => Some(Self::Preferences),
            "metrics" => Some(Self::Metrics),
            "reading-minutes" => Some(Self::ReadingMinutes),
            "type-share" => Some(Self::TypeShare),
            "reading-heatmap" => Some(Self::ReadingHeatmap),
            _ => None,
        }
    }
}

impl Serialize for OverviewModuleId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for OverviewModuleId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).ok_or_else(|| {
            serde::de::Error::custom(
                "总览模块 ID 必须是 preferences / metrics / reading-minutes / type-share / reading-heatmap",
            )
        })
    }
}

/// 总览模块档位（闭合集合）。档位决定模块在**三列**网格里占几列。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverviewModuleSize {
    Small,
    Medium,
    Large,
}

impl OverviewModuleSize {
    pub const ALL: [Self; 3] = [Self::Small, Self::Medium, Self::Large];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "small" => Some(Self::Small),
            "medium" => Some(Self::Medium),
            "large" => Some(Self::Large),
            _ => None,
        }
    }

    /// 该档位占用的列数（三列网格：Large 占满整行）。
    pub const fn column_span(self) -> u16 {
        match self {
            Self::Small => 1,
            Self::Medium => 2,
            Self::Large => OVERVIEW_LAYOUT_GRID_COLUMNS as u16,
        }
    }

    /// 该档位占用的行数。
    ///
    /// 当前所有档位都只占一行：档位只决定列跨度。引入两行高的档位必须先有新迁移重建
    /// 表约束，不能在领域里单方面放开。
    pub const fn row_span(self) -> u16 {
        1
    }
}

/// 总览模块放置：模块、档位与网格坐标。
///
/// `row` / `column` 是**起始格**，模块实际占用 `column..column + 档位列跨度`（所有档位
/// 都只占一行）。`order` 是布局内的稳定排序号，在同一份布局里**唯一**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "OverviewModulePlacementWire")]
pub struct OverviewModulePlacement {
    pub module: OverviewModuleId,
    pub size: OverviewModuleSize,
    pub row: u16,
    pub column: u16,
    pub order: u16,
}

impl OverviewModulePlacement {
    /// 校验构造器：行/列必须落在固定网格内，且档位跨度不得越出右侧边界。
    pub fn new(
        module: OverviewModuleId,
        size: OverviewModuleSize,
        row: u16,
        column: u16,
        order: u16,
    ) -> Result<Self, AppError> {
        if usize::from(row) >= OVERVIEW_LAYOUT_GRID_ROWS {
            return Err(invalid_overview_layout(format!(
                "总览模块行坐标必须小于 {OVERVIEW_LAYOUT_GRID_ROWS}"
            )));
        }
        if usize::from(column) + usize::from(size.column_span()) > OVERVIEW_LAYOUT_GRID_COLUMNS {
            return Err(invalid_overview_layout(format!(
                "总览模块列坐标加档位跨度不得超出 {OVERVIEW_LAYOUT_GRID_COLUMNS} 列"
            )));
        }
        if usize::from(order) >= OVERVIEW_LAYOUT_MAX_MODULES {
            return Err(invalid_overview_layout(format!(
                "总览模块排序必须小于 {OVERVIEW_LAYOUT_MAX_MODULES}"
            )));
        }
        Ok(Self {
            module,
            size,
            row,
            column,
            order,
        })
    }

    /// 起始格 `(row, column)`。
    ///
    /// 布局的不变量是**占用格子**不重叠；起始格不重复只是它的推论，光看起始格不够——
    /// `small` 从第 2 列开始、`medium` 从第 1 列开始时起始格不同，第 2 列却同属两者。
    pub const fn position(&self) -> (u16, u16) {
        (self.row, self.column)
    }

    /// 该放置**实际占用**的全部格子（含档位跨度），从左到右。
    pub fn occupied_cells(&self) -> Vec<(u16, u16)> {
        let span = self.size.column_span();
        (self.column..self.column + span)
            .map(|column| (self.row, column))
            .collect()
    }

    /// 两个放置是否占用了同一个格子（行跨度与列跨度都要比较）。
    pub fn overlaps(&self, other: &Self) -> bool {
        let rows_overlap = self.row < other.row + other.size.row_span()
            && other.row < self.row + self.size.row_span();
        let columns_overlap = self.column < other.column + other.size.column_span()
            && other.column < self.column + self.size.column_span();
        rows_overlap && columns_overlap
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct OverviewModulePlacementWire {
    module: OverviewModuleId,
    /// 必填：缺失档位必须拒绝，不得静默补 `Medium`。
    size: OverviewModuleSize,
    row: u16,
    column: u16,
    order: u16,
}

impl TryFrom<OverviewModulePlacementWire> for OverviewModulePlacement {
    type Error = AppError;

    fn try_from(wire: OverviewModulePlacementWire) -> Result<Self, Self::Error> {
        Self::new(wire.module, wire.size, wire.row, wire.column, wire.order)
    }
}

/// 总览布局：schema 版本 + 有界模块列表。
///
/// 自定义校验拒绝：非法 schema 版本、模块过多、未知模块 ID（由闭合枚举在反序列化边界
/// 拒绝）、重复模块、**占用格子重叠**（按档位跨度展开）与越界坐标；排序号唯一是稳定
/// 顺序的事实而不是约定。
/// 空模块列表是合法的（用户隐藏全部模块），但**没有布局行**不等于空布局：读取侧把缺失
/// 回落到 [`OverviewLayout::default`]，不伪造用户没做过的自定义。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "OverviewLayoutWire")]
pub struct OverviewLayout {
    pub schema_version: u32,
    pub modules: Vec<OverviewModulePlacement>,
}

impl OverviewLayout {
    /// 校验构造器：唯一的合法入口（反序列化同样走这里）。
    pub fn new(
        schema_version: u32,
        modules: Vec<OverviewModulePlacement>,
    ) -> Result<Self, AppError> {
        Self::validate_parts(schema_version, &modules)?;
        Ok(Self {
            schema_version,
            modules,
        })
    }

    /// 重新校验全局不变量（例如从存储恢复时）。
    pub fn validate(&self) -> Result<(), AppError> {
        Self::validate_parts(self.schema_version, &self.modules)
    }

    /// 与 [`Self::new`] 共享的判断主体，避免「构造与复检」两套规则漂移。
    fn validate_parts(
        schema_version: u32,
        modules: &[OverviewModulePlacement],
    ) -> Result<(), AppError> {
        if schema_version != OVERVIEW_LAYOUT_SCHEMA_VERSION {
            return Err(invalid_overview_layout(format!(
                "总览布局 schema 版本必须为 {OVERVIEW_LAYOUT_SCHEMA_VERSION}"
            )));
        }
        if modules.len() > OVERVIEW_LAYOUT_MAX_MODULES {
            return Err(invalid_overview_layout(format!(
                "总览模块不得超过 {OVERVIEW_LAYOUT_MAX_MODULES} 个"
            )));
        }
        for (index, placement) in modules.iter().enumerate() {
            let rebuilt = OverviewModulePlacement::new(
                placement.module,
                placement.size,
                placement.row,
                placement.column,
                placement.order,
            )?;
            if rebuilt != *placement {
                return Err(invalid_overview_layout("总览模块放置必须保持规范形态"));
            }
            if modules[..index]
                .iter()
                .any(|other| other.module == placement.module)
            {
                return Err(invalid_overview_layout("总览模块不得重复"));
            }
            if modules[..index]
                .iter()
                .any(|other| other.overlaps(placement))
            {
                return Err(invalid_overview_layout("总览模块占用的网格格子不得重叠"));
            }
            if modules[..index]
                .iter()
                .any(|other| other.order == placement.order)
            {
                return Err(invalid_overview_layout("总览模块的排序号不得重复"));
            }
        }
        Ok(())
    }

    /// 规范形态：模块按 `order` 升序排列。
    ///
    /// 这是「写入形态 == 读回形态」的不动点：存储读取侧本来就按 `sort_order` 返回模块，
    /// 所以写入路径先规范化，幂等比较才成立——否则「同一份布局、不同的数组顺序」会在每次
    /// 保存时都被判为变化并写出新 revision，`changed` 变成噪声。
    pub fn canonicalized(mut self) -> Self {
        self.modules.sort_by_key(|placement| placement.order);
        self
    }

    /// 按 `order` 稳定排序后的模块（读取侧消费顺序）。`order` 唯一，因此是全序。
    pub fn ordered_modules(&self) -> Vec<&OverviewModulePlacement> {
        let mut modules: Vec<&OverviewModulePlacement> = self.modules.iter().collect();
        modules.sort_by_key(|placement| placement.order);
        modules
    }
}

impl Default for OverviewLayout {
    /// 「从未自定义」的布局：当前 `SettingsOverview` 真实渲染的五个内容块，位置就是
    /// 升级前的版式——两个整宽行（当前偏好、阅读指标）、一行 2:1 的两张图、一行整宽热力图。
    fn default() -> Self {
        let modules = vec![
            OverviewModulePlacement::new(
                OverviewModuleId::Preferences,
                OverviewModuleSize::Large,
                0,
                0,
                0,
            ),
            OverviewModulePlacement::new(
                OverviewModuleId::Metrics,
                OverviewModuleSize::Large,
                1,
                0,
                1,
            ),
            OverviewModulePlacement::new(
                OverviewModuleId::ReadingMinutes,
                OverviewModuleSize::Medium,
                2,
                0,
                2,
            ),
            OverviewModulePlacement::new(
                OverviewModuleId::TypeShare,
                OverviewModuleSize::Small,
                2,
                2,
                3,
            ),
            OverviewModulePlacement::new(
                OverviewModuleId::ReadingHeatmap,
                OverviewModuleSize::Large,
                3,
                0,
                4,
            ),
        ];
        Self::new(
            OVERVIEW_LAYOUT_SCHEMA_VERSION,
            modules
                .into_iter()
                .map(|placement| placement.expect("内置默认总览模块必须合法"))
                .collect(),
        )
        .expect("内置默认总览布局必须合法")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct OverviewLayoutWire {
    schema_version: u32,
    modules: Vec<OverviewModulePlacement>,
}

impl TryFrom<OverviewLayoutWire> for OverviewLayout {
    type Error = AppError;

    fn try_from(wire: OverviewLayoutWire) -> Result<Self, Self::Error> {
        Self::new(wire.schema_version, wire.modules)
    }
}

// ---------- 资产占用（删除前的事实判定） ----------

/// 持久化外观设置里引用某个资产的位置。
///
/// 引用只存在于 `settings.appearance` 这一行 JSON 里（`customFontAssetId` 与
/// `wallpaper.assetId`），因此没有外键可以表达它。删除资产时必须先问清「谁还在用」，
/// 否则会留下一条指向已删除资产的外观设置——读路径只能报不可用，用户却看不出原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppearanceAssetReference {
    /// 界面自定义字体（`appearance.customFontAssetId`）。
    CustomFont,
    /// 首页壁纸（`appearance.wallpaper`，静态或动态）。
    Wallpaper,
}

impl AppearanceAssetReference {
    /// 全部引用位置（顺序即展示顺序：先字体，后壁纸）。
    pub const ALL: [Self; 2] = [Self::CustomFont, Self::Wallpaper];

    /// 用户可见的引用位置名。
    pub fn label(self) -> &'static str {
        match self {
            Self::CustomFont => "界面字体",
            Self::Wallpaper => "首页壁纸",
        }
    }

    /// 该引用在 `settings.appearance` 序列化 JSON 里的路径（点号分隔，逐段一个键）。
    ///
    /// 这条路径是「哪个字段持有资产 ID」的**唯一说法**，有两个消费者：
    /// 存储层的占用判定（`json_extract` 按它取引用）与领域侧的对接测试（在
    /// `serde_json` 的输出上按它再取一次）。占用判定读错路径，删除守卫就会在
    /// 「明明还有人用」的时候放行——留下一条指向已删除资产的外观设置。
    ///
    /// 路径的合法性由 serde 输出本身钉住：`appearance_asset_reference_paths_point_at_serde_output`
    /// 逐个变体解析真实序列化结果，因此改字段名而忘了改这里会在测试里暴露。
    pub const fn json_path(self) -> &'static str {
        match self {
            Self::CustomFont => "customFontAssetId",
            Self::Wallpaper => "wallpaper.assetId",
        }
    }
}

/// 沿 [`AppearanceAssetReference::json_path`] 在 JSON 值上取一段；路径不存在时返回 `None`。
///
/// 只接受对象逐段下降，不接受数组下标与转义：外观设置里持有资产 ID 的两个位置都是
/// 普通对象字段，路径一旦写成别的东西，这里就会取不到值而不是猜一个。
pub fn appearance_reference_value_at<'a>(
    value: &'a serde_json::Value,
    path: &str,
) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.as_object()?.get(segment)?;
    }
    Some(current)
}

/// 删除资产的原子结果。
///
/// 两部分事实分开报告：`deleted` 说登记行是否真的被删除，`references` 说本次删除
/// 是否被占用挡住。两者同时为真不成立——被挡住时零写入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppearanceAssetDeleteOutcome {
    /// 登记行是否被删除。`false` 表示资产本来就不存在（幂等空操作），
    /// 或本次删除被 `references` 挡住。
    pub deleted: bool,
    /// 挡住本次删除的引用位置；为空表示没有任何外观设置引用该资产。
    pub references: Vec<AppearanceAssetReference>,
}

impl AppearanceAssetDeleteOutcome {
    /// 资产不存在（幂等空操作）。
    pub fn missing() -> Self {
        Self {
            deleted: false,
            references: Vec::new(),
        }
    }

    /// 被占用挡住：零写入。
    pub fn blocked(references: Vec<AppearanceAssetReference>) -> Self {
        Self {
            deleted: false,
            references,
        }
    }

    /// 登记行已删除。
    pub fn removed() -> Self {
        Self {
            deleted: true,
            references: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASSET_UUID: &str = "0196f0d2-0000-7000-8000-0000000a0001";

    fn palette_json(extra: &str) -> String {
        let mut json = String::from(
            r##"{"background":"#000000","foreground":"#FFFFFF","card":"#111111","cardForeground":"#ffffff","popover":"#111111","popoverForeground":"#ffffff","primary":"#3366ff","primaryForeground":"#ffffff","secondary":"#222222","secondaryForeground":"#ffffff","muted":"#222222","mutedForeground":"#aaaaaa","accent":"#222222","accentForeground":"#ffffff","destructive":"#cc0000","destructiveForeground":"#ffffff","border":"#333333","input":"#333333","ring":"#3366ff""##,
        );
        json.push_str(extra);
        json.push('}');
        json
    }

    fn asset_id() -> AppearanceAssetId {
        AppearanceAssetId::parse(ASSET_UUID).unwrap()
    }

    /// 合法放置的构造快捷方式（越界坐标/跨度会在这里 panic，测试用例本身就要求它们合法）。
    fn placement(
        module: HomeModuleId,
        size: HomeModuleSize,
        row: u16,
        column: u16,
        order: u16,
    ) -> HomeModulePlacement {
        HomeModulePlacement::new(module, size, row, column, order)
            .expect("测试用例里的放置必须合法")
    }

    // ---------- 颜色 ----------

    #[test]
    fn colors_accept_uppercase_and_normalize_to_lowercase() {
        let color = HexColor::parse("#AABBCC").unwrap();
        assert_eq!(color.as_str(), "#aabbcc");
        assert_eq!(color.to_string(), "#aabbcc");
        assert_eq!(serde_json::to_string(&color).unwrap(), "\"#aabbcc\"");
        let decoded: HexColor = serde_json::from_str("\"#AABBCC\"").unwrap();
        assert_eq!(decoded, color);
        assert_eq!(color, "#aabbcc".parse::<HexColor>().unwrap());
    }

    /// 8 位写法承载现有前端 token 的真实 alpha：`--border` 是
    /// `rgba(60, 60, 67, 0.16)`（等价 `#3c3c4329`）。归一化规则与 6 位一致，
    /// 且 6 位与 8 位是**两种不同**的文本身份，不互相折叠。
    #[test]
    fn colors_accept_eight_digit_alpha_and_normalize_to_lowercase() {
        let border = HexColor::parse("#3C3C4329").unwrap();
        assert_eq!(border.as_str(), "#3c3c4329");
        assert_eq!(border.to_string(), "#3c3c4329");
        assert_eq!(serde_json::to_string(&border).unwrap(), "\"#3c3c4329\"");
        let decoded: HexColor = serde_json::from_str("\"#3C3C4329\"").unwrap();
        assert_eq!(decoded, border);
        assert_eq!(border, "#3c3c4329".parse::<HexColor>().unwrap());

        // alpha 两端（全透明 / 全不透明）同样是规范值。
        assert_eq!(HexColor::parse("#00000000").unwrap().as_str(), "#00000000");
        assert_eq!(HexColor::parse("#FFFFFFFF").unwrap().as_str(), "#ffffffff");
        assert_ne!(
            HexColor::parse("#3366ff").unwrap(),
            HexColor::parse("#3366ffff").unwrap(),
            "6 位与 8 位是不同的文本形态，不得互相折叠"
        );

        // 带 alpha 的 token 与 6 位 token 并存于同一份调色板。
        let palette: AppThemePalette = serde_json::from_str(
            &palette_json("").replace(r##""border":"#333333""##, r##""border":"#3C3C4329""##),
        )
        .unwrap();
        assert_eq!(palette.border.as_str(), "#3c3c4329");
        assert_eq!(palette.input.as_str(), "#333333");
    }

    #[test]
    fn colors_reject_non_rrggbb_forms() {
        for invalid in [
            "",
            "#",
            "#fff",
            "#ffff",
            "#fffff",
            "#fffffff",
            "#fffffffff",
            "aabbcc",
            "aabbccdd",
            " #aabbcc",
            "#aabbcc ",
            "#aabbccdd ",
            "#gggggg",
            "#gggggggg",
            "#-12345",
            "#3c3c432",
            "#3c3c43299",
            "rgb(1,2,3)",
            "rgba(60, 60, 67, 0.16)",
            "hsl(210 100% 50%)",
            "red",
            "rebeccapurple",
            "var(--primary)",
            "color-mix(in srgb, #000 10%, transparent)",
            // 颜色字段不得承载路径或 URL：本类型只表达规范十六进制，资产身份由
            // `AppearanceAssetId` 承载（路径/URL 在那里同样进不来）。
            "C:/assets/wallpaper.png",
            "file:///wallpapers/a.png",
            "https://example.com/wallpaper.png#aabbcc",
        ] {
            assert!(
                HexColor::parse(invalid).is_none(),
                "非法颜色必须被拒绝：{invalid}"
            );
            assert!(
                serde_json::from_str::<HexColor>(&format!("\"{invalid}\"")).is_err(),
                "非法颜色不得从 JSON 进入领域模型：{invalid}"
            );
        }
    }

    #[test]
    fn palette_and_theme_are_closed_and_strict() {
        let palette: AppThemePalette = serde_json::from_str(&palette_json("")).unwrap();
        assert_eq!(palette.foreground.as_str(), "#ffffff", "大写归一化为小写");

        let with_unknown =
            serde_json::from_str::<AppThemePalette>(&palette_json(r##","shadow":"#000000""##));
        assert!(with_unknown.is_err(), "未知 token 必须拒绝");

        let missing_token = palette_json("").replace(r##","ring":"#3366ff""##, "");
        assert!(
            serde_json::from_str::<AppThemePalette>(&missing_token).is_err(),
            "缺失的 token 必须拒绝，不得静默补默认值"
        );

        let theme_json = format!(
            r##"{{"light":{},"dark":{},"accentColor":"#3366FF"}}"##,
            palette_json(""),
            palette_json("")
        );
        let theme: AppTheme = serde_json::from_str(&theme_json).unwrap();
        assert_eq!(theme.accent_color.as_str(), "#3366ff");
        assert_eq!(theme.light, theme.dark);

        let unknown_theme = serde_json::from_str::<AppTheme>(&format!(
            r##"{{"light":{},"dark":{},"accentColor":"#3366ff","wallpaper":"x"}}"##,
            palette_json(""),
            palette_json("")
        ));
        assert!(unknown_theme.is_err(), "未知主题字段必须拒绝");

        let bad_accent = serde_json::from_str::<AppTheme>(&format!(
            r##"{{"light":{},"dark":{},"accentColor":"#12345"}}"##,
            palette_json(""),
            palette_json("")
        ));
        assert!(bad_accent.is_err(), "强调色同样必须严格 #rrggbb");

        let roundtrip: AppTheme =
            serde_json::from_str(&serde_json::to_string(&theme).unwrap()).unwrap();
        assert_eq!(roundtrip, theme);
    }

    // ---------- 资产 ----------

    #[test]
    fn asset_id_is_opaque_and_canonical() {
        let id = AppearanceAssetId::new();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{id}\""));
        assert_eq!(AppearanceAssetId::parse(&id.to_string()).unwrap(), id);

        for invalid in [
            "",
            "C:/wallpapers/a.png",
            "https://example.com/a.png",
            "0196f0d2-0000-7000-8000-0000000A0001",
            "0196f0d20000700080000000a0001",
            "{0196f0d2-0000-7000-8000-0000000a0001}",
            "file:///wallpapers/a.png",
        ] {
            assert!(
                AppearanceAssetId::parse(invalid).is_none(),
                "非规范 UUID 必须被拒绝：{invalid}"
            );
            assert!(
                serde_json::from_str::<AppearanceAssetId>(&format!("\"{invalid}\"")).is_err(),
                "非规范 UUID 不得从 JSON 进入领域模型：{invalid}"
            );
        }
    }

    #[test]
    fn wallpapers_only_reference_opaque_asset_ids() {
        assert_eq!(
            serde_json::from_str::<WallpaperSelection>(r#"{"kind":"none"}"#).unwrap(),
            WallpaperSelection::None
        );
        let static_wallpaper: WallpaperSelection =
            serde_json::from_str(&format!(r#"{{"kind":"static","assetId":"{ASSET_UUID}"}}"#))
                .unwrap();
        assert_eq!(static_wallpaper.asset_id(), Some(asset_id()));
        assert_eq!(static_wallpaper.kind_str(), "static");
        let dynamic_wallpaper: WallpaperSelection =
            serde_json::from_str(&format!(r#"{{"kind":"dynamic","assetId":"{ASSET_UUID}"}}"#))
                .unwrap();
        assert_eq!(dynamic_wallpaper.kind_str(), "dynamic");

        // 形状必须自洽：none 不带资产，static/dynamic 必须带资产。
        for invalid in [
            r#"{"kind":"none","assetId":"0196f0d2-0000-7000-8000-0000000a0001"}"#.to_owned(),
            r#"{"kind":"static"}"#.to_owned(),
            r#"{"kind":"dynamic"}"#.to_owned(),
            r#"{"kind":"video","assetId":"0196f0d2-0000-7000-8000-0000000a0001"}"#.to_owned(),
            r#"{"kind":"static","assetId":"C:/wallpapers/a.png"}"#.to_owned(),
            r#"{"kind":"static","assetId":"https://example.com/a.png"}"#.to_owned(),
            format!(r#"{{"kind":"static","assetId":"{ASSET_UUID}","path":"C:/a.png"}}"#),
            format!(r#"{{"kind":"static","assetId":"{ASSET_UUID}","url":"https://a"}}"#),
        ] {
            assert!(
                serde_json::from_str::<WallpaperSelection>(&invalid).is_err(),
                "非法壁纸选择必须拒绝：{invalid}"
            );
        }

        let roundtrip: WallpaperSelection =
            serde_json::from_str(&serde_json::to_string(&static_wallpaper).unwrap()).unwrap();
        assert_eq!(roundtrip, static_wallpaper);
        assert_eq!(
            serde_json::from_str::<WallpaperSelection>(
                &serde_json::to_string(&WallpaperSelection::None).unwrap()
            )
            .unwrap(),
            WallpaperSelection::None
        );
    }

    #[test]
    fn asset_metadata_enforces_per_kind_byte_limits() {
        assert_eq!(AppearanceAssetKind::Font.max_bytes(), MAX_FONT_BYTES);
        assert!(MAX_DYNAMIC_WALLPAPER_BYTES > MAX_STATIC_WALLPAPER_BYTES);

        let font = AppearanceAssetMetadata::new(
            asset_id(),
            AppearanceAssetKind::Font,
            AssetValidationState::Validated,
            MAX_FONT_BYTES,
            Some("  思源宋体  ".to_owned()),
        )
        .unwrap();
        assert_eq!(
            font.display_name.as_deref(),
            Some("思源宋体"),
            "展示名修剪首尾空白"
        );
        font.validate().unwrap();

        assert!(
            AppearanceAssetMetadata::new(
                asset_id(),
                AppearanceAssetKind::Font,
                AssetValidationState::Validated,
                MAX_FONT_BYTES + 1,
                None,
            )
            .is_err(),
            "字体超出上限必须拒绝"
        );
        assert!(
            AppearanceAssetMetadata::new(
                asset_id(),
                AppearanceAssetKind::StaticWallpaper,
                AssetValidationState::Validated,
                MAX_DYNAMIC_WALLPAPER_BYTES,
                None,
            )
            .is_err(),
            "静态壁纸不得使用动态壁纸的上限"
        );
        assert!(
            AppearanceAssetMetadata::new(
                asset_id(),
                AppearanceAssetKind::DynamicWallpaper,
                AssetValidationState::Validated,
                MAX_DYNAMIC_WALLPAPER_BYTES,
                None,
            )
            .is_ok(),
            "动态壁纸允许更大的体积上限"
        );
        assert!(
            AppearanceAssetMetadata::new(
                asset_id(),
                AppearanceAssetKind::DynamicWallpaper,
                AssetValidationState::Validated,
                MAX_DYNAMIC_WALLPAPER_BYTES + 1,
                None,
            )
            .is_err(),
            "动态壁纸超出上限必须拒绝（上限本身可接受，见上一条）"
        );
        assert!(
            AppearanceAssetMetadata::new(
                asset_id(),
                AppearanceAssetKind::Font,
                AssetValidationState::Pending,
                0,
                None,
            )
            .is_err(),
            "体积必须为正"
        );
        assert!(
            AppearanceAssetMetadata::new(
                asset_id(),
                AppearanceAssetKind::Font,
                AssetValidationState::Pending,
                1,
                Some("a".repeat(MAX_ASSET_DISPLAY_NAME_CHARS + 1)),
            )
            .is_err(),
            "展示名超长必须拒绝"
        );
        assert!(
            AppearanceAssetMetadata::new(
                asset_id(),
                AppearanceAssetKind::Font,
                AssetValidationState::Pending,
                1,
                Some("坏\u{7f}名字".to_owned()),
            )
            .is_err(),
            "展示名含控制字符必须拒绝"
        );
        assert!(
            AppearanceAssetMetadata::new(
                asset_id(),
                AppearanceAssetKind::Font,
                AssetValidationState::Pending,
                1,
                Some("   ".to_owned()),
            )
            .unwrap()
            .display_name
            .is_none(),
            "空白展示名等价于没有展示名"
        );
    }

    #[test]
    fn asset_metadata_decode_goes_through_the_validating_constructor() {
        assert!(
            serde_json::from_str::<AppearanceAssetMetadata>(&format!(
                r#"{{"id":"{ASSET_UUID}","kind":"font","state":"validated","byteSize":0}}"#
            ))
            .is_err(),
            "反序列化不得绕过校验构造器"
        );
        assert!(
            serde_json::from_str::<AppearanceAssetMetadata>(&format!(
                r#"{{"id":"{ASSET_UUID}","kind":"video","state":"validated","byteSize":10}}"#
            ))
            .is_err(),
            "未知资产种类必须拒绝"
        );
        assert!(
            serde_json::from_str::<AppearanceAssetMetadata>(&format!(
                r#"{{"id":"{ASSET_UUID}","kind":"font","state":"unknown","byteSize":10}}"#
            ))
            .is_err(),
            "未知校验状态必须拒绝"
        );
        assert!(
            serde_json::from_str::<AppearanceAssetMetadata>(&format!(
                r#"{{"id":"{ASSET_UUID}","kind":"font","state":"validated","byteSize":10,"path":"C:/a.ttf"}}"#
            ))
            .is_err(),
            "未知字段（例如路径）必须拒绝"
        );

        // 缺失 state 不是「旧数据」而是形状不完整：本阶段类型严格闭合，
        // 不做「缺失即 pending」的静默补齐。
        assert!(
            serde_json::from_str::<AppearanceAssetMetadata>(&format!(
                r#"{{"id":"{ASSET_UUID}","kind":"static_wallpaper","byteSize":1024}}"#
            ))
            .is_err(),
            "缺失 state 必须拒绝，不得静默补 pending"
        );
        // 显式写出的 state 照常接受，且不伪造「已校验」。
        let pending: AppearanceAssetMetadata = serde_json::from_str(&format!(
            r#"{{"id":"{ASSET_UUID}","kind":"static_wallpaper","state":"pending","byteSize":1024}}"#
        ))
        .unwrap();
        assert_eq!(pending.state, AssetValidationState::Pending);
        assert!(!pending.state.is_usable());
        assert_eq!(pending.display_name, None);

        let json = serde_json::to_string(&pending).unwrap();
        assert!(json.contains("\"byteSize\":1024"), "{json}");
        assert!(!json.contains("displayName"), "未设置的展示名不进入 wire");
        let roundtrip: AppearanceAssetMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip, pending);
    }

    /// 展示名的首尾空白必须与 045 迁移的 `display_name` CHECK 同集合：
    /// 这里钉住 `str::trim` 的真实空白集合（Unicode `White_Space`），迁移里
    /// `trim(display_name, char(...))` 写的就是同一组码位；控制字符用
    /// [`has_opaque_control_character`] 的集合（C0 0x00..=0x1f 与 DEL 0x7f），
    /// 格式/双向控制字符用 [`has_display_name_format_control`] 的 18 个码位
    /// （见 `display_name_rejects_format_and_bidi_control_characters`）。
    /// 任一端集合漂移都会让本测试与 `migration_045_display_name_*` 同时失败。
    #[test]
    fn display_name_trims_exactly_unicode_white_space() {
        // Unicode White_Space 全集（与 045 迁移里的 char(...) 列表逐项对应）。
        const UNICODE_WHITE_SPACE: [u32; 25] = [
            0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x20, 0x85, 0xa0, 0x1680, 0x2000, 0x2001, 0x2002, 0x2003,
            0x2004, 0x2005, 0x2006, 0x2007, 0x2008, 0x2009, 0x200a, 0x2028, 0x2029, 0x202f, 0x205f,
            0x3000,
        ];
        // 与空白「长得像」但不是 White_Space、也不是格式/双向控制字符：两端都必须
        // 原样保留，否则领域修剪了 SQL 不修剪（或反之）的码位，就会漂移。
        // （0x200b / 0x2060 / 0xfeff 也曾列在这里，现在它们由
        // `display_name_rejects_format_and_bidi_control_characters` 负责拒绝。）
        const NOT_WHITE_SPACE: [u32; 2] = [0x00ad, 0x180e];

        let name = |raw: String| {
            AppearanceAssetMetadata::new(
                asset_id(),
                AppearanceAssetKind::Font,
                AssetValidationState::Pending,
                1,
                Some(raw),
            )
        };

        for code_point in UNICODE_WHITE_SPACE {
            let character = char::from_u32(code_point).unwrap();
            assert!(
                character.is_whitespace(),
                "U+{code_point:04X} 必须是 Unicode White_Space，否则两端集合已经漂移"
            );
            let padded = format!("{character}思源宋体{character}");
            let metadata = name(padded).unwrap();
            assert_eq!(
                metadata.display_name.as_deref(),
                Some("思源宋体"),
                "U+{code_point:04X} 作为首尾空白必须被修剪"
            );
            metadata.validate().unwrap();
            // 全空白修剪后为空 → 「没有展示名」（045 同样只接受 NULL）。
            assert_eq!(
                name(character.to_string()).unwrap().display_name,
                None,
                "U+{code_point:04X} 单字符全空白等价于没有展示名"
            );
        }

        for code_point in NOT_WHITE_SPACE {
            let character = char::from_u32(code_point).unwrap();
            assert!(
                !character.is_whitespace(),
                "U+{code_point:04X} 不是 White_Space，两端都不得修剪"
            );
            let raw = format!("{character}名{character}");
            assert_eq!(
                name(raw.clone()).unwrap().display_name.as_deref(),
                Some(raw.as_str()),
                "U+{code_point:04X} 不是空白，必须原样保留"
            );
        }
    }

    /// 展示名的控制字符集合：C0（0x00..=0x1f）与 DEL（0x7f）在任何位置都拒绝，
    /// 与 045 迁移的 `instr(display_name, char(0))` + `NOT GLOB('*[..]*')` 同集合。
    ///
    /// 序列上唯一要留意的是：领域先修剪首尾空白、再判控制字符，所以**首尾位置**上
    /// 同时属于 White_Space 的 0x09..=0x0d 会被修剪掉（`"\t名"` → `"名"`）。
    /// 落库的是修剪后的形态，SQL 又只校验落库形态，因此两端仍是一致的：
    /// 库里永远不会出现任何含 C0/DEL 的展示名。
    #[test]
    fn display_name_rejects_c0_control_characters_and_del() {
        let name = |raw: String| {
            AppearanceAssetMetadata::new(
                asset_id(),
                AppearanceAssetKind::Font,
                AssetValidationState::Pending,
                1,
                Some(raw),
            )
        };
        let accepted_name = |raw: String| {
            name(raw)
                .expect("首尾的空白类控制字符必须被修剪，而不是让整个名字失效")
                .display_name
        };

        for code_point in (0x00u32..=0x1f).chain(std::iter::once(0x7f)) {
            let character = char::from_u32(code_point).unwrap();
            // 中间位置：任何 C0/DEL 都拒绝（修剪救不了它）。
            assert!(
                name(format!("名{character}字")).is_err(),
                "U+{code_point:04X} 出现在中间时必须拒绝"
            );
            if (0x09..=0x0d).contains(&code_point) {
                // 首尾位置：这些码位同时是 Unicode White_Space，先被修剪。
                for raw in [
                    format!("{character}名"),
                    format!("名{character}"),
                    format!(" {character}名 "),
                ] {
                    assert_eq!(
                        accepted_name(raw.clone()).as_deref(),
                        Some("名"),
                        "U+{code_point:04X} 在首尾时必须只是被修剪：{raw:?}"
                    );
                }
            } else {
                // 其余 C0 与 DEL 不是空白：藏在首尾或普通空白里同样拒绝。
                for raw in [
                    format!("{character}名"),
                    format!("名{character}"),
                    format!("  {character}名  "),
                ] {
                    assert!(
                        name(raw.clone()).is_err(),
                        "U+{code_point:04X} 必须拒绝：{raw:?}"
                    );
                }
            }
        }
    }

    /// 展示名里会**伪造视觉顺序或隐藏可见文本**的格式/双向控制字符：任何位置都拒绝，
    /// 与 045 迁移的 `NOT GLOB` 字符类同集合（18 个码位）。
    ///
    /// 与 C0/DEL 的测试一样，序列上唯一要留意的是 U+2028/U+2029 同时属于
    /// Unicode `White_Space`：它们在**首尾**只是被修剪（`"\u{2028}名"` → `"名"`），
    /// 只有中间位置才以格式字符被拒绝。落库的是修剪后的形态，SQL 又只校验落库形态，
    /// 两端因此仍然一致。
    #[test]
    fn display_name_rejects_format_and_bidi_control_characters() {
        // 逐项对应 045 迁移里的 char(...) 字符类，以及领域里的
        // `has_display_name_format_control`。
        const FORMAT_CONTROLS: [u32; 18] = [
            0x200b, 0x200c, 0x200d, 0x200e, 0x200f, // 零宽与双向标记
            0x2028, 0x2029, // 行分隔符 / 段分隔符
            0x202a, 0x202b, 0x202c, 0x202d, 0x202e, // 双向嵌入/覆盖
            0x2060, // 无宽不换行
            0x2066, 0x2067, 0x2068, 0x2069, // 双向隔离
            0xfeff, // BOM / 零宽不换行
        ];

        let name = |raw: String| {
            AppearanceAssetMetadata::new(
                asset_id(),
                AppearanceAssetKind::Font,
                AssetValidationState::Pending,
                1,
                Some(raw),
            )
        };

        for code_point in FORMAT_CONTROLS {
            let character = char::from_u32(code_point).unwrap();
            assert!(
                has_display_name_format_control(character),
                "U+{code_point:04X} 必须属于格式/双向控制字符集合"
            );
            // 这个集合与共享的 C0/DEL 边界互不重叠：`has_opaque_control_character`
            // 的语义没有被本阶段改动。
            assert!(
                !has_opaque_control_character(&character.to_string()),
                "U+{code_point:04X} 不得被算进共享的 C0/DEL 边界"
            );
            assert!(
                name(format!("名{character}字")).is_err(),
                "U+{code_point:04X} 出现在中间时必须拒绝"
            );
            if character.is_whitespace() {
                // U+2028/U+2029 同时是 White_Space：首尾只是被修剪。
                for raw in [
                    format!("{character}名"),
                    format!("名{character}"),
                    format!(" {character}名 "),
                ] {
                    assert_eq!(
                        name(raw.clone()).unwrap().display_name.as_deref(),
                        Some("名"),
                        "U+{code_point:04X} 在首尾时必须只是被修剪：{raw:?}"
                    );
                }
            } else {
                // 其余格式字符不是空白：藏在首尾或普通空白里同样拒绝。
                for raw in [
                    format!("{character}名"),
                    format!("名{character}"),
                    format!("  {character}名  "),
                ] {
                    assert!(
                        name(raw.clone()).is_err(),
                        "U+{code_point:04X} 必须拒绝：{raw:?}"
                    );
                }
            }
        }

        // 反向：C0/DEL 不进格式字符集合，两个集合各自守自己的码位。
        for code_point in (0x00u32..=0x1f).chain(std::iter::once(0x7f)) {
            assert!(
                !has_display_name_format_control(char::from_u32(code_point).unwrap()),
                "U+{code_point:04X} 属于共享的 C0/DEL 边界，不得被格式字符集合接管"
            );
        }
        assert!(has_opaque_control_character("\u{7f}"));
        assert!(!has_opaque_control_character("\u{200b}"));

        // 相邻但不在清单里的码位不受影响：清单是精确的，不是「所有 0x20xx 都拒绝」。
        assert!(!has_display_name_format_control('\u{200a}'));
        assert!(!has_display_name_format_control('\u{2027}'));
        assert!(!has_display_name_format_control('\u{2065}'));
        assert!(!has_display_name_format_control('\u{fe00}'));
        assert_eq!(
            name("名\u{2065}字".to_owned())
                .unwrap()
                .display_name
                .as_deref(),
            Some("名\u{2065}字"),
            "不在清单里的码位必须原样保留"
        );
    }

    #[test]
    fn asset_and_module_enums_parse_only_closed_values() {
        for kind in AppearanceAssetKind::ALL {
            assert_eq!(AppearanceAssetKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(AppearanceAssetKind::parse("wallpaper"), None);
        for state in AssetValidationState::ALL {
            assert_eq!(AssetValidationState::parse(state.as_str()), Some(state));
        }
        assert_eq!(AssetValidationState::parse("ok"), None);
        for module in HomeModuleId::ALL {
            assert_eq!(HomeModuleId::parse(module.as_str()), Some(module));
        }
        assert_eq!(
            HomeModuleId::parse("shelf_favorites"),
            None,
            "下划线不是合法模块 ID"
        );
        assert_eq!(HomeModuleId::parse("trending"), None);
        for size in HomeModuleSize::ALL {
            assert_eq!(HomeModuleSize::parse(size.as_str()), Some(size));
        }
        assert_eq!(HomeModuleSize::parse("huge"), None);
    }

    /// 断言一个闭合枚举值的 wire 形态恰好是给定的 `as_str` 文本，且该文本能读回它。
    fn assert_wire_round_trip<T>(value: T, wire: &str)
    where
        T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        assert_eq!(
            serde_json::to_string(&value).unwrap(),
            format!("\"{wire}\""),
            "{value:?} 的序列化形态必须就是它的 as_str"
        );
        assert_eq!(
            serde_json::from_str::<T>(&format!("\"{wire}\"")).unwrap(),
            value,
            "{wire} 必须能读回 {value:?}"
        );
    }

    /// 四个闭合枚举的 wire 值必须**就是** `as_str`：序列化写出 `as_str`，反序列化只认
    /// `as_str`。`#[serde(rename)]` / `rename_all` 里的字面量一旦与 `as_str` 漂移，
    /// 或者有人给某个变体加了默认回落，这里立刻失败。
    #[test]
    fn closed_enums_round_trip_through_their_as_str_wire_values() {
        for kind in AppearanceAssetKind::ALL {
            assert_wire_round_trip(kind, kind.as_str());
        }
        for state in AssetValidationState::ALL {
            assert_wire_round_trip(state, state.as_str());
        }
        for module in HomeModuleId::ALL {
            assert_wire_round_trip(module, module.as_str());
        }
        for size in HomeModuleSize::ALL {
            assert_wire_round_trip(size, size.as_str());
        }

        // 逐项钉住字面量：wire 值的稳定性由这里和 045 迁移的 CHECK 一起保证
        // （`shelf-favorites` 是连字符，不是下划线）。
        assert_wire_round_trip(AppearanceAssetKind::Font, "font");
        assert_wire_round_trip(AppearanceAssetKind::StaticWallpaper, "static_wallpaper");
        assert_wire_round_trip(AppearanceAssetKind::DynamicWallpaper, "dynamic_wallpaper");
        assert_wire_round_trip(AssetValidationState::Pending, "pending");
        assert_wire_round_trip(AssetValidationState::Validated, "validated");
        assert_wire_round_trip(AssetValidationState::Rejected, "rejected");
        assert_wire_round_trip(HomeModuleId::Continue, "continue");
        assert_wire_round_trip(HomeModuleId::RecentlyAdded, "recently_added");
        assert_wire_round_trip(HomeModuleId::ShelfFavorites, "shelf-favorites");
        assert_wire_round_trip(HomeModuleSize::Small, "small");
        assert_wire_round_trip(HomeModuleSize::Medium, "medium");
        assert_wire_round_trip(HomeModuleSize::Large, "large");
    }

    /// 未知值不得从 JSON 进入领域模型：四个闭合枚举的反序列化都必须拒绝，不能落成
    /// 某个默认变体，也不能被当成「旧版本字段」放行。
    #[test]
    fn closed_enums_reject_unknown_wire_values() {
        for invalid in [
            "",
            " ",
            "Font",
            "FONT",
            "font ",
            " Font",
            "static-wallpaper",
            "shelf_favorites",
            "trending",
            "huge",
        ] {
            assert!(
                serde_json::from_str::<AppearanceAssetKind>(&format!("\"{invalid}\"")).is_err(),
                "未知资产种类必须拒绝：{invalid:?}"
            );
            assert!(
                serde_json::from_str::<AssetValidationState>(&format!("\"{invalid}\"")).is_err(),
                "未知校验状态必须拒绝：{invalid:?}"
            );
            assert!(
                serde_json::from_str::<HomeModuleId>(&format!("\"{invalid}\"")).is_err(),
                "未知首页模块必须拒绝：{invalid:?}"
            );
            assert!(
                serde_json::from_str::<HomeModuleSize>(&format!("\"{invalid}\"")).is_err(),
                "未知档位必须拒绝：{invalid:?}"
            );
        }

        // 非字符串形态同样不是合法 wire（枚举没有数字/布尔/null 表示）。
        for raw in ["1", "true", "null", "[]", "{}"] {
            assert!(
                serde_json::from_str::<AppearanceAssetKind>(raw).is_err(),
                "非字符串 wire 必须拒绝：{raw}"
            );
            assert!(
                serde_json::from_str::<AssetValidationState>(raw).is_err(),
                "非字符串 wire 必须拒绝：{raw}"
            );
            assert!(
                serde_json::from_str::<HomeModuleId>(raw).is_err(),
                "非字符串 wire 必须拒绝：{raw}"
            );
            assert!(
                serde_json::from_str::<HomeModuleSize>(raw).is_err(),
                "非字符串 wire 必须拒绝：{raw}"
            );
        }
    }

    // ---------- 首页布局 ----------

    #[test]
    fn default_home_layout_is_the_three_real_modules() {
        let layout = HomeLayout::default();
        assert_eq!(layout.schema_version, HOME_LAYOUT_SCHEMA_VERSION);
        let modules: Vec<(HomeModuleId, HomeModuleSize, u16, u16, u16)> = layout
            .modules
            .iter()
            .map(|placement| {
                (
                    placement.module,
                    placement.size,
                    placement.row,
                    placement.column,
                    placement.order,
                )
            })
            .collect();
        assert_eq!(
            modules,
            vec![
                (HomeModuleId::Continue, HomeModuleSize::Medium, 0, 0, 0),
                (HomeModuleId::RecentlyAdded, HomeModuleSize::Medium, 0, 2, 1),
                (
                    HomeModuleId::ShelfFavorites,
                    HomeModuleSize::Medium,
                    1,
                    0,
                    2
                ),
            ],
            "默认布局必须是当前真实的三个首页模块，位置稳定"
        );
        assert_eq!(layout.ordered_modules().len(), 3);
        layout.validate().unwrap();
        assert_eq!(
            serde_json::from_str::<HomeLayout>(&serde_json::to_string(&layout).unwrap()).unwrap(),
            layout
        );
    }

    #[test]
    fn home_layout_rejects_duplicates_positions_and_bad_versions() {
        let continue_module =
            HomeModulePlacement::new(HomeModuleId::Continue, HomeModuleSize::Medium, 0, 0, 0)
                .unwrap();
        let duplicate_module =
            HomeModulePlacement::new(HomeModuleId::Continue, HomeModuleSize::Small, 1, 0, 1)
                .unwrap();
        let duplicate_position =
            HomeModulePlacement::new(HomeModuleId::RecentlyAdded, HomeModuleSize::Medium, 0, 0, 1)
                .unwrap();

        assert!(
            HomeLayout::new(
                HOME_LAYOUT_SCHEMA_VERSION,
                vec![continue_module.clone(), duplicate_module]
            )
            .is_err(),
            "重复模块必须拒绝"
        );
        assert!(
            HomeLayout::new(
                HOME_LAYOUT_SCHEMA_VERSION,
                vec![continue_module.clone(), duplicate_position]
            )
            .is_err(),
            "重复网格位置必须拒绝"
        );
        assert!(
            HomeLayout::new(0, vec![continue_module.clone()]).is_err(),
            "schema 版本 0 必须拒绝"
        );
        assert!(
            HomeLayout::new(
                HOME_LAYOUT_SCHEMA_VERSION + 1,
                vec![continue_module.clone()]
            )
            .is_err(),
            "未知 schema 版本必须拒绝"
        );
        assert!(
            HomeLayout::new(
                HOME_LAYOUT_SCHEMA_VERSION,
                (0..(HOME_LAYOUT_MAX_MODULES as u16 + 1))
                    .map(|index| HomeModulePlacement {
                        module: HomeModuleId::Continue,
                        size: HomeModuleSize::Small,
                        row: index,
                        column: 0,
                        order: index,
                    })
                    .collect()
            )
            .is_err(),
            "模块过多必须拒绝"
        );

        // 越界坐标必须在放置层就被拒绝。
        assert!(
            HomeModulePlacement::new(
                HomeModuleId::Continue,
                HomeModuleSize::Small,
                HOME_LAYOUT_GRID_ROWS as u16,
                0,
                0,
            )
            .is_err()
        );
        assert!(
            HomeModulePlacement::new(
                HomeModuleId::Continue,
                HomeModuleSize::Small,
                0,
                HOME_LAYOUT_GRID_COLUMNS as u16,
                0,
            )
            .is_err()
        );
        assert!(
            HomeModulePlacement::new(
                HomeModuleId::Continue,
                HomeModuleSize::Small,
                0,
                HOME_LAYOUT_GRID_COLUMNS as u16 - 1,
                HOME_LAYOUT_MAX_MODULES as u16,
            )
            .is_err()
        );
        assert!(
            HomeModulePlacement::new(
                HomeModuleId::ShelfFavorites,
                HomeModuleSize::Large,
                0,
                HOME_LAYOUT_GRID_COLUMNS as u16 - 2,
                0,
            )
            .is_err(),
            "档位跨度越出右侧边界必须拒绝"
        );
        assert!(
            HomeModulePlacement::new(HomeModuleId::ShelfFavorites, HomeModuleSize::Large, 0, 0, 0,)
                .is_ok()
        );

        // 反序列化同样走校验：重复模块/位置与未知模块 ID 都不能从 JSON 进来。
        assert!(
            serde_json::from_str::<HomeLayout>(
                r#"{"schemaVersion":1,"modules":[
                    {"module":"continue","size":"small","row":0,"column":0,"order":0},
                    {"module":"continue","size":"small","row":1,"column":0,"order":1}]}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<HomeLayout>(
                r#"{"schemaVersion":1,"modules":[
                    {"module":"continue","size":"small","row":0,"column":0,"order":0},
                    {"module":"recently_added","size":"small","row":0,"column":0,"order":1}]}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<HomeLayout>(
                r#"{"schemaVersion":1,"modules":[
                    {"module":"shelf_favorites","size":"small","row":0,"column":0,"order":0}]}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<HomeLayout>(
                r#"{"schemaVersion":2,"modules":[
                    {"module":"continue","size":"small","row":0,"column":0,"order":0}]}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<HomeLayout>(
                r#"{"schemaVersion":1,"modules":[
                    {"module":"continue","size":"small","row":0,"column":0,"order":0,"width":2}]}"#
            )
            .is_err(),
            "未知放置字段必须拒绝"
        );
    }

    #[test]
    fn home_layout_rejects_module_payloads_without_size() {
        // 缺失 size 不是「旧 payload」而是形状不完整：本阶段类型严格闭合，
        // 不做「缺失即 medium」的静默补齐。
        assert!(
            serde_json::from_str::<HomeLayout>(
                r#"{"schemaVersion":1,"modules":[
                    {"module":"continue","row":0,"column":0,"order":0}]}"#
            )
            .is_err(),
            "缺失 size 必须拒绝，不得静默补 medium"
        );

        // 显式写出的档位照常接受，且与默认布局一致。
        let layout: HomeLayout = serde_json::from_str(
            r#"{"schemaVersion":1,"modules":[
                {"module":"continue","size":"medium","row":0,"column":0,"order":0},
                {"module":"recently_added","size":"medium","row":0,"column":2,"order":1},
                {"module":"shelf-favorites","size":"medium","row":1,"column":0,"order":2}]}"#,
        )
        .unwrap();
        assert_eq!(layout, HomeLayout::default());

        // 空模块列表是合法的显式自定义（用户隐藏了全部模块），
        // 它与「没有布局行」不同：后者由读取侧回落到默认布局。
        let empty: HomeLayout =
            serde_json::from_str(r#"{"schemaVersion":1,"modules":[]}"#).unwrap();
        assert!(empty.modules.is_empty());
        empty.validate().unwrap();
    }

    /// 档位就是列跨度：Small=1、Medium=2、Large=整行 4 列，且**每个档位都只占一行**。
    ///
    /// 045 的越界 CHECK（`column_index + 1/2/4 <= 4`）与占用格触发器写的是同一组数字，
    /// 这里逐项钉住，避免领域与 SQL 各自漂移成两套网格。
    #[test]
    fn home_module_sizes_span_columns_and_only_one_row() {
        assert_eq!(HomeModuleSize::Small.column_span(), 1);
        assert_eq!(HomeModuleSize::Medium.column_span(), 2);
        assert_eq!(HomeModuleSize::Large.column_span(), 4);
        assert_eq!(
            HomeModuleSize::Large.column_span(),
            HOME_LAYOUT_GRID_COLUMNS as u16,
            "Large 必须正好占满整行"
        );
        for size in HomeModuleSize::ALL {
            assert_eq!(size.row_span(), 1, "{size:?} 当前只占一行");
            assert!(
                size.column_span() <= HOME_LAYOUT_GRID_COLUMNS as u16,
                "{size:?} 的列跨度不得超出网格宽度"
            );
        }
    }

    /// 占用格子按档位跨度展开：`column` 是**起始格**，模块占的是
    /// `column..column + 列跨度` 这一整段，并且只占 `row` 这一行。
    #[test]
    fn placement_occupied_cells_expand_by_size_span() {
        let cells = |size: HomeModuleSize, row: u16, column: u16| {
            placement(HomeModuleId::Continue, size, row, column, 0).occupied_cells()
        };

        assert_eq!(cells(HomeModuleSize::Small, 0, 0), vec![(0, 0)]);
        assert_eq!(cells(HomeModuleSize::Small, 3, 3), vec![(3, 3)]);
        assert_eq!(cells(HomeModuleSize::Medium, 1, 0), vec![(1, 0), (1, 1)]);
        assert_eq!(cells(HomeModuleSize::Medium, 4, 2), vec![(4, 2), (4, 3)]);
        assert_eq!(
            cells(HomeModuleSize::Large, 2, 0),
            vec![(2, 0), (2, 1), (2, 2), (2, 3)],
            "Large 占满整行"
        );
        // 每个档位都只占一行：占用格的行坐标恒等于起始行。
        for (size, row, column) in [
            (HomeModuleSize::Small, 5, 0),
            (HomeModuleSize::Medium, 5, 0),
            (HomeModuleSize::Large, 5, 0),
        ] {
            assert!(
                cells(size, row, column)
                    .iter()
                    .all(|&(occupied_row, _)| occupied_row == row),
                "{size:?} 的占用格必须全部落在起始行上"
            );
        }
        assert_eq!(
            cells(HomeModuleSize::Large, 5, 0).len(),
            HOME_LAYOUT_GRID_COLUMNS,
            "整行就是网格的全部列"
        );
    }

    /// 重叠判定必须按**实际占用格**展开，而不是只比起始格。
    ///
    /// `medium` 从第 1 列开始占 `(0,0)(0,1)`，`small` 从第 2 列开始占 `(0,1)`：
    /// 两者的**起始格不同**，第 2 列却同时属于两者。只比较起始格的唯一性检查
    /// （例如 `UNIQUE (row_index, column_index)`）对这种情况完全无感。
    ///
    /// 045 的触发器表达同一条不变量
    /// （`trg_appearance_home_modules_cell_overlap_insert` / `_update`），
    /// 对应的 SQL 侧证据是 `migration_045_rejects_span_expanded_home_module_overlap`。
    #[test]
    fn home_layout_rejects_span_expanded_cell_overlap() {
        let medium_head = placement(HomeModuleId::Continue, HomeModuleSize::Medium, 0, 0, 0);
        let small_tail_cell =
            placement(HomeModuleId::RecentlyAdded, HomeModuleSize::Small, 0, 1, 1);
        let whole_row = placement(HomeModuleId::Continue, HomeModuleSize::Large, 0, 0, 0);
        let small_last_cell =
            placement(HomeModuleId::RecentlyAdded, HomeModuleSize::Small, 0, 3, 1);

        // 先把判定本身钉住：起始格不同，占用格却相交。
        assert_ne!(medium_head.position(), small_tail_cell.position());
        assert!(medium_head.overlaps(&small_tail_cell));
        assert_ne!(whole_row.position(), small_last_cell.position());
        assert!(whole_row.overlaps(&small_last_cell));

        let overlap = HomeLayout::new(
            HOME_LAYOUT_SCHEMA_VERSION,
            vec![medium_head.clone(), small_tail_cell],
        )
        .expect_err("跨档位的占用格重叠必须拒绝");
        assert_eq!(overlap.code().as_str(), APPEARANCE_INVALID_HOME_LAYOUT);

        assert!(
            HomeLayout::new(HOME_LAYOUT_SCHEMA_VERSION, vec![whole_row, small_last_cell]).is_err(),
            "Large 占满整行，同一行的任何模块都与它重叠"
        );

        // 反序列化同样不能绕过：起始格不同的重叠布局不得从 JSON 进入领域模型。
        assert!(
            serde_json::from_str::<HomeLayout>(
                r#"{"schemaVersion":1,"modules":[
                    {"module":"continue","size":"medium","row":0,"column":0,"order":0},
                    {"module":"recently_added","size":"small","row":0,"column":1,"order":1}]}"#
            )
            .is_err(),
            "跨档位的占用格重叠不得从 JSON 进入领域模型"
        );

        // 相邻而不相交是合法的：medium 占 (0,0)(0,1)，另一个 medium 从第 3 列开始占
        // (0,2)(0,3)，正好接上。这条守住「把网格位置比较得过头」的过度拒绝。
        let adjacent = HomeLayout::new(
            HOME_LAYOUT_SCHEMA_VERSION,
            vec![
                medium_head,
                placement(HomeModuleId::RecentlyAdded, HomeModuleSize::Medium, 0, 2, 1),
            ],
        )
        .expect("相邻而不相交必须合法");
        adjacent.validate().unwrap();
        assert_eq!(adjacent.ordered_modules().len(), 2);

        // 换一行就不重叠：两个 Large 各占一整行。
        let stacked = HomeLayout::new(
            HOME_LAYOUT_SCHEMA_VERSION,
            vec![
                placement(HomeModuleId::Continue, HomeModuleSize::Large, 0, 0, 0),
                placement(HomeModuleId::RecentlyAdded, HomeModuleSize::Large, 1, 0, 1),
            ],
        )
        .expect("不同行的整行模块不得被判为重叠");
        stacked.validate().unwrap();
    }

    /// `order` 在同一份布局里必须唯一：并列排序号会让读取侧的顺序变成
    /// 「存储返回顺序」这个偶然事实。域内全局校验与 045 的 `UNIQUE (sort_order)`
    /// 是同一件事的两端。
    #[test]
    fn home_layout_rejects_duplicate_order() {
        let first =
            HomeModulePlacement::new(HomeModuleId::Continue, HomeModuleSize::Small, 0, 0, 0)
                .unwrap();
        let second =
            HomeModulePlacement::new(HomeModuleId::RecentlyAdded, HomeModuleSize::Small, 1, 0, 0)
                .unwrap();

        let duplicate = HomeLayout::new(HOME_LAYOUT_SCHEMA_VERSION, vec![first, second])
            .expect_err("并列 order 必须拒绝");
        assert_eq!(duplicate.code().as_str(), APPEARANCE_INVALID_HOME_LAYOUT);
        // 反序列化同样不能绕过全局校验。
        assert!(
            serde_json::from_str::<HomeLayout>(
                r#"{"schemaVersion":1,"modules":[
                    {"module":"continue","size":"small","row":0,"column":0,"order":0},
                    {"module":"recently_added","size":"small","row":1,"column":0,"order":0}]}"#
            )
            .is_err(),
            "并列 order 不得从 JSON 进入领域模型"
        );

        // 排序号只要求唯一，不要求连续：单个模块可以用任何合法 order。
        let sparse =
            HomeModulePlacement::new(HomeModuleId::Continue, HomeModuleSize::Small, 0, 0, 7)
                .unwrap();
        let sparse_layout = HomeLayout::new(HOME_LAYOUT_SCHEMA_VERSION, vec![sparse]).unwrap();
        sparse_layout.validate().unwrap();
        assert_eq!(sparse_layout.ordered_modules().len(), 1);
    }

    /// 规范形态按 `order` 升序，且是**幂等**的：同一份布局无论以什么数组顺序提交，
    /// 规范化后都落到同一个值。写入路径靠它让「同一份布局重复保存」不再被判为变化
    /// （`changed` 不抖动），因为存储读取侧本来就按 `sort_order` 返回模块。
    #[test]
    fn home_layout_canonicalized_sorts_by_order_and_is_idempotent() {
        let continue_module = placement(HomeModuleId::Continue, HomeModuleSize::Small, 0, 0, 0);
        let recently_added = placement(HomeModuleId::RecentlyAdded, HomeModuleSize::Small, 1, 0, 1);
        let shelf = placement(HomeModuleId::ShelfFavorites, HomeModuleSize::Small, 2, 0, 2);

        // 存储读回的形态（order 升序）就是规范形态。
        let read_back = HomeLayout::new(
            HOME_LAYOUT_SCHEMA_VERSION,
            vec![
                continue_module.clone(),
                recently_added.clone(),
                shelf.clone(),
            ],
        )
        .unwrap();
        let canonical = read_back.clone().canonicalized();
        assert_eq!(canonical, read_back);
        let ordered: Vec<HomeModulePlacement> =
            read_back.ordered_modules().into_iter().cloned().collect();
        assert_eq!(canonical.modules, ordered);

        // 乱序提交（order 与数组顺序不一致）规范化后与读回形态**相等**。
        let out_of_order = HomeLayout::new(
            HOME_LAYOUT_SCHEMA_VERSION,
            vec![shelf, continue_module, recently_added],
        )
        .unwrap();
        assert_ne!(out_of_order.modules, read_back.modules);
        assert_eq!(out_of_order.clone().canonicalized(), read_back);

        // 规范化是幂等的，且不破坏任何被校验的不变量。
        let once = out_of_order.canonicalized();
        assert_eq!(once.clone().canonicalized(), once);
        once.validate().unwrap();

        // 空布局的规范形态仍是空布局（空 ≠ 从未保存，那是读取侧的事实）。
        let empty = HomeLayout::new(HOME_LAYOUT_SCHEMA_VERSION, vec![]).unwrap();
        assert!(empty.clone().canonicalized().modules.is_empty());
    }

    /// 公开的展示名校验入口与 `AppearanceAssetMetadata::new` 是**同一条规则**：
    /// 服务层在复制字节之前用它拦非法展示名，因此这里必须与构造器逐项一致。
    #[test]
    fn public_display_name_normalization_matches_the_constructor() {
        for raw in [
            None,
            Some("".to_owned()),
            Some("   ".to_owned()),
            Some("  思源宋体  ".to_owned()),
            Some("名\u{7f}字".to_owned()),
            Some("名\u{200b}字".to_owned()),
            Some("a".repeat(MAX_ASSET_DISPLAY_NAME_CHARS)),
            Some("a".repeat(MAX_ASSET_DISPLAY_NAME_CHARS + 1)),
            Some("名\u{200b}字\u{feff}".to_owned()),
        ] {
            let normalized = normalize_asset_display_name(raw.clone());
            let constructed = AppearanceAssetMetadata::new(
                asset_id(),
                AppearanceAssetKind::Font,
                AssetValidationState::Pending,
                1,
                raw.clone(),
            )
            .map(|metadata| metadata.display_name);

            match (&normalized, &constructed) {
                (Ok(expected), Ok(actual)) => assert_eq!(expected, actual),
                (Err(expected), Err(actual)) => {
                    assert_eq!(expected.code().as_str(), actual.code().as_str())
                }
                _ => panic!("公开入口与构造器结论不一致：{raw:?}"),
            }
        }
    }

    // ---------- 总览布局 ----------

    /// 总览模块的构造快捷方式（越界坐标/跨度会在这里 panic，用例本身要求它们合法）。
    fn overview(
        module: OverviewModuleId,
        size: OverviewModuleSize,
        row: u16,
        column: u16,
        order: u16,
    ) -> OverviewModulePlacement {
        OverviewModulePlacement::new(module, size, row, column, order)
            .expect("测试用例里的总览放置必须合法")
    }

    fn overview_layout(placements: Vec<OverviewModulePlacement>) -> OverviewLayout {
        OverviewLayout::new(OVERVIEW_LAYOUT_SCHEMA_VERSION, placements).unwrap()
    }

    /// 默认总览布局必须就是 `SettingsOverview` 当前渲染的五个内容块，且位置**逐格**
    /// 复现升级前的版式：两行整宽、一行 2:1 的两张图、一行整宽热力图。
    ///
    /// 这条断言是「默认布局与升级前视觉等价」在领域侧的唯一证据：3 列网格存在的理由
    /// 就是 2:1 这一档（medium 2 列 + small 1 列）。改成 4 列网格时这里会立刻失败。
    #[test]
    fn default_overview_layout_reproduces_the_current_overview_screen() {
        let layout = OverviewLayout::default();
        assert_eq!(layout.schema_version, OVERVIEW_LAYOUT_SCHEMA_VERSION);

        let modules: Vec<(OverviewModuleId, OverviewModuleSize, u16, u16, u16)> = layout
            .modules
            .iter()
            .map(|placement| {
                (
                    placement.module,
                    placement.size,
                    placement.row,
                    placement.column,
                    placement.order,
                )
            })
            .collect();
        assert_eq!(
            modules,
            vec![
                (
                    OverviewModuleId::Preferences,
                    OverviewModuleSize::Large,
                    0,
                    0,
                    0
                ),
                (
                    OverviewModuleId::Metrics,
                    OverviewModuleSize::Large,
                    1,
                    0,
                    1
                ),
                (
                    OverviewModuleId::ReadingMinutes,
                    OverviewModuleSize::Medium,
                    2,
                    0,
                    2
                ),
                (
                    OverviewModuleId::TypeShare,
                    OverviewModuleSize::Small,
                    2,
                    2,
                    3
                ),
                (
                    OverviewModuleId::ReadingHeatmap,
                    OverviewModuleSize::Large,
                    3,
                    0,
                    4
                ),
            ],
            "默认总览布局必须是当前真实的五个内容块，且位置与升级前逐格一致"
        );
        assert_eq!(layout.ordered_modules().len(), 5);
        layout.validate().unwrap();
        let encoded = serde_json::to_string(&layout).unwrap();
        assert_eq!(
            serde_json::from_str::<OverviewLayout>(&encoded).unwrap(),
            layout
        );

        // 两张图在**同一行**并排，且宽度比是 2:1（medium 占 2 列、small 占 1 列）：
        // 这正是升级前 `xl:grid-cols-[minmax(0,1.92fr)_minmax(300px,1fr)]` 的形状。
        let minutes = layout
            .modules
            .iter()
            .find(|placement| placement.module == OverviewModuleId::ReadingMinutes)
            .expect("默认布局必须包含每日阅读时长");
        let shares = layout
            .modules
            .iter()
            .find(|placement| placement.module == OverviewModuleId::TypeShare)
            .expect("默认布局必须包含作品类型分布");
        assert_eq!(minutes.row, shares.row, "两张图必须在同一行");
        assert!(
            minutes.column < shares.column,
            "每日时长在左、类型分布在右（与升级前一致）"
        );
        assert_eq!(
            (minutes.size.column_span(), shares.size.column_span()),
            (2, 1),
            "两张图必须是 2:1，而不是均分"
        );
    }

    /// 档位就是三列网格里的列跨度：Small=1、Medium=2、Large=整行 3 列，且每个档位都只占一行。
    #[test]
    fn overview_module_sizes_span_three_columns_and_only_one_row() {
        assert_eq!(OVERVIEW_LAYOUT_GRID_COLUMNS, 3);
        assert_eq!(OverviewModuleSize::Small.column_span(), 1);
        assert_eq!(OverviewModuleSize::Medium.column_span(), 2);
        assert_eq!(OverviewModuleSize::Large.column_span(), 3);
        assert_eq!(
            OverviewModuleSize::Large.column_span(),
            OVERVIEW_LAYOUT_GRID_COLUMNS as u16,
            "Large 必须正好占满整行"
        );
        for size in OverviewModuleSize::ALL {
            assert_eq!(size.row_span(), 1, "{size:?} 当前只占一行");
            assert!(size.column_span() <= OVERVIEW_LAYOUT_GRID_COLUMNS as u16);
        }

        // 三列网格与首页的四列网格是两套事实：档位名字相同，跨度不同。
        assert_ne!(
            OverviewModuleSize::Large.column_span(),
            HomeModuleSize::Large.column_span(),
            "总览与首页的网格列数不同，档位跨度不得互相折叠"
        );
    }

    /// 占用格按档位跨度展开，且行坐标恒等于起始行。
    #[test]
    fn overview_placement_occupied_cells_expand_by_size_span() {
        let cells = |size: OverviewModuleSize, row: u16, column: u16| {
            overview(OverviewModuleId::Metrics, size, row, column, 0).occupied_cells()
        };
        assert_eq!(cells(OverviewModuleSize::Small, 0, 0), vec![(0, 0)]);
        assert_eq!(cells(OverviewModuleSize::Small, 3, 2), vec![(3, 2)]);
        assert_eq!(
            cells(OverviewModuleSize::Medium, 1, 0),
            vec![(1, 0), (1, 1)]
        );
        assert_eq!(
            cells(OverviewModuleSize::Medium, 4, 1),
            vec![(4, 1), (4, 2)]
        );
        assert_eq!(
            cells(OverviewModuleSize::Large, 2, 0),
            vec![(2, 0), (2, 1), (2, 2)],
            "Large 占满三列"
        );
        for (size, row, column) in [
            (OverviewModuleSize::Small, 5, 0),
            (OverviewModuleSize::Medium, 5, 0),
            (OverviewModuleSize::Large, 5, 0),
        ] {
            assert!(
                cells(size, row, column)
                    .iter()
                    .all(|&(occupied_row, _)| occupied_row == row),
                "{size:?} 的占用格必须全部落在起始行上"
            );
        }
    }

    /// 越界坐标、重复模块、重复排序号、非法 schema 版本、模块过多都必须拒绝。
    #[test]
    fn overview_layout_rejects_duplicates_positions_and_bad_versions() {
        let head = overview(
            OverviewModuleId::Preferences,
            OverviewModuleSize::Large,
            0,
            0,
            0,
        );
        // 起始格不同但压在同一格上（large 占满整行，small 落在它覆盖的最后一列）。
        let duplicate_cell = overview(
            OverviewModuleId::Metrics,
            OverviewModuleSize::Small,
            0,
            2,
            1,
        );
        let duplicate_module = overview(
            OverviewModuleId::Preferences,
            OverviewModuleSize::Small,
            1,
            0,
            1,
        );
        let duplicate_order = overview(
            OverviewModuleId::Metrics,
            OverviewModuleSize::Small,
            1,
            0,
            0,
        );

        assert!(
            OverviewLayout::new(
                OVERVIEW_LAYOUT_SCHEMA_VERSION,
                vec![head.clone(), duplicate_module]
            )
            .is_err(),
            "重复模块必须拒绝"
        );
        assert!(
            OverviewLayout::new(
                OVERVIEW_LAYOUT_SCHEMA_VERSION,
                vec![head.clone(), duplicate_order]
            )
            .is_err(),
            "并列 order 必须拒绝"
        );
        let overlap = OverviewLayout::new(
            OVERVIEW_LAYOUT_SCHEMA_VERSION,
            vec![head.clone(), duplicate_cell],
        )
        .expect_err("跨档位的占用格重叠必须拒绝");
        assert_eq!(overlap.code().as_str(), APPEARANCE_INVALID_OVERVIEW_LAYOUT);
        assert!(!overlap.retryable(), "布局非法是校验失败，重试同样会失败");

        assert!(
            OverviewLayout::new(0, vec![head.clone()]).is_err(),
            "schema 版本 0 必须拒绝"
        );
        assert!(
            OverviewLayout::new(OVERVIEW_LAYOUT_SCHEMA_VERSION + 1, vec![head.clone()]).is_err(),
            "未知 schema 版本必须拒绝"
        );
        assert!(
            OverviewLayout::new(
                OVERVIEW_LAYOUT_SCHEMA_VERSION,
                (0..(OVERVIEW_LAYOUT_MAX_MODULES as u16 + 1))
                    .map(|index| OverviewModulePlacement {
                        module: OverviewModuleId::Metrics,
                        size: OverviewModuleSize::Small,
                        row: index,
                        column: 0,
                        order: index,
                    })
                    .collect()
            )
            .is_err(),
            "模块过多必须拒绝"
        );

        // 越界坐标在放置层就被拒绝。
        assert!(
            OverviewModulePlacement::new(
                OverviewModuleId::Metrics,
                OverviewModuleSize::Small,
                OVERVIEW_LAYOUT_GRID_ROWS as u16,
                0,
                0,
            )
            .is_err()
        );
        assert!(
            OverviewModulePlacement::new(
                OverviewModuleId::Metrics,
                OverviewModuleSize::Small,
                0,
                OVERVIEW_LAYOUT_GRID_COLUMNS as u16,
                0,
            )
            .is_err()
        );
        assert!(
            OverviewModulePlacement::new(
                OverviewModuleId::Metrics,
                OverviewModuleSize::Medium,
                0,
                OVERVIEW_LAYOUT_GRID_COLUMNS as u16 - 1,
                0,
            )
            .is_err(),
            "medium 的跨度在最后一列会越出右侧边界"
        );
        assert!(
            OverviewModulePlacement::new(
                OverviewModuleId::Metrics,
                OverviewModuleSize::Large,
                0,
                0,
                OVERVIEW_LAYOUT_MAX_MODULES as u16,
            )
            .is_err()
        );
        assert!(
            OverviewModulePlacement::new(
                OverviewModuleId::Metrics,
                OverviewModuleSize::Medium,
                0,
                1,
                0,
            )
            .is_ok(),
            "medium 从第 2 列开始正好占满整行"
        );
    }

    /// 反序列化同样走校验：未知模块 ID、缺失 size、越界坐标与重叠布局都进不来；
    /// 空模块列表是合法的显式自定义。
    #[test]
    fn overview_layout_decoding_goes_through_the_validating_constructor() {
        // 缺失 size 不是「旧 payload」而是形状不完整。
        assert!(
            serde_json::from_str::<OverviewLayout>(
                r#"{"schemaVersion":1,"modules":[
                    {"module":"metrics","row":0,"column":0,"order":0}]}"#
            )
            .is_err(),
            "缺失 size 必须拒绝，不得静默补 medium"
        );
        // 首页的模块 ID 不是总览模块 ID：两套闭合集合互不通用。
        assert!(
            serde_json::from_str::<OverviewLayout>(
                r#"{"schemaVersion":1,"modules":[
                    {"module":"shelf-favorites","size":"small","row":0,"column":0,"order":0}]}"#
            )
            .is_err(),
            "首页模块不得进入总览布局"
        );
        // 下划线写法不是合法 wire 值（连字符才是）。
        assert!(
            serde_json::from_str::<OverviewLayout>(
                r#"{"schemaVersion":1,"modules":[
                    {"module":"type_share","size":"small","row":0,"column":0,"order":0}]}"#
            )
            .is_err(),
            "下划线不是合法总览模块 ID"
        );
        // 未知放置字段必须拒绝。
        assert!(
            serde_json::from_str::<OverviewLayout>(
                r#"{"schemaVersion":1,"modules":[
                    {"module":"metrics","size":"small","row":0,"column":0,"order":0,"width":2}]}"#
            )
            .is_err()
        );
        // 默认布局的 wire 形态必须逐字段读回默认布局本身。
        let decoded: OverviewLayout =
            serde_json::from_str(&serde_json::to_string(&OverviewLayout::default()).unwrap())
                .unwrap();
        assert_eq!(decoded, OverviewLayout::default());

        // 空模块列表是合法的显式自定义（用户隐藏了全部模块）。
        let empty: OverviewLayout =
            serde_json::from_str(r#"{"schemaVersion":1,"modules":[]}"#).unwrap();
        assert!(empty.modules.is_empty());
        empty.validate().unwrap();
        assert!(empty.clone().canonicalized().modules.is_empty());
    }

    /// 规范形态按 `order` 升序且幂等：同一份布局无论以什么数组顺序提交，规范化后相等，
    /// 写入路径靠它让「同一份布局重复保存」不再被判为变化（`changed` 不抖动）。
    #[test]
    fn overview_layout_canonicalized_sorts_by_order_and_is_idempotent() {
        let preferences = overview(
            OverviewModuleId::Preferences,
            OverviewModuleSize::Large,
            0,
            0,
            0,
        );
        let metrics = overview(
            OverviewModuleId::Metrics,
            OverviewModuleSize::Large,
            1,
            0,
            1,
        );
        let heatmap = overview(
            OverviewModuleId::ReadingHeatmap,
            OverviewModuleSize::Large,
            2,
            0,
            2,
        );

        let read_back =
            overview_layout(vec![preferences.clone(), metrics.clone(), heatmap.clone()]);
        assert_eq!(read_back.clone().canonicalized(), read_back);
        let ordered: Vec<OverviewModulePlacement> =
            read_back.ordered_modules().into_iter().cloned().collect();
        assert_eq!(read_back.modules, ordered);

        let out_of_order = overview_layout(vec![heatmap, preferences, metrics]);
        assert_ne!(out_of_order.modules, read_back.modules);
        assert_eq!(out_of_order.clone().canonicalized(), read_back);

        let once = out_of_order.canonicalized();
        assert_eq!(once.clone().canonicalized(), once);
        once.validate().unwrap();
    }

    /// 总览模块的闭合枚举：wire 值就是 `as_str`，未知值与下划线写法一律拒绝。
    #[test]
    fn overview_module_enums_round_trip_and_reject_unknown_values() {
        for module in OverviewModuleId::ALL {
            assert_eq!(OverviewModuleId::parse(module.as_str()), Some(module));
            assert_wire_round_trip(module, module.as_str());
        }
        for size in OverviewModuleSize::ALL {
            assert_eq!(OverviewModuleSize::parse(size.as_str()), Some(size));
            assert_wire_round_trip(size, size.as_str());
        }

        // 逐项钉住字面量：连字符不是下划线。
        assert_wire_round_trip(OverviewModuleId::Preferences, "preferences");
        assert_wire_round_trip(OverviewModuleId::Metrics, "metrics");
        assert_wire_round_trip(OverviewModuleId::ReadingMinutes, "reading-minutes");
        assert_wire_round_trip(OverviewModuleId::TypeShare, "type-share");
        assert_wire_round_trip(OverviewModuleId::ReadingHeatmap, "reading-heatmap");
        assert_eq!(OverviewModuleId::parse("reading_minutes"), None);
        assert_eq!(OverviewModuleId::parse("heatmap"), None);
        // 首页的模块 ID 不属于总览。
        assert_eq!(OverviewModuleId::parse("continue"), None);
        assert_eq!(OverviewModuleId::parse("shelf-favorites"), None);

        for invalid in [
            "",
            " ",
            "Preferences",
            "type_share",
            "reading-minutes ",
            "unknown",
        ] {
            assert!(
                serde_json::from_str::<OverviewModuleId>(&format!("\"{invalid}\"")).is_err(),
                "未知总览模块必须拒绝：{invalid:?}"
            );
            assert!(
                serde_json::from_str::<OverviewModuleSize>(&format!("\"{invalid}\"")).is_err(),
                "未知总览档位必须拒绝：{invalid:?}"
            );
        }
        for raw in ["1", "true", "null", "[]", "{}"] {
            assert!(serde_json::from_str::<OverviewModuleId>(raw).is_err());
            assert!(serde_json::from_str::<OverviewModuleSize>(raw).is_err());
        }
    }

    /// 隐藏全部模块是**显式空布局**，与「从未保存过」由 revision 区分（存储层的事实）。
    #[test]
    fn empty_overview_layout_is_a_real_saved_state() {
        let empty = overview_layout(vec![]);
        assert!(empty.modules.is_empty());
        empty.validate().unwrap();
        assert!(empty.ordered_modules().is_empty());
        assert_ne!(
            empty,
            OverviewLayout::default(),
            "空布局不是默认布局：两者的模块集合不同"
        );
    }

    // ---------- 资产引用路径（删除守卫的契约） ----------

    /// 声明的引用路径必须在 `AppearanceSettings` 的 **serde 输出**上真的取得到值。
    ///
    /// 这是「占用判定读的路径」与「外观设置真正写下的 JSON」之间唯一的一道锁：存储层按
    /// 同一条路径做 `json_extract`，字段一旦改名而路径没跟着改，删除守卫就会在
    /// 「明明还有人用」时放行。所以这里不比对字符串常量，而是解析一份真实的序列化结果。
    #[test]
    fn appearance_asset_reference_paths_point_at_serde_output() {
        let font_id = AppearanceAssetId::new();
        let wallpaper_id = AppearanceAssetId::new();
        let settings = crate::settings::AppearanceSettings {
            custom_font_asset_id: Some(font_id),
            wallpaper: WallpaperSelection::Static(wallpaper_id),
            ..crate::settings::AppearanceSettings::default()
        };
        let json = serde_json::to_value(&settings).unwrap();

        let expected = [
            (AppearanceAssetReference::CustomFont, font_id),
            (AppearanceAssetReference::Wallpaper, wallpaper_id),
        ];
        for (reference, asset_id) in expected {
            let expected_id = asset_id.to_string();
            let value =
                appearance_reference_value_at(&json, reference.json_path()).unwrap_or_else(|| {
                    panic!(
                        "路径 {} 在序列化的外观设置里取不到值：{json}",
                        reference.json_path()
                    )
                });
            assert_eq!(
                value.as_str(),
                Some(expected_id.as_str()),
                "路径 {} 必须正好指向该资产的 ID",
                reference.json_path()
            );
        }
    }

    /// 声明集合是闭合的：每个变体都有自己的路径，且路径互不相同——两条引用共用一个路径
    /// 会让占用判定把「字体在用」和「壁纸在用」混成同一件事。
    #[test]
    fn every_asset_reference_declares_its_own_json_path() {
        let mut paths: Vec<&str> = AppearanceAssetReference::ALL
            .into_iter()
            .map(AppearanceAssetReference::json_path)
            .collect();
        assert_eq!(paths.len(), 2);
        paths.sort_unstable();
        paths.dedup();
        assert_eq!(paths.len(), 2, "两个引用位置必须各自有自己的路径");

        // 路径取不到值时返回 None（而不是造一个空值出来）。
        let empty = serde_json::json!({ "wallpaper": { "kind": "none" } });
        assert!(appearance_reference_value_at(&empty, "wallpaper.assetId").is_none());
        assert!(appearance_reference_value_at(&empty, "customFontAssetId").is_none());
        assert!(appearance_reference_value_at(&empty, "wallpaper.kind").is_some());
    }

    // ---------- Native 选择器的扩展名过滤 ----------

    /// 选择器按种类过滤用到的扩展名：与各类资产真正接受的格式同集合。
    ///
    /// 这不是校验（判定在存储层按字节签名做），但两者必须是同一份清单：选择器多列一个
    /// 格式，用户就会先选中一个必然被拒的文件；少列一个，能导入的文件在对话框里看不见。
    #[test]
    fn picker_extensions_cover_exactly_the_accepted_formats() {
        assert_eq!(
            AppearanceAssetKind::Font.picker_extensions(),
            ["ttf", "otf", "woff", "woff2"]
        );
        assert_eq!(
            AppearanceAssetKind::StaticWallpaper.picker_extensions(),
            ["png", "jpg", "jpeg", "webp"]
        );
        assert_eq!(
            AppearanceAssetKind::DynamicWallpaper.picker_extensions(),
            ["mp4", "webm"]
        );

        // 静态与动态两条路径的格式集合不重叠：GIF / APNG / 动画 WebP 不在任何一侧。
        for kind in AppearanceAssetKind::ALL {
            let extensions = kind.picker_extensions();
            assert!(!extensions.is_empty());
            assert!(
                extensions.iter().all(|extension| {
                    !extension.is_empty()
                        && !extension.bytes().any(|byte| byte.is_ascii_uppercase())
                }),
                "{kind:?} 的扩展名必须是小写规范形态"
            );
            let unique: std::collections::HashSet<&&str> = extensions.iter().collect();
            assert_eq!(unique.len(), extensions.len(), "{kind:?} 的扩展名不得重复");
        }
    }

    // ---------- 文件名词干 → 展示名 ----------

    /// 文件名是外部输入：归一化之后必须落在展示名的合法形态里，而**不是**报错。
    ///
    /// 「名字不好看」可以接受，「一次导入因为文件名而失败」不可以——所以每一步都是收敛
    /// 而不是拒绝，只有「归一化之后什么都不剩」才如实变成「没有展示名」。
    #[test]
    fn file_stems_normalize_into_legal_display_names() {
        // 正常文件名原样保留（含空格与中文）。
        assert_eq!(
            appearance_display_name_from_file_stem("思源宋体 变体"),
            Some("思源宋体 变体".to_owned())
        );
        // 零宽字符（伪顺序/隐藏文本）直接剔除，而不是让整个名字非法。
        assert_eq!(
            appearance_display_name_from_file_stem("思源\u{200b}宋体"),
            Some("思源宋体".to_owned())
        );
        assert_eq!(
            appearance_display_name_from_file_stem("名\u{feff}字"),
            Some("名字".to_owned())
        );
        // 控制字符当分隔符：连续空白并成一个空格，首尾的整个去掉。
        assert_eq!(
            appearance_display_name_from_file_stem("  my\u{7f}font  "),
            Some("my font".to_owned())
        );
        assert_eq!(
            appearance_display_name_from_file_stem("a\n\n\tb"),
            Some("a b".to_owned())
        );
        // 超长按字符数截断到上限之内，而不是被判成非法。
        let long = "字".repeat(MAX_ASSET_DISPLAY_NAME_CHARS * 2);
        let truncated = appearance_display_name_from_file_stem(&long).unwrap();
        assert_eq!(truncated.chars().count(), MAX_ASSET_DISPLAY_NAME_CHARS);
        // 归一化之后什么都不剩 → 没有展示名（界面上回落到「资产 <短前缀>」）。
        assert_eq!(appearance_display_name_from_file_stem("   "), None);
        assert_eq!(
            appearance_display_name_from_file_stem("\u{200b}\u{feff}"),
            None
        );
        assert_eq!(appearance_display_name_from_file_stem("\u{7f}"), None);
        assert_eq!(appearance_display_name_from_file_stem(""), None);

        // 返回值必须是落库形态的不动点：再进一次校验原样通过。
        for stem in ["思源宋体 变体", "my font", long.as_str()] {
            let normalized = appearance_display_name_from_file_stem(stem).unwrap();
            assert_eq!(
                normalize_asset_display_name(Some(normalized.clone())).unwrap(),
                Some(normalized),
                "归一化结果必须是 bounded_display_name 的不动点"
            );
        }
    }
}
