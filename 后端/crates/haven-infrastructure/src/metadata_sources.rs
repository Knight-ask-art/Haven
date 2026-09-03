//! 固定公开元数据来源与 M3U 播放列表搜索适配器。
//!
//! 这些 Provider 只访问源码中固定的公开 API（或用户在来源设置中明确配置的
//! M3U 地址），返回 `WorkCardDto` 搜索投影。它们不把远端海报 URL 写进 Wire，
//! 也不接受前端传入任意 URL、Header、Cookie 或凭据。

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use haven_application::services::search_source::SearchSourceParticipant;
use haven_application::services::source_import::CONTENT_CANDIDATE_PREFIX;
use haven_application::services::source_registry::SourceRegistryService;
use haven_application::wire::{
    ContentCategory, ExternalIdDto, ExternalIdProviderDto, MediaTypeDto, QueryCategory, WorkCardDto,
};
use haven_common::network::{HttpUrlPolicy, parse_http_url};
use haven_common::{AppError, ErrorKind};
use quick_xml::events::Event;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::http_security::{pin_client_builder, resolve_public_http_target};

const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(6);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(1_500);
const MAX_REDIRECTS: usize = 3;
const SOURCE_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

const TVMAZE_URL: &str = "https://api.tvmaze.com/search/shows";
const BANGUMI_URL: &str = "https://api.bgm.tv/v0/search/subjects";
const ANILIST_URL: &str = "https://graphql.anilist.co";
const ITUNES_URL: &str = "https://itunes.apple.com/search";
const GUTENBERG_OPDS_URL: &str = "https://www.gutenberg.org/ebooks/search.opds";
const INTERNET_ARCHIVE_URL: &str = "https://archive.org/advancedsearch.php";
const MANGADEX_URL: &str = "https://api.mangadex.org/manga";
const ARXIV_URL: &str = "https://export.arxiv.org/api/query";
const EUROPE_PMC_URL: &str = "https://www.ebi.ac.uk/europepmc/webservices/rest/search";
const WIKISOURCE_URL: &str = "https://zh.wikisource.org/w/api.php";
const CROSSREF_URL: &str = "https://api.crossref.org/works";
const OPENALEX_URL: &str = "https://api.openalex.org/works";

/// 固定公开 metadata Provider 的 sourceId 快照。保留该公开常量供诊断/测试使用；
/// 运行时覆盖校验使用下面的 `METADATA_SOURCE_KINDS`，并由同一数组驱动参与者工厂，
/// 避免“清单更新了但工厂忘记注册”时仍被误判为已接入。
pub const METADATA_SOURCE_IDS: [&str; 12] = [
    "tvmaze",
    "bangumi",
    "anilist",
    "itunes",
    "gutenberg",
    "archive",
    "mangadex",
    "arxiv",
    "europepmc",
    "wikisource",
    "crossref",
    "openalex",
];

pub const M3U_SOURCE_ID: &str = "m3u";

const METADATA_SOURCE_KINDS: [MetadataSourceKind; 12] = [
    MetadataSourceKind::Tvmaze,
    MetadataSourceKind::Bangumi,
    MetadataSourceKind::Anilist,
    MetadataSourceKind::Itunes,
    MetadataSourceKind::Gutenberg,
    MetadataSourceKind::InternetArchive,
    MetadataSourceKind::Mangadex,
    MetadataSourceKind::Arxiv,
    MetadataSourceKind::EuropePmc,
    MetadataSourceKind::Wikisource,
    MetadataSourceKind::Crossref,
    MetadataSourceKind::Openalex,
];

/// 校验内置目录与 Composition Root 预期注册的搜索参与者完全一致。
///
/// 这项检查故意放在 Infrastructure 的 Provider 清单旁边，并由 Tauri
/// Composition Root 在启动时调用：新增内置来源时如果忘记实现/注册 Provider，
/// 应用直接 fail closed，而不是让搜索调度静默跳过该来源并把它误报为可用。
/// 动态 `custom_` OPDS 来源不属于静态内置目录，因此不纳入此集合。
pub fn validate_builtin_search_coverage() -> Result<(), AppError> {
    let manifest: Value = serde_json::from_str(include_str!(
        "../../haven-application/resources/builtin-sources.json"
    ))
    .map_err(|_| internal_error("内置来源目录格式无效"))?;
    let catalog: HashSet<&str> = manifest["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|source| source["sourceId"].as_str())
        .collect();
    let registered: HashSet<&str> = METADATA_SOURCE_KINDS
        .iter()
        .map(|source| source.source_id())
        .chain(["cms10", M3U_SOURCE_ID])
        .chain(crate::opds::OPDS_SOURCE_IDS)
        .collect();
    if catalog != registered {
        return Err(internal_error("内置来源搜索能力未完整注册"));
    }
    // ID 集合一致还不够：目录分类也必须由对应 Provider 支持，否则来源会在
    // 某些分类筛选下被静默跳过，看起来像“已接入”但实际上不可搜索。
    for source in manifest["sources"].as_array().into_iter().flatten() {
        let Some(source_id) = source["sourceId"].as_str() else {
            return Err(internal_error("内置来源搜索能力未完整注册"));
        };
        let Some(categories) = source["categories"].as_array() else {
            return Err(internal_error("内置来源搜索能力未完整注册"));
        };
        for category in categories {
            let Some(category) = category.as_str() else {
                return Err(internal_error("内置来源搜索能力未完整注册"));
            };
            if !provider_supports_category(source_id, category) {
                return Err(internal_error("内置来源分类与搜索能力不匹配"));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum MetadataSourceKind {
    Tvmaze,
    Bangumi,
    Anilist,
    Itunes,
    Gutenberg,
    InternetArchive,
    Mangadex,
    Arxiv,
    EuropePmc,
    Wikisource,
    Crossref,
    Openalex,
}

impl MetadataSourceKind {
    fn source_id(self) -> &'static str {
        match self {
            Self::Tvmaze => "tvmaze",
            Self::Bangumi => "bangumi",
            Self::Anilist => "anilist",
            Self::Itunes => "itunes",
            Self::Gutenberg => "gutenberg",
            Self::InternetArchive => "archive",
            Self::Mangadex => "mangadex",
            Self::Arxiv => "arxiv",
            Self::EuropePmc => "europepmc",
            Self::Wikisource => "wikisource",
            Self::Crossref => "crossref",
            Self::Openalex => "openalex",
        }
    }

    /// The fixed authority for this built-in metadata provider. The source
    /// constants are the provider trust boundary, so a redirect must remain
    /// on this exact HTTPS host. User-configured M3U uses the separate generic
    /// public-URL path.
    fn fixed_host(self) -> &'static str {
        match self {
            Self::Tvmaze => "api.tvmaze.com",
            Self::Bangumi => "api.bgm.tv",
            Self::Anilist => "graphql.anilist.co",
            Self::Itunes => "itunes.apple.com",
            Self::Gutenberg => "www.gutenberg.org",
            Self::InternetArchive => "archive.org",
            Self::Mangadex => "api.mangadex.org",
            Self::Arxiv => "export.arxiv.org",
            Self::EuropePmc => "www.ebi.ac.uk",
            Self::Wikisource => "zh.wikisource.org",
            Self::Crossref => "api.crossref.org",
            Self::Openalex => "api.openalex.org",
        }
    }

    fn supports_category(self, category: Option<QueryCategory>) -> bool {
        match self {
            Self::Bangumi | Self::Anilist => matches!(
                category,
                None | Some(QueryCategory::All)
                    | Some(QueryCategory::Video)
                    | Some(QueryCategory::Comic)
            ),
            Self::Tvmaze | Self::Itunes => {
                matches!(
                    category,
                    None | Some(QueryCategory::All) | Some(QueryCategory::Video)
                )
            }
            Self::Gutenberg | Self::InternetArchive => {
                matches!(
                    category,
                    None | Some(QueryCategory::All) | Some(QueryCategory::Book)
                )
            }
            Self::Mangadex => {
                matches!(
                    category,
                    None | Some(QueryCategory::All) | Some(QueryCategory::Comic)
                )
            }
            Self::Arxiv | Self::Crossref | Self::Openalex => matches!(
                category,
                None | Some(QueryCategory::All) | Some(QueryCategory::Periodical)
            ),
            Self::EuropePmc | Self::Wikisource => matches!(
                category,
                None | Some(QueryCategory::All) | Some(QueryCategory::Periodical)
            ),
        }
    }
}

fn provider_supports_category(source_id: &str, category: &str) -> bool {
    let query_category = match category {
        "video" => QueryCategory::Video,
        "book" => QueryCategory::Book,
        "comic" => QueryCategory::Comic,
        "periodical" => QueryCategory::Periodical,
        _ => return false,
    };
    if let Some(source) = METADATA_SOURCE_KINDS
        .iter()
        .copied()
        .find(|source| source.source_id() == source_id)
    {
        return source.supports_category(Some(query_category));
    }
    match source_id {
        "cms10" | M3U_SOURCE_ID => query_category == QueryCategory::Video,
        id if crate::opds::OPDS_SOURCE_IDS.contains(&id) => query_category == QueryCategory::Book,
        _ => false,
    }
}

/// 固定 API 的共享客户端。每个来源仍使用自己的固定 URL；共享仅限传输层配置。
#[derive(Clone, Copy, Debug, Default)]
pub struct MetadataClient;

impl MetadataClient {
    pub fn new() -> Result<Self, AppError> {
        Ok(Self)
    }

    async fn get_json(&self, url: &str, source: MetadataSourceKind) -> Result<Value, AppError> {
        let body = self
            .send_limited(
                reqwest::Method::GET,
                url,
                None,
                "application/json",
                Some(source),
            )
            .await?;
        serde_json::from_slice(&body).map_err(|_| source_unavailable("来源返回了无效 JSON"))
    }

    async fn post_json(
        &self,
        url: &str,
        payload: &Value,
        source: MetadataSourceKind,
    ) -> Result<Value, AppError> {
        let body = self
            .send_limited(
                reqwest::Method::POST,
                url,
                Some(payload),
                "application/json",
                Some(source),
            )
            .await?;
        serde_json::from_slice(&body).map_err(|_| source_unavailable("来源返回了无效 JSON"))
    }

    async fn get_text(&self, url: &str, source: MetadataSourceKind) -> Result<String, AppError> {
        let body = self
            .send_limited(
                reqwest::Method::GET,
                url,
                None,
                "application/atom+xml,application/xml,text/plain;q=0.9,*/*;q=0.1",
                Some(source),
            )
            .await?;
        String::from_utf8(body).map_err(|_| source_unavailable("来源返回了无法读取的文本"))
    }

    async fn send_limited(
        &self,
        method: reqwest::Method,
        url: &str,
        payload: Option<&Value>,
        accept: &str,
        fixed_source: Option<MetadataSourceKind>,
    ) -> Result<Vec<u8>, AppError> {
        let mut current = match fixed_source {
            Some(source) => validate_fixed_metadata_url(url, source)?,
            None => parse_http_url(url, HttpUrlPolicy::SourceEndpoint)
                .map_err(|_| source_unavailable("来源地址不安全"))?
                .into_url(),
        };
        for _ in 0..=MAX_REDIRECTS {
            if let Some(source) = fixed_source {
                // Re-check the complete URL after every join. This preserves
                // the fixed provider authority even if a redirect is an
                // otherwise valid public HTTP(S) URL.
                validate_fixed_metadata_url(current.as_str(), source)?;
            }
            let target =
                resolve_public_http_target(current.as_str(), HttpUrlPolicy::SourceEndpoint)
                    .await
                    .map_err(|_| source_unavailable("来源地址解析不安全"))?;
            let builder = reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                // Keep the fixed-source client predictable on Windows while
                // preserving HTTPS certificate validation.
                .http1_only()
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(SOURCE_USER_AGENT);
            let client = pin_client_builder(builder, &target)
                .build()
                .map_err(|_| internal_error("元数据来源客户端初始化失败"))?;
            let mut request = client
                .request(method.clone(), target.url.clone())
                .header("Accept", accept);
            if let Some(payload) = payload {
                request = request.json(payload);
            }
            let response = request
                .send()
                .await
                .map_err(|_| source_unavailable("来源服务暂时不可达"))?;
            if response.status().is_redirection() {
                // Only GET source reads are safe to replay. The sole POST
                // provider (AniList) must fail closed on a redirect rather
                // than sending its JSON body to another authority.
                if method != reqwest::Method::GET {
                    return Err(source_unavailable("来源请求重定向不受支持"));
                }
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| source_unavailable("来源重定向地址无效"))?;
                current = target
                    .url
                    .join(location)
                    .map_err(|_| source_unavailable("来源重定向地址无效"))?;
                if let Some(source) = fixed_source {
                    validate_fixed_metadata_url(current.as_str(), source)?;
                }
                continue;
            }
            if !response.status().is_success() {
                return Err(source_unavailable("来源服务返回异常状态"));
            }
            if response
                .content_length()
                .is_some_and(|length| length > MAX_BODY_BYTES as u64)
            {
                return Err(source_unavailable("来源响应超出大小上限"));
            }
            let mut body = Vec::with_capacity(64 * 1024);
            let mut response = response;
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| source_unavailable("来源响应读取中断"))?
            {
                if body.len().saturating_add(chunk.len()) > MAX_BODY_BYTES {
                    return Err(source_unavailable("来源响应超出大小上限"));
                }
                body.extend_from_slice(chunk.as_ref());
            }
            return Ok(body);
        }
        Err(source_unavailable("来源重定向次数过多"))
    }

    async fn search(
        &self,
        source: MetadataSourceKind,
        query: &str,
        limit: u32,
    ) -> Result<Vec<WorkCardDto>, AppError> {
        match source {
            MetadataSourceKind::Tvmaze => self.search_tvmaze(query, limit).await,
            MetadataSourceKind::Bangumi => self.search_bangumi(query, limit).await,
            MetadataSourceKind::Anilist => self.search_anilist(query, limit).await,
            MetadataSourceKind::Itunes => self.search_itunes(query, limit).await,
            MetadataSourceKind::Gutenberg => self.search_gutenberg(query, limit).await,
            MetadataSourceKind::InternetArchive => self.search_internet_archive(query, limit).await,
            MetadataSourceKind::Mangadex => self.search_mangadex(query, limit).await,
            MetadataSourceKind::Arxiv => self.search_arxiv(query, limit).await,
            MetadataSourceKind::EuropePmc => self.search_europe_pmc(query, limit).await,
            MetadataSourceKind::Wikisource => self.search_wikisource(query, limit).await,
            MetadataSourceKind::Crossref => self.search_crossref(query, limit).await,
            MetadataSourceKind::Openalex => self.search_openalex(query, limit).await,
        }
    }

    async fn search_tvmaze(&self, query: &str, limit: u32) -> Result<Vec<WorkCardDto>, AppError> {
        let url = format!("{TVMAZE_URL}?q={}", urlencode(query));
        let value = self.get_json(&url, MetadataSourceKind::Tvmaze).await?;
        let items = value
            .as_array()
            .ok_or_else(|| source_unavailable("TVMaze 返回结构异常"))?;
        let mut cards = Vec::new();
        for item in items.iter().take(limit as usize) {
            let show = item.get("show").unwrap_or(item);
            let Some(title) = string_field(show, "name") else {
                continue;
            };
            let id = show
                .get("id")
                .and_then(Value::as_i64)
                .map(|v| v.to_string())
                .unwrap_or_else(|| stable_key(&title));
            cards.push(card(
                "tvmaze",
                &id,
                title,
                string_field(show, "summary").map(|v| clean_text(&v)),
                year_from_value(show.get("premiered")),
                (ContentCategory::Video, MediaTypeDto::Series),
                Some(ExternalIdProviderDto::Tvmaze),
            ));
        }
        Ok(cards)
    }

    async fn search_bangumi(&self, query: &str, limit: u32) -> Result<Vec<WorkCardDto>, AppError> {
        let payload = serde_json::json!({
            "keyword": query,
            "sort": "match",
            "filter": { "type": [1, 2] }
        });
        let value = self
            .post_json(BANGUMI_URL, &payload, MetadataSourceKind::Bangumi)
            .await?;
        let items = value
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| source_unavailable("Bangumi 返回结构异常"))?;
        let mut cards = Vec::new();
        for item in items.iter().take(limit as usize) {
            let Some(id) = item.get("id").and_then(Value::as_i64) else {
                continue;
            };
            let title = string_field(item, "name_cn")
                .filter(|value| !value.trim().is_empty())
                .or_else(|| string_field(item, "name"));
            let Some(title) = title else { continue };
            let is_comic = item.get("type").and_then(Value::as_i64) == Some(1);
            let (category, media_type) = if is_comic {
                (ContentCategory::Comic, MediaTypeDto::Comic)
            } else {
                (ContentCategory::Video, MediaTypeDto::Series)
            };
            cards.push(card(
                "bangumi",
                &id.to_string(),
                title,
                string_field(item, "summary").map(|v| clean_text(&v)),
                year_from_value(item.get("date")),
                (category, media_type),
                Some(ExternalIdProviderDto::Bangumi),
            ));
        }
        Ok(cards)
    }

    async fn search_anilist(&self, query: &str, limit: u32) -> Result<Vec<WorkCardDto>, AppError> {
        const QUERY: &str = r#"
          query($search: String!, $type: MediaType!, $perPage: Int!) {
            Page(perPage: $perPage) {
              media(search: $search, type: $type) {
                id type seasonYear
                title { romaji english native }
                description(asHtml: false)
              }
            }
          }
        "#;
        let mut cards = Vec::new();
        let mut succeeded = false;
        for media_type in ["ANIME", "MANGA"] {
            let payload = serde_json::json!({
                "query": QUERY,
                "variables": { "search": query, "type": media_type, "perPage": limit }
            });
            let value = match self
                .post_json(ANILIST_URL, &payload, MetadataSourceKind::Anilist)
                .await
            {
                Ok(value) => {
                    succeeded = true;
                    value
                }
                Err(_) => continue,
            };
            let items = value
                .get("data")
                .and_then(|v| v.get("Page"))
                .and_then(|v| v.get("media"))
                .and_then(Value::as_array)
                .ok_or_else(|| source_unavailable("AniList 返回结构异常"))?;
            for item in items.iter().take(limit as usize) {
                let Some(id) = item.get("id").and_then(Value::as_i64) else {
                    continue;
                };
                let title = item
                    .get("title")
                    .and_then(|v| v.get("english"))
                    .and_then(Value::as_str)
                    .filter(|v| !v.trim().is_empty())
                    .or_else(|| {
                        item.get("title")
                            .and_then(|v| v.get("native"))
                            .and_then(Value::as_str)
                    })
                    .or_else(|| {
                        item.get("title")
                            .and_then(|v| v.get("romaji"))
                            .and_then(Value::as_str)
                    })
                    .unwrap_or("")
                    .trim();
                if title.is_empty() {
                    continue;
                }
                let is_manga = media_type == "MANGA";
                cards.push(card(
                    "anilist",
                    &id.to_string(),
                    title.to_owned(),
                    string_field(item, "description").map(|v| clean_text(&v)),
                    year_from_value(item.get("seasonYear")),
                    (
                        if is_manga {
                            ContentCategory::Comic
                        } else {
                            ContentCategory::Video
                        },
                        if is_manga {
                            MediaTypeDto::Comic
                        } else {
                            MediaTypeDto::Series
                        },
                    ),
                    Some(ExternalIdProviderDto::Anilist),
                ));
            }
        }
        if !succeeded {
            return Err(source_unavailable("AniList 暂时不可用"));
        }
        Ok(cards)
    }

    async fn search_itunes(&self, query: &str, limit: u32) -> Result<Vec<WorkCardDto>, AppError> {
        // Apple 已将 movie entity 从 Search API 的可检索集合中移除；tvSeason
        // 仍返回真实剧集条目。固定限定 entity，避免把音乐/播客误投影成影视作品。
        let url = format!(
            "{ITUNES_URL}?term={}&media=tvShow&entity=tvSeason&country=us&limit={limit}",
            urlencode(query)
        );
        let value = self.get_json(&url, MetadataSourceKind::Itunes).await?;
        let items = value
            .get("results")
            .and_then(Value::as_array)
            .ok_or_else(|| source_unavailable("iTunes Search 返回结构异常"))?;
        Ok(items
            .iter()
            .take(limit as usize)
            .filter_map(|item| {
                let title = string_field(item, "trackName")
                    .or_else(|| string_field(item, "collectionName"))?;
                let id = item
                    .get("trackId")
                    .or_else(|| item.get("collectionId"))
                    .and_then(Value::as_i64)
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| stable_key(&title));
                Some(card(
                    "itunes",
                    &id,
                    title,
                    string_field(item, "longDescription")
                        .or_else(|| string_field(item, "shortDescription"))
                        .map(|v| clean_text(&v)),
                    year_from_value(item.get("releaseDate")),
                    (ContentCategory::Video, MediaTypeDto::Series),
                    None,
                ))
            })
            .collect())
    }

    async fn search_gutenberg(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<WorkCardDto>, AppError> {
        let url = format!("{GUTENBERG_OPDS_URL}/?query={}", urlencode(query));
        let xml = self.get_text(&url, MetadataSourceKind::Gutenberg).await?;
        Ok(parse_atom_titles(&xml)
            .iter()
            .filter_map(|(id, title)| {
                let id = id
                    .split("/ebooks/")
                    .nth(1)
                    .and_then(|value| value.split(|ch: char| !ch.is_ascii_digit()).next())
                    .filter(|value| !value.is_empty())?;
                Some(card(
                    "gutenberg",
                    id,
                    title.clone(),
                    None,
                    None,
                    (ContentCategory::Book, MediaTypeDto::Book),
                    Some(ExternalIdProviderDto::Gutenberg),
                ))
            })
            .take(limit as usize)
            .collect())
    }

    async fn search_internet_archive(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<WorkCardDto>, AppError> {
        // Advanced Search 是 Internet Archive 的公开 JSON API。限定 mediatype:texts，
        // 避免把视频、音频和软件条目投影成图书；该端点不需要 API Key。
        let archive_query = format!("title:({}) AND mediatype:texts", query.trim());
        let url = format!(
            "{INTERNET_ARCHIVE_URL}?q={}&fl%5B%5D=identifier&fl%5B%5D=title&fl%5B%5D=description&fl%5B%5D=year&rows={limit}&page=1&output=json",
            urlencode(&archive_query)
        );
        let value = self
            .get_json(&url, MetadataSourceKind::InternetArchive)
            .await?;
        let items = value
            .get("response")
            .and_then(|response| response.get("docs"))
            .and_then(Value::as_array)
            .ok_or_else(|| source_unavailable("Internet Archive 返回结构异常"))?;
        Ok(items
            .iter()
            .take(limit as usize)
            .filter_map(|item| {
                let title = stringish_field(item, "title")?;
                let external_id = stringish_field(item, "identifier")?;
                Some(card(
                    "archive",
                    &external_id,
                    title,
                    stringish_field(item, "description").map(|v| clean_text(&v)),
                    year_from_value(item.get("year")),
                    (ContentCategory::Book, MediaTypeDto::Book),
                    None,
                ))
            })
            .collect())
    }

    async fn search_mangadex(&self, query: &str, limit: u32) -> Result<Vec<WorkCardDto>, AppError> {
        let url = format!(
            "{MANGADEX_URL}?title={}&limit={limit}&includes[]=cover_art",
            urlencode(query)
        );
        let value = self.get_json(&url, MetadataSourceKind::Mangadex).await?;
        let items = value
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| source_unavailable("MangaDex 返回结构异常"))?;
        Ok(items
            .iter()
            .take(limit as usize)
            .filter_map(|item| {
                let id = string_field(item, "id")?;
                let attrs = item.get("attributes")?;
                let title = localized_title(attrs.get("title")?)?;
                Some(card(
                    "mangadex",
                    &id,
                    title,
                    attrs
                        .get("description")
                        .and_then(localized_title)
                        .map(|v| clean_text(&v)),
                    year_from_value(attrs.get("year")),
                    (ContentCategory::Comic, MediaTypeDto::Comic),
                    None,
                ))
            })
            .collect())
    }

    async fn search_arxiv(&self, query: &str, limit: u32) -> Result<Vec<WorkCardDto>, AppError> {
        let url = format!(
            "{ARXIV_URL}?search_query=all:{}&start=0&max_results={limit}",
            urlencode(query)
        );
        let xml = self.get_text(&url, MetadataSourceKind::Arxiv).await?;
        let entries = parse_arxiv_entries(&xml);
        Ok(entries
            .into_iter()
            .take(limit as usize)
            .map(|entry| {
                let external_id = normalize_arxiv_id(&entry.id).unwrap_or(entry.id);
                card(
                    "arxiv",
                    &external_id,
                    entry.title,
                    Some(entry.summary),
                    entry.year,
                    (ContentCategory::Periodical, MediaTypeDto::Article),
                    None,
                )
            })
            .collect())
    }

    async fn search_europe_pmc(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<WorkCardDto>, AppError> {
        // 只选择开放获取文章，并要求返回 PMCID；没有 PMCID 的记录无法在本地
        // 获取全文，因此不把它们投影为可导入候选。
        let url = format!(
            "{EUROPE_PMC_URL}?query={}%20AND%20OPEN_ACCESS:Y&format=json&resultType=core&pageSize={limit}",
            urlencode(query)
        );
        let value = self.get_json(&url, MetadataSourceKind::EuropePmc).await?;
        let items = value
            .get("resultList")
            .and_then(|v| v.get("result"))
            .and_then(Value::as_array)
            .ok_or_else(|| source_unavailable("Europe PMC 返回结构异常"))?;
        Ok(items
            .iter()
            .take(limit as usize)
            .filter_map(|item| {
                let pmcid = string_field(item, "pmcid")
                    .or_else(|| string_field(item, "pmcId"))
                    .filter(|id| is_pmcid(id))?;
                let title = string_field(item, "title")?;
                let year = string_field(item, "firstPublicationDate")
                    .and_then(|value| value.get(0..4).and_then(|part| part.parse().ok()))
                    .or_else(|| {
                        item.get("pubYear")
                            .and_then(Value::as_i64)
                            .and_then(|value| i32::try_from(value).ok())
                    });
                Some(card(
                    "europepmc",
                    &pmcid,
                    title,
                    string_field(item, "abstractText").map(|v| clean_text(&v)),
                    year,
                    (ContentCategory::Periodical, MediaTypeDto::Article),
                    None,
                ))
            })
            .collect())
    }

    async fn search_wikisource(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<WorkCardDto>, AppError> {
        let url = format!(
            "{WIKISOURCE_URL}?action=query&list=search&srsearch={}&srlimit={limit}&format=json&formatversion=2",
            urlencode(query)
        );
        let value = self.get_json(&url, MetadataSourceKind::Wikisource).await?;
        let items = value
            .get("query")
            .and_then(|v| v.get("search"))
            .and_then(Value::as_array)
            .ok_or_else(|| source_unavailable("Wikisource 返回结构异常"))?;
        Ok(items
            .iter()
            .take(limit as usize)
            .filter_map(|item| {
                let title = string_field(item, "title")?;
                // 标题会在 content-candidate 中进行 UTF-8 百分号编码；这里仍
                // 只把页面标题放到候选外部 ID，不把页面 URL暴露给前端。
                Some(card(
                    "wikisource",
                    &title,
                    title.clone(),
                    string_field(item, "snippet").map(|v| clean_text(&v)),
                    None,
                    (ContentCategory::Periodical, MediaTypeDto::Article),
                    None,
                ))
            })
            .collect())
    }

    async fn search_crossref(&self, query: &str, limit: u32) -> Result<Vec<WorkCardDto>, AppError> {
        let url = format!("{CROSSREF_URL}?query={}&rows={limit}", urlencode(query));
        let value = self.get_json(&url, MetadataSourceKind::Crossref).await?;
        let items = value
            .get("message")
            .and_then(|v| v.get("items"))
            .and_then(Value::as_array)
            .ok_or_else(|| source_unavailable("Crossref 返回结构异常"))?;
        Ok(items
            .iter()
            .take(limit as usize)
            .filter_map(|item| {
                let title = item
                    .get("title")
                    .and_then(Value::as_array)
                    .and_then(|values| values.first())
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)?;
                let id = string_field(item, "DOI")
                    .or_else(|| string_field(item, "URL"))
                    .unwrap_or_else(|| stable_key(&title));
                Some(card(
                    "crossref",
                    &id,
                    title,
                    None,
                    year_from_crossref(item),
                    (ContentCategory::Periodical, MediaTypeDto::Article),
                    None,
                ))
            })
            .collect())
    }

    async fn search_openalex(&self, query: &str, limit: u32) -> Result<Vec<WorkCardDto>, AppError> {
        let url = format!(
            "{OPENALEX_URL}?search={}&per-page={limit}",
            urlencode(query)
        );
        let value = self.get_json(&url, MetadataSourceKind::Openalex).await?;
        let items = value
            .get("results")
            .and_then(Value::as_array)
            .ok_or_else(|| source_unavailable("OpenAlex 返回结构异常"))?;
        Ok(items
            .iter()
            .take(limit as usize)
            .filter_map(|item| {
                let title = string_field(item, "title")?;
                let id = string_field(item, "id").unwrap_or_else(|| stable_key(&title));
                Some(card(
                    "openalex",
                    &id,
                    title,
                    None,
                    item.get("publication_year")
                        .and_then(Value::as_i64)
                        .and_then(|v| i32::try_from(v).ok()),
                    (ContentCategory::Periodical, MediaTypeDto::Article),
                    None,
                ))
            })
            .collect())
    }

    async fn fetch_limited_url(&self, url: &str) -> Result<Vec<u8>, AppError> {
        self.send_limited(
            reqwest::Method::GET,
            url,
            None,
            "application/vnd.apple.mpegurl,application/x-mpegurl,text/plain;q=0.9,*/*;q=0.1",
            None,
        )
        .await
    }
}

/// Validate a built-in Provider URL before resolving or sending it. The
/// shared URL policy handles generic syntax and public-address classes; this
/// second policy keeps each fixed Provider on its exact HTTPS authority and
/// rejects explicit ports, including an explicitly written default `:443`.
fn validate_fixed_metadata_url(
    raw: &str,
    source: MetadataSourceKind,
) -> Result<reqwest::Url, AppError> {
    let safe = parse_http_url(raw, HttpUrlPolicy::SourceEndpoint)
        .map_err(|_| source_unavailable("来源地址不安全"))?;
    let host = safe.host().to_owned();
    let url = safe.into_url();
    if url.scheme() != "https"
        || url.port().is_some()
        || has_explicit_authority_port(raw)
        || host != source.fixed_host()
    {
        return Err(source_unavailable("固定来源地址不安全"));
    }
    Ok(url)
}

/// `url::Url` normalizes an explicit default port away from `Url::port()`;
/// fixed Provider policies still need to distinguish `https://host` from
/// `https://host:443` so an upstream change cannot widen the authority.
fn has_explicit_authority_port(raw: &str) -> bool {
    let Some((_, after_scheme)) = raw.split_once("://") else {
        return false;
    };
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if host.starts_with('[') {
        return host.find(']').is_some_and(|end| {
            host.get(end + 1..)
                .is_some_and(|tail| tail.starts_with(':'))
        });
    }
    host.contains(':')
}

/// 固定 API 来源参与者工厂。每个条目都有真实搜索实现并由 Composition Root 注册。
pub fn metadata_participants(client: Arc<MetadataClient>) -> Vec<Arc<dyn SearchSourceParticipant>> {
    METADATA_SOURCE_KINDS
        .into_iter()
        .map(|source| {
            Arc::new(MetadataSearchParticipant {
                source,
                client: client.clone(),
            }) as Arc<dyn SearchSourceParticipant>
        })
        .collect()
}

struct MetadataSearchParticipant {
    source: MetadataSourceKind,
    client: Arc<MetadataClient>,
}

#[async_trait]
impl SearchSourceParticipant for MetadataSearchParticipant {
    fn source_id(&self) -> &str {
        self.source.source_id()
    }

    fn supports_category(&self, category: Option<QueryCategory>) -> bool {
        self.source.supports_category(category)
    }

    async fn search(
        &self,
        query: &str,
        limit: u32,
        is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<Vec<WorkCardDto>, AppError> {
        if is_cancelled() {
            return Ok(Vec::new());
        }
        let mut cards = self.client.search(self.source, query, limit).await?;
        if is_cancelled() {
            cards.clear();
        }
        Ok(cards)
    }
}

/// M3U 播放列表搜索参与者。端点必须由用户在设置中显式配置；解析只返回频道名
/// 和安全的搜索投影，不把播放 URL放入 Wire。
pub struct M3uSearchParticipant {
    registry: SourceRegistryService,
    client: Arc<MetadataClient>,
}

impl M3uSearchParticipant {
    pub fn new(registry: SourceRegistryService, client: Arc<MetadataClient>) -> Self {
        Self { registry, client }
    }
}

#[async_trait]
impl SearchSourceParticipant for M3uSearchParticipant {
    fn source_id(&self) -> &str {
        M3U_SOURCE_ID
    }

    fn supports_category(&self, category: Option<QueryCategory>) -> bool {
        matches!(
            category,
            None | Some(QueryCategory::All) | Some(QueryCategory::Video)
        )
    }

    async fn search(
        &self,
        query: &str,
        limit: u32,
        is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<Vec<WorkCardDto>, AppError> {
        let Some(endpoint) = self.registry.endpoint("m3u").await? else {
            return Ok(Vec::new());
        };
        if is_cancelled() {
            return Ok(Vec::new());
        }
        let body = self.client.fetch_limited_url(&endpoint).await?;
        let text = String::from_utf8(body).map_err(|_| source_unavailable("M3U 编码无法读取"))?;
        let needle = query.trim().to_lowercase();
        Ok(parse_m3u_entries(&text)
            .into_iter()
            .filter(|entry| entry.title.to_lowercase().contains(&needle))
            .take(limit as usize)
            .map(|entry| {
                let key = format!("{}\u{1}{}", entry.title, entry.url);
                card(
                    M3U_SOURCE_ID,
                    // Keep the title/URL pair inside the opaque candidate
                    // payload. Hashing it here would make the import side
                    // unable to recover the validated stream URL.
                    &key,
                    entry.title,
                    entry.group,
                    None,
                    (ContentCategory::Video, MediaTypeDto::Series),
                    None,
                )
            })
            .collect())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct M3uEntry {
    title: String,
    url: String,
    group: Option<String>,
}

fn parse_m3u_entries(text: &str) -> Vec<M3uEntry> {
    let mut entries = Vec::new();
    let mut pending: Option<(String, Option<String>)> = None;
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if line.starts_with("#EXTINF") {
            let title = line
                .split_once(',')
                .map(|(_, title)| title.trim().to_owned())
                .filter(|title| !title.is_empty())
                .or_else(|| attribute(line, "tvg-name"))
                .unwrap_or_else(|| "未命名频道".to_owned());
            pending = Some((title, attribute(line, "group-title")));
        } else if !line.starts_with('#') {
            if let Some((title, group)) = pending.take() {
                if parse_http_url(line, HttpUrlPolicy::MediaResource).is_ok() {
                    entries.push(M3uEntry {
                        title,
                        url: line.to_owned(),
                        group,
                    });
                }
            }
        }
    }
    entries
}

fn attribute(line: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = line.find(&needle)? + needle.len();
    let end = line[start..].find('"')? + start;
    let value = line[start..end].trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArxivEntry {
    id: String,
    title: String,
    summary: String,
    year: Option<i32>,
}

fn parse_arxiv_entries(xml: &str) -> Vec<ArxivEntry> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut entries = Vec::new();
    let mut current: Option<ArxivEntry> = None;
    let mut field: Option<&'static str> = None;
    let mut entry_depth = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = local_name(event.name().as_ref());
                if name == "entry" {
                    entry_depth += 1;
                    if entry_depth == 1 {
                        current = Some(ArxivEntry {
                            id: String::new(),
                            title: String::new(),
                            summary: String::new(),
                            year: None,
                        });
                    }
                } else if entry_depth == 1 {
                    field = match name.as_str() {
                        "id" => Some("id"),
                        "title" => Some("title"),
                        "summary" => Some("summary"),
                        "published" => Some("published"),
                        _ => None,
                    };
                }
            }
            Ok(Event::Text(text)) if entry_depth == 1 => {
                if let Some(target) = field {
                    let value = text.unescape().map(|v| v.into_owned()).unwrap_or_default();
                    if let Some(entry) = current.as_mut() {
                        match target {
                            "id" => entry.id.push_str(value.trim()),
                            "title" => entry.title.push_str(value.trim()),
                            "summary" => entry.summary.push_str(value.trim()),
                            "published" => {
                                entry.year = value.get(0..4).and_then(|v| v.parse().ok())
                            }
                            _ => {}
                        }
                    }
                }
            }
            Ok(Event::End(event)) => {
                let name = local_name(event.name().as_ref());
                if name == "entry" && entry_depth > 0 {
                    entry_depth -= 1;
                    if entry_depth == 0 {
                        if let Some(mut entry) = current.take() {
                            entry.title = clean_text(&entry.title);
                            entry.summary = clean_text(&entry.summary);
                            if !entry.id.trim().is_empty() && !entry.title.is_empty() {
                                entries.push(entry);
                            }
                        }
                    }
                }
                field = None;
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    entries
}

fn parse_atom_titles(xml: &str) -> Vec<(String, String)> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut entries = Vec::new();
    let mut depth = 0usize;
    let mut id = String::new();
    let mut title = String::new();
    let mut field: Option<&'static str> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = local_name(event.name().as_ref());
                if name == "entry" {
                    depth += 1;
                    if depth == 1 {
                        id.clear();
                        title.clear();
                    }
                } else if depth == 1 {
                    field = match name.as_str() {
                        "id" => Some("id"),
                        "title" => Some("title"),
                        _ => None,
                    };
                }
            }
            Ok(Event::Text(text)) if depth == 1 => {
                let value = text.unescape().map(|v| v.into_owned()).unwrap_or_default();
                match field {
                    Some("id") => id.push_str(value.trim()),
                    Some("title") => title.push_str(value.trim()),
                    _ => {}
                }
            }
            Ok(Event::End(event)) => {
                let name = local_name(event.name().as_ref());
                if name == "entry" && depth > 0 {
                    depth -= 1;
                    if depth == 0 && !id.trim().is_empty() && !title.trim().is_empty() {
                        entries.push((id.clone(), clean_text(&title)));
                    }
                }
                field = None;
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    entries
}

fn local_name(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw)
        .rsplit(':')
        .next()
        .unwrap_or("")
        .to_owned()
}

fn localized_title(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return (!text.trim().is_empty()).then(|| text.trim().to_owned());
    }
    let object = value.as_object()?;
    for key in ["zh", "zh-hans", "en", "ja", "ja-ro", "*", "und"] {
        if let Some(text) = object.get(key).and_then(Value::as_str) {
            if !text.trim().is_empty() {
                return Some(text.trim().to_owned());
            }
        }
    }
    object
        .values()
        .find_map(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// Internet Archive 的字段有时是字符串、有时是字符串数组；只取第一个非空值，
/// 以保持卡片投影稳定，不把远端原始数组暴露到 Wire。
fn stringish_field(value: &Value, key: &str) -> Option<String> {
    match value.get(key) {
        Some(Value::String(text)) => (!text.trim().is_empty()).then(|| text.trim().to_owned()),
        Some(Value::Array(values)) => values.iter().find_map(|item| {
            item.as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        }),
        _ => None,
    }
}

fn year_from_value(value: Option<&Value>) -> Option<i32> {
    match value {
        Some(Value::Number(number)) => number.as_i64().and_then(|v| i32::try_from(v).ok()),
        Some(Value::String(text)) => text.get(0..4).and_then(|v| v.parse().ok()),
        _ => None,
    }
}

fn year_from_crossref(value: &Value) -> Option<i32> {
    value
        .get("published")
        .or_else(|| value.get("published-print"))
        .or_else(|| value.get("issued"))
        .and_then(|v| v.get("date-parts"))
        .and_then(Value::as_array)
        .and_then(|v| v.first())
        .and_then(Value::as_array)
        .and_then(|v| v.first())
        .and_then(Value::as_i64)
        .and_then(|v| i32::try_from(v).ok())
}

fn clean_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn stable_key(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())[..24].to_owned()
}

fn candidate_id(source_id: &str, external_id: &str) -> String {
    if matches!(
        source_id,
        "mangadex" | "arxiv" | "europepmc" | "wikisource" | M3U_SOURCE_ID
    ) {
        return format!(
            "{}{}-{}",
            CONTENT_CANDIDATE_PREFIX,
            source_id,
            encode_candidate_component(external_id)
        );
    }
    format!("metadata-candidate-{source_id}-{}", stable_key(external_id))
}

/// Candidate ID 不是 URL；对可能包含 `/` 或非 ASCII 标题的外部标识做
/// UTF-8 百分号编码，之后只由 SourceImportService 在来源 allowlist 通过后解码。
fn encode_candidate_component(value: &str) -> String {
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

fn normalize_arxiv_id(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let candidate = trimmed
        .split_once("/abs/")
        .map(|(_, id)| id)
        .or_else(|| trimmed.split_once("/pdf/").map(|(_, id)| id))
        .unwrap_or(trimmed)
        .trim_end_matches(".pdf")
        .trim_matches('/');
    if candidate.is_empty()
        || candidate.len() > 128
        || candidate
            .chars()
            .any(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | '/')))
    {
        return None;
    }
    Some(candidate.to_owned())
}

fn is_pmcid(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("PMC") else {
        return false;
    };
    !rest.is_empty() && rest.len() <= 12 && rest.bytes().all(|byte| byte.is_ascii_digit())
}

type CardKind = (ContentCategory, MediaTypeDto);

fn card(
    source_id: &str,
    external_id: &str,
    title: String,
    description: Option<String>,
    release_year: Option<i32>,
    (category, media_type): CardKind,
    provider: Option<ExternalIdProviderDto>,
) -> WorkCardDto {
    WorkCardDto {
        work_id: candidate_id(source_id, external_id),
        title,
        original_title: None,
        description,
        categories: vec![category],
        available_media_types: vec![media_type],
        poster_uri: None,
        backdrop_uri: None,
        release_year,
        rating_value: None,
        rating_scale: None,
        favorite: false,
        progress: None,
        primary_action: None,
        external_ids: provider
            .map(|provider| {
                vec![ExternalIdDto {
                    provider,
                    external_id: external_id.to_owned(),
                }]
            })
            .unwrap_or_default(),
    }
}

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

fn source_unavailable(message: &'static str) -> AppError {
    AppError::new("SOURCE_UNAVAILABLE", ErrorKind::Network, message, true)
}

fn internal_error(message: &'static str) -> AppError {
    AppError::new("INTERNAL_ERROR", ErrorKind::Internal, message, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_fixed_sources_have_real_participants() {
        assert_eq!(METADATA_SOURCE_IDS.len(), 12);
        assert!(METADATA_SOURCE_IDS.contains(&"mangadex"));
        assert!(METADATA_SOURCE_IDS.contains(&"europepmc"));
        assert!(METADATA_SOURCE_IDS.contains(&"wikisource"));
        assert!(METADATA_SOURCE_IDS.contains(&"openalex"));
        assert!(METADATA_SOURCE_IDS.contains(&"archive"));
        let factory_ids: Vec<&str> = METADATA_SOURCE_KINDS
            .iter()
            .map(|source| source.source_id())
            .collect();
        assert_eq!(factory_ids, METADATA_SOURCE_IDS);
    }

    #[test]
    fn fixed_metadata_urls_stay_on_their_provider_authority() {
        for source in METADATA_SOURCE_KINDS {
            let url = format!("https://{}/health", source.fixed_host());
            assert!(
                validate_fixed_metadata_url(&url, source).is_ok(),
                "fixed provider URL rejected: {url}"
            );
        }
        for url in [
            "http://api.tvmaze.com/search/shows",
            "https://api.tvmaze.com:443/search/shows",
            "https://api.tvmaze.com:8443/search/shows",
            "https://evil.example/search/shows",
        ] {
            assert!(
                validate_fixed_metadata_url(url, MetadataSourceKind::Tvmaze).is_err(),
                "unsafe fixed provider URL accepted: {url}"
            );
        }
    }

    #[test]
    fn builtin_catalog_has_a_registered_search_path_for_every_entry() {
        validate_builtin_search_coverage().expect("内置来源必须全部绑定真实搜索参与者");
    }

    #[test]
    fn parses_bangumi_video_and_comic_shapes() {
        let value = serde_json::json!({
            "data": [
                { "id": 1, "type": 2, "name": "Anime", "summary": " story ", "date": "2024-01-01" },
                { "id": 2, "type": 1, "name_cn": "漫画", "summary": " manga ", "date": "2023" }
            ]
        });
        let data = value.get("data").unwrap().as_array().unwrap();
        assert_eq!(data[0].get("type").and_then(Value::as_i64), Some(2));
        assert_eq!(data[1].get("type").and_then(Value::as_i64), Some(1));
        assert_eq!(year_from_value(data[0].get("date")), Some(2024));
        assert_eq!(year_from_value(data[1].get("date")), Some(2023));
    }

    #[test]
    fn parses_m3u_only_pairs_and_filters_non_http() {
        let entries = parse_m3u_entries(
            "#EXTM3U\n#EXTINF:-1 tvg-name=\"News\" group-title=\"Live\",News\nhttps://cdn.example/news.m3u8\n#EXTINF:-1,Offline\nfile:///tmp/nope\n#EXTINF:-1,LAN\nhttp://127.0.0.1:8080/live.m3u8\n",
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "News");
        assert_eq!(entries[0].group.as_deref(), Some("Live"));
    }

    #[test]
    fn m3u_candidate_id_keeps_encoded_title_and_stream_payload() {
        let external_id = "新闻\u{1}https://cdn.example/live.m3u8?token=opaque";
        let candidate = candidate_id(M3U_SOURCE_ID, external_id);
        assert!(candidate.starts_with("content-candidate-m3u-"));
        assert!(candidate.contains("%01"), "separator must be encoded");
        assert!(
            candidate.contains("%3A%2F%2F"),
            "URL punctuation must be encoded"
        );
        assert!(!candidate.contains("https://"));
        assert!(!candidate.contains('\u{1}'));
        assert!(!candidate.contains(' '));
    }

    #[test]
    fn parses_arxiv_atom_entries() {
        let xml = r#"<feed xmlns="http://www.w3.org/2005/Atom"><entry><id>http://arxiv.org/abs/1</id><title> A   paper </title><summary> A summary </summary><published>2025-02-01T00:00:00Z</published></entry></feed>"#;
        let entries = parse_arxiv_entries(xml);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "A paper");
        assert_eq!(entries[0].year, Some(2025));
    }

    #[test]
    fn parses_gutenberg_atom_entries_and_numeric_ids() {
        let xml = r#"<feed xmlns="http://www.w3.org/2005/Atom"><entry><id>https://www.gutenberg.org/ebooks/84</id><title> Frankenstein </title></entry><entry><id>https://www.gutenberg.org/ebooks/authors/1</id><title>Author</title></entry></feed>"#;
        let entries = parse_atom_titles(xml);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].1, "Frankenstein");
        assert_eq!(
            entries[0]
                .0
                .split("/ebooks/")
                .nth(1)
                .unwrap()
                .split(|ch: char| !ch.is_ascii_digit())
                .next(),
            Some("84")
        );
    }

    #[test]
    fn parses_internet_archive_string_and_array_fields() {
        let value = serde_json::json!({
            "response": {
                "docs": [
                    {
                        "identifier": "frankenstein0000shel",
                        "title": "Frankenstein",
                        "description": ["A classic novel", "secondary"],
                        "year": 1818
                    }
                ]
            }
        });
        let item = &value["response"]["docs"][0];
        assert_eq!(
            stringish_field(item, "identifier").as_deref(),
            Some("frankenstein0000shel")
        );
        assert_eq!(
            stringish_field(item, "description").as_deref(),
            Some("A classic novel")
        );
        assert_eq!(year_from_value(item.get("year")), Some(1818));
    }

    #[test]
    fn category_gate_matches_catalog() {
        assert!(MetadataSourceKind::Mangadex.supports_category(Some(QueryCategory::Comic)));
        assert!(!MetadataSourceKind::Mangadex.supports_category(Some(QueryCategory::Video)));
        assert!(MetadataSourceKind::Arxiv.supports_category(Some(QueryCategory::Periodical)));
        assert!(provider_supports_category("opds_gutenberg", "book"));
        assert!(provider_supports_category("cms10", "video"));
        assert!(!provider_supports_category("cms10", "book"));
        assert!(!provider_supports_category("unknown", "video"));
    }
}
