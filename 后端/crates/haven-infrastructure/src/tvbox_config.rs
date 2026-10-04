//! TVBox / FongMi JSON 配置解析（纯数据切片）。
//!
//! 这个模块只做一件事：把一份**不可信**的 TVBox/FongMi 配置正文解析成仅后端可见的
//! 结构化数据。它不联网、不落库、不注册来源、不生成 IPC DTO，也不执行配置里声明的
//! 任何第三方代码。
//!
//! Intent lock（本切片不越界）：
//! - 只解析 UTF-8（可带 BOM）JSON。空输入、非法 UTF-8、非法 JSON、顶层不是对象、
//!   超过字节/条目/header/未知字段上限时，返回带稳定错误码的 [`AppError`]，
//!   绝不尝试“修复”或静默截断。
//! - TVBox 里形如 `csp_*` 的 api、以及 `.jar` / `.js` / `.py` 指向的实现，只被**识别
//!   并标记**，永远不会被下载、加载或执行。数字 `type` 的语义没有协议证据，因此原样
//!   保留字面量，绝不映射成内部能力或“支持/不支持”的结论。
//! - 未知字段原样保留在后端对象里（整行 JSON 都保留）；对外摘要只暴露经
//!   `expose_field_name` 过滤后的字段名与 JSON 形态，不含任何字段值。
//! - 展示标签另外拒绝双向覆盖/隔离符与零宽不可见格式控制字符（如 U+202E、U+200B），
//!   避免「看到的名字」与真实字符串不一致；这类标签整条不进入摘要，原文只留在后端。
//! - 错误信息、日志和摘要里都不出现配置中的 URL、凭据、Cookie、Authorization 或
//!   token 值：所有面向外部的字符串都经过 `expose_field_name` / `expose_label`
//!   过滤，所有错误文案要么是固定字符串，要么只含行号。
//! - 可能承载原始配置值的结构体**不实现 `Debug`**（也不实现 `Serialize`），
//!   避免被顺手写进日志或 IPC。摘要结构体只承载已过滤文本，因此可以安全地
//!   `Debug`。
//!
//! 本轮明确不覆盖（留给后续批次）：配置下载/缓存/ETag/last-known-good、opaque site ID、
//! 持久化、IPC 命令与 ACL、站点协议运行（Native JSON/XML/CMS）、M3U/Channel/Group/EPG
//! 投影、相对 URL 解析、site `ext` 内嵌 JSON 的解码。

use serde_json::{Map, Value};

use haven_common::{AppError, ErrorKind};

// ---------- 上限 ----------

/// 单个配置正文的字节上限（8 MiB）。
pub const MAX_CONFIG_BYTES: usize = 8 * 1024 * 1024;
/// 站点行数上限。
pub const MAX_SITES: usize = 1_000;
/// 直播源行数上限。
pub const MAX_LIVES: usize = 1_000;
/// 外部解析器行数上限。
pub const MAX_PARSES: usize = 200;
/// 单行 header 条数上限。
pub const MAX_HEADERS_PER_ROW: usize = 32;
/// 单行未知字段条数上限。
pub const MAX_UNKNOWN_FIELDS_PER_ROW: usize = 128;
/// 顶层保留字段（已识别但未结构化 + 未知）的条数上限。
pub const MAX_TOP_LEVEL_FIELDS: usize = 128;
/// 摘要里标签/字面量的最大字符数。
pub const MAX_EXPOSED_TEXT_CHARS: usize = 120;
/// 摘要里字段名的最大字符数。
pub const MAX_EXPOSED_FIELD_NAME_CHARS: usize = 64;

// ---------- 稳定错误码 ----------

/// 空输入（或只有 BOM/空白）。
pub const CODE_EMPTY: &str = "TVBOX_CONFIG_EMPTY";
/// 正文超过 [`MAX_CONFIG_BYTES`]。
pub const CODE_TOO_LARGE: &str = "TVBOX_CONFIG_TOO_LARGE";
/// 正文不是合法 UTF-8（例如 UTF-16 或二进制）。
pub const CODE_NOT_UTF8: &str = "TVBOX_CONFIG_NOT_UTF8";
/// 正文不是合法 JSON。
pub const CODE_INVALID_JSON: &str = "TVBOX_CONFIG_INVALID_JSON";
/// 顶层不是 JSON 对象。
pub const CODE_NOT_OBJECT: &str = "TVBOX_CONFIG_NOT_OBJECT";
/// 已知字段的 JSON 形态不符（例如 `sites` 不是数组、`headers` 不是对象）。
pub const CODE_FIELD_SHAPE: &str = "TVBOX_CONFIG_FIELD_SHAPE";
/// 站点行数超过 [`MAX_SITES`]。
pub const CODE_TOO_MANY_SITES: &str = "TVBOX_CONFIG_TOO_MANY_SITES";
/// 直播源行数超过 [`MAX_LIVES`]。
pub const CODE_TOO_MANY_LIVES: &str = "TVBOX_CONFIG_TOO_MANY_LIVES";
/// 解析器行数超过 [`MAX_PARSES`]。
pub const CODE_TOO_MANY_PARSES: &str = "TVBOX_CONFIG_TOO_MANY_PARSES";
/// 单行 header 条数超过 [`MAX_HEADERS_PER_ROW`]。
pub const CODE_TOO_MANY_HEADERS: &str = "TVBOX_CONFIG_TOO_MANY_HEADERS";
/// 单行未知字段条数超过 [`MAX_UNKNOWN_FIELDS_PER_ROW`]。
pub const CODE_TOO_MANY_UNKNOWN_FIELDS: &str = "TVBOX_CONFIG_TOO_MANY_UNKNOWN_FIELDS";
/// 顶层保留字段条数超过 [`MAX_TOP_LEVEL_FIELDS`]。
pub const CODE_TOO_MANY_TOP_LEVEL_FIELDS: &str = "TVBOX_CONFIG_TOO_MANY_TOP_LEVEL_FIELDS";

// ---------- 键集合 ----------

/// 顶层结构化字段（本模块真正建模的部分）。
const STRUCTURED_TOP_LEVEL_KEYS: &[&str] = &["spider", "sites", "lives", "parses"];
/// 顶层已识别但本轮不结构化的字段：保留原值，只暴露字段名与形态。
const OPAQUE_TOP_LEVEL_KEYS: &[&str] = &[
    "wallpaper",
    "rules",
    "flags",
    "doh",
    "ads",
    "logo",
    "epg",
    "danmaku",
];

const SITE_KNOWN_KEYS: &[&str] = &[
    "name",
    "api",
    "type",
    "ext",
    "searchable",
    "quickSearch",
    "filterable",
    "headers",
    "header",
    "ua",
    "referer",
];

const LIVE_KNOWN_KEYS: &[&str] = &[
    "name", "url", "type", "epg", "logo", "ua", "referer", "headers", "header",
];

const PARSER_KNOWN_KEYS: &[&str] = &["name", "type", "url", "ext", "header", "headers", "ua"];

/// 单行 header 的取值顺序。解析与摘要必须共用同一组常量，否则 `headers: null`
/// 会在两边得到不同结果（一边回退到 `header`，另一边得到空列表）。
const SITE_HEADER_KEYS: &[&str] = &["headers", "header"];
const LIVE_HEADER_KEYS: &[&str] = &["headers", "header"];
const PARSER_HEADER_KEYS: &[&str] = &["header", "headers"];

/// 命中即不允许出现在任何摘要/日志里的字段名片段（小写、去掉分隔符后比较）。
///
/// 只覆盖“名称本身就可能泄露凭据语义”的词；值从来不会被暴露，因此这里是额外一层。
const SENSITIVE_NAME_MARKERS: &[&str] = &[
    "auth",
    "cookie",
    "token",
    "secret",
    "password",
    "passwd",
    "credential",
    "bearer",
    "apikey",
    "key",
    "sign",
    "session",
];

// ---------- 基础类型 ----------

/// JSON 形态。只描述“是什么形状”，不含任何值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TvboxJsonShape {
    Null,
    Bool,
    Number,
    Text,
    Array,
    Object,
}

impl TvboxJsonShape {
    pub fn from_value(value: &Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(_) => Self::Bool,
            Value::Number(_) => Self::Number,
            Value::String(_) => Self::Text,
            Value::Array(_) => Self::Array,
            Value::Object(_) => Self::Object,
        }
    }
}

/// 配置里 `type` 字段的**字面量**，不做任何语义解释。
///
/// TVBox 的数字 `type` 语义未经协议证据确认，因此这里只保留原文：`3` 就是 `3`，
/// 不会被翻译成任何内部能力。
#[derive(Clone, PartialEq)]
pub enum TvboxRawType {
    Integer(i64),
    Float(f64),
    Text(String),
    /// 数组/对象/布尔/null 等非数字非文本形态，只记录形态。
    Other(TvboxJsonShape),
}

impl TvboxRawType {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Number(number) => match number.as_i64() {
                Some(integer) => Self::Integer(integer),
                None => match number.as_f64() {
                    Some(float) => Self::Float(float),
                    None => Self::Other(TvboxJsonShape::Number),
                },
            },
            Value::String(text) => Self::Text(text.clone()),
            other => Self::Other(TvboxJsonShape::from_value(other)),
        }
    }

    pub fn shape(&self) -> TvboxJsonShape {
        match self {
            Self::Integer(_) | Self::Float(_) => TvboxJsonShape::Number,
            Self::Text(_) => TvboxJsonShape::Text,
            Self::Other(shape) => *shape,
        }
    }

    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(integer) => Some(*integer),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }

    /// 安全投影：只保留形态与经过过滤的字面量文本。
    pub fn summary(&self) -> TvboxRawTypeSummary {
        match self {
            Self::Integer(integer) => TvboxRawTypeSummary {
                shape: TvboxJsonShape::Number,
                literal: Some(integer.to_string()),
            },
            Self::Float(float) => TvboxRawTypeSummary {
                shape: TvboxJsonShape::Number,
                literal: Some(float.to_string()),
            },
            Self::Text(text) => TvboxRawTypeSummary {
                shape: TvboxJsonShape::Text,
                literal: expose_label(text),
            },
            Self::Other(shape) => TvboxRawTypeSummary {
                shape: *shape,
                literal: None,
            },
        }
    }
}

/// 第三方实现类型。识别到即代表“需要执行第三方代码”，本模块**只标记不执行**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TvboxImplementationKind {
    /// `.jar`（TVBox Spider 容器）。
    Jar,
    /// `.js`（远程 JavaScript）。
    JavaScript,
    /// `.py`（远程 Python）。
    Python,
    /// `.json`（spider 清单，不是可执行代码）。
    JsonManifest,
    /// 有 references 但无法从形态判断；保持未识别，不猜。
    Unidentified,
}

/// 站点行的结构性分类。分类只依据 api 的**形态**，不依据数字 `type`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TvboxSiteKind {
    /// `api` 是 http(s) 端点；协议细节（JSON/XML/CMS）本轮不判定。
    HttpEndpoint,
    /// `api` 使用 spider 协议前缀（`csp_*`），运行需要第三方代码；本轮不执行。
    Spider(TvboxImplementationKind),
    /// 证据不足或形态未知。保留原文，不猜语义。
    Unclassified,
}

// ---------- 结构化行 ----------

/// 一对方便后端发起请求的 header。值可能包含凭据，只允许后端使用。
#[derive(Clone, PartialEq)]
pub struct TvboxHeader {
    pub name: String,
    value: String,
}

impl TvboxHeader {
    /// 仅后端使用；不得写入日志、错误或摘要。
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// 站点 `ext` 字段：保留原文 + 记录形态与可执行实现指示。
#[derive(Clone, PartialEq)]
pub struct TvboxExt {
    pub shape: TvboxJsonShape,
    pub implementation: Option<TvboxImplementationKind>,
    raw: Value,
}

impl TvboxExt {
    /// 仅后端使用；不得写入日志、错误或摘要。
    pub fn retained_value(&self) -> &Value {
        &self.raw
    }
}

/// 一条站点。结构化字段之外，整行原始 JSON 也保留在后端。
#[derive(Clone, PartialEq)]
pub struct TvboxSite {
    /// 已过滤的展示名；命中敏感形态或 URL 形态时为 `None`（原文仍在保留行里）。
    pub name: Option<String>,
    /// 站点 api。可能是 http(s) 端点，也可能是 `csp_*` spider 名；仅后端使用。
    pub api: Option<String>,
    /// `type` 的字面量，不做语义映射。
    pub raw_type: Option<TvboxRawType>,
    pub ext: Option<TvboxExt>,
    pub searchable: Option<bool>,
    pub quick_search: Option<bool>,
    pub filterable: Option<bool>,
    pub headers: Vec<TvboxHeader>,
    /// 仅后端使用。
    pub user_agent: Option<String>,
    /// 仅后端使用。
    pub referer: Option<String>,
    pub kind: TvboxSiteKind,
    raw: Value,
}

impl TvboxSite {
    /// 整行原始 JSON（含未知字段与敏感值）。仅后端使用。
    pub fn retained_row(&self) -> &Value {
        &self.raw
    }

    pub fn summary(&self) -> TvboxSiteSummary {
        let (header_names, withheld_header_count) = header_summary(&self.raw, SITE_HEADER_KEYS);
        let (unknown_fields, withheld_unknown_field_count) =
            unknown_field_summaries(&self.raw, SITE_KNOWN_KEYS);
        TvboxSiteSummary {
            name: self.name.clone(),
            kind: self.kind,
            raw_type: self.raw_type.as_ref().map(TvboxRawType::summary),
            searchable: self.searchable,
            quick_search: self.quick_search,
            filterable: self.filterable,
            ext_shape: self.ext.as_ref().map(|ext| ext.shape),
            ext_implementation: self.ext.as_ref().and_then(|ext| ext.implementation),
            header_names,
            withheld_header_count,
            unknown_fields,
            withheld_unknown_field_count,
        }
    }
}

/// 一条直播源。字段只被解析成数据，不会被请求或播放。
#[derive(Clone, PartialEq)]
pub struct TvboxLive {
    pub name: Option<String>,
    /// 仅后端使用。
    pub url: Option<String>,
    pub raw_type: Option<TvboxRawType>,
    /// 仅后端使用。
    pub epg: Option<String>,
    /// 仅后端使用。
    pub logo: Option<String>,
    /// 仅后端使用。
    pub user_agent: Option<String>,
    /// 仅后端使用。
    pub referer: Option<String>,
    pub headers: Vec<TvboxHeader>,
    raw: Value,
}

impl TvboxLive {
    /// 整行原始 JSON。仅后端使用。
    pub fn retained_row(&self) -> &Value {
        &self.raw
    }

    pub fn summary(&self) -> TvboxLiveSummary {
        let (header_names, withheld_header_count) = header_summary(&self.raw, LIVE_HEADER_KEYS);
        let (unknown_fields, withheld_unknown_field_count) =
            unknown_field_summaries(&self.raw, LIVE_KNOWN_KEYS);
        TvboxLiveSummary {
            name: self.name.clone(),
            raw_type: self.raw_type.as_ref().map(TvboxRawType::summary),
            has_url: self.url.is_some(),
            has_epg: self.epg.is_some(),
            has_logo: self.logo.is_some(),
            has_user_agent: self.user_agent.is_some(),
            has_referer: self.referer.is_some(),
            header_names,
            withheld_header_count,
            unknown_fields,
            withheld_unknown_field_count,
        }
    }
}

/// 一条外部解析器。**只作为数据识别**：不调用、不缓存、不注册。
#[derive(Clone, PartialEq)]
pub struct TvboxParser {
    pub name: Option<String>,
    pub raw_type: Option<TvboxRawType>,
    /// 仅后端使用。
    pub url: Option<String>,
    pub ext: Option<TvboxExt>,
    pub headers: Vec<TvboxHeader>,
    raw: Value,
}

impl TvboxParser {
    /// 整行原始 JSON。仅后端使用。
    pub fn retained_row(&self) -> &Value {
        &self.raw
    }

    pub fn summary(&self) -> TvboxParserSummary {
        let (header_names, withheld_header_count) = header_summary(&self.raw, PARSER_HEADER_KEYS);
        let (unknown_fields, withheld_unknown_field_count) =
            unknown_field_summaries(&self.raw, PARSER_KNOWN_KEYS);
        TvboxParserSummary {
            name: self.name.clone(),
            raw_type: self.raw_type.as_ref().map(TvboxRawType::summary),
            has_url: self.url.is_some(),
            ext_shape: self.ext.as_ref().map(|ext| ext.shape),
            ext_implementation: self.ext.as_ref().and_then(|ext| ext.implementation),
            header_names,
            withheld_header_count,
            unknown_fields,
            withheld_unknown_field_count,
        }
    }
}

/// 顶层 `spider` 指示。原文（可能含 md5 摘要）只保留在后端。
#[derive(Clone, PartialEq)]
pub struct TvboxSpiderRef {
    pub kind: TvboxImplementationKind,
    /// 原文里带 `;md5;...` / `;sha256;...` 形式的完整性摘要段。摘要值本身不结构化保存。
    pub has_integrity_digest: bool,
    pub shape: TvboxJsonShape,
    reference: String,
}

impl TvboxSpiderRef {
    /// 仅后端使用；不得写入日志、错误或摘要。
    pub fn retained_reference(&self) -> &str {
        &self.reference
    }
}

/// 已识别但未结构化的顶层字段，或未知的顶层字段。
///
/// 键名本身可能敏感，因此原始键名只通过 [`Self::retained_name`] 暴露给后端。
#[derive(Clone, PartialEq)]
pub struct TvboxRetainedField {
    name: String,
    pub shape: TvboxJsonShape,
    /// 数组字段的元素个数；其它形态为 `None`。
    pub entry_count: Option<usize>,
    raw: Value,
}

impl TvboxRetainedField {
    fn new(name: &str, value: &Value) -> Self {
        Self {
            name: name.to_owned(),
            shape: TvboxJsonShape::from_value(value),
            entry_count: value.as_array().map(Vec::len),
            raw: value.clone(),
        }
    }

    /// 原始键名。仅后端使用；对外只用 `summary()`。
    pub fn retained_name(&self) -> &str {
        &self.name
    }

    /// 仅后端使用；不得写入日志、错误或摘要。
    pub fn retained_value(&self) -> &Value {
        &self.raw
    }

    fn summary(&self) -> Option<TvboxFieldSummary> {
        expose_field_name(&self.name).map(|name| TvboxFieldSummary {
            name,
            shape: self.shape,
            entry_count: self.entry_count,
        })
    }
}

/// 一份解析完成的 TVBox 配置。所有字段都是后端事实，不是 IPC DTO。
#[derive(PartialEq)]
pub struct TvboxConfig {
    spider: Option<TvboxSpiderRef>,
    sites: Vec<TvboxSite>,
    lives: Vec<TvboxLive>,
    parsers: Vec<TvboxParser>,
    opaque_top_level: Vec<TvboxRetainedField>,
    unknown_top_level: Vec<TvboxRetainedField>,
    skipped_site_rows: usize,
    skipped_live_rows: usize,
    skipped_parser_rows: usize,
}

impl TvboxConfig {
    pub fn spider(&self) -> Option<&TvboxSpiderRef> {
        self.spider.as_ref()
    }

    pub fn sites(&self) -> &[TvboxSite] {
        &self.sites
    }

    pub fn lives(&self) -> &[TvboxLive] {
        &self.lives
    }

    pub fn parsers(&self) -> &[TvboxParser] {
        &self.parsers
    }

    /// 已识别但本轮未结构化的顶层字段（wallpaper/rules/flags/doh/ads/logo/epg/danmaku）。
    pub fn opaque_top_level(&self) -> &[TvboxRetainedField] {
        &self.opaque_top_level
    }

    /// 未识别的顶层字段，原值保留。仅后端使用。
    pub fn unknown_top_level(&self) -> &[TvboxRetainedField] {
        &self.unknown_top_level
    }

    /// `sites` 数组中非对象的行数（被跳过，不静默：调用方可在摘要里看到）。
    pub fn skipped_site_rows(&self) -> usize {
        self.skipped_site_rows
    }

    pub fn skipped_live_rows(&self) -> usize {
        self.skipped_live_rows
    }

    pub fn skipped_parser_rows(&self) -> usize {
        self.skipped_parser_rows
    }

    /// 安全摘要：只含形态、计数和经过过滤的名称/标签，不含任何值。
    pub fn summary(&self) -> TvboxConfigSummary {
        let opaque_top_level_fields: Vec<TvboxFieldSummary> = self
            .opaque_top_level
            .iter()
            .filter_map(TvboxRetainedField::summary)
            .collect();
        let unknown_top_level_fields: Vec<TvboxFieldSummary> = self
            .unknown_top_level
            .iter()
            .filter_map(TvboxRetainedField::summary)
            .collect();
        TvboxConfigSummary {
            spider: self.spider.as_ref().map(|spider| spider.kind),
            spider_has_integrity_digest: self
                .spider
                .as_ref()
                .is_some_and(|spider| spider.has_integrity_digest),
            site_count: self.sites.len(),
            live_count: self.lives.len(),
            parser_count: self.parsers.len(),
            skipped_site_rows: self.skipped_site_rows,
            skipped_live_rows: self.skipped_live_rows,
            skipped_parser_rows: self.skipped_parser_rows,
            withheld_top_level_field_count: self
                .opaque_top_level
                .len()
                .saturating_sub(opaque_top_level_fields.len())
                + self
                    .unknown_top_level
                    .len()
                    .saturating_sub(unknown_top_level_fields.len()),
            opaque_top_level_fields,
            unknown_top_level_fields,
            sites: self.sites.iter().map(TvboxSite::summary).collect(),
            lives: self.lives.iter().map(TvboxLive::summary).collect(),
            parsers: self.parsers.iter().map(TvboxParser::summary).collect(),
        }
    }
}

// ---------- 摘要（唯一允许离开后端的形态） ----------

/// 一个字段的安全投影：只有名字、形态和数组长度。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TvboxFieldSummary {
    pub name: String,
    pub shape: TvboxJsonShape,
    pub entry_count: Option<usize>,
}

/// `type` 的安全投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TvboxRawTypeSummary {
    pub shape: TvboxJsonShape,
    /// 数字字面量的文本；文本字面量经过过滤，命中敏感形态时为 `None`。
    pub literal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TvboxSiteSummary {
    pub name: Option<String>,
    pub kind: TvboxSiteKind,
    pub raw_type: Option<TvboxRawTypeSummary>,
    pub searchable: Option<bool>,
    pub quick_search: Option<bool>,
    pub filterable: Option<bool>,
    pub ext_shape: Option<TvboxJsonShape>,
    pub ext_implementation: Option<TvboxImplementationKind>,
    pub header_names: Vec<String>,
    pub withheld_header_count: usize,
    pub unknown_fields: Vec<TvboxFieldSummary>,
    pub withheld_unknown_field_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TvboxLiveSummary {
    pub name: Option<String>,
    pub raw_type: Option<TvboxRawTypeSummary>,
    pub has_url: bool,
    pub has_epg: bool,
    pub has_logo: bool,
    pub has_user_agent: bool,
    pub has_referer: bool,
    pub header_names: Vec<String>,
    pub withheld_header_count: usize,
    pub unknown_fields: Vec<TvboxFieldSummary>,
    pub withheld_unknown_field_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TvboxParserSummary {
    pub name: Option<String>,
    pub raw_type: Option<TvboxRawTypeSummary>,
    pub has_url: bool,
    pub ext_shape: Option<TvboxJsonShape>,
    pub ext_implementation: Option<TvboxImplementationKind>,
    pub header_names: Vec<String>,
    pub withheld_header_count: usize,
    pub unknown_fields: Vec<TvboxFieldSummary>,
    pub withheld_unknown_field_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TvboxConfigSummary {
    pub spider: Option<TvboxImplementationKind>,
    pub spider_has_integrity_digest: bool,
    pub site_count: usize,
    pub live_count: usize,
    pub parser_count: usize,
    pub skipped_site_rows: usize,
    pub skipped_live_rows: usize,
    pub skipped_parser_rows: usize,
    pub opaque_top_level_fields: Vec<TvboxFieldSummary>,
    pub unknown_top_level_fields: Vec<TvboxFieldSummary>,
    pub withheld_top_level_field_count: usize,
    pub sites: Vec<TvboxSiteSummary>,
    pub lives: Vec<TvboxLiveSummary>,
    pub parsers: Vec<TvboxParserSummary>,
}

// ---------- 入口 ----------

/// 解析一份 TVBox/FongMi 配置正文。
///
/// 接受 UTF-8（可带 BOM）。所有失败路径都返回带稳定错误码、且不含配置内容的
/// [`AppError`]。
pub fn parse_config(bytes: &[u8]) -> Result<TvboxConfig, AppError> {
    if bytes.is_empty() {
        return Err(empty_error());
    }
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(limit_error(CODE_TOO_LARGE, "TVBox 配置超过大小上限"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| not_utf8_error())?;
    // 只剥离开头 BOM；UTF-16 会在上面就失败，不会被当作可解析文本。
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.trim().is_empty() {
        return Err(empty_error());
    }
    let root = serde_json::from_str::<Value>(text).map_err(|_| invalid_json_error())?;
    let Value::Object(root) = root else {
        return Err(not_object_error());
    };

    let (sites, skipped_site_rows) = parse_rows(
        rows_of(&root, "sites")?,
        MAX_SITES,
        CODE_TOO_MANY_SITES,
        "TVBox 配置的站点数量超过上限",
        parse_site_row,
    )?;
    let (lives, skipped_live_rows) = parse_rows(
        rows_of(&root, "lives")?,
        MAX_LIVES,
        CODE_TOO_MANY_LIVES,
        "TVBox 配置的直播源数量超过上限",
        parse_live_row,
    )?;
    let (parsers, skipped_parser_rows) = parse_rows(
        rows_of(&root, "parses")?,
        MAX_PARSES,
        CODE_TOO_MANY_PARSES,
        "TVBox 配置的解析器数量超过上限",
        parse_parser_row,
    )?;

    let spider = root.get("spider").map(parse_spider);

    // 先数一遍将要保留的顶层字段，再决定是否构造列表：避免用一份超长键名集合
    // 先把内存与后续摘要撑起来，之后才失败。
    let retained_top_level_count = root
        .keys()
        .filter(|key| !STRUCTURED_TOP_LEVEL_KEYS.contains(&key.as_str()))
        .count();
    if retained_top_level_count > MAX_TOP_LEVEL_FIELDS {
        return Err(limit_error(
            CODE_TOO_MANY_TOP_LEVEL_FIELDS,
            "TVBox 配置的顶层字段数量超过上限",
        ));
    }

    let mut opaque_top_level = Vec::new();
    let mut unknown_top_level = Vec::new();
    for (key, value) in &root {
        if OPAQUE_TOP_LEVEL_KEYS.contains(&key.as_str()) {
            opaque_top_level.push(TvboxRetainedField::new(key, value));
        } else if !STRUCTURED_TOP_LEVEL_KEYS.contains(&key.as_str()) {
            unknown_top_level.push(TvboxRetainedField::new(key, value));
        }
    }

    Ok(TvboxConfig {
        spider,
        sites,
        lives,
        parsers,
        opaque_top_level,
        unknown_top_level,
        skipped_site_rows,
        skipped_live_rows,
        skipped_parser_rows,
    })
}

// ---------- 行解析 ----------

fn rows_of<'a>(root: &'a Map<String, Value>, key: &str) -> Result<Option<&'a [Value]>, AppError> {
    match root.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(rows)) => Ok(Some(rows.as_slice())),
        Some(_) => {
            let message = format!("TVBox 配置中的 {key} 字段不是 JSON 数组");
            Err(field_shape_error(message))
        }
    }
}

fn parse_rows<T>(
    rows: Option<&[Value]>,
    limit: usize,
    limit_code: &'static str,
    limit_message: &'static str,
    parse: fn(&Map<String, Value>, usize) -> Result<T, AppError>,
) -> Result<(Vec<T>, usize), AppError> {
    let Some(rows) = rows else {
        return Ok((Vec::new(), 0));
    };
    if rows.len() > limit {
        return Err(limit_error(limit_code, limit_message));
    }
    let mut parsed = Vec::with_capacity(rows.len());
    let mut skipped = 0usize;
    for (index, row) in rows.iter().enumerate() {
        match row {
            Value::Object(map) => parsed.push(parse(map, index)?),
            // 非对象行不静默：计数后由摘要暴露给调用方。
            _ => skipped += 1,
        }
    }
    Ok((parsed, skipped))
}

fn parse_site_row(row: &Map<String, Value>, index: usize) -> Result<TvboxSite, AppError> {
    ensure_unknown_budget(row, SITE_KNOWN_KEYS)?;
    let headers = parse_headers(row, SITE_HEADER_KEYS, index)?;
    let ext = row.get("ext").map(parse_ext);
    let api = text_field(row, "api");
    Ok(TvboxSite {
        name: label_field(row, "name"),
        raw_type: row.get("type").map(TvboxRawType::from_value),
        kind: classify_site(api.as_deref(), ext.as_ref()),
        ext,
        searchable: row.get("searchable").and_then(scalar_bool),
        quick_search: row.get("quickSearch").and_then(scalar_bool),
        filterable: row.get("filterable").and_then(scalar_bool),
        headers,
        user_agent: text_field(row, "ua"),
        referer: text_field(row, "referer"),
        api,
        raw: Value::Object(row.clone()),
    })
}

fn parse_live_row(row: &Map<String, Value>, index: usize) -> Result<TvboxLive, AppError> {
    ensure_unknown_budget(row, LIVE_KNOWN_KEYS)?;
    Ok(TvboxLive {
        name: label_field(row, "name"),
        url: text_field(row, "url"),
        raw_type: row.get("type").map(TvboxRawType::from_value),
        epg: text_field(row, "epg"),
        logo: text_field(row, "logo"),
        user_agent: text_field(row, "ua"),
        referer: text_field(row, "referer"),
        headers: parse_headers(row, LIVE_HEADER_KEYS, index)?,
        raw: Value::Object(row.clone()),
    })
}

fn parse_parser_row(row: &Map<String, Value>, index: usize) -> Result<TvboxParser, AppError> {
    ensure_unknown_budget(row, PARSER_KNOWN_KEYS)?;
    Ok(TvboxParser {
        name: label_field(row, "name"),
        raw_type: row.get("type").map(TvboxRawType::from_value),
        url: text_field(row, "url"),
        ext: row.get("ext").map(parse_ext),
        headers: parse_headers(row, PARSER_HEADER_KEYS, index)?,
        raw: Value::Object(row.clone()),
    })
}

fn parse_spider(value: &Value) -> TvboxSpiderRef {
    let shape = TvboxJsonShape::from_value(value);
    let reference = match value {
        Value::String(text) => text.trim().to_owned(),
        _ => String::new(),
    };
    TvboxSpiderRef {
        kind: detect_implementation(&reference),
        has_integrity_digest: has_integrity_digest(&reference),
        shape,
        reference,
    }
}

/// `path;md5;digest` / `path;sha256;digest` 形态的探测。只判断第二段的算法名与第三段
/// 是否非空；摘要值本身既不参与判断也不保存。
///
/// 只有算法名而缺少摘要段（`./spider.jar;md5`、`./spider.jar;md5;`）不算完整性摘要，
/// 否则会把「声称有校验」当成真的存在校验。
fn has_integrity_digest(reference: &str) -> bool {
    let mut segments = reference.split(';');
    let _ = segments.next();
    let algorithm = segments.next().unwrap_or("");
    if !matches!(
        algorithm.trim().to_ascii_lowercase().as_str(),
        "md5" | "sha256"
    ) {
        return false;
    }
    segments
        .next()
        .is_some_and(|digest| !digest.trim().is_empty())
}

fn parse_ext(value: &Value) -> TvboxExt {
    let implementation = match value {
        Value::String(text) => Some(detect_implementation(text))
            .filter(|kind| *kind != TvboxImplementationKind::Unidentified),
        _ => None,
    };
    TvboxExt {
        shape: TvboxJsonShape::from_value(value),
        implementation,
        raw: value.clone(),
    }
}

/// 单行 header 的统一选择规则：按 `keys` 顺序取第一个「存在且非 null」的字段。
///
/// 解析与摘要共用它，保证 `headers: null` 两边都回退到 `header`；而一旦某个键存在且
/// 非 null，就由它决定结果（形态不对时解析侧整行失败，不会静默回退到备用键）。
fn select_header_field<'a>(
    row: &'a Map<String, Value>,
    keys: &[&'static str],
) -> Option<(&'static str, &'a Value)> {
    keys.iter().find_map(|key| match row.get(*key) {
        None | Some(Value::Null) => None,
        Some(value) => Some((*key, value)),
    })
}

fn parse_headers(
    row: &Map<String, Value>,
    keys: &[&'static str],
    index: usize,
) -> Result<Vec<TvboxHeader>, AppError> {
    let Some((key, value)) = select_header_field(row, keys) else {
        return Ok(Vec::new());
    };
    let Value::Object(map) = value else {
        let message = format!("TVBox 配置第 {index} 行的 {key} 字段不是 JSON 对象");
        return Err(field_shape_error(message));
    };
    if map.len() > MAX_HEADERS_PER_ROW {
        return Err(limit_error(
            CODE_TOO_MANY_HEADERS,
            "TVBox 配置单行的 header 数量超过上限",
        ));
    }
    let mut headers = Vec::with_capacity(map.len());
    for (name, value) in map {
        // 非标量 header 值不进入结构化形式，但整行原始 JSON 仍然保留。
        if let Some(text) = scalar_text(value) {
            headers.push(TvboxHeader {
                name: name.clone(),
                value: text,
            });
        }
    }
    Ok(headers)
}

// ---------- 分类与识别 ----------

/// 只按 `api` 的形态分类，绝不按数字 `type` 推断能力。
fn classify_site(api: Option<&str>, ext: Option<&TvboxExt>) -> TvboxSiteKind {
    let Some(api) = api else {
        return TvboxSiteKind::Unclassified;
    };
    let lowered = api.to_ascii_lowercase();
    if lowered.starts_with("csp_") {
        return TvboxSiteKind::Spider(
            ext.and_then(|ext| ext.implementation)
                .unwrap_or(TvboxImplementationKind::Unidentified),
        );
    }
    if lowered.starts_with("http://") || lowered.starts_with("https://") {
        return TvboxSiteKind::HttpEndpoint;
    }
    TvboxSiteKind::Unclassified
}

/// 从引用文本识别实现类型。**只识别，不加载、不执行。**
fn detect_implementation(reference: &str) -> TvboxImplementationKind {
    // TVBox 形如 `path;md5;digest`：只取第一段，摘要值不参与判断也不保存。
    let candidate = reference.split(';').next().unwrap_or("").trim();
    let candidate = candidate.split(['?', '#']).next().unwrap_or("").trim();
    let file = candidate
        .rsplit('/')
        .next()
        .unwrap_or(candidate)
        .to_ascii_lowercase();
    if file.ends_with(".jar") {
        TvboxImplementationKind::Jar
    } else if file.ends_with(".js") {
        TvboxImplementationKind::JavaScript
    } else if file.ends_with(".py") {
        TvboxImplementationKind::Python
    } else if file.ends_with(".json") {
        TvboxImplementationKind::JsonManifest
    } else {
        TvboxImplementationKind::Unidentified
    }
}

// ---------- 取值助手 ----------

fn text_field(row: &Map<String, Value>, key: &str) -> Option<String> {
    row.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn label_field(row: &Map<String, Value>, key: &str) -> Option<String> {
    row.get(key).and_then(Value::as_str).and_then(expose_label)
}

fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// 宽容的布尔解码：`true` / `1` / `"1"` / `"yes"` / `"on"` 都算真，反之算假。
/// 无法识别或缺失时返回 `None`——「没写」和「写了假」必须可区分。
fn scalar_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                return match integer {
                    0 => Some(false),
                    1 => Some(true),
                    _ => None,
                };
            }
            match number.as_f64() {
                Some(0.0) => Some(false),
                Some(1.0) => Some(true),
                _ => None,
            }
        }
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

// ---------- 过滤 ----------

/// 过滤字段名：只放行「像标识符」且不含敏感词的键名。
///
/// 返回 `None` 表示该名字不允许出现在任何对外文本里；原值仍然保留在后端。
fn expose_field_name(key: &str) -> Option<String> {
    let trimmed = key.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_EXPOSED_FIELD_NAME_CHARS {
        return None;
    }
    if !trimmed
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.'))
    {
        return None;
    }
    if contains_sensitive_marker(trimmed) {
        return None;
    }
    Some(trimmed.to_owned())
}

/// 不可见的双向/零宽格式控制字符。它们不产生可见字形，却能在展示层制造
/// 「看到的名字」与真实字符串不一致的欺骗（例如 U+202E 反转其后文字的显示顺序，
/// U+200B 让两个不同的名字看起来一样）。命中即拒绝整条标签。
fn is_invisible_format_control(character: char) -> bool {
    matches!(
        character,
        '\u{00ad}'              // SOFT HYPHEN
            | '\u{061c}'        // ARABIC LETTER MARK
            | '\u{180e}'        // MONGOLIAN VOWEL SEPARATOR
            | '\u{200b}'..='\u{200f}' // ZWSP, ZWNJ, ZWJ, LRM, RLM
            | '\u{202a}'..='\u{202e}' // LRE, RLE, PDF, LRO, RLO
            | '\u{2060}'..='\u{2064}' // WORD JOINER, INVISIBLE TIMES/SEPARATOR/PLUS
            | '\u{2066}'..='\u{2069}' // LRI, RLI, FSI, PDI
            | '\u{feff}'        // ZERO WIDTH NO-BREAK SPACE
    )
}

/// 过滤展示标签：去掉控制字符、压缩空白、封顶长度，并拦掉 URL 形态、敏感词以及
/// 双向/零宽不可见格式控制字符。中文与普通标签原样保留；命中任一禁用形态时返回
/// `None`（原始文本仍保留在后端，只是不进入摘要）。
fn expose_label(raw: &str) -> Option<String> {
    let mut cleaned = String::with_capacity(raw.len());
    for character in raw.chars() {
        if is_invisible_format_control(character) {
            return None;
        }
        if character.is_whitespace() {
            if !cleaned.is_empty() && !cleaned.ends_with(' ') {
                cleaned.push(' ');
            }
            continue;
        }
        if character.is_control() {
            continue;
        }
        cleaned.push(character);
    }
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || cleaned.contains("://") || contains_sensitive_marker(cleaned) {
        return None;
    }
    Some(cleaned.chars().take(MAX_EXPOSED_TEXT_CHARS).collect())
}

fn contains_sensitive_marker(text: &str) -> bool {
    let flattened: String = text
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_lowercase())
        .collect();
    SENSITIVE_NAME_MARKERS
        .iter()
        .any(|marker| flattened.contains(marker))
}

/// 摘要侧 header 名单：与 [`parse_headers`] 共用 [`select_header_field`]，
/// 因此 `headers: null` 时会和解析一样回退到 `header`。形态不符的 header 在解析侧
/// 已经失败，这里只需对非对象形态返回空，不会泄露任何值。
fn header_summary(raw: &Value, keys: &[&'static str]) -> (Vec<String>, usize) {
    let Some(row) = raw.as_object() else {
        return (Vec::new(), 0);
    };
    let Some((_, value)) = select_header_field(row, keys) else {
        return (Vec::new(), 0);
    };
    let Some(map) = value.as_object() else {
        return (Vec::new(), 0);
    };
    let mut names = Vec::new();
    let mut withheld = 0usize;
    for name in map.keys() {
        match expose_field_name(name) {
            Some(exposed) => names.push(exposed),
            None => withheld += 1,
        }
    }
    names.sort();
    (names, withheld)
}

fn unknown_field_summaries(raw: &Value, known: &[&str]) -> (Vec<TvboxFieldSummary>, usize) {
    let Some(map) = raw.as_object() else {
        return (Vec::new(), 0);
    };
    let mut summaries = Vec::new();
    let mut withheld = 0usize;
    for (key, value) in map {
        if known.contains(&key.as_str()) {
            continue;
        }
        match expose_field_name(key) {
            Some(name) => summaries.push(TvboxFieldSummary {
                name,
                shape: TvboxJsonShape::from_value(value),
                entry_count: value.as_array().map(Vec::len),
            }),
            None => withheld += 1,
        }
    }
    (summaries, withheld)
}

fn ensure_unknown_budget(row: &Map<String, Value>, known: &[&str]) -> Result<(), AppError> {
    let unknown = row
        .keys()
        .filter(|key| !known.contains(&key.as_str()))
        .count();
    if unknown > MAX_UNKNOWN_FIELDS_PER_ROW {
        return Err(limit_error(
            CODE_TOO_MANY_UNKNOWN_FIELDS,
            "TVBox 配置单行的未知字段数量超过上限",
        ));
    }
    Ok(())
}

// ---------- 错误 ----------

/// 错误文案全部是固定字符串或只含行号，绝不含配置内容。
fn empty_error() -> AppError {
    AppError::new(CODE_EMPTY, ErrorKind::Validation, "TVBox 配置为空", false)
}

fn not_utf8_error() -> AppError {
    AppError::new(
        CODE_NOT_UTF8,
        ErrorKind::Validation,
        "TVBox 配置不是 UTF-8 文本",
        false,
    )
}

fn invalid_json_error() -> AppError {
    AppError::new(
        CODE_INVALID_JSON,
        ErrorKind::Parse,
        "TVBox 配置不是合法 JSON",
        false,
    )
}

fn not_object_error() -> AppError {
    AppError::new(
        CODE_NOT_OBJECT,
        ErrorKind::Parse,
        "TVBox 配置顶层必须是 JSON 对象",
        false,
    )
}

fn field_shape_error(message: String) -> AppError {
    AppError::new(CODE_FIELD_SHAPE, ErrorKind::Validation, message, false)
}

fn limit_error(code: &'static str, message: &'static str) -> AppError {
    AppError::new(code, ErrorKind::Validation, message, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floating_boolean_flags_preserve_zero_negative_zero_and_one() {
        for (raw, expected) in [
            ("0.0", Some(false)),
            ("-0.0", Some(false)),
            ("1.0", Some(true)),
            ("2.0", None),
        ] {
            let value: Value = serde_json::from_str(raw).unwrap();
            assert_eq!(
                scalar_bool(&value),
                expected,
                "浮点布尔标志的兼容语义必须保持"
            );
        }
    }

    const SAMPLE_CONFIG: &str = r#"{
      "spider": "./spider.jar;md5;deadbeefdeadbeef",
      "wallpaper": "https://img.example.invalid/wallpaper.jpg",
      "rules": "./rules.json",
      "flags": ["youku", "qq"],
      "doh": [{"name": "doh", "url": "https://doh.example.invalid/dns-query"}],
      "ads": ["https://ads.example.invalid/ads.txt"],
      "logo": "https://img.example.invalid/logo.png",
      "epg": "https://epg.example.invalid/epg.xml",
      "danmaku": "https://danmaku.example.invalid/dm.xml",
      "sites": [
        {
          "name": "示例站点",
          "api": "https://api.example.invalid/vod",
          "type": 1,
          "searchable": 1,
          "quickSearch": "1",
          "filterable": true,
          "ua": "Mozilla/5.0 (Example)",
          "referer": "https://api.example.invalid/",
          "headers": {"User-Agent": "Mozilla/5.0", "Authorization": "Bearer hidden"},
          "customParam": {"nested": true}
        },
        {"name": "Spider 站点", "api": "csp_Demo", "type": 3, "ext": "./js/demo.js"},
        {"name": "未知类型站点", "api": "demo", "type": 7}
      ],
      "lives": [
        {
          "name": "示例直播",
          "url": "https://live.example.invalid/list.m3u",
          "type": 0,
          "epg": "https://epg.example.invalid/live.xml",
          "logo": "https://img.example.invalid/live.png",
          "ua": "LiveUA",
          "referer": "https://live.example.invalid/",
          "headers": {"User-Agent": "LiveUA"}
        }
      ],
      "parses": [
        {
          "name": "示例解析",
          "type": 1,
          "url": "https://parse.example.invalid/api",
          "ext": {"flag": ["qq"]},
          "header": {"User-Agent": "ParserUA"}
        }
      ],
      "vendorExtension": "opaque-value"
    }"#;

    const SPIDER_WITH_DIGEST: &str = r#"{"spider":"./spider.jar;md5;deadbeef"}"#;
    const PYTHON_EXT_CONFIG: &str = r#"{"sites":[{"api":"csp_demo","ext":"./site.py"}]}"#;
    const SKIPPED_ROWS_CONFIG: &str = r#"{"sites":[null,{"api":"https://a.invalid"},7]}"#;
    const HEADERS_SHAPE_CONFIG: &str = r#"{"sites":[{"api":"https://a.invalid","headers":"x"}]}"#;

    fn expect_error(bytes: &[u8]) -> AppError {
        match parse_config(bytes) {
            Ok(_) => panic!("该输入应当被拒绝"),
            Err(error) => error,
        }
    }

    fn header_value<'a>(headers: &'a [TvboxHeader], name: &str) -> Option<&'a str> {
        headers
            .iter()
            .find(|header| header.name == name)
            .map(TvboxHeader::value)
    }

    #[test]
    fn bom_is_accepted_and_matches_plain_text() {
        let plain = parse_config(SAMPLE_CONFIG.as_bytes()).expect("应解析成功");
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice(SAMPLE_CONFIG.as_bytes());
        let parsed = parse_config(&with_bom).expect("带 BOM 的配置应解析成功");
        assert_eq!(parsed.summary(), plain.summary(), "BOM 不应改变解析结果");
    }

    #[test]
    fn minimal_config_parses_without_unknown_fields() {
        let empty = parse_config(b"{}").expect("空对象应解析成功");
        assert_eq!(empty.sites().len(), 0);
        assert!(empty.spider().is_none());
        assert!(empty.unknown_top_level().is_empty());

        let minimal =
            parse_config(r#"{"sites":[{"name":"示例","api":"https://a.invalid/vod"}]}"#.as_bytes())
                .expect("最小配置应解析成功");
        assert_eq!(minimal.sites().len(), 1);
        assert!(minimal.opaque_top_level().is_empty());
        assert!(minimal.unknown_top_level().is_empty());
        assert_eq!(minimal.skipped_site_rows(), 0);
        let site = &minimal.sites()[0];
        assert_eq!(site.name.as_deref(), Some("示例"));
        assert_eq!(site.kind, TvboxSiteKind::HttpEndpoint);
        assert!(site.searchable.is_none());
        assert!(site.quick_search.is_none());
        assert!(site.filterable.is_none());
        assert!(site.raw_type.is_none());
    }

    #[test]
    fn site_scalar_flags_accept_mixed_encodings_and_report_missing() {
        let sites: Vec<Value> = vec![
            serde_json::json!({"name": "a", "searchable": true}),
            serde_json::json!({"name": "b", "searchable": 1}),
            serde_json::json!({"name": "c", "searchable": "1"}),
            serde_json::json!({"name": "d", "searchable": "yes"}),
            serde_json::json!({"name": "e", "searchable": "on"}),
            serde_json::json!({"name": "f", "searchable": false}),
            serde_json::json!({"name": "g", "searchable": 0}),
            serde_json::json!({"name": "h", "searchable": "0"}),
            serde_json::json!({"name": "i", "searchable": "no"}),
            serde_json::json!({"name": "j", "searchable": "off"}),
            serde_json::json!({"name": "k", "searchable": 2}),
            serde_json::json!({"name": "l", "searchable": "maybe"}),
            serde_json::json!({"name": "m"}),
        ];
        let payload = serde_json::json!({"sites": sites}).to_string();
        let config = parse_config(payload.as_bytes()).expect("应解析成功");
        let flags: Vec<Option<bool>> = config.sites().iter().map(|site| site.searchable).collect();
        assert_eq!(
            flags,
            vec![
                Some(true),
                Some(true),
                Some(true),
                Some(true),
                Some(true),
                Some(false),
                Some(false),
                Some(false),
                Some(false),
                Some(false),
                None,
                None,
                None,
            ],
            "无法识别的标量必须保持未知，不能被当成 false"
        );
    }

    #[test]
    fn sites_lives_and_parsers_are_parsed_as_data() {
        let config = parse_config(SAMPLE_CONFIG.as_bytes()).expect("应解析成功");
        assert_eq!(config.sites().len(), 3);
        assert_eq!(config.lives().len(), 1);
        assert_eq!(config.parsers().len(), 1);

        let site = &config.sites()[0];
        assert_eq!(site.name.as_deref(), Some("示例站点"));
        assert_eq!(site.api.as_deref(), Some("https://api.example.invalid/vod"));
        assert!(matches!(site.raw_type, Some(TvboxRawType::Integer(1))));
        assert_eq!(site.searchable, Some(true));
        assert_eq!(site.quick_search, Some(true));
        assert_eq!(site.filterable, Some(true));
        assert_eq!(site.user_agent.as_deref(), Some("Mozilla/5.0 (Example)"));
        assert_eq!(
            site.referer.as_deref(),
            Some("https://api.example.invalid/")
        );
        assert_eq!(
            header_value(&site.headers, "User-Agent"),
            Some("Mozilla/5.0")
        );
        assert_eq!(
            header_value(&site.headers, "Authorization"),
            Some("Bearer hidden")
        );

        let spider_site = &config.sites()[1];
        assert_eq!(
            spider_site.kind,
            TvboxSiteKind::Spider(TvboxImplementationKind::JavaScript)
        );
        let ext = spider_site.ext.as_ref().expect("ext 应存在");
        assert_eq!(ext.shape, TvboxJsonShape::Text);
        assert_eq!(
            ext.implementation,
            Some(TvboxImplementationKind::JavaScript)
        );
        assert_eq!(ext.retained_value().as_str(), Some("./js/demo.js"));

        let live = &config.lives()[0];
        assert_eq!(live.name.as_deref(), Some("示例直播"));
        assert_eq!(
            live.url.as_deref(),
            Some("https://live.example.invalid/list.m3u")
        );
        assert!(matches!(live.raw_type, Some(TvboxRawType::Integer(0))));
        assert_eq!(
            live.epg.as_deref(),
            Some("https://epg.example.invalid/live.xml")
        );
        assert_eq!(
            live.logo.as_deref(),
            Some("https://img.example.invalid/live.png")
        );
        assert_eq!(live.user_agent.as_deref(), Some("LiveUA"));
        assert_eq!(
            live.referer.as_deref(),
            Some("https://live.example.invalid/")
        );
        assert_eq!(header_value(&live.headers, "User-Agent"), Some("LiveUA"));

        let parser = &config.parsers()[0];
        assert_eq!(parser.name.as_deref(), Some("示例解析"));
        assert_eq!(
            parser.url.as_deref(),
            Some("https://parse.example.invalid/api")
        );
        assert!(matches!(parser.raw_type, Some(TvboxRawType::Integer(1))));
        let parser_ext = parser.ext.as_ref().expect("ext 应存在");
        assert_eq!(parser_ext.shape, TvboxJsonShape::Object);
        assert_eq!(
            parser_ext.implementation, None,
            "解析器 ext 不是可执行文件引用"
        );
        assert_eq!(
            header_value(&parser.headers, "User-Agent"),
            Some("ParserUA")
        );
    }

    #[test]
    fn unknown_fields_are_retained_but_never_leak_into_summaries() {
        let payload = serde_json::json!({
            "authToken": "top-secret-token",
            "sites": [{
                "name": "站点",
                "api": "https://api.example.invalid/vod",
                "headers": {"Authorization": "Bearer super-secret-token"},
                "sessionKey": "top-secret-value",
                "note": "普通备注"
            }]
        })
        .to_string();
        let config = parse_config(payload.as_bytes()).expect("应解析成功");

        // 后端仍然完整保留原文。
        let site = &config.sites()[0];
        let row = site.retained_row();
        assert_eq!(row["headers"]["Authorization"], "Bearer super-secret-token");
        assert_eq!(row["sessionKey"], "top-secret-value");
        assert_eq!(
            config.unknown_top_level()[0].retained_value(),
            "top-secret-token"
        );
        assert_eq!(
            header_value(&site.headers, "Authorization"),
            Some("Bearer super-secret-token")
        );

        // 摘要只暴露安全的字段名与计数。
        let summary = config.summary();
        let site_summary = &summary.sites[0];
        let names: Vec<&str> = site_summary
            .unknown_fields
            .iter()
            .map(|field| field.name.as_str())
            .collect();
        assert_eq!(names, vec!["note"], "安全字段名应暴露");
        assert_eq!(site_summary.withheld_unknown_field_count, 1);
        assert_eq!(site_summary.header_names, Vec::<String>::new());
        assert_eq!(site_summary.withheld_header_count, 1);
        assert_eq!(summary.withheld_top_level_field_count, 1);
        assert!(summary.unknown_top_level_fields.is_empty());
        assert_eq!(
            config.unknown_top_level()[0].retained_name(),
            "authToken",
            "键名本身仍然保留在后端，只是不进入摘要"
        );

        let rendered = format!("{summary:?}");
        for secret in [
            "top-secret-token",
            "super-secret-token",
            "top-secret-value",
            "api.example.invalid",
            "Authorization",
            "sessionKey",
            "authToken",
        ] {
            assert!(!rendered.contains(secret), "摘要不得暴露敏感字段或原始值");
        }
    }

    #[test]
    fn opaque_top_level_fields_are_recognized_not_unknown() {
        let config = parse_config(SAMPLE_CONFIG.as_bytes()).expect("应解析成功");
        assert_eq!(config.opaque_top_level().len(), 8);
        assert_eq!(config.unknown_top_level().len(), 1);
        let summary = config.summary();
        let mut names: Vec<&str> = summary
            .opaque_top_level_fields
            .iter()
            .map(|field| field.name.as_str())
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec![
                "ads",
                "danmaku",
                "doh",
                "epg",
                "flags",
                "logo",
                "rules",
                "wallpaper"
            ]
        );
        let flags = summary
            .opaque_top_level_fields
            .iter()
            .find(|field| field.name == "flags")
            .expect("flags 应被识别");
        assert_eq!(flags.shape, TvboxJsonShape::Array);
        assert_eq!(flags.entry_count, Some(2));
        assert_eq!(summary.unknown_top_level_fields[0].name, "vendorExtension");
        assert_eq!(summary.withheld_top_level_field_count, 0);
    }

    #[test]
    fn unknown_site_types_stay_unclassified_and_keep_their_literal() {
        let payload = serde_json::json!({
            "sites": [
                {"name": "a", "api": "demo", "type": 7},
                {"name": "b", "type": 3},
                {"name": "c", "api": "csp_Demo", "type": 3},
                {"name": "d", "api": "https://api.example.invalid/vod", "type": "3"}
            ]
        })
        .to_string();
        let config = parse_config(payload.as_bytes()).expect("应解析成功");

        let unclassified = &config.sites()[0];
        assert_eq!(unclassified.kind, TvboxSiteKind::Unclassified);
        assert_eq!(
            unclassified
                .raw_type
                .as_ref()
                .and_then(TvboxRawType::as_integer),
            Some(7)
        );

        let missing_api = &config.sites()[1];
        assert_eq!(missing_api.kind, TvboxSiteKind::Unclassified);
        assert_eq!(
            missing_api
                .raw_type
                .as_ref()
                .and_then(TvboxRawType::as_integer),
            Some(3)
        );

        let spider_without_ext = &config.sites()[2];
        assert_eq!(
            spider_without_ext.kind,
            TvboxSiteKind::Spider(TvboxImplementationKind::Unidentified)
        );

        // 文本 `type` 原样保留，不因为它是 "3" 就当作 spider。
        let textual = &config.sites()[3];
        assert_eq!(textual.kind, TvboxSiteKind::HttpEndpoint);
        assert_eq!(
            textual.raw_type.as_ref().and_then(TvboxRawType::as_text),
            Some("3")
        );

        let summary = config.summary();
        assert_eq!(
            summary.sites[0].raw_type,
            Some(TvboxRawTypeSummary {
                shape: TvboxJsonShape::Number,
                literal: Some("7".to_owned()),
            })
        );
        assert_eq!(
            summary.sites[3].raw_type,
            Some(TvboxRawTypeSummary {
                shape: TvboxJsonShape::Text,
                literal: Some("3".to_owned()),
            })
        );
    }

    #[test]
    fn spider_and_script_indicators_are_identified_but_not_executed() {
        let cases: [(&str, TvboxImplementationKind); 6] = [
            (
                "./spider.jar;md5;deadbeefdeadbeef",
                TvboxImplementationKind::Jar,
            ),
            (
                "https://code.example.invalid/spider.js?1",
                TvboxImplementationKind::JavaScript,
            ),
            (
                "https://code.example.invalid/spider.py",
                TvboxImplementationKind::Python,
            ),
            (
                "https://code.example.invalid/spider.json",
                TvboxImplementationKind::JsonManifest,
            ),
            ("csp_custom", TvboxImplementationKind::Unidentified),
            ("", TvboxImplementationKind::Unidentified),
        ];
        for (reference, expected) in cases {
            let payload = serde_json::json!({"spider": reference}).to_string();
            let config = parse_config(payload.as_bytes()).expect("应解析成功");
            let spider = config.spider().expect("spider 应被识别");
            assert_eq!(spider.kind, expected, "spider 引用应被识别为 {expected:?}");
            assert_eq!(spider.shape, TvboxJsonShape::Text);
            assert_eq!(spider.retained_reference(), reference);
            // 只识别形态：解析过程没有下载、加载或执行任何引用。
            assert_eq!(config.sites().len(), 0);
        }

        let with_digest = parse_config(SPIDER_WITH_DIGEST.as_bytes()).expect("应解析成功");
        assert!(
            with_digest
                .spider()
                .expect("spider 应存在")
                .has_integrity_digest
        );
        assert!(!format!("{:?}", with_digest.summary()).contains("deadbeef"));

        let with_python_ext = parse_config(PYTHON_EXT_CONFIG.as_bytes()).expect("应解析成功");
        assert_eq!(
            with_python_ext.sites()[0].kind,
            TvboxSiteKind::Spider(TvboxImplementationKind::Python)
        );
        assert_eq!(
            with_python_ext.summary().sites[0].ext_implementation,
            Some(TvboxImplementationKind::Python)
        );
    }

    #[test]
    fn malformed_inputs_return_stable_codes() {
        assert_eq!(expect_error(b"").code().as_str(), CODE_EMPTY);
        assert_eq!(expect_error(b"\xEF\xBB\xBF").code().as_str(), CODE_EMPTY);
        assert_eq!(expect_error(b"   ").code().as_str(), CODE_EMPTY);
        assert_eq!(
            expect_error(b"{not json").code().as_str(),
            CODE_INVALID_JSON
        );
        assert_eq!(expect_error(b"[1,2,3]").code().as_str(), CODE_NOT_OBJECT);
        assert_eq!(
            expect_error(b"\xFF\xFE{\"sites\":[]}").code().as_str(),
            CODE_NOT_UTF8,
            "UTF-16 或二进制输入必须被拒绝，而不是被猜测编码"
        );
        assert_eq!(
            expect_error(br#"{"sites":"nope"}"#).code().as_str(),
            CODE_FIELD_SHAPE
        );
        assert_eq!(
            expect_error(br#"{"sites":[{"headers":"nope"}]}"#)
                .code()
                .as_str(),
            CODE_FIELD_SHAPE
        );
        // 非对象行被跳过并计数，而不是让整份配置失败。
        let config = parse_config(SKIPPED_ROWS_CONFIG.as_bytes()).expect("应解析成功");
        assert_eq!(config.sites().len(), 1);
        assert_eq!(config.skipped_site_rows(), 2);
        assert_eq!(config.summary().skipped_site_rows, 2);
    }

    #[test]
    fn byte_and_entry_limits_are_enforced() {
        let sites: Vec<Value> = (0..=MAX_SITES)
            .map(|index| serde_json::json!({"name": format!("站点{index}")}))
            .collect();
        let oversized_sites = serde_json::json!({"sites": sites}).to_string();
        assert_eq!(
            expect_error(oversized_sites.as_bytes()).code().as_str(),
            CODE_TOO_MANY_SITES
        );

        let mut oversized_body = String::with_capacity(MAX_CONFIG_BYTES + 64);
        oversized_body.push_str("{\"pad\":\"");
        oversized_body.push_str(&"a".repeat(MAX_CONFIG_BYTES));
        oversized_body.push_str("\"}");
        assert!(oversized_body.len() > MAX_CONFIG_BYTES);
        assert_eq!(
            expect_error(oversized_body.as_bytes()).code().as_str(),
            CODE_TOO_LARGE
        );

        let mut headers = Map::new();
        for index in 0..=MAX_HEADERS_PER_ROW {
            headers.insert(format!("X-Header-{index}"), Value::String("v".to_owned()));
        }
        let many_headers = serde_json::json!({
            "sites": [{"api": "https://api.example.invalid", "headers": Value::Object(headers)}]
        })
        .to_string();
        assert_eq!(
            expect_error(many_headers.as_bytes()).code().as_str(),
            CODE_TOO_MANY_HEADERS
        );

        let mut row = Map::new();
        row.insert(
            "api".to_owned(),
            Value::String("https://api.example.invalid".to_owned()),
        );
        for index in 0..=MAX_UNKNOWN_FIELDS_PER_ROW {
            row.insert(format!("extra{index}"), Value::Bool(true));
        }
        let many_unknowns = serde_json::json!({"sites": [Value::Object(row)]}).to_string();
        assert_eq!(
            expect_error(many_unknowns.as_bytes()).code().as_str(),
            CODE_TOO_MANY_UNKNOWN_FIELDS
        );
    }

    #[test]
    fn errors_never_echo_config_values() {
        let secret = "https://api.example.invalid/vod?token=hidden";
        let payload = serde_json::json!({
            "sites": [{"api": secret, "headers": {"Authorization": "Bearer hidden"}}],
            "pad": "a".repeat(MAX_CONFIG_BYTES)
        })
        .to_string();
        let error = expect_error(payload.as_bytes());
        assert_eq!(error.code().as_str(), CODE_TOO_LARGE);
        assert!(!error.user_message().contains(secret));
        assert!(!error.user_message().contains("hidden"));
        assert!(!format!("{error:?}").contains("hidden"));
        assert!(!format!("{error}").contains(secret));

        let shape_error = expect_error(HEADERS_SHAPE_CONFIG.as_bytes());
        assert!(!shape_error.user_message().contains("a.invalid"));
    }

    #[test]
    fn integrity_digest_requires_a_nonempty_digest_segment() {
        let cases: [(&str, bool); 8] = [
            ("./spider.jar;md5;deadbeefdeadbeef", true),
            ("./spider.jar;sha256;deadbeef", true),
            ("./spider.jar; MD5 ;deadbeef", true),
            ("./spider.jar;md5", false),
            ("./spider.jar;md5;", false),
            ("./spider.jar;md5;   ", false),
            ("./spider.jar;sha256", false),
            ("./spider.jar", false),
        ];
        for (reference, expected) in cases {
            let payload = serde_json::json!({"spider": reference}).to_string();
            let config = parse_config(payload.as_bytes()).expect("应解析成功");
            let spider = config.spider().expect("spider 应被识别");
            assert_eq!(
                spider.has_integrity_digest, expected,
                "{reference:?} 的完整性摘要判定应为 {expected}"
            );
            assert_eq!(
                config.summary().spider_has_integrity_digest,
                expected,
                "摘要必须与结构化结果一致"
            );
        }

        // 只有算法名不算校验；摘要里也不出现任何摘要原文。
        let without_digest = parse_config(br#"{"spider":"./spider.jar;md5"}"#).expect("应解析成功");
        assert!(!without_digest.summary().spider_has_integrity_digest);
        assert!(!format!("{:?}", without_digest.summary()).contains("md5"));
    }

    #[test]
    fn top_level_field_limit_is_enforced_at_the_boundary() {
        let build = |count: usize| {
            let mut root = Map::new();
            for index in 0..count {
                root.insert(format!("vendor{index}"), Value::Bool(true));
            }
            Value::Object(root).to_string()
        };

        let at_limit =
            parse_config(build(MAX_TOP_LEVEL_FIELDS).as_bytes()).expect("恰好达到上限应被接受");
        assert_eq!(at_limit.unknown_top_level().len(), MAX_TOP_LEVEL_FIELDS);
        assert_eq!(
            at_limit.summary().unknown_top_level_fields.len(),
            MAX_TOP_LEVEL_FIELDS
        );

        let error = expect_error(build(MAX_TOP_LEVEL_FIELDS + 1).as_bytes());
        assert_eq!(
            error.code().as_str(),
            CODE_TOO_MANY_TOP_LEVEL_FIELDS,
            "超过上限必须返回固定错误码"
        );
        assert!(!format!("{error:?}").contains("vendor"));
    }

    #[test]
    fn headers_null_falls_back_to_header_for_parse_and_summary() {
        let payload = serde_json::json!({
            "sites": [{
                "api": "https://a.invalid",
                "headers": Value::Null,
                "header": {"X-Test": "1"}
            }],
            "lives": [{
                "url": "https://b.invalid/list.m3u",
                "headers": null,
                "header": {"X-Live": "1"}
            }],
            "parses": [{
                "url": "https://c.invalid/api",
                "headers": null,
                "header": {"X-Parse": "1"}
            }]
        })
        .to_string();
        let config = parse_config(payload.as_bytes()).expect("应解析成功");

        assert_eq!(
            header_value(&config.sites()[0].headers, "X-Test"),
            Some("1"),
            "headers:null 时解析应回退到 header"
        );
        assert_eq!(
            header_value(&config.lives()[0].headers, "X-Live"),
            Some("1")
        );
        assert_eq!(
            header_value(&config.parsers()[0].headers, "X-Parse"),
            Some("1")
        );

        // 摘要必须与解析选出同一组 header，不能一边回退一边落空。
        let summary = config.summary();
        assert_eq!(summary.sites[0].header_names, vec!["X-Test".to_owned()]);
        assert_eq!(summary.lives[0].header_names, vec!["X-Live".to_owned()]);
        assert_eq!(summary.parsers[0].header_names, vec!["X-Parse".to_owned()]);
        assert_eq!(summary.sites[0].withheld_header_count, 0);
    }

    #[test]
    fn malformed_non_null_headers_fail_closed_without_falling_back() {
        let cases = [
            serde_json::json!({"sites": [{
                "api": "https://a.invalid",
                "headers": "not-an-object",
                "header": {"X-Test": "1"}
            }]}),
            serde_json::json!({"lives": [{
                "url": "https://b.invalid/list.m3u",
                "headers": ["nope"],
                "header": {"X-Live": "1"}
            }]}),
            // 解析器以 header 优先：header 形态不对时同样整行失败。
            serde_json::json!({"parses": [{
                "url": "https://c.invalid/api",
                "header": 7,
                "headers": {"X-Parse": "1"}
            }]}),
        ];
        for payload in cases {
            let error = expect_error(payload.to_string().as_bytes());
            assert_eq!(
                error.code().as_str(),
                CODE_FIELD_SHAPE,
                "形态不对的 header 必须失败，且不静默回退到备用键"
            );
            assert!(!error.user_message().contains(".invalid"));
        }
    }

    #[test]
    fn invisible_format_controls_are_rejected_in_labels() {
        for raw in [
            "示例\u{202e}站点",
            "示例\u{200b}站点",
            "\u{200e}leading",
            "trailing\u{2069}",
        ] {
            let payload = serde_json::json!({
                "sites": [{"name": raw, "api": "https://a.invalid"}]
            })
            .to_string();
            let config = parse_config(payload.as_bytes()).expect("应解析成功");
            let site = &config.sites()[0];
            assert_eq!(site.name, None, "{raw:?} 不应作为展示标签暴露");
            assert_eq!(
                site.retained_row()["name"],
                Value::String(raw.to_owned()),
                "原文必须仍然保留在后端"
            );
            assert_eq!(config.summary().sites[0].name, None);
        }

        // 中文与普通标签不受影响。
        let plain = parse_config(
            r#"{"sites":[{"name":"示例站点 A","api":"https://a.invalid"}]}"#.as_bytes(),
        )
        .expect("应解析成功");
        assert_eq!(plain.sites()[0].name.as_deref(), Some("示例站点 A"));
        assert_eq!(plain.summary().sites[0].name.as_deref(), Some("示例站点 A"));
    }
}
