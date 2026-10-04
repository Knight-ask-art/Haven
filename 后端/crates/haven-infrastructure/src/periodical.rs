//! 专用 Europe PMC 报刊（Periodical）Provider。
//!
//! 报刊导入需要的是期刊名、print/electronic ISSN、卷、期、发行日期、文章标题、
//! DOI、页码/文章序号与 PMCID 这类结构化事实，因此这里是一个独立的固定来源
//! Provider，而不是继续扩张通用 metadata 搜索。
//!
//! 边界：
//! - 主机固定为 `www.ebi.ac.uk`（复用 `online_sources::OnlineContentClient` 的
//!   同一条白名单与有界读取链路，不新建第二套 HTTP 栈）；
//! - 只接受校验后的 PMCID；响应没有声明 PMCID、或声明的 PMCID 与请求不一致时
//!   直接失败，绝不把结果写进请求身份的归属链；
//! - 记录里没有 URL、Cookie、请求头或原始响应；`<self-uri>`/`<uri>` 等地址
//!   元素只用于判定「有正文」，永不进入记录；
//! - 正文可读性显式区分 `FullText` 与 `MetadataOnly`：只有存在真实
//!   `<body>` 段落（`<p>`）时才宣称全文；全文接口明确不存在时回退到受控的
//!   检索元数据投影，元数据条目不会被宣称可读。

use std::sync::Arc;

use async_trait::async_trait;
use haven_application::services::periodical::{
    PeriodicalArticleAvailability, PeriodicalArticleRecord, PeriodicalIssueRecord,
    PeriodicalJournalRecord, PeriodicalProvider, PeriodicalVolumeRecord,
};
use haven_common::{AppError, ErrorKind};
use haven_domain::periodical::{Doi, Issn, PageRange};
use quick_xml::events::{BytesStart, Event};
use serde_json::Value;

use crate::online_sources::{EUROPE_PMC_API, HostPolicy, OnlineContentClient, is_pmcid};

/// 固定来源 key 由应用层单点定义，Provider 只重导出以避免取值漂移。
pub use haven_application::services::periodical::EUROPE_PMC_SOURCE_KEY;

/// 全文 XML 的有界读取上限（与该来源既有的受控正文读取保持一致）。
const MAX_ARTICLE_XML_BYTES: usize = 8 * 1024 * 1024;

/// Europe PMC 检索接口的受控元数据投影（JSON）。
const EUROPE_PMC_SEARCH_FORMAT: &str = "resultType=core&format=json";

/// Europe PMC 报刊 Provider。
#[derive(Clone)]
pub struct EuropePmcPeriodicalProvider {
    client: Arc<OnlineContentClient>,
}

impl EuropePmcPeriodicalProvider {
    pub fn new(client: Arc<OnlineContentClient>) -> Self {
        Self { client }
    }

    /// 受控的元数据回退：仅在全文接口明确「不存在」时使用。
    ///
    /// 它只读 Europe PMC 的检索元数据，产出的记录永远是 `MetadataOnly`，
    /// 因此调用方不会为它建立可读 Resource；网络、解析或身份校验失败仍然
    /// 照常失败，来源故障不会被伪装成「只有元数据」。
    async fn metadata_record(&self, pmcid: &str) -> Result<PeriodicalArticleRecord, AppError> {
        let url =
            format!("{EUROPE_PMC_API}/search?query=EXT_ID:{pmcid}&{EUROPE_PMC_SEARCH_FORMAT}");
        let value = self.client.json(&url, HostPolicy::EuropePmc).await?;
        parse_europe_pmc_metadata(&value, pmcid)
    }
}

#[async_trait]
impl PeriodicalProvider for EuropePmcPeriodicalProvider {
    async fn article(
        &self,
        source_key: &str,
        remote_article_id: &str,
    ) -> Result<PeriodicalArticleRecord, AppError> {
        if source_key != EUROPE_PMC_SOURCE_KEY {
            return Err(invalid_argument("该来源不是 Europe PMC 报刊来源"));
        }
        let pmcid = remote_article_id.trim();
        if !is_pmcid(pmcid) {
            return Err(invalid_argument("Europe PMC 标识非法"));
        }
        let url = format!("{EUROPE_PMC_API}/{pmcid}/fullTextXML");
        match self
            .client
            .text_optional(&url, HostPolicy::EuropePmc, MAX_ARTICLE_XML_BYTES)
            .await?
        {
            Some(xml) => parse_europe_pmc_article(&xml, pmcid),
            None => self.metadata_record(pmcid).await,
        }
    }
}

// ---------- 纯解析（不依赖网络，可独立测试） ----------

#[derive(Debug, Default)]
struct JournalMeta {
    title: String,
    issn_print: Option<String>,
    issn_electronic: Option<String>,
    publisher: Option<String>,
}

#[derive(Debug, Default, Clone, Copy)]
struct JatsDate {
    year: Option<i32>,
    month: Option<u32>,
    day: Option<u32>,
}

impl JatsDate {
    fn is_empty(&self) -> bool {
        self.year.is_none()
    }

    /// 只保留可解释的日期分量：越界或自相矛盾的取值被丢弃，而不是被
    /// 格式化成 `2024-13-45` 这类看似事实的假日期。
    ///
    /// - 年份必须落在 1000..=2999；
    /// - 月必须在 1..=12；
    /// - 日必须落在该年该月的实际天数（含闰年）；
    /// - 只有日、没有月不构成日期。
    fn sanitized(self) -> Self {
        let Some(year) = self.year.filter(|year| (1000..=2999).contains(year)) else {
            return Self::default();
        };
        let Some(month) = self.month.filter(|month| (1..=12).contains(month)) else {
            return Self {
                year: Some(year),
                ..Self::default()
            };
        };
        let day = self
            .day
            .filter(|day| (1..=days_in_month(year, month)).contains(day));
        Self {
            year: Some(year),
            month: Some(month),
            day,
        }
    }

    /// 保留来源精度：只有年份时输出 `YYYY`，不补零伪造月日。
    fn format(&self) -> Option<String> {
        let year = self.year?;
        match (self.month, self.day) {
            (Some(month), Some(day)) => Some(format!("{year:04}-{month:02}-{day:02}")),
            (Some(month), None) => Some(format!("{year:04}-{month:02}")),
            _ => Some(format!("{year:04}")),
        }
    }
}

/// 公历该年该月的天数（含闰年规则）。
fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

#[derive(Debug, Default)]
struct ArticleMeta {
    pmcid: Option<String>,
    doi: Option<String>,
    title: Option<String>,
    volume: Option<String>,
    issue: Option<String>,
    fpage: Option<String>,
    lpage: Option<String>,
    elocation_id: Option<String>,
    pub_date: Option<JatsDate>,
    /// 0 = 正式出版期号（ppub/epub/collection），1 = 其他；只保留最优的一条。
    pub_date_rank: u8,
}

fn source_unavailable(message: &'static str) -> AppError {
    AppError::new("SOURCE_UNAVAILABLE", ErrorKind::Network, message, true)
}

fn invalid_argument(message: &'static str) -> AppError {
    AppError::new("INVALID_ARGUMENT", ErrorKind::Validation, message, false)
}

fn local_name(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw)
        .rsplit(':')
        .next()
        .unwrap_or("")
        .to_owned()
}

fn decode_xml_text(text: &quick_xml::events::BytesText<'_>) -> Option<String> {
    let decoded = text.decode().ok()?;
    quick_xml::escape::unescape(decoded.as_ref())
        .ok()
        .map(|value| value.into_owned())
}

fn decode_xml_reference(reference: &quick_xml::events::BytesRef<'_>) -> Option<String> {
    let name = reference.decode().ok()?;
    let raw = format!("&{name};");
    quick_xml::escape::unescape(&raw)
        .ok()
        .map(|value| value.into_owned())
}

fn clean_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn attribute(event: &BytesStart<'_>, name: &str) -> Option<String> {
    event
        .attributes()
        .flatten()
        .find(|attribute| local_name(attribute.key.as_ref()) == name)
        .and_then(|attribute| String::from_utf8(attribute.value.into_owned()).ok())
        .map(|value| value.trim().to_ascii_lowercase())
}

fn under(path: &[String], ancestor: &str) -> bool {
    path.iter().any(|name| name == ancestor)
}

/// 正文段落：必须是 `<body>` 下的 `<p>`。
///
/// 只统计 `<body>` 内的非空文本会让「只有小节标题」的条目被宣称全文可读，而
/// 该来源既有的阅读路径只渲染 `<p>` 段落——两边会对同一个 PMCID 给出矛盾的
/// 结论。因此可读性必须与阅读路径看到的内容一致。
fn is_body_paragraph(path: &[String]) -> bool {
    under(path, "body") && under(path, "p")
}

fn parent_is(path: &[String], expected: &str) -> bool {
    path.last().is_some_and(|name| name == expected)
}

fn set_once(slot: &mut Option<String>, value: String) {
    if slot.is_none() && !value.is_empty() {
        *slot = Some(value);
    }
}

fn pub_date_rank(kind: &str) -> u8 {
    match kind {
        "ppub" | "ppublish" | "print" | "epub" | "epublish" | "electronic" | "collection" => 0,
        _ => 1,
    }
}

/// 解析 Europe PMC 全文 XML（JATS）中的期刊与文章归属事实。
///
/// 该函数是纯函数：没有网络、文件或全局状态，因此可以用固定 fixture 测试。
pub fn parse_europe_pmc_article(
    xml: &str,
    expected_pmcid: &str,
) -> Result<PeriodicalArticleRecord, AppError> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut journal = JournalMeta::default();
    let mut meta = ArticleMeta::default();
    let mut body_has_text = false;
    let mut path: Vec<String> = Vec::new();
    let mut text_stack: Vec<String> = Vec::new();
    let mut issn_pub_type: Option<String> = None;
    let mut article_id_type: Option<String> = None;
    let mut current_pub_date: Option<JatsDate> = None;
    let mut current_pub_date_rank = 1u8;

    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = local_name(event.name().as_ref());
                match name.as_str() {
                    "issn" => issn_pub_type = attribute(&event, "pub-type"),
                    "article-id" => article_id_type = attribute(&event, "pub-id-type"),
                    "pub-date" => {
                        current_pub_date = Some(JatsDate::default());
                        current_pub_date_rank =
                            pub_date_rank(attribute(&event, "pub-type").as_deref().unwrap_or(""));
                    }
                    _ => {}
                }
                path.push(name);
                text_stack.push(String::new());
            }
            Ok(Event::Text(text)) => {
                let value = decode_xml_text(&text)
                    .ok_or_else(|| source_unavailable("Europe PMC 全文 XML 文本无效"))?;
                if is_body_paragraph(&path) && !value.trim().is_empty() {
                    body_has_text = true;
                }
                if let Some(current) = text_stack.last_mut() {
                    current.push_str(&value);
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                let value = decode_xml_reference(&reference)
                    .ok_or_else(|| source_unavailable("Europe PMC 全文 XML 实体引用无效"))?;
                if is_body_paragraph(&path) && !value.trim().is_empty() {
                    body_has_text = true;
                }
                if let Some(current) = text_stack.last_mut() {
                    current.push_str(&value);
                }
            }
            Ok(Event::CData(text)) => {
                let value = String::from_utf8_lossy(text.as_ref()).into_owned();
                if is_body_paragraph(&path) && !value.trim().is_empty() {
                    body_has_text = true;
                }
                if let Some(current) = text_stack.last_mut() {
                    current.push_str(&value);
                }
            }
            Ok(Event::End(event)) => {
                let name = local_name(event.name().as_ref());
                if path.last() != Some(&name) || text_stack.is_empty() {
                    return Err(source_unavailable("Europe PMC 全文 XML 标签结构无效"));
                }
                let own = text_stack.pop().unwrap_or_default();
                path.pop();
                if let Some(parent) = text_stack.last_mut() {
                    parent.push(' ');
                    parent.push_str(&own);
                }
                let value = clean_text(&own);
                match name.as_str() {
                    "issn" => {
                        if under(&path, "journal-meta") && !value.is_empty() {
                            match issn_pub_type.take().as_deref() {
                                Some("ppub") | Some("print") => {
                                    set_once(&mut journal.issn_print, value)
                                }
                                Some("epub") | Some("electronic") | Some("online") => {
                                    set_once(&mut journal.issn_electronic, value)
                                }
                                _ => {}
                            }
                        }
                    }
                    "article-id" => {
                        if parent_is(&path, "article-meta") && !value.is_empty() {
                            match article_id_type.take().as_deref() {
                                Some("pmc") | Some("pmcid") => set_once(&mut meta.pmcid, value),
                                Some("doi") => set_once(&mut meta.doi, value),
                                _ => {}
                            }
                        }
                    }
                    "journal-title" => {
                        if under(&path, "journal-meta")
                            && journal.title.is_empty()
                            && !value.is_empty()
                        {
                            journal.title = value;
                        }
                    }
                    "publisher-name" => {
                        if under(&path, "journal-meta") {
                            set_once(&mut journal.publisher, value);
                        }
                    }
                    // 只有 article-meta 的直接子元素才是文章归属事实；`<back><ref-list>`
                    // 里的同名元素（引用条目的卷/页码）绝不能进入文章归属。
                    "article-title" if under(&path, "article-meta") => {
                        set_once(&mut meta.title, value)
                    }
                    "volume" if parent_is(&path, "article-meta") => {
                        set_once(&mut meta.volume, value)
                    }
                    "issue" if parent_is(&path, "article-meta") => set_once(&mut meta.issue, value),
                    "fpage" if parent_is(&path, "article-meta") => set_once(&mut meta.fpage, value),
                    "lpage" if parent_is(&path, "article-meta") => set_once(&mut meta.lpage, value),
                    "elocation-id" if parent_is(&path, "article-meta") => {
                        set_once(&mut meta.elocation_id, value)
                    }
                    "year" | "month" | "day"
                        if under(&path, "article-meta") && under(&path, "pub-date") =>
                    {
                        if let Some(date) = current_pub_date.as_mut() {
                            let parsed = value.parse::<i32>().ok();
                            match name.as_str() {
                                "year" => date.year = parsed,
                                "month" => date.month = parsed.and_then(|v| u32::try_from(v).ok()),
                                "day" => date.day = parsed.and_then(|v| u32::try_from(v).ok()),
                                _ => {}
                            }
                        }
                    }
                    "pub-date" if under(&path, "article-meta") => {
                        if let Some(date) = current_pub_date.take().map(JatsDate::sanitized)
                            && !date.is_empty()
                            && (meta.pub_date.is_none()
                                || current_pub_date_rank < meta.pub_date_rank)
                        {
                            meta.pub_date = Some(date);
                            meta.pub_date_rank = current_pub_date_rank;
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => {
                if !path.is_empty() || !text_stack.is_empty() {
                    return Err(source_unavailable("Europe PMC 全文 XML 未闭合"));
                }
                break;
            }
            Err(_) => return Err(source_unavailable("Europe PMC 全文 XML 结构无效")),
            _ => {}
        }
    }

    build_record(journal, meta, body_has_text, expected_pmcid)
}

fn build_record(
    journal: JournalMeta,
    meta: ArticleMeta,
    body_has_text: bool,
    expected_pmcid: &str,
) -> Result<PeriodicalArticleRecord, AppError> {
    // 来源必须自己声明 PMCID，且与请求一致：缺少声明时无法证明这份正文属于
    // 请求的条目，把结果写进请求身份的归属链就是在猜。
    let Some(pmcid) = meta.pmcid.as_deref() else {
        return Err(source_unavailable("Europe PMC 条目未声明 PMCID"));
    };
    if !pmcid.eq_ignore_ascii_case(expected_pmcid) {
        return Err(source_unavailable("Europe PMC 条目身份与请求不一致"));
    }

    // 期刊名缺失时不建立期刊身份（标题不是身份）；非法 ISSN 直接丢弃而不是猜测。
    let journal = if journal.title.is_empty() {
        None
    } else {
        Some(PeriodicalJournalRecord {
            title: journal.title,
            issn_print: journal.issn_print.as_deref().and_then(Issn::parse),
            issn_electronic: journal.issn_electronic.as_deref().and_then(Issn::parse),
            publisher: journal.publisher,
        })
    };

    let issue = meta.issue.as_deref().map(|raw| PeriodicalIssueRecord {
        label: Some(raw.to_owned()),
        number: parse_journal_number(raw),
        publication_date: meta.pub_date.as_ref().and_then(JatsDate::format),
    });
    let volume = meta.volume.as_deref().map(|raw| PeriodicalVolumeRecord {
        label: Some(raw.to_owned()),
        number: parse_journal_number(raw),
        year: meta.pub_date.as_ref().and_then(|date| date.year),
    });

    let doi = meta.doi.as_deref().and_then(Doi::parse);
    // elocation-id 是文章号（如 `e12345`），不是页码；页码只由 fpage/lpage 产生。
    let page_range = meta
        .fpage
        .as_deref()
        .and_then(|start| PageRange::new(start, meta.lpage.as_deref()));
    // `elocation-id` 是来源明确给出的文章号；`fpage` 只表示页码，不能把
    // 页码起始值猜成期内文章序号（例如 `120-128` 的 ordinal 不是 120）。
    let ordinal = meta.elocation_id.as_deref().and_then(article_ordinal);

    // 标题缺失时退化为 PMCID 展示值，与既有 Europe PMC 阅读路径一致。
    let title = meta
        .title
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| expected_pmcid.to_owned());

    Ok(PeriodicalArticleRecord {
        source_key: EUROPE_PMC_SOURCE_KEY.to_owned(),
        remote_article_id: expected_pmcid.to_owned(),
        journal,
        volume,
        issue,
        title,
        doi,
        page_range,
        ordinal,
        availability: if body_has_text {
            PeriodicalArticleAvailability::FullText
        } else {
            PeriodicalArticleAvailability::MetadataOnly
        },
        // 没有正文段落时连 MIME 也不声明：元数据条目没有可读的正文快照。
        mime_type: body_has_text.then(|| "text/html; charset=utf-8".to_owned()),
    })
}

/// 期刊卷号/期号只在整体是一个数时才有数值身份；`Suppl 1` 这类不规则值保留为标签。
fn parse_journal_number(value: &str) -> Option<f64> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || !trimmed
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return None;
    }
    let number = trimmed.parse::<f64>().ok()?;
    number.is_finite().then_some(number)
}

/// 从来源明确的文章号提取文章序号（`e12345` → 12345）。
///
/// 该函数只用于 `<elocation-id>`，因此允许来源把文章号写成纯数字；调用
/// 方不能把它用于 `fpage`/`lpage`，因为页码不是文章序号。
fn article_ordinal(value: &str) -> Option<u32> {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return None;
    }
    let digits = value
        .strip_prefix('e')
        .or_else(|| value.strip_prefix('E'))
        .unwrap_or(value);
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// 从检索结果的 `pageInfo` 提取显式文章号。
///
/// `pageInfo` 同时承载页码（`120`、`120-128`）和电子文章号（`e42`）。
/// 只有带 `e` 前缀的值能在这个字段中证明是文章号；纯数字一律只保留在
/// `page_range`，不猜测其为期内排序序号。
fn page_info_article_ordinal(value: &str) -> Option<u32> {
    let value = value.trim();
    value
        .strip_prefix('e')
        .or_else(|| value.strip_prefix('E'))
        .and_then(article_ordinal)
}

// ---------- 受控元数据回退（Europe PMC `search?resultType=core`） ----------

/// 解析 Europe PMC 检索接口的元数据投影。
///
/// 该函数是纯函数：没有网络、文件或全局状态，因此可以用固定 fixture 测试。
/// 它只产出 `MetadataOnly` 记录——元数据里没有正文段落可读，任何「可读」声明
/// 都会让调用方为不存在的正文建立 Resource。
pub fn parse_europe_pmc_metadata(
    value: &Value,
    expected_pmcid: &str,
) -> Result<PeriodicalArticleRecord, AppError> {
    let results = value
        .get("resultList")
        .and_then(|list| list.get("result"))
        .and_then(Value::as_array)
        .ok_or_else(|| source_unavailable("Europe PMC 元数据响应结构异常"))?;

    // 命中集可能包含同一篇文章的其他来源记录（例如 MED），只有显式声明了
    // 请求 PMCID 的那条才是本次请求的条目。
    let result = results
        .iter()
        .find(|result| {
            result
                .get("pmcid")
                .and_then(Value::as_str)
                .is_some_and(|pmcid| pmcid.trim().eq_ignore_ascii_case(expected_pmcid))
        })
        .ok_or_else(|| source_unavailable("Europe PMC 元数据条目身份与请求不一致"))?;

    let journal_info = result.get("journalInfo");
    let journal = journal_info.and_then(|info| info.get("journal"));
    let journal_title = first_field(journal, &["title", "journalTitle"])
        .or_else(|| first_field(Some(result), &["journalTitle"]))
        .and_then(Value::as_str)
        .map(clean_text)
        .filter(|title| !title.is_empty());
    let journal_record = journal_title.map(|title| {
        let print_issn = first_field(journal, &["ISSN", "issn"])
            .or_else(|| first_field(Some(result), &["journalIssn"]))
            .and_then(Value::as_str)
            .and_then(Issn::parse);
        let electronic_issn = first_field(
            journal,
            &[
                "ESSN",
                "essn",
                "eISSN",
                "eissn",
                "electronicIssn",
                "journalEissn",
            ],
        )
        .or_else(|| first_field(Some(result), &["journalEissn", "journalEssn"]))
        .and_then(Value::as_str)
        .and_then(Issn::parse);
        let publisher = first_field(journal, &["publisher"])
            .or_else(|| first_field(Some(result), &["publisher"]))
            .and_then(Value::as_str)
            .map(clean_text)
            .filter(|publisher| !publisher.is_empty());
        PeriodicalJournalRecord {
            title,
            issn_print: print_issn,
            issn_electronic: electronic_issn,
            publisher,
        }
    });

    let publication_date = [
        first_field(
            journal_info,
            &[
                "dateOfPublication",
                "electronicPublicationDate",
                "printPublicationDate",
            ],
        ),
        first_field(Some(result), &["firstPublicationDate"]),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .find_map(normalize_publication_date);

    let volume_label = first_field(journal_info, &["volume"])
        .or_else(|| first_field(Some(result), &["journalVolume"]))
        .and_then(json_text);
    let issue_label = first_field(journal_info, &["issue"])
        .or_else(|| first_field(Some(result), &["issue"]))
        .and_then(json_text);
    let volume_year = first_field(journal_info, &["yearOfPublication"])
        .or_else(|| first_field(Some(result), &["pubYear"]))
        .and_then(json_integer)
        .and_then(|year| i32::try_from(year).ok())
        .filter(|year| (1000..=2999).contains(year));

    let page_info = first_field(Some(result), &["pageInfo"]).and_then(Value::as_str);
    let page_range = page_info.and_then(parse_page_range);
    let ordinal = page_info.and_then(page_info_article_ordinal);

    let title = result
        .get("title")
        .and_then(Value::as_str)
        .map(clean_text)
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| expected_pmcid.to_owned());

    Ok(PeriodicalArticleRecord {
        source_key: EUROPE_PMC_SOURCE_KEY.to_owned(),
        remote_article_id: expected_pmcid.to_owned(),
        journal: journal_record,
        volume: volume_label.map(|label| PeriodicalVolumeRecord {
            number: parse_journal_number(&label),
            label: Some(label),
            year: volume_year,
        }),
        issue: issue_label.map(|label| PeriodicalIssueRecord {
            number: parse_journal_number(&label),
            label: Some(label),
            publication_date,
        }),
        title,
        doi: result
            .get("doi")
            .and_then(Value::as_str)
            .and_then(Doi::parse),
        page_range,
        ordinal,
        availability: PeriodicalArticleAvailability::MetadataOnly,
        mime_type: None,
    })
}

/// JSON 标量到文本：Europe PMC 的卷/期既可能是字符串也可能是数字。
fn json_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let cleaned = clean_text(text);
            (!cleaned.is_empty()).then_some(cleaned)
        }
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn first_field<'a>(value: Option<&'a Value>, keys: &[&str]) -> Option<&'a Value> {
    let object = value?;
    keys.iter().find_map(|key| object.get(*key))
}

fn json_integer(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
}

/// `pageInfo` 是元数据里唯一的页码事实，形态有 `123`、`120-128`、`e12345`。
fn parse_page_range(value: &str) -> Option<PageRange> {
    let cleaned = clean_text(value);
    if cleaned.is_empty() {
        return None;
    }
    match cleaned.split_once('-') {
        Some((start, end)) => PageRange::new(start, Some(end)),
        None => PageRange::new(&cleaned, None),
    }
}

/// 发行日期只接受可解释的 ISO 形态；越界分量按精度回退，而不是伪造日期。
fn normalize_publication_date(value: &str) -> Option<String> {
    let cleaned = clean_text(value);
    let mut parts = cleaned.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next().and_then(|month| month.parse::<u32>().ok());
    let day = parts.next().and_then(|day| day.parse::<u32>().ok());
    if parts.next().is_some() {
        return None;
    }
    JatsDate {
        year: Some(year),
        month,
        day,
    }
    .sanitized()
    .format()
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_application::services::periodical::PeriodicalProvider;

    /// 固定 fixture：与 Europe PMC `/{PMCID}/fullTextXML` 的 JATS 结构一致，
    /// 并额外带上 `<back><ref-list>` 中同名的 volume/fpage，用于验证不会污染归属。
    const FULL_TEXT_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<article article-type="research-article">
  <front>
    <journal-meta>
      <journal-id journal-id-type="nlm-ta">Nat Commun</journal-id>
      <journal-title-group>
        <journal-title>Nature Communications</journal-title>
        <abbrev-journal-title>Nat Commun</abbrev-journal-title>
      </journal-title-group>
      <issn pub-type="ppub">2041-1723</issn>
      <issn pub-type="epub">1420-682X</issn>
      <publisher><publisher-name>Nature Portfolio</publisher-name></publisher>
    </journal-meta>
    <article-meta>
      <article-id pub-id-type="pmc">PMC1234567</article-id>
      <article-id pub-id-type="doi">10.1038/s41467-024-00001-2</article-id>
      <title-group>
        <article-title>Single-cell <italic>atlas</italic> of the human gut</article-title>
      </title-group>
      <pub-date pub-type="epub"><day>15</day><month>3</month><year>2024</year></pub-date>
      <volume>15</volume>
      <issue>1</issue>
      <elocation-id>e12345</elocation-id>
      <self-uri xlink:href="https://www.ebi.ac.uk/europepmc/webservices/rest/PMC1234567/fullTextXML"/>
    </article-meta>
  </front>
  <body>
    <sec><title>Introduction</title>
      <p>Body text.</p>
      <p>A second paragraph with <italic>inline</italic> markup.</p>
    </sec>
  </body>
  <back>
    <ref-list>
      <ref><element-citation>
        <article-title>An unrelated reference</article-title>
        <volume>99</volume>
        <fpage>1</fpage>
      </element-citation></ref>
    </ref-list>
  </back>
</article>"#;

    /// 只有小节标题、没有任何正文段落的 `<body>`：该来源既有的阅读路径只渲染
    /// `<p>`，因此这种条目不能因为「body 里有文字」就被宣称全文可读。
    const HEADINGS_ONLY_XML: &str = r#"<article>
  <front>
    <journal-meta>
      <journal-title-group><journal-title>Nature Communications</journal-title></journal-title-group>
      <issn pub-type="ppub">2041-1723</issn>
    </journal-meta>
    <article-meta>
      <article-id pub-id-type="pmc">PMC1234567</article-id>
      <title-group><article-title>Headings only</article-title></title-group>
      <pub-date pub-type="ppub"><year>2024</year></pub-date>
      <volume>15</volume>
      <issue>1</issue>
    </article-meta>
  </front>
  <body>
    <sec><title>Introduction</title><p></p></sec>
    <sec><title>Methods</title></sec>
  </body>
</article>"#;

    /// 没有声明 PMCID 的条目：无法证明它属于请求的条目。
    const MISSING_PMCID_XML: &str = r#"<article>
  <front>
    <journal-meta>
      <journal-title-group><journal-title>Nature Communications</journal-title></journal-title-group>
      <issn pub-type="ppub">2041-1723</issn>
    </journal-meta>
    <article-meta>
      <article-id pub-id-type="doi">10.1038/s41467-024-00001-2</article-id>
      <title-group><article-title>No PMC id</article-title></title-group>
    </article-meta>
  </front>
</article>"#;

    /// Europe PMC `search?resultType=core&format=json` 的固定响应结构。
    const METADATA_SEARCH_JSON: &str = r#"{
      "version": "6.9",
      "hitCount": 2,
      "resultList": {
        "result": [
          {
            "id": "9999999",
            "source": "MED",
            "pmid": "9999999",
            "title": "A different record without a PMCID"
          },
          {
            "id": "7654321",
            "source": "PMC",
            "pmcid": "PMC7654321",
            "doi": "10.1038/s41467-019-00001-2",
            "title": "A metadata only entry.",
            "pageInfo": "120-128",
            "journalInfo": {
              "volume": "7",
              "issue": "Suppl 2",
              "yearOfPublication": 2019,
              "printPublicationDate": "2019-05-01",
              "journal": {
                "title": "Journal of Examples",
                "issn": "0378-5955",
                "essn": "1420-682X"
              }
            }
          }
        ]
      }
    }"#;

    /// 只有元数据、没有 `<body>` 的条目（欧洲 PMC 对非开放获取全文的形态）。
    const METADATA_ONLY_XML: &str = r#"<article>
  <front>
    <journal-meta>
      <journal-title-group><journal-title>Journal of Examples</journal-title></journal-title-group>
      <issn pub-type="ppub">0378-5955</issn>
    </journal-meta>
    <article-meta>
      <article-id pub-id-type="pmc">PMC7654321</article-id>
      <title-group><article-title>Metadata only entry</article-title></title-group>
      <pub-date pub-type="ppub"><year>2019</year></pub-date>
      <volume>7</volume>
      <issue>Suppl 2</issue>
      <fpage>120</fpage><lpage>128</lpage>
    </article-meta>
  </front>
</article>"#;

    #[test]
    fn parses_journal_volume_issue_and_article_identity() {
        let record =
            parse_europe_pmc_article(FULL_TEXT_XML, "PMC1234567").expect("parse full text xml");
        let journal = record.journal.as_ref().expect("journal identity");
        assert_eq!(journal.title, "Nature Communications");
        assert_eq!(
            journal.issn_print.as_ref().map(Issn::as_str),
            Some("2041-1723")
        );
        assert_eq!(
            journal.issn_electronic.as_ref().map(Issn::as_str),
            Some("1420-682X")
        );
        assert_eq!(journal.publisher.as_deref(), Some("Nature Portfolio"));
        assert_eq!(
            journal.preferred_issn().map(Issn::as_str),
            Some("1420-682X"),
            "electronic ISSN 是期刊绑定身份的首选"
        );
        assert!(journal.has_stable_identity());

        assert_eq!(record.volume.as_ref().and_then(|v| v.number), Some(15.0));
        assert_eq!(record.volume.as_ref().and_then(|v| v.year), Some(2024));
        let issue = record.issue.as_ref().expect("issue");
        assert_eq!(issue.number, Some(1.0));
        assert_eq!(issue.publication_date.as_deref(), Some("2024-03-15"));
        assert_eq!(issue.publication_year(), Some(2024));
        assert_eq!(
            record.title, "Single-cell atlas of the human gut",
            "内联标记必须归并进标题"
        );
        assert_eq!(
            record.doi.as_ref().map(Doi::as_str),
            Some("10.1038/s41467-024-00001-2")
        );
        assert_eq!(record.ordinal, Some(12345));
        assert_eq!(record.availability, PeriodicalArticleAvailability::FullText);
        assert!(record.is_readable());
        assert_eq!(
            record.mime_type.as_deref(),
            Some("text/html; charset=utf-8"),
            "有正文段落时才声明正文 MIME"
        );
        assert_eq!(record.source_key, EUROPE_PMC_SOURCE_KEY);
        assert_eq!(record.remote_article_id, "PMC1234567");
    }

    #[test]
    fn preserves_xml_general_references_in_article_fields() {
        let xml = FULL_TEXT_XML
            .replace("Nature Communications", "Nature &amp; Communications")
            .replace(
                "Single-cell <italic>atlas</italic> of the human gut",
                "Single-cell <italic>atlas</italic> &amp; research",
            );
        let record = parse_europe_pmc_article(&xml, "PMC1234567").expect("parse entity fixture");

        assert_eq!(
            record
                .journal
                .as_ref()
                .map(|journal| journal.title.as_str()),
            Some("Nature & Communications")
        );
        assert_eq!(record.title, "Single-cell atlas & research");
    }

    #[test]
    fn reference_list_metadata_never_overrides_article_attribution() {
        let record = parse_europe_pmc_article(FULL_TEXT_XML, "PMC1234567").unwrap();
        assert_eq!(
            record.volume.as_ref().and_then(|v| v.number),
            Some(15.0),
            "参考文献里的 volume 不得覆盖文章归属"
        );
        assert_eq!(
            record.page_range, None,
            "该条目只有 elocation-id，没有 fpage/lpage 时不得伪造页码"
        );
    }

    #[test]
    fn metadata_only_articles_are_not_claimed_readable() {
        let record = parse_europe_pmc_article(METADATA_ONLY_XML, "PMC7654321").unwrap();
        assert_eq!(
            record.availability,
            PeriodicalArticleAvailability::MetadataOnly
        );
        assert!(!record.is_readable(), "无正文条目不得被宣称可读");
        assert_eq!(record.mime_type, None, "无正文条目不声明正文 MIME");
        let issue = record.issue.as_ref().unwrap();
        assert_eq!(
            issue.label.as_deref(),
            Some("Suppl 2"),
            "不规则期号保留原文"
        );
        assert_eq!(issue.number, None, "非数值期号不得被解析成数字");
        assert_eq!(issue.publication_date.as_deref(), Some("2019"));
        assert_eq!(
            record
                .page_range
                .as_ref()
                .map(|range| (range.start.as_str(), range.end.as_deref())),
            Some(("120", Some("128")))
        );
        assert_eq!(record.ordinal, None, "页码不能被猜成期内文章序号");
    }

    #[test]
    fn pmcid_mismatch_is_rejected() {
        let error = parse_europe_pmc_article(FULL_TEXT_XML, "PMC9999999").unwrap_err();
        assert_eq!(error.code().as_str(), "SOURCE_UNAVAILABLE");
    }

    #[test]
    fn invalid_issn_and_doi_are_dropped_instead_of_fabricated() {
        let xml = r#"<article>
  <front>
    <journal-meta>
      <journal-title-group><journal-title>Odd Journal</journal-title></journal-title-group>
      <issn pub-type="ppub">2041-1724</issn>
      <issn pub-type="epub">not-an-issn</issn>
    </journal-meta>
    <article-meta>
      <article-id pub-id-type="pmc">PMC1</article-id>
      <article-id pub-id-type="doi">https://doi.org/10.1038/x</article-id>
      <title-group><article-title>Title</article-title></title-group>
      <volume>Suppl A</volume>
    </article-meta>
  </front>
</article>"#;
        let record = parse_europe_pmc_article(xml, "PMC1").unwrap();
        let journal = record.journal.as_ref().unwrap();
        assert_eq!(journal.issn_print, None, "校验位错误的 ISSN 必须被丢弃");
        assert_eq!(journal.issn_electronic, None);
        assert!(!journal.has_stable_identity());
        assert_eq!(record.doi, None, "URL 形状的 DOI 不得进入记录");
        let volume = record.volume.as_ref().unwrap();
        assert_eq!(volume.label.as_deref(), Some("Suppl A"));
        assert_eq!(volume.number, None);
    }

    #[test]
    fn entries_without_a_declared_pmcid_are_rejected() {
        let error = parse_europe_pmc_article(MISSING_PMCID_XML, "PMC1234567").unwrap_err();
        assert_eq!(
            error.code().as_str(),
            "SOURCE_UNAVAILABLE",
            "响应未声明 PMCID 时无法证明归属于请求条目"
        );
    }

    #[test]
    fn body_without_paragraphs_is_not_claimed_as_full_text() {
        let record = parse_europe_pmc_article(HEADINGS_ONLY_XML, "PMC1234567").unwrap();
        assert_eq!(
            record.availability,
            PeriodicalArticleAvailability::MetadataOnly,
            "只有小节标题的 body 不构成可读全文"
        );
        assert!(!record.is_readable());
        assert_eq!(record.mime_type, None, "元数据条目不声明正文 MIME");
    }

    #[test]
    fn out_of_range_jats_dates_are_dropped_instead_of_fabricated() {
        let xml = r#"<article>
  <front>
    <journal-meta>
      <journal-title-group><journal-title>Nature Communications</journal-title></journal-title-group>
      <issn pub-type="ppub">2041-1723</issn>
    </journal-meta>
    <article-meta>
      <article-id pub-id-type="pmc">PMC1234567</article-id>
      <title-group><article-title>Bad dates</article-title></title-group>
      <pub-date pub-type="ppub"><year>2024</year><month>13</month><day>45</day></pub-date>
      <volume>15</volume>
      <issue>1</issue>
    </article-meta>
  </front>
</article>"#;
        let record = parse_europe_pmc_article(xml, "PMC1234567").unwrap();
        assert_eq!(
            record
                .issue
                .as_ref()
                .and_then(|issue| issue.publication_date.as_deref()),
            Some("2024"),
            "越界的月/日必须被丢弃，而不是格式化成 2024-13-45"
        );

        let leap = r#"<article><front>
            <journal-meta><journal-title-group><journal-title>T</journal-title></journal-title-group></journal-meta>
            <article-meta>
              <article-id pub-id-type="pmc">PMC1</article-id>
              <pub-date pub-type="ppub"><year>2023</year><month>2</month><day>29</day></pub-date>
              <issue>1</issue>
            </article-meta>
          </front></article>"#;
        let record = parse_europe_pmc_article(leap, "PMC1").unwrap();
        assert_eq!(
            record
                .issue
                .as_ref()
                .and_then(|issue| issue.publication_date.as_deref()),
            Some("2023-02"),
            "平年 2 月 29 日不是日期，只能保留到月"
        );
    }

    #[test]
    fn metadata_search_projection_stays_metadata_only_and_matches_the_pmcid() {
        let value: Value = serde_json::from_str(METADATA_SEARCH_JSON).unwrap();
        let record = parse_europe_pmc_metadata(&value, "PMC7654321").unwrap();
        assert_eq!(record.remote_article_id, "PMC7654321");
        assert_eq!(
            record.availability,
            PeriodicalArticleAvailability::MetadataOnly
        );
        assert!(!record.is_readable(), "元数据回退不得宣称全文可读");
        assert_eq!(record.mime_type, None);

        let journal = record.journal.as_ref().expect("journal identity");
        assert_eq!(journal.title, "Journal of Examples");
        assert_eq!(
            journal.issn_print.as_ref().map(Issn::as_str),
            Some("0378-5955")
        );
        assert_eq!(
            journal.issn_electronic.as_ref().map(Issn::as_str),
            Some("1420-682X")
        );
        assert_eq!(
            journal.preferred_issn().map(Issn::as_str),
            Some("1420-682X")
        );
        assert_eq!(record.volume.as_ref().and_then(|v| v.number), Some(7.0));
        assert_eq!(record.volume.as_ref().and_then(|v| v.year), Some(2019));
        let issue = record.issue.as_ref().expect("issue");
        assert_eq!(issue.label.as_deref(), Some("Suppl 2"));
        assert_eq!(issue.number, None);
        assert_eq!(issue.publication_date.as_deref(), Some("2019-05-01"));
        assert_eq!(
            record
                .page_range
                .as_ref()
                .map(|range| (range.start.as_str(), range.end.as_deref())),
            Some(("120", Some("128")))
        );
        assert_eq!(record.ordinal, None, "pageInfo 页码不能被猜成文章序号");
        assert_eq!(
            record.doi.as_ref().map(Doi::as_str),
            Some("10.1038/s41467-019-00001-2")
        );
    }

    #[test]
    fn metadata_search_accepts_core_field_casing_and_flattened_fallbacks() {
        let value: Value = serde_json::from_str(
            r#"{
              "resultList": {"result": [{
                "pmcid": "PMC7654321",
                "title": "Core result",
                "journalTitle": "Fallback Journal",
                "journalIssn": "0378-5955",
                "journalEissn": "1420-682X",
                "journalVolume": "8",
                "pubYear": "2020",
                "firstPublicationDate": "2020-02-29",
                "pageInfo": "e42"
              }]}
            }"#,
        )
        .unwrap();
        let record = parse_europe_pmc_metadata(&value, "PMC7654321").unwrap();
        let journal = record.journal.as_ref().expect("flattened journal identity");
        assert_eq!(journal.title, "Fallback Journal");
        assert_eq!(
            journal.issn_print.as_ref().map(Issn::as_str),
            Some("0378-5955")
        );
        assert_eq!(
            journal.issn_electronic.as_ref().map(Issn::as_str),
            Some("1420-682X")
        );
        assert_eq!(
            record.volume.as_ref().and_then(|volume| volume.number),
            Some(8.0)
        );
        assert_eq!(
            record.volume.as_ref().and_then(|volume| volume.year),
            Some(2020)
        );
        assert_eq!(
            record
                .issue
                .as_ref()
                .and_then(|issue| issue.publication_date.as_deref()),
            None,
            "没有期号时不应凭日期伪造期号"
        );
        assert_eq!(record.ordinal, Some(42));
    }

    #[test]
    fn metadata_search_rejects_missing_or_mismatched_pmcid() {
        let value: Value = serde_json::from_str(METADATA_SEARCH_JSON).unwrap();
        for pmcid in ["PMC9999999", "PMC1"] {
            let error = parse_europe_pmc_metadata(&value, pmcid).unwrap_err();
            assert_eq!(
                error.code().as_str(),
                "SOURCE_UNAVAILABLE",
                "{pmcid} 不得命中未声明该 PMCID 的记录"
            );
        }
        let empty: Value = serde_json::from_str(r#"{"resultList":{"result":[]}}"#).unwrap();
        assert!(parse_europe_pmc_metadata(&empty, "PMC7654321").is_err());
        let malformed: Value = serde_json::from_str(r#"{"hitCount":0}"#).unwrap();
        assert!(parse_europe_pmc_metadata(&malformed, "PMC7654321").is_err());
    }

    #[test]
    fn malformed_full_text_xml_is_rejected_instead_of_being_treated_as_metadata() {
        let error = parse_europe_pmc_article(
            "<article><front><article-meta><article-id pub-id-type=\"pmc\">PMC1</article-id>",
            "PMC1",
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "SOURCE_UNAVAILABLE");
    }

    #[test]
    fn metadata_publication_dates_out_of_range_fall_back_to_their_precision() {
        let json = r#"{"resultList":{"result":[
            {"pmcid":"PMC1","journalInfo":{"issue":"1","dateOfPublication":"2019-13-40"}}
        ]}}"#;
        let value: Value = serde_json::from_str(json).unwrap();
        let record = parse_europe_pmc_metadata(&value, "PMC1").unwrap();
        assert_eq!(
            record
                .issue
                .as_ref()
                .and_then(|issue| issue.publication_date.as_deref()),
            Some("2019")
        );
    }

    #[test]
    fn record_never_carries_urls_or_request_details() {
        let record = parse_europe_pmc_article(FULL_TEXT_XML, "PMC1234567").unwrap();
        let mut flattened = format!(
            "{} {} {} {} {}",
            record.title,
            record
                .journal
                .as_ref()
                .map(|journal| journal.title.clone())
                .unwrap_or_default(),
            record
                .journal
                .as_ref()
                .and_then(|journal| journal.publisher.clone())
                .unwrap_or_default(),
            record.doi.as_ref().map(Doi::as_str).unwrap_or_default(),
            record.remote_article_id,
        );
        for value in [
            record.volume.as_ref().and_then(|v| v.label.clone()),
            record.issue.as_ref().and_then(|i| i.label.clone()),
            record
                .issue
                .as_ref()
                .and_then(|i| i.publication_date.clone()),
        ]
        .into_iter()
        .flatten()
        {
            flattened.push(' ');
            flattened.push_str(&value);
        }
        assert!(
            !flattened.contains("http") && !flattened.contains("://"),
            "记录不得携带 URL 或请求细节: {flattened}"
        );
    }

    #[test]
    fn missing_journal_title_does_not_establish_a_journal_identity() {
        let xml = r#"<article><front>
            <journal-meta><issn pub-type="ppub">2041-1723</issn></journal-meta>
            <article-meta>
              <article-id pub-id-type="pmc">PMC42</article-id>
              <title-group><article-title>Untitled journal</article-title></title-group>
            </article-meta>
          </front></article>"#;
        let record = parse_europe_pmc_article(xml, "PMC42").unwrap();
        assert!(record.journal.is_none(), "没有期刊名就不建立期刊身份");
        assert_eq!(record.title, "Untitled journal");
    }

    #[tokio::test]
    async fn provider_rejects_other_sources_and_invalid_pmcid_without_network() {
        let provider = EuropePmcPeriodicalProvider::new(Arc::new(
            OnlineContentClient::new().expect("fixed host client"),
        ));
        let wrong_source = provider
            .article("mangadex", "PMC1234567")
            .await
            .unwrap_err();
        assert_eq!(wrong_source.code().as_str(), "INVALID_ARGUMENT");
        let invalid_id = provider
            .article(EUROPE_PMC_SOURCE_KEY, "1234567")
            .await
            .unwrap_err();
        assert_eq!(invalid_id.code().as_str(), "INVALID_ARGUMENT");
        let url_shaped = provider
            .article(
                EUROPE_PMC_SOURCE_KEY,
                "https://www.ebi.ac.uk/europepmc/webservices/rest/PMC1/fullTextXML",
            )
            .await
            .unwrap_err();
        assert_eq!(url_shaped.code().as_str(), "INVALID_ARGUMENT");
    }

    #[test]
    fn record_identity_validation_matches_request() {
        let record = parse_europe_pmc_article(FULL_TEXT_XML, "PMC1234567").unwrap();
        record
            .validate_identity(EUROPE_PMC_SOURCE_KEY, "PMC1234567")
            .unwrap();
        assert!(
            record
                .validate_identity(EUROPE_PMC_SOURCE_KEY, "PMC9")
                .is_err()
        );
    }
}
