//! 报刊（Periodical）领域切片：期刊 → 卷 → 期 → 文章的真实归属。
//!
//! 规范对照：`plan/DOMAIN_MODEL.md`（Work/Edition/MediaItem 层级）与
//! `ContentCategory::Periodical`（一级分类「报刊资料」）。
//!
//! 设计原则：
//! - 期刊层级是显式的逐级归属（`Periodical.work_id`、`Volume.periodical_id`、
//!   `Issue.volume_id`、`Article.issue_id`），既不复用通用 `parent_id`，也不由
//!   文章标题推断；
//! - 期刊身份是 ISSN（print / electronic 是两个独立身份），标题只作展示事实；
//! - 卷/期身份允许「来源未给出」这一显式状态（`Unassigned`），不伪造卷号或期号；
//! - 文章绑定既有 `MediaItem`，因此阅读、进度、资源与下载链路完全复用既有模型；
//! - provider 的 opaque 来源身份（如 PMCID）只作为文章的来源事实，不是 Haven ID。
//!
//! 本模块只做纯领域判断，不访问 SQLite、网络、文件系统或 Tauri。

use serde::{Deserialize, Serialize};

use haven_common::{AppError, ErrorKind, UtcMillis};

use crate::comic_identity::has_opaque_control_character;
use crate::ids::{
    MediaItemId, PeriodicalArticleId, PeriodicalId, PeriodicalIssueId, PeriodicalVolumeId, WorkId,
};

/// 身份文本的统一长度上限（标题、卷标、期号、来源标识）。
const MAX_IDENTITY_TEXT: usize = 512;
/// 页码文本上限。
const MAX_PAGE_TEXT: usize = 64;

fn invalid_periodical_identity(message: impl Into<String>) -> AppError {
    AppError::new(
        "INVALID_PERIODICAL_IDENTITY",
        ErrorKind::Validation,
        message,
        false,
    )
}

/// 身份文本的统一边界：修剪空白，但拒绝空值、控制字符、URL 形状与超长值。
fn identity_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAX_IDENTITY_TEXT
        || has_opaque_control_character(trimmed)
        || trimmed.contains("://")
        || trimmed.to_ascii_lowercase().starts_with("data:")
    {
        return None;
    }
    Some(trimmed.to_owned())
}

/// 比较用的规范化标签（折叠空白 + 小写）；只用于身份比较，不覆盖原文展示值。
pub fn normalize_identity_label(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// 数值身份的统一定点表示（千分之一），避免 `f64` 无法参与 `Hash` 与唯一键。
fn number_millis(value: f64) -> Option<i64> {
    if !value.is_finite() {
        return None;
    }
    let millis = (value * 1000.0).round();
    (millis.abs() < i64::MAX as f64).then_some(millis as i64)
}

fn non_negative_number_millis(value: f64) -> Option<i64> {
    number_millis(value).filter(|millis| *millis >= 0)
}

/// ISSN（国际标准连续出版物号）。
///
/// print 与 electronic 是两个独立身份，不互相推断。`parse` 做完整格式校验
/// （8 位、mod-11 校验位、允许 `X` 校验位）并归一化为 `NNNN-NNNX` 形态；
/// 反序列化同样校验，避免非法 ISSN 从 JSON 进入领域模型。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Issn(String);

impl Issn {
    pub fn parse(value: &str) -> Option<Self> {
        let mut digits: Vec<char> = Vec::with_capacity(8);
        for character in value.chars() {
            match character {
                '0'..='9' => digits.push(character),
                '-' | ' ' | '\t' => {}
                'X' | 'x' if digits.len() == 7 => digits.push('X'),
                _ => return None,
            }
        }
        if digits.len() != 8 {
            return None;
        }
        let mut sum = 0u32;
        for (index, character) in digits.iter().enumerate() {
            let digit = match character {
                'X' => 10,
                other => other.to_digit(10)?,
            };
            sum += digit * (8 - index as u32);
        }
        if sum % 11 != 0 {
            return None;
        }
        let canonical: String = digits.iter().collect();
        Some(Self(format!("{}-{}", &canonical[..4], &canonical[4..])))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Issn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for Issn {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or_else(|| invalid_periodical_identity("ISSN 非法"))
    }
}

impl Serialize for Issn {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Issn {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).ok_or_else(|| serde::de::Error::custom("ISSN 非法"))
    }
}

/// DOI（数字对象标识符）。只保存规范化文本，不保存解析得到的 URL。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Doi(String);

impl Doi {
    pub fn parse(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        if trimmed.len() > 256
            || has_opaque_control_character(trimmed)
            || trimmed
                .chars()
                .any(|ch| ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | '\\'))
            || trimmed.to_ascii_lowercase().contains("://")
        {
            return None;
        }
        let suffix = trimmed.strip_prefix("10.")?;
        let (registrant, rest) = suffix.split_once('/')?;
        if registrant.is_empty() || rest.is_empty() {
            return None;
        }
        Some(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Doi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for Doi {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or_else(|| invalid_periodical_identity("DOI 非法"))
    }
}

impl Serialize for Doi {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Doi {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).ok_or_else(|| serde::de::Error::custom("DOI 非法"))
    }
}

/// 起止页码。期刊页码允许非连续与不规则形态（`e12345`、`S1-S5`），
/// 因此保留来源文本而不是强行解析成整数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PageRange {
    pub start: String,
    pub end: Option<String>,
}

impl PageRange {
    pub fn new(start: &str, end: Option<&str>) -> Option<Self> {
        let start = page_text(start)?;
        let end = match end {
            Some(value) => Some(page_text(value)?),
            None => None,
        };
        Some(Self { start, end })
    }

    pub fn is_single_page(&self) -> bool {
        self.end.is_none()
    }

    pub fn validate(&self) -> Result<(), AppError> {
        if page_text(&self.start).is_none()
            || self
                .end
                .as_deref()
                .is_some_and(|value| page_text(value).is_none())
        {
            return Err(invalid_periodical_identity("文章页码范围非法"));
        }
        Ok(())
    }
}

fn page_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()
        && trimmed.len() <= MAX_PAGE_TEXT
        && !has_opaque_control_character(trimmed)
        && !trimmed.chars().any(char::is_whitespace))
    .then(|| trimmed.to_owned())
}

/// 发行日期的持久化精度：来源可以只给年份、年份+月份或完整日期，但不能把
/// 任意展示文本写进日期字段。校验接受未补零的月/日以兼容来源，Provider 会在
/// 进入领域记录前统一输出 `YYYY-MM`/`YYYY-MM-DD`。
fn publication_date_year(value: &str) -> Option<i32> {
    let parts: Vec<&str> = value.trim().split('-').collect();
    if !(1..=3).contains(&parts.len()) || parts[0].len() != 4 {
        return None;
    }
    if !parts[0].bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let year = parts[0].parse::<i32>().ok()?;
    if !(1000..=2999).contains(&year) {
        return None;
    }
    if parts.len() >= 2
        && (parts[1].is_empty()
            || parts[1].len() > 2
            || !parts[1].bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    let month = if let Some(raw) = parts.get(1) {
        let month = raw.parse::<u32>().ok()?;
        if !(1..=12).contains(&month) {
            return None;
        }
        Some(month)
    } else {
        None
    };
    if parts.len() == 3
        && (month.is_none()
            || parts[2].is_empty()
            || parts[2].len() > 2
            || !parts[2].bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    if let (Some(month), Some(raw_day)) = (month, parts.get(2)) {
        let day = raw_day.parse::<u32>().ok()?;
        if !(1..=days_in_month(year, month)).contains(&day) {
            return None;
        }
    }
    Some(year)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

/// 期刊。`work_id` 是期刊在 Haven 中的 Work 归属（期刊本身是作品）；文章通过
/// Edition → MediaItem 消费，不把期刊降级成某篇文章的父节点。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Periodical {
    pub id: PeriodicalId,
    pub work_id: WorkId,
    pub title: String,
    /// ISSN（印刷版）。与 electronic 互相独立。
    pub issn_print: Option<Issn>,
    /// ISSN（电子版）。
    pub issn_electronic: Option<Issn>,
    pub publisher: Option<String>,
    pub created_at: UtcMillis,
    pub updated_at: UtcMillis,
}

impl Periodical {
    /// 该期刊已声明的全部 ISSN 身份（print 优先，不含重复项）。
    pub fn identities(&self) -> Vec<&Issn> {
        let mut identities = Vec::with_capacity(2);
        if let Some(issn) = self.issn_print.as_ref() {
            identities.push(issn);
        }
        if let Some(issn) = self.issn_electronic.as_ref()
            && self.issn_print.as_ref() != Some(issn)
        {
            identities.push(issn);
        }
        identities
    }

    pub fn matches_issn(&self, issn: &Issn) -> bool {
        self.issn_print.as_ref() == Some(issn) || self.issn_electronic.as_ref() == Some(issn)
    }

    pub fn validate(&self) -> Result<(), AppError> {
        if identity_text(&self.title).is_none() {
            return Err(invalid_periodical_identity("期刊标题非法"));
        }
        if self.issn_print.is_none() && self.issn_electronic.is_none() {
            return Err(invalid_periodical_identity("期刊至少需要一个 ISSN 身份"));
        }
        if let Some(publisher) = self.publisher.as_deref()
            && identity_text(publisher).is_none()
        {
            return Err(invalid_periodical_identity("期刊出版方非法"));
        }
        if self.issn_print.is_some() && self.issn_print.as_ref() == self.issn_electronic.as_ref() {
            return Err(invalid_periodical_identity(
                "print 与 electronic ISSN 不得是同一个身份",
            ));
        }
        Ok(())
    }
}

/// 卷在期刊内的可比较身份。
///
/// 数值卷号使用定点表示（`*1000` 取整）以便参与 `Hash` 与数据库唯一键；
/// `Unassigned` 明确表示「来源没有给出卷号或卷标」，不是推断出的占位卷。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeriodicalVolumeIdentity {
    Number(i64),
    Label(String),
    Unassigned,
}

impl PeriodicalVolumeIdentity {
    /// 持久化唯一键。Repository 与 Application 共用这一实现，禁止各自拼接。
    pub fn key(&self) -> String {
        match self {
            Self::Number(millis) => format!("number:{millis}"),
            Self::Label(label) => format!("label:{label}"),
            Self::Unassigned => "unassigned".to_owned(),
        }
    }
}

/// 期刊卷。`label` 保留来源原文（例如 `Suppl 1`），`number` 仅在可解析时存在。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PeriodicalVolume {
    pub id: PeriodicalVolumeId,
    pub periodical_id: PeriodicalId,
    pub label: Option<String>,
    pub number: Option<f64>,
    pub year: Option<i32>,
    /// 来源观察顺序；不参与身份，只用于展示排序。
    pub ordinal: u32,
    pub created_at: UtcMillis,
    pub updated_at: UtcMillis,
}

impl PeriodicalVolume {
    pub fn identity(&self) -> PeriodicalVolumeIdentity {
        if let Some(number) = self.number
            && let Some(millis) = non_negative_number_millis(number)
        {
            return PeriodicalVolumeIdentity::Number(millis);
        }
        if let Some(label) = self.label.as_deref()
            && identity_text(label).is_some()
        {
            return PeriodicalVolumeIdentity::Label(normalize_identity_label(label));
        }
        PeriodicalVolumeIdentity::Unassigned
    }

    pub fn validate(&self) -> Result<(), AppError> {
        if let Some(label) = self.label.as_deref()
            && identity_text(label).is_none()
        {
            return Err(invalid_periodical_identity("期刊卷标非法"));
        }
        if let Some(number) = self.number
            && non_negative_number_millis(number).is_none()
        {
            return Err(invalid_periodical_identity("期刊卷号非法"));
        }
        if let Some(year) = self.year
            && !(1000..=2999).contains(&year)
        {
            return Err(invalid_periodical_identity("期刊卷年份非法"));
        }
        Ok(())
    }
}

/// 期号在卷内的可比较身份。语义与 [`PeriodicalVolumeIdentity`] 相同。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeriodicalIssueIdentity {
    Number(i64),
    Label(String),
    Unassigned,
}

impl PeriodicalIssueIdentity {
    pub fn key(&self) -> String {
        match self {
            Self::Number(millis) => format!("number:{millis}"),
            Self::Label(label) => format!("label:{label}"),
            Self::Unassigned => "unassigned".to_owned(),
        }
    }
}

/// 期刊期号。`label` 保留不规则期号原文（`3-4`、`Suppl 2`、`Spring`），
/// `publication_date` 同样保留来源文本而非强行规范化。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PeriodicalIssue {
    pub id: PeriodicalIssueId,
    pub volume_id: PeriodicalVolumeId,
    pub label: Option<String>,
    pub number: Option<f64>,
    pub publication_date: Option<String>,
    pub ordinal: u32,
    pub created_at: UtcMillis,
    pub updated_at: UtcMillis,
}

impl PeriodicalIssue {
    pub fn identity(&self) -> PeriodicalIssueIdentity {
        if let Some(number) = self.number
            && let Some(millis) = non_negative_number_millis(number)
        {
            return PeriodicalIssueIdentity::Number(millis);
        }
        if let Some(label) = self.label.as_deref()
            && identity_text(label).is_some()
        {
            return PeriodicalIssueIdentity::Label(normalize_identity_label(label));
        }
        PeriodicalIssueIdentity::Unassigned
    }

    /// 发行日期前四位年份（仅在形如 `2024`/`2024-03-15` 时成立）。
    pub fn publication_year(&self) -> Option<i32> {
        let date = self.publication_date.as_deref()?;
        publication_date_year(date)
    }

    pub fn validate(&self) -> Result<(), AppError> {
        if let Some(label) = self.label.as_deref()
            && identity_text(label).is_none()
        {
            return Err(invalid_periodical_identity("期刊期号非法"));
        }
        if let Some(number) = self.number
            && non_negative_number_millis(number).is_none()
        {
            return Err(invalid_periodical_identity("期刊期号数值非法"));
        }
        if let Some(date) = self.publication_date.as_deref()
            && (identity_text(date).is_none() || publication_date_year(date).is_none())
        {
            return Err(invalid_periodical_identity("期刊发行日期非法"));
        }
        Ok(())
    }
}

/// 文章在来源侧的 opaque 身份（例如 `europepmc` + `PMC1234567`）。
///
/// 这不是 Haven ID，也不包含 URL、Cookie 或请求细节；它只用于导入幂等。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PeriodicalArticleSourceIdentity {
    pub source_key: String,
    pub remote_article_id: String,
}

impl PeriodicalArticleSourceIdentity {
    pub fn new(source_key: &str, remote_article_id: &str) -> Option<Self> {
        Some(Self {
            source_key: identity_text(source_key)?,
            remote_article_id: identity_text(remote_article_id)?,
        })
    }

    pub fn validate(&self) -> Result<(), AppError> {
        Self::new(&self.source_key, &self.remote_article_id)
            .map(|_| ())
            .ok_or_else(|| invalid_periodical_identity("文章来源身份非法"))
    }
}

/// 期刊文章。通过 `media_item_id` 绑定既有 MediaItem，因此阅读、进度、资源与
/// 下载链路完全复用现有模型；`issue_id` 是唯一的上级归属。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PeriodicalArticle {
    pub id: PeriodicalArticleId,
    pub issue_id: PeriodicalIssueId,
    pub media_item_id: MediaItemId,
    /// 期内文章序号（来源可解析时给出，否则为 None，不猜测）。
    pub ordinal: Option<u32>,
    pub title: String,
    pub doi: Option<Doi>,
    pub page_range: Option<PageRange>,
    pub source: PeriodicalArticleSourceIdentity,
    pub created_at: UtcMillis,
    pub updated_at: UtcMillis,
}

impl PeriodicalArticle {
    pub fn validate(&self) -> Result<(), AppError> {
        if identity_text(&self.title).is_none() {
            return Err(invalid_periodical_identity("文章标题非法"));
        }
        if let Some(page_range) = self.page_range.as_ref() {
            page_range.validate()?;
        }
        self.source.validate()
    }
}

/// 文章在期刊层级中的完整归属链。
///
/// Repository 读取与导入计划都以这一条链为单位，避免调用方分别读取
/// Volume/Issue/Periodical 后自行拼出不一致的归属。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PeriodicalPlacement {
    pub periodical: Periodical,
    pub volume: PeriodicalVolume,
    pub issue: PeriodicalIssue,
    pub article: PeriodicalArticle,
}

impl PeriodicalPlacement {
    /// 校验四层归属逐级一致（Volume→Periodical、Issue→Volume、Article→Issue）。
    ///
    /// 这是「期刊层级不是 parent_id 复用」这一约束的可执行形式：任何一层指向
    /// 其他父节点都必须失败，而不是被当成同一棵树。
    pub fn validate(&self) -> Result<(), AppError> {
        self.periodical.validate()?;
        self.volume.validate()?;
        self.issue.validate()?;
        self.article.validate()?;
        if self.volume.periodical_id != self.periodical.id {
            return Err(invalid_periodical_identity("卷不属于该期刊"));
        }
        if self.issue.volume_id != self.volume.id {
            return Err(invalid_periodical_identity("期号不属于该卷"));
        }
        if self.article.issue_id != self.issue.id {
            return Err(invalid_periodical_identity("文章不属于该期号"));
        }
        Ok(())
    }

    pub fn work_id(&self) -> WorkId {
        self.periodical.work_id
    }
}

/// 期刊的来源绑定键。
///
/// 期刊身份只能由 ISSN 建立；标题不是身份，因此没有 ISSN 的来源不能建立
/// 期刊层级，调用方必须回退到既有的单篇导入路径而不是按标题猜期刊。
pub fn periodical_source_ref_key(source_key: &str, issn: &Issn) -> Option<String> {
    let source_key = identity_text(source_key)?;
    Some(format!("{source_key}:issn:{}", issn.as_str()))
}

/// 从 print/electronic ISSN 中选择期刊绑定身份：electronic 优先（在线版是
/// Europe PMC 的主身份），否则回退 print。
pub fn preferred_journal_issn<'a>(
    issn_print: Option<&'a Issn>,
    issn_electronic: Option<&'a Issn>,
) -> Option<&'a Issn> {
    issn_electronic.or(issn_print)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> UtcMillis {
        UtcMillis(1_000)
    }

    fn issn(value: &str) -> Issn {
        Issn::parse(value).unwrap()
    }

    fn periodical(id: PeriodicalId) -> Periodical {
        Periodical {
            id,
            work_id: WorkId::new(),
            title: "Nature Communications".into(),
            issn_print: Some(issn("2041-1723")),
            issn_electronic: None,
            publisher: Some("Nature Portfolio".into()),
            created_at: now(),
            updated_at: now(),
        }
    }

    fn volume(periodical_id: PeriodicalId, number: Option<f64>) -> PeriodicalVolume {
        PeriodicalVolume {
            id: PeriodicalVolumeId::new(),
            periodical_id,
            label: None,
            number,
            year: Some(2024),
            ordinal: 0,
            created_at: now(),
            updated_at: now(),
        }
    }

    fn issue(volume_id: PeriodicalVolumeId, label: &str, number: Option<f64>) -> PeriodicalIssue {
        PeriodicalIssue {
            id: PeriodicalIssueId::new(),
            volume_id,
            label: Some(label.to_owned()),
            number,
            publication_date: Some("2024-03-15".into()),
            ordinal: 0,
            created_at: now(),
            updated_at: now(),
        }
    }

    fn article(issue_id: PeriodicalIssueId) -> PeriodicalArticle {
        PeriodicalArticle {
            id: PeriodicalArticleId::new(),
            issue_id,
            media_item_id: MediaItemId::new(),
            ordinal: Some(12),
            title: "A periodical article".into(),
            doi: Doi::parse("10.1038/s41467-024-00001-2"),
            page_range: PageRange::new("e12345", None),
            source: PeriodicalArticleSourceIdentity::new("europepmc", "PMC1234567").unwrap(),
            created_at: now(),
            updated_at: now(),
        }
    }

    #[test]
    fn issn_requires_valid_check_digit_and_normalizes_to_hyphenated_form() {
        assert_eq!(issn("0378-5955").as_str(), "0378-5955");
        assert_eq!(issn("03785955").as_str(), "0378-5955");
        assert_eq!(issn("1420682X").as_str(), "1420-682X");
        assert!(Issn::parse("0378-5954").is_none(), "校验位错误必须被拒绝");
        assert!(Issn::parse("0378-595").is_none());
        assert!(Issn::parse("ISSN 0378-5955").is_none());
        assert!(Issn::parse("0378-5955\n").is_none());
    }

    #[test]
    fn issn_deserialization_rejects_non_canonical_values() {
        let parsed: Issn = serde_json::from_str("\"03785955\"").unwrap();
        assert_eq!(parsed.as_str(), "0378-5955");
        assert!(serde_json::from_str::<Issn>("\"0378-5954\"").is_err());
        assert!(serde_json::from_str::<Issn>("\"https://example.invalid\"").is_err());
    }

    #[test]
    fn doi_and_page_range_reject_url_shaped_or_control_text() {
        assert_eq!(
            Doi::parse(" 10.1038/s41467-024-00001-2 ").unwrap().as_str(),
            "10.1038/s41467-024-00001-2"
        );
        assert!(Doi::parse("https://doi.org/10.1038/x").is_none());
        assert!(Doi::parse("10.1038").is_none());
        assert!(Doi::parse("10.1038/").is_none());
        assert!(PageRange::new("e12345", Some("e12346")).is_some());
        assert!(PageRange::new("", None).is_none());
        assert!(PageRange::new("12 3", None).is_none());
    }

    #[test]
    fn periodical_keeps_print_and_electronic_issn_as_distinct_identities() {
        let mut journal = periodical(PeriodicalId::new());
        journal.issn_electronic = Some(issn("1420-682X"));
        journal.validate().unwrap();
        assert_eq!(journal.identities().len(), 2);
        assert!(journal.matches_issn(&issn("1420-682X")));

        // 同一个 ISSN 不能同时声明为两种身份。
        journal.issn_electronic = Some(issn("2041-1723"));
        assert_eq!(
            journal.validate().unwrap_err().code().as_str(),
            "INVALID_PERIODICAL_IDENTITY"
        );
    }

    #[test]
    fn periodical_requires_an_issn_and_rejects_invalid_volume_facts() {
        let mut journal = periodical(PeriodicalId::new());
        journal.issn_print = None;
        assert!(journal.validate().is_err(), "没有 ISSN 不能建立期刊身份");

        let periodical_id = PeriodicalId::new();
        assert!(
            volume(periodical_id, Some(-1.0)).validate().is_err(),
            "负卷号不是有效的出版事实"
        );
        assert!(
            PeriodicalVolume {
                year: Some(999),
                ..volume(periodical_id, Some(1.0))
            }
            .validate()
            .is_err(),
            "卷年份必须是可解释的公历年份"
        );
        assert!(
            PeriodicalIssue {
                number: Some(-1.0),
                ..issue(PeriodicalVolumeId::new(), "1", None)
            }
            .validate()
            .is_err(),
            "负期号不是有效的出版事实"
        );
    }

    #[test]
    fn volume_and_issue_identity_prefers_number_then_label_then_unassigned() {
        let periodical_id = PeriodicalId::new();
        let numbered = volume(periodical_id, Some(12.5));
        assert_eq!(
            numbered.identity(),
            PeriodicalVolumeIdentity::Number(12_500)
        );
        assert_eq!(numbered.identity().key(), "number:12500");

        let labelled = PeriodicalVolume {
            label: Some("Suppl 1".into()),
            ..volume(periodical_id, None)
        };
        assert_eq!(
            labelled.identity(),
            PeriodicalVolumeIdentity::Label("suppl 1".into())
        );

        let unassigned = volume(periodical_id, None);
        assert_eq!(unassigned.identity(), PeriodicalVolumeIdentity::Unassigned);
        assert_eq!(unassigned.identity().key(), "unassigned");
        unassigned.validate().unwrap();

        let irregular = issue(PeriodicalVolumeId::new(), "3-4", None);
        assert_eq!(
            irregular.identity(),
            PeriodicalIssueIdentity::Label("3-4".into()),
            "不规则期号必须保留原文并按标签身份比较"
        );
        assert_eq!(irregular.publication_year(), Some(2024));
        assert_eq!(
            issue(PeriodicalVolumeId::new(), "2", Some(2.0)).identity(),
            PeriodicalIssueIdentity::Number(2_000)
        );

        let invalid_date = PeriodicalIssue {
            publication_date: Some("2023-02-29".into()),
            ..irregular
        };
        assert!(
            invalid_date.validate().is_err(),
            "非法发行日期不能进入领域模型"
        );
        let year_only = PeriodicalIssue {
            publication_date: Some("2024".into()),
            ..invalid_date.clone()
        };
        assert_eq!(year_only.publication_year(), Some(2024));
        assert!(
            PeriodicalIssue {
                publication_date: Some("2024-03-15-extra".into()),
                ..year_only
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn placement_rejects_cross_links_between_hierarchies() {
        let journal = periodical(PeriodicalId::new());
        let vol = volume(journal.id, Some(1.0));
        let iss = issue(vol.id, "2", Some(2.0));
        let art = article(iss.id);
        let valid = PeriodicalPlacement {
            periodical: journal.clone(),
            volume: vol.clone(),
            issue: iss.clone(),
            article: art.clone(),
        };
        valid.validate().unwrap();
        assert_eq!(valid.work_id(), journal.work_id);

        let cross_volume = PeriodicalPlacement {
            periodical: Periodical {
                id: PeriodicalId::new(),
                ..journal.clone()
            },
            volume: vol.clone(),
            issue: iss.clone(),
            article: art.clone(),
        };
        assert!(cross_volume.validate().is_err(), "卷必须属于该期刊");

        let cross_issue = PeriodicalPlacement {
            periodical: journal.clone(),
            volume: vol.clone(),
            issue: PeriodicalIssue {
                id: PeriodicalIssueId::new(),
                volume_id: PeriodicalVolumeId::new(),
                ..iss.clone()
            },
            article: art.clone(),
        };
        assert!(cross_issue.validate().is_err(), "期号必须属于该卷");

        let cross_article = PeriodicalPlacement {
            periodical: journal,
            volume: vol,
            issue: iss,
            article: PeriodicalArticle {
                id: PeriodicalArticleId::new(),
                issue_id: PeriodicalIssueId::new(),
                ..art
            },
        };
        assert!(cross_article.validate().is_err(), "文章必须属于该期号");
    }

    #[test]
    fn article_source_identity_is_opaque_and_rejects_urls() {
        assert!(PeriodicalArticleSourceIdentity::new("europepmc", "PMC1234567").is_some());
        assert!(PeriodicalArticleSourceIdentity::new("europepmc", "  ").is_none());
        assert!(
            PeriodicalArticleSourceIdentity::new(
                "europepmc",
                "https://www.ebi.ac.uk/europepmc/PMC1"
            )
            .is_none(),
            "来源身份不能是 URL"
        );
    }

    #[test]
    fn article_validation_rejects_page_ranges_that_bypass_the_constructor() {
        let mut article = article(PeriodicalIssueId::new());
        article.page_range = Some(PageRange {
            start: String::new(),
            end: None,
        });
        assert!(
            article.validate().is_err(),
            "反序列化或外部构造的非法页码也必须拒绝"
        );
    }

    #[test]
    fn periodical_source_ref_key_requires_an_issn_identity() {
        let issn = issn("2041-1723");
        assert_eq!(
            periodical_source_ref_key("europepmc", &issn).as_deref(),
            Some("europepmc:issn:2041-1723")
        );
        assert!(periodical_source_ref_key("", &issn).is_none());
        assert_eq!(
            preferred_journal_issn(None, Some(&issn)).map(Issn::as_str),
            Some("2041-1723")
        );
        assert_eq!(preferred_journal_issn(None, None), None);
    }

    #[test]
    fn placement_serde_roundtrip_preserves_irregular_labels() {
        let journal = periodical(PeriodicalId::new());
        let vol = volume(journal.id, None);
        let iss = issue(vol.id, "3-4", None);
        let art = article(iss.id);
        let placement = PeriodicalPlacement {
            periodical: journal,
            volume: vol,
            issue: iss,
            article: art,
        };
        let json = serde_json::to_string(&placement).unwrap();
        let back: PeriodicalPlacement = serde_json::from_str(&json).unwrap();
        assert_eq!(back, placement);
        back.validate().unwrap();
    }
}
