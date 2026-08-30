//! CMS10（苹果CMS V10 JSON API）适配器（V2-B 实战批次）。
//!
//! 安全边界（SECURITY_PRIVACY_COMPLIANCE S8：防 SSRF / 出站最小化）：
//! - 仅访问用户显式配置的单个端点；搜索与详情都是同一端点上的查询参数变体
//!   （`ac=videolist&wd=` / `ac=videolist&ids=`），不跟随任何响应体中的跳转地址。
//! - 响应体上限 8 MiB；超时 15s；UA 标识应用版本。
//! - 日志只允许 host，不允许完整 URL/查询串/播放地址。
//!
//! 协议事实（SOURCES_INVENTORY §22 实测）：`ac=videolist` 返回
//！ `{ code, list: [{ vod_id, vod_name, type_name, vod_year, vod_pic, vod_play_url }] }`；
//！ `vod_play_url` 形如 `第01集$https://...m3u8#第02集$https://...`（多播放源以 `$$$` 分隔，取第一组）。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use haven_application::services::CMS10_CANDIDATE_PREFIX;
use haven_application::services::SourceRegistryService;
use haven_application::services::search_source::SearchSourceParticipant;
use haven_application::wire::WorkCardDto;
use haven_common::{AppError, ErrorKind};
use serde::Deserialize;

pub const CMS10_SOURCE_ID: &str = "cms10";

/// 单条采集条目（已解析的播放组 + 详情）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cms10Entry {
    pub vod_id: String,
    pub name: String,
    pub year: Option<i32>,
    pub type_name: Option<String>,
    /// 海报地址（vod_pic；只接受 http(s)，其余丢弃）。
    pub pic: Option<String>,
    /// 首播放组内的集数（标签 + 播放地址）。
    pub play_urls: Vec<(String, String)>,
    /// 详情：简介（vod_content，已去 HTML/截断 2000）。
    pub content: Option<String>,
    /// 导演（vod_director，已清洗 / 分隔）。
    pub director: Option<String>,
    /// 主演（vod_actor，已清洗 / 分隔）。
    pub actor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Cms10Response {
    #[serde(default)]
    list: Vec<Cms10RawItem>,
}

#[derive(Debug, Deserialize)]
struct Cms10RawItem {
    #[serde(default)]
    vod_id: serde_json::Value,
    #[serde(default)]
    vod_name: String,
    #[serde(default)]
    type_name: Option<String>,
    #[serde(default)]
    vod_year: Option<String>,
    #[serde(default)]
    vod_play_url: Option<String>,
    #[serde(default)]
    vod_pic: Option<String>,
    #[serde(default)]
    vod_content: Option<String>,
    #[serde(default)]
    vod_director: Option<String>,
    #[serde(default)]
    vod_actor: Option<String>,
}

const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

impl TryFrom<Cms10RawItem> for Cms10Entry {
    type Error = AppError;

    fn try_from(item: Cms10RawItem) -> Result<Self, Self::Error> {
        let vod_id = match item.vod_id {
            serde_json::Value::Number(number) => number.to_string(),
            serde_json::Value::String(text) => text.trim().to_owned(),
            _ => return Err(source_unavailable("采集站返回缺少 vod_id")),
        };
        if vod_id.is_empty() {
            return Err(source_unavailable("采集站返回缺少 vod_id"));
        }
        let name = item.vod_name.trim().to_owned();
        if name.is_empty() {
            return Err(source_unavailable("采集站返回缺少标题"));
        }
        let year = item
            .vod_year
            .as_deref()
            .and_then(|value| value.trim().parse::<i32>().ok());
        let play_urls = parse_play_urls(item.vod_play_url.as_deref());
        let pic = item
            .vod_pic
            .as_deref()
            .map(str::trim)
            .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
            .map(str::to_owned);
        let content = clean_content(item.vod_content.as_deref());
        let director = clean_person(item.vod_director.as_deref());
        let actor = clean_person(item.vod_actor.as_deref());
        Ok(Self {
            vod_id,
            name,
            year,
            type_name: item.type_name.filter(|value| !value.trim().is_empty()),
            pic,
            play_urls,
            content,
            director,
            actor,
        })
    }
}

fn normalize_label(label: &str) -> String {
    let trimmed = label.trim();
    // 去除常见前缀/后缀噪音，保留核心集数标识
    // 例如 "第0001集" → "第1集" 的数字归一在 parse_episode_number 中处理，标签保持原样但去空格
    trimmed.to_owned()
}

fn is_special_episode(label: &str) -> bool {
    let lower = label.to_lowercase();
    lower.contains("特别篇")
        || lower.contains("特别")
        || lower.contains("sp")
        || lower.contains("special")
        || lower.contains("pv")
        || lower.contains("预告")
}

/// 解析播放组：`第01集$https://a#第02集$https://b$$$备用$...` → [(标签, 地址)]，多组择优（取集数最多且含 http 的组）。
fn parse_play_urls(raw: Option<&str>) -> Vec<(String, String)> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let mut best: Vec<(String, String)> = Vec::new();
    for group in raw.split("$$$") {
        let parsed: Vec<(String, String)> = group
            .split('#')
            .filter_map(|entry| {
                let (label, url) = entry.split_once('$')?;
                let url = url.trim().to_owned();
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return None;
                }
                let label = normalize_label(label);
                if label.is_empty() {
                    return None;
                }
                Some((label, url))
            })
            .collect();
        // 过滤后若含有效集数，取最长的组为最佳（通常主线路集数最多）
        if parsed.len() > best.len() {
            best = parsed;
        }
    }
    // 过滤特别篇：若组内含大量常规集 + 少量特别篇，去除特别篇以避免错位
    if best.iter().any(|(l, _)| !is_special_episode(l)) {
        let filtered: Vec<(String, String)> = best
            .iter()
            .filter(|(l, _)| !is_special_episode(l))
            .cloned()
            .collect();
        if !filtered.is_empty() {
            return filtered;
        }
    }
    best
}

fn clean_content(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    let mut s = raw.to_string();
    while let Some(start) = s.find('<') {
        if let Some(end) = s[start..].find('>') {
            s.replace_range(start..start + end + 1, " ");
        } else {
            break;
        }
    }
    s = s
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    let cleaned = s
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned();
    if cleaned.is_empty() {
        return None;
    }
    let truncated = if cleaned.chars().count() > 2000 {
        let mut t: String = cleaned.chars().take(2000).collect();
        t.push_str("...");
        t
    } else {
        cleaned
    };
    Some(truncated)
}

fn clean_person(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    let s = raw.replace(['，', '、', '/', ' '], ",");
    let parts: Vec<String> = s
        .split(',')
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
        .map(|x| x.to_owned())
        .collect();
    if parts.is_empty() {
        return None;
    }
    Some(parts.join(" / "))
}

fn source_unavailable(message: &'static str) -> AppError {
    AppError::new("SOURCE_UNAVAILABLE", ErrorKind::Network, message, true)
}

/// CMS10 HTTP 客户端。
pub struct Cms10Client {
    http: reqwest::Client,
}

impl Cms10Client {
    pub fn new() -> Result<Self, AppError> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("Haven/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| {
                AppError::new(
                    "INTERNAL_ERROR",
                    ErrorKind::Internal,
                    "HTTP 客户端初始化失败",
                    false,
                )
                .with_source(e)
            })?;
        Ok(Self { http })
    }

    async fn fetch_entries(
        &self,
        endpoint: &str,
        query: &str,
    ) -> Result<Vec<Cms10Entry>, AppError> {
        // 查询串必须是 ? 参数（MacCMS 路由按 query 分发 ac）；拼进路径会被站点忽略，
        // 导致搜索不过滤、详情缺 vod_play_url（实战验收发现的回归）。
        let url = format!("{}?ac=videolist&{}", endpoint.trim_end_matches('/'), query);
        let response = self.http.get(&url).send().await.map_err(map_network_err)?;
        if !response.status().is_success() {
            return Err(source_unavailable("采集站返回非成功状态"));
        }
        let bytes = response.bytes().await.map_err(map_network_err)?;
        if bytes.len() > MAX_BODY_BYTES {
            return Err(source_unavailable("采集站响应超出大小上限"));
        }
        let parsed: Cms10Response = serde_json::from_slice(&bytes)
            .map_err(|_| source_unavailable("采集站响应不是有效 CMS10 JSON"))?;
        parsed.list.into_iter().map(Cms10Entry::try_from).collect()
    }

    /// 关键词搜索（按 limit 截断）。
    pub async fn search(
        &self,
        endpoint: &str,
        query: &str,
        limit: u32,
    ) -> Result<Vec<Cms10Entry>, AppError> {
        let encoded = urlencode(query);
        let mut entries = self
            .fetch_entries(endpoint, &format!("wd={encoded}"))
            .await?;
        entries.truncate(limit as usize);
        Ok(entries)
    }

    /// 按 vod_id 取详情（含播放组）。
    pub async fn detail(&self, endpoint: &str, vod_id: &str) -> Result<Cms10Entry, AppError> {
        let mut entries = self
            .fetch_entries(endpoint, &format!("ids={}", urlencode(vod_id)))
            .await?;
        entries
            .pop()
            .ok_or_else(|| source_unavailable("采集站无此条目"))
    }
}

fn map_network_err(err: reqwest::Error) -> AppError {
    // 只保留错误分类，不携带完整 URL（防日志泄漏端点查询串）。
    let kind = if err.is_timeout() {
        "采集站请求超时"
    } else if err.is_connect() {
        "采集站连接失败"
    } else {
        "采集站请求失败"
    };
    source_unavailable(kind)
}

/// 极简百分号编码（查询值足够用；空格 → %20）。
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// CMS10 目录适配器：把 Cms10Client 适配为 application 的 SourceCatalogProvider。
pub struct Cms10CatalogProvider {
    client: Arc<Cms10Client>,
}

impl Cms10CatalogProvider {
    pub fn new(client: Arc<Cms10Client>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl haven_application::services::SourceCatalogProvider for Cms10CatalogProvider {
    async fn detail(
        &self,
        source_id: &str,
        endpoint: &str,
        external_id: &str,
    ) -> Result<haven_application::services::SourceCatalogEntry, AppError> {
        if source_id != CMS10_SOURCE_ID {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "未知来源目录",
                false,
            ));
        }
        let entry = self.client.detail(endpoint, external_id).await?;
        Ok(haven_application::services::SourceCatalogEntry {
            external_id: entry.vod_id,
            title: entry.name,
            year: entry.year,
            type_name: entry.type_name,
            pic: entry.pic,
            episodes: entry.play_urls,
            content: entry.content,
            director: entry.director,
            actor: entry.actor,
            local_file: None,
        })
    }

    async fn search(
        &self,
        source_id: &str,
        endpoint: &str,
        query: &str,
        limit: u32,
    ) -> Result<Vec<haven_application::services::SourceCatalogEntry>, AppError> {
        if source_id != CMS10_SOURCE_ID {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "未知来源目录",
                false,
            ));
        }
        let entries = self.client.search(endpoint, query, limit).await?;
        Ok(entries
            .into_iter()
            .map(|entry| haven_application::services::SourceCatalogEntry {
                external_id: entry.vod_id,
                title: entry.name,
                year: entry.year,
                type_name: entry.type_name,
                pic: entry.pic,
                episodes: entry.play_urls,
                content: entry.content,
                director: entry.director,
                actor: entry.actor,
                local_file: None,
            })
            .collect())
    }
}

/// CMS10 搜索参与者：注册表启用 + 端点已配置时参与渐进式搜索；
/// 未配置端点视为组合缺口，诚实返回空集（契约 §36.3）。
pub struct Cms10SearchParticipant {
    registry: SourceRegistryService,
    client: Arc<Cms10Client>,
}

impl Cms10SearchParticipant {
    pub fn new(registry: SourceRegistryService, client: Arc<Cms10Client>) -> Self {
        Self { registry, client }
    }
}

#[async_trait]
impl SearchSourceParticipant for Cms10SearchParticipant {
    fn source_id(&self) -> &str {
        CMS10_SOURCE_ID
    }

    fn supports_category(&self, category: Option<haven_application::wire::QueryCategory>) -> bool {
        matches!(
            category,
            None | Some(haven_application::wire::QueryCategory::All)
                | Some(haven_application::wire::QueryCategory::Video)
        )
    }

    async fn search(
        &self,
        query: &str,
        limit: u32,
        is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<Vec<WorkCardDto>, AppError> {
        let Some(endpoint) = self.registry.endpoint(CMS10_SOURCE_ID).await? else {
            return Ok(Vec::new());
        };
        if is_cancelled() {
            return Ok(Vec::new());
        }
        let entries = self.client.search(&endpoint, query, limit).await?;
        Ok(entries
            .into_iter()
            .map(|entry| WorkCardDto {
                // 候选句柄：非 UUID 前缀形状明确标识"未入库候选"。
                work_id: format!("{CMS10_CANDIDATE_PREFIX}{}", entry.vod_id),
                title: entry.name,
                original_title: None,
                description: None,
                categories: vec![haven_application::wire::ContentCategory::Video],
                available_media_types: vec![haven_application::wire::MediaTypeDto::Series],
                poster_uri: None,
                backdrop_uri: None,
                release_year: entry.year,
                rating_value: None,
                rating_scale: None,
                favorite: false,
                progress: None,
                primary_action: None,
                external_ids: Vec::new(),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_first_play_group_and_filters_non_http() {
        let urls = parse_play_urls(Some(
            "第01集$https://cdn.example.com/1/index.m3u8#第02集$https://cdn.example.com/2/index.m3u8$$$备用$http://x/y",
        ));
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0].0, "第01集");
        assert!(urls[0].1.ends_with("/1/index.m3u8"));

        assert!(parse_play_urls(Some("正片$magnet:?xt=1")).is_empty());
        assert!(parse_play_urls(None).is_empty());
    }

    #[test]
    fn entry_requires_id_and_name() {
        let raw: Cms10RawItem =
            serde_json::from_str(r#"{"vod_id": 42, "vod_name": "测试", "vod_year": "2024"}"#)
                .unwrap();
        let entry = Cms10Entry::try_from(raw).unwrap();
        assert_eq!(entry.vod_id, "42");
        assert_eq!(entry.year, Some(2024));

        let no_name: Cms10RawItem = serde_json::from_str(r#"{"vod_id": 1}"#).unwrap();
        assert_eq!(
            Cms10Entry::try_from(no_name).unwrap_err().code().as_str(),
            "SOURCE_UNAVAILABLE"
        );
    }

    #[test]
    fn urlencode_matches_query_expectations() {
        assert_eq!(urlencode("庆余年"), "%E5%BA%86%E4%BD%99%E5%B9%B4");
        assert_eq!(urlencode("abc-1"), "abc-1");
    }
}
