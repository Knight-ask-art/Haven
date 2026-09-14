//! 专用报刊（Periodical）Provider 端口与 provider 中立的文章记录。
//!
//! 报刊来源不能用通用 metadata 搜索 Provider 承载：期刊层级需要卷、期、发行
//! 日期、DOI、页码和 PMCID 这类结构化事实，而且必须能明确区分「有全文可读」
//! 与「只有元数据」。
//!
//! 安全边界：
//! - 记录里没有 URL、Cookie、请求头或原始响应；实现方持有固定主机与请求细节，
//!   Application 只看到经过校验的 opaque 身份与结构化字段；
//! - `remote_article_id` 必须由实现方按来源规则校验后才能出现在记录中。

use async_trait::async_trait;
use haven_common::{AppError, ErrorKind};
use haven_domain::periodical::{
    Doi, Issn, PageRange, Periodical, PeriodicalArticle, PeriodicalIssue, PeriodicalVolume,
};

/// Europe PMC 的固定来源 key（与来源注册表、资源身份和 Provider 一致）。
pub const EUROPE_PMC_SOURCE_KEY: &str = "europepmc";

/// 期刊层级的只读聚合（应用查询能力；不是 Wire DTO）。
#[derive(Debug, Clone, PartialEq)]
pub struct PeriodicalTree {
    pub periodical: Periodical,
    pub volumes: Vec<PeriodicalVolumeTree>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PeriodicalVolumeTree {
    pub volume: PeriodicalVolume,
    pub issues: Vec<PeriodicalIssueTree>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PeriodicalIssueTree {
    pub issue: PeriodicalIssue,
    pub articles: Vec<PeriodicalArticle>,
}

/// 期刊身份（provider 中立投影）。标题只作展示事实，身份由 ISSN 建立。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeriodicalJournalRecord {
    pub title: String,
    pub issn_print: Option<Issn>,
    pub issn_electronic: Option<Issn>,
    pub publisher: Option<String>,
}

impl PeriodicalJournalRecord {
    /// 建立期刊层级所使用的 ISSN：electronic 优先，其次 print。
    pub fn preferred_issn(&self) -> Option<&Issn> {
        self.issn_electronic.as_ref().or(self.issn_print.as_ref())
    }

    /// 没有 ISSN 就不能建立稳定的期刊身份（标题不是身份）。
    pub fn has_stable_identity(&self) -> bool {
        self.preferred_issn().is_some()
    }
}

/// 卷的 provider 观察值。`label` 保留来源原文，`number` 仅在可解析时存在。
#[derive(Debug, Clone, PartialEq)]
pub struct PeriodicalVolumeRecord {
    pub label: Option<String>,
    pub number: Option<f64>,
    pub year: Option<i32>,
}

/// 期号的 provider 观察值；`label` 保留不规则期号原文。
#[derive(Debug, Clone, PartialEq)]
pub struct PeriodicalIssueRecord {
    pub label: Option<String>,
    pub number: Option<f64>,
    pub publication_date: Option<String>,
}

impl PeriodicalIssueRecord {
    /// 发行日期前四位年份（`2024`/`2024-03-15` 成立，其余为 None）。
    pub fn publication_year(&self) -> Option<i32> {
        let date = self.publication_date.as_deref()?;
        let digits: String = date.chars().take_while(char::is_ascii_digit).collect();
        if digits.len() != 4 {
            return None;
        }
        digits.parse().ok()
    }
}

/// 文章正文可用性。Provider 必须显式声明，Application 不把无正文条目
/// 宣称成可读。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeriodicalArticleAvailability {
    /// 来源提供可读全文；导入后才允许把 Resource 标记为可用。
    FullText,
    /// 只有元数据；内容不存在或尚未开放，Resource 不得声明为可用。
    MetadataOnly,
}

/// 一篇文章及其期刊归属的 provider 观察。
#[derive(Debug, Clone, PartialEq)]
pub struct PeriodicalArticleRecord {
    pub source_key: String,
    pub remote_article_id: String,
    /// 来源没有给出期刊身份时为 None；调用方必须回退到单篇导入，不能按标题猜。
    pub journal: Option<PeriodicalJournalRecord>,
    pub volume: Option<PeriodicalVolumeRecord>,
    pub issue: Option<PeriodicalIssueRecord>,
    pub title: String,
    pub doi: Option<Doi>,
    pub page_range: Option<PageRange>,
    /// 期内文章序号（来源可解析时给出）。
    pub ordinal: Option<u32>,
    pub availability: PeriodicalArticleAvailability,
    pub mime_type: Option<String>,
}

impl PeriodicalArticleRecord {
    pub fn is_readable(&self) -> bool {
        self.availability == PeriodicalArticleAvailability::FullText
    }

    /// 请求身份与响应身份必须一致，否则不能把结果写进请求 PMCID 的归属链。
    pub fn validate_identity(
        &self,
        source_key: &str,
        remote_article_id: &str,
    ) -> Result<(), AppError> {
        if self.source_key == source_key && self.remote_article_id == remote_article_id {
            return Ok(());
        }
        Err(AppError::new(
            "SOURCE_UNAVAILABLE",
            ErrorKind::Network,
            "报刊条目标识与请求不一致",
            true,
        ))
    }
}

/// 报刊 Provider 端口。
///
/// 实现方（infrastructure）负责固定主机、响应大小上限、XML 校验和来源身份
/// 校验；Application 只接收 provider 中立的结构化记录。
#[async_trait]
pub trait PeriodicalProvider: Send + Sync {
    async fn article(
        &self,
        source_key: &str,
        remote_article_id: &str,
    ) -> Result<PeriodicalArticleRecord, AppError>;
}
