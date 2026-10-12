//! CMS10（苹果CMS V10 JSON API）适配器（V2-B 实战批次）。
//!
//! 安全边界（SECURITY_PRIVACY_COMPLIANCE S8：防 SSRF / 出站最小化）：
//! - 仅访问用户显式配置的单个端点；搜索与详情都是同一端点上的查询参数变体
//!   （`ac=videolist&wd=` / `ac=videolist&ids=`），不跟随任何响应体中的跳转地址。
//! - 端点是**用户提供**的：保存（`source_registry`）与逐跳出站都使用
//!   `HttpUrlPolicy::UserProvidedSource`，支持用户明确配置的本机/局域网地址和任意非零端口；
//!   链路本地/元数据/保留地址、userinfo 与 fragment 仍然逐条 fail closed。
//! - 响应体上限 8 MiB；超时 15s；UA 标识应用版本。
//! - 日志只允许 host，不允许完整 URL/查询串/播放地址。
//! - 搜索卡片里的海报只经 `ImageProxyRepository` 登记并投影成
//!   `haven://artwork/<opaque>`；不在搜索阶段逐条抓图。
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
use haven_common::network::{HttpUrlPolicy, parse_http_url};
use haven_common::{AppError, ErrorKind};
use haven_domain::contracts::ImageProxyRepository;
use serde::Deserialize;

use crate::http_security::{ResolvedHttpTarget, pin_client_builder, resolve_public_http_target};

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
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_REDIRECTS: usize = 3;

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
#[derive(Clone, Copy, Debug, Default)]
pub struct Cms10Client;

impl Cms10Client {
    pub fn new() -> Result<Self, AppError> {
        Ok(Self)
    }

    /// 解析一个采集站地址。默认走**用户提供来源**策略：用户可选本机/局域网主机和
    /// 任意非零端口；链路本地/保留地址、userinfo 与 fragment 仍然 fail closed。
    fn parse_endpoint(&self, raw: &str) -> Result<reqwest::Url, AppError> {
        parse_http_url(raw, HttpUrlPolicy::UserProvidedSource)
            .map_err(|_| endpoint_invalid())
            .map(|safe| safe.into_url())
    }

    /// 出站目标：逐次解析并把结果钉进客户端；用户配置的回环/局域网地址会按来源策略
    /// 原样连接，不经过系统代理。
    async fn resolve_target(&self, url: &reqwest::Url) -> Result<ResolvedHttpTarget, AppError> {
        resolve_public_http_target(url.as_str(), HttpUrlPolicy::UserProvidedSource)
            .await
            .map_err(|_| source_unavailable("采集站地址解析不安全"))
    }

    /// 查询参数始终通过 URL API 写入，避免 endpoint 中已有 query 时生成第二个 `?`，
    /// 也避免把用户输入拼进路径或解释成额外参数。
    fn endpoint_url(
        &self,
        endpoint: &str,
        key: &str,
        value: &str,
    ) -> Result<reqwest::Url, AppError> {
        if !matches!(key, "wd" | "ids") {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "CMS10 查询参数非法",
                false,
            ));
        }
        let mut url = self.parse_endpoint(endpoint)?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("ac", "videolist");
            query.append_pair(key, value);
        }
        Ok(url)
    }

    async fn fetch_limited(
        &self,
        method: reqwest::Method,
        url: &str,
        payload: Option<&serde_json::Value>,
    ) -> Result<Vec<Cms10Entry>, AppError> {
        // CMS10 端点由用户提供：保存与出站都使用同一用户来源策略，支持自建服务的
        // 本机/局域网地址与非零端口。链路本地、云元数据和保留目标仍会被拒绝。
        let mut current = self.parse_endpoint(url)?;
        for _ in 0..=MAX_REDIRECTS {
            let target = self.resolve_target(&current).await?;
            let builder = reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .http1_only()
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(concat!("Haven/", env!("CARGO_PKG_VERSION")));
            let client = pin_client_builder(builder, &target)
                .build()
                .map_err(|_| source_unavailable("采集站客户端初始化失败"))?;
            let mut request = client.request(method.clone(), target.url.clone());
            if let Some(payload) = payload {
                request = request.json(payload);
            }
            let response = request.send().await.map_err(map_network_err)?;
            if response.status().is_redirection() {
                // The CMS10 path is GET-only. Refusing to replay another method
                // avoids silently changing its semantics at a redirect target.
                if method != reqwest::Method::GET {
                    return Err(source_unavailable("采集站请求重定向不受支持"));
                }
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| source_unavailable("采集站重定向地址无效"))?;
                current = target
                    .url
                    .join(location)
                    .map_err(|_| source_unavailable("采集站重定向地址无效"))?;
                continue;
            }
            if !response.status().is_success() {
                return Err(source_unavailable("采集站返回非成功状态"));
            }
            if response
                .content_length()
                .is_some_and(|length| length > MAX_BODY_BYTES as u64)
            {
                return Err(source_unavailable("采集站响应超出大小上限"));
            }
            let mut bytes = Vec::with_capacity(64 * 1024);
            let mut response = response;
            while let Some(chunk) = response.chunk().await.map_err(map_network_err)? {
                if bytes.len().saturating_add(chunk.len()) > MAX_BODY_BYTES {
                    return Err(source_unavailable("采集站响应超出大小上限"));
                }
                bytes.extend_from_slice(chunk.as_ref());
            }
            let parsed: Cms10Response = serde_json::from_slice(&bytes)
                .map_err(|_| source_unavailable("采集站响应不是有效 CMS10 JSON"))?;
            return parsed.list.into_iter().map(Cms10Entry::try_from).collect();
        }
        Err(source_unavailable("采集站重定向次数过多"))
    }

    /// 关键词搜索（按 limit 截断）。
    pub async fn search(
        &self,
        endpoint: &str,
        query: &str,
        limit: u32,
    ) -> Result<Vec<Cms10Entry>, AppError> {
        let url = self.endpoint_url(endpoint, "wd", query)?;
        let mut entries = self
            .fetch_limited(reqwest::Method::GET, url.as_str(), None)
            .await?;
        entries.truncate(limit as usize);
        Ok(entries)
    }

    /// 按 vod_id 取详情（含播放组）。
    pub async fn detail(&self, endpoint: &str, vod_id: &str) -> Result<Cms10Entry, AppError> {
        let url = self.endpoint_url(endpoint, "ids", vod_id)?;
        let mut entries = self
            .fetch_limited(reqwest::Method::GET, url.as_str(), None)
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

/// 非法的采集站端点/查询参数（与保存路径同一口径）。
fn endpoint_invalid() -> AppError {
    AppError::new(
        "INVALID_ARGUMENT",
        ErrorKind::Validation,
        "采集站端点非法",
        false,
    )
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
            media_type: None,
            remote: None,
            comic_catalog: None,
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
                media_type: None,
                remote: None,
                comic_catalog: None,
            })
            .collect())
    }
}

/// CMS10 搜索参与者：注册表启用 + 端点已配置时参与渐进式搜索；
/// 未配置端点视为组合缺口，诚实返回空集（契约 §36.3）。
pub struct Cms10SearchParticipant {
    registry: SourceRegistryService,
    client: Arc<Cms10Client>,
    /// 受控图片登记端口。未注入时搜索照常工作，只是没有海报。
    image_proxy: Option<Arc<dyn ImageProxyRepository>>,
}

impl Cms10SearchParticipant {
    pub fn new(registry: SourceRegistryService, client: Arc<Cms10Client>) -> Self {
        Self {
            registry,
            client,
            image_proxy: None,
        }
    }

    /// 注入受控 Artwork 登记端口（`image_proxy` migration 015 的 ImageProxyRepository）。
    pub fn with_image_proxy(mut self, proxy: Arc<dyn ImageProxyRepository>) -> Self {
        self.image_proxy = Some(proxy);
        self
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
        let mut cards: Vec<WorkCardDto> = Vec::with_capacity(entries.len());
        for entry in entries {
            // 海报地址来自采集站响应本身；登记交给受控 ImageProxyRepository，卡片只拿
            // opaque `haven://artwork/<id>`。登记失败/被拒/缺失/取消时卡片保持原样。
            let poster_uri = crate::opds::projected_poster(
                self.image_proxy.as_deref(),
                CMS10_SOURCE_ID,
                entry.pic.as_deref(),
                is_cancelled,
            )
            .await;
            cards.push(cms10_card(entry, poster_uri));
        }
        Ok(cards)
    }
}

/// 单条 CMS10 条目 → 搜索卡片：候选句柄是"未入库候选"形状，海报只接收已经登记过的
/// 受控 URI（`None` = 没有海报）。
fn cms10_card(entry: Cms10Entry, poster_uri: Option<String>) -> WorkCardDto {
    WorkCardDto {
        // 候选句柄：非 UUID 前缀形状明确标识"未入库候选"。
        work_id: format!("{CMS10_CANDIDATE_PREFIX}{}", entry.vod_id),
        title: entry.name,
        original_title: None,
        description: None,
        categories: vec![haven_application::wire::ContentCategory::Video],
        available_media_types: vec![haven_application::wire::MediaTypeDto::Series],
        poster_uri,
        backdrop_uri: None,
        release_year: entry.year,
        rating_value: None,
        rating_scale: None,
        favorite: false,
        progress: None,
        primary_action: None,
        external_ids: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opds::fixture::{
        ArtworkOutcome, FixtureResponse, FixtureServer, RecordingImageProxy, seeded_cms10_registry,
    };

    /// 采集条目里的 `vod_pic` 必须成为受控海报：登记使用固定的 CMS10 来源身份，卡片只拿
    /// opaque `haven://artwork/<id>`；被拒 / 缺失 / 取消一律保持无海报并保留元数据。
    #[tokio::test]
    async fn cms10_search_card_projects_only_registered_controlled_artwork() {
        let entry = || {
            Cms10Entry::try_from(
                serde_json::from_str::<Cms10RawItem>(
                    r#"{"vod_id": 42, "vod_name": "庆余年", "vod_year": "2024", "vod_pic": "https://img.picbf.com/42.jpg"}"#,
                )
                .unwrap(),
            )
            .unwrap()
        };
        let pic = "https://img.picbf.com/42.jpg";
        let not_cancelled = || false;

        let proxy = RecordingImageProxy::new(ArtworkOutcome::Registered);
        let poster = crate::opds::projected_poster(
            Some(proxy.as_ref()),
            CMS10_SOURCE_ID,
            entry().pic.as_deref(),
            &not_cancelled,
        )
        .await;
        assert_eq!(poster.as_deref(), Some("haven://artwork/artwork-1"));
        assert_eq!(
            proxy.calls(),
            vec![(CMS10_SOURCE_ID.to_owned(), pic.to_owned())],
            "登记必须使用固定的 CMS10 来源身份与上游解析出来的地址"
        );

        let card = cms10_card(entry(), poster);
        assert_eq!(
            card.poster_uri.as_deref(),
            Some("haven://artwork/artwork-1")
        );
        assert_eq!(card.title, "庆余年");
        assert_eq!(card.work_id, format!("{CMS10_CANDIDATE_PREFIX}42"));
        let wire = serde_json::to_string(&card).unwrap();
        assert!(wire.contains("haven://artwork/artwork-1"));
        assert!(
            !wire.contains("img.picbf.com"),
            "远端图片地址不得进入 Wire: {wire}"
        );

        // 被策略拒绝 / id 形状非法：元数据保留，海报为 None。
        for outcome in [
            ArtworkOutcome::Rejected,
            ArtworkOutcome::RegisteredInvalidId,
        ] {
            let rejected = RecordingImageProxy::new(outcome);
            let poster = crate::opds::projected_poster(
                Some(rejected.as_ref()),
                CMS10_SOURCE_ID,
                entry().pic.as_deref(),
                &not_cancelled,
            )
            .await;
            assert_eq!(poster, None);
            assert_eq!(
                rejected.calls(),
                vec![(CMS10_SOURCE_ID.to_owned(), pic.to_owned())],
                "带图片的条目必须经过登记，才能按拒绝结果回退"
            );
            let card = cms10_card(entry(), poster);
            assert_eq!(card.poster_uri, None);
            assert_eq!(card.title, "庆余年");
        }

        // 没有 pic 字段的条目 / 取消之后的条目：根本不进入登记路径。
        let rejected = RecordingImageProxy::new(ArtworkOutcome::Rejected);
        let no_pic: Cms10RawItem =
            serde_json::from_str(r#"{"vod_id": 43, "vod_name": "无封面"}"#).unwrap();
        let no_pic = Cms10Entry::try_from(no_pic).unwrap();
        assert_eq!(
            crate::opds::projected_poster(
                Some(rejected.as_ref()),
                CMS10_SOURCE_ID,
                no_pic.pic.as_deref(),
                &not_cancelled,
            )
            .await,
            None
        );
        let cancelled = || true;
        assert_eq!(
            crate::opds::projected_poster(
                Some(rejected.as_ref()),
                CMS10_SOURCE_ID,
                Some(pic),
                &cancelled
            )
            .await,
            None
        );
        assert!(rejected.calls().is_empty(), "缺失/取消的海报不得登记");
    }

    /// 真实链路：`Cms10SearchParticipant::search` → 用户配置的本机采集站 JSON 响应 → 解析出的
    /// `vod_pic` → 受控图片登记 → 卡片。这里使用真实的用户来源 URL/DNS 策略，不借助回环夹具缝隙。
    #[tokio::test]
    async fn cms10_search_registers_catalog_artwork_through_the_real_search_path() {
        let server = FixtureServer::start(|origin| {
            vec![(
                "/api.php".to_owned(),
                FixtureResponse::json(format!(
                    r#"{{"code":1,"list":[{{"vod_id":42,"vod_name":"庆余年","vod_year":"2024","vod_pic":"{origin}/img/42.jpg","vod_play_url":"第01集${origin}/1.m3u8"}}]}}"#
                )),
            )]
        });
        let endpoint = format!("{}/api.php", server.origin());
        // 回环地址属于用户明确配置的本机采集站；设置行由 fixture helper 初始化。
        let registry = seeded_cms10_registry(&endpoint).await;
        let proxy = RecordingImageProxy::new(ArtworkOutcome::Registered);
        let client = Arc::new(Cms10Client::new().unwrap());
        let participant =
            Cms10SearchParticipant::new(registry, client).with_image_proxy(proxy.clone());

        let not_cancelled = || false;
        let cards = participant
            .search("庆余年", 10, &not_cancelled)
            .await
            .expect("夹具采集站必须可搜索");
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].title, "庆余年");
        assert_eq!(cards[0].work_id, format!("{CMS10_CANDIDATE_PREFIX}42"));
        assert_eq!(
            cards[0].poster_uri.as_deref(),
            Some("haven://artwork/artwork-1")
        );
        assert_eq!(
            proxy.calls(),
            vec![(
                CMS10_SOURCE_ID.to_owned(),
                format!("{}/img/42.jpg", server.origin())
            )],
            "登记使用固定的 CMS10 来源身份与响应里的原始地址"
        );

        // 只有一次采集请求，且没有为海报额外抓图。
        assert_eq!(server.connections(), 1);
        assert_eq!(server.paths(), vec!["/api.php"]);
        let target = server.requests()[0].target().to_owned();
        assert!(target.contains("ac=videolist"), "目标: {target}");
        assert!(
            target.contains("wd=%E5%BA%86%E4%BD%99%E5%B9%B4"),
            "目标: {target}"
        );
        assert!(
            !target.contains("img/42.jpg"),
            "搜索阶段不得为结果发起图片请求"
        );

        let wire = serde_json::to_string(&cards).unwrap();
        assert!(wire.contains("haven://artwork/artwork-1"));
        assert!(!wire.contains("img/42.jpg"));
    }

    /// 用户明确配置的本机采集站可以直接使用，不需要测试专用的回环绕过。
    #[tokio::test]
    async fn production_client_accepts_an_explicitly_configured_loopback_catalog() {
        let server = FixtureServer::start(|_| {
            vec![(
                "/api.php".to_owned(),
                FixtureResponse::json(r#"{"code":1,"list":[]}"#.to_owned()),
            )]
        });
        let endpoint = format!("{}/api.php", server.origin());
        let production = Cms10Client::new().unwrap();
        let cards = production
            .search(&endpoint, "任意", 10)
            .await
            .expect("用户明确配置的本机采集站应可访问");
        assert!(cards.is_empty());
        assert_eq!(server.connections(), 1, "配置的本机端点应收到一次请求");
    }

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
    fn cms10_query_is_encoded_without_path_concatenation() {
        let url = Cms10Client
            .endpoint_url(
                "https://media.example.invalid/api.php?token=opaque",
                "wd",
                "庆余年",
            )
            .unwrap();
        assert_eq!(url.path(), "/api.php");
        let query = url.query().unwrap();
        assert!(query.contains("ac=videolist"));
        assert!(query.contains("wd=%E5%BA%86%E4%BD%99%E5%B9%B4"));
        assert!(query.contains("token=opaque"));
    }
}
