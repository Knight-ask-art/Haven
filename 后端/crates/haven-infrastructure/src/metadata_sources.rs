//! 固定公开元数据来源与 M3U 播放列表搜索适配器。
//!
//! 这些 Provider 只访问源码中固定的公开 API（或用户在来源设置中明确配置的
//! M3U 地址），返回 `WorkCardDto` 搜索投影。它们不把远端海报 URL 写进 Wire，
//! 也不接受前端传入任意 URL、Header、Cookie 或凭据。
//!
//! 响应里带回的海报地址由 `artwork` 子模块在 Provider 边界校验，并交给注入的
//! `ImageProxyRepository` 登记成 `haven://artwork/<opaque>`；没有注入 Repository、
//! Provider 没给图片或地址被策略拒绝时，卡片保持无海报，搜索本身不受影响。

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
use haven_common::network::{HttpUrlPolicy, parse_http_url, url_origin};
use haven_common::{AppError, ErrorKind};
use haven_domain::contracts::ImageProxyRepository;
use quick_xml::events::{BytesStart, Event};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::http_security::{HttpTargetError, pin_client_builder, resolve_public_http_target};

mod artwork;

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

/// AniList GraphQL 搜索查询。`coverImage` 必须保留：海报地址由同一次响应带回，
/// 搜索阶段只做登记，不再为卡片额外请求图片。
const ANILIST_SEARCH_QUERY: &str = r#"
  query($search: String!, $type: MediaType!, $perPage: Int!) {
    Page(perPage: $perPage) {
      media(search: $search, type: $type) {
        id type seasonYear
        title { romaji english native }
        description(asHtml: false)
        coverImage { extraLarge large medium }
      }
    }
  }
"#;

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
    /// on this exact HTTPS host. User-configured M3U uses the explicit
    /// `UserProvidedSource` policy and stays on its registered origin.
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
///
/// 可选的 `ImageProxyRepository` 只用于登记 Provider 返回的海报地址，使它变成
/// opaque artwork id。它不抓取图片：响应已经带回地址时不再为每张卡片发网络
/// 请求，抓取留给渲染时的受控 Artwork 缓存。
#[derive(Clone, Default)]
pub struct MetadataClient {
    image_proxy: Option<Arc<dyn ImageProxyRepository>>,
}

impl std::fmt::Debug for MetadataClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 不打印 Repository 内部实现，避免把受控映射的细节带进日志。
        formatter
            .debug_struct("MetadataClient")
            .field("artwork_registration", &self.image_proxy.is_some())
            .finish()
    }
}

impl MetadataClient {
    /// 不登记 Artwork 的客户端（M3U 播放列表等只需要固定传输层）。
    pub fn new() -> Result<Self, AppError> {
        Ok(Self::default())
    }

    /// 组合根注入受控图片代理仓库：带图的搜索结果会登记成
    /// `haven://artwork/<opaque>`。未注入时搜索仍然可用，只是没有海报。
    pub fn with_image_proxy(proxy: Arc<dyn ImageProxyRepository>) -> Result<Self, AppError> {
        Ok(Self {
            image_proxy: Some(proxy),
        })
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
        let policy = if fixed_source.is_some() {
            HttpUrlPolicy::SourceEndpoint
        } else {
            // The only non-fixed caller is M3U, whose playlist endpoint is
            // explicitly supplied by the user and may live on the local LAN.
            HttpUrlPolicy::UserProvidedSource
        };
        let mut current = match fixed_source {
            Some(source) => validate_fixed_metadata_url(url, source)?,
            None => validate_user_m3u_url(url)?,
        };
        let user_source_origin = if fixed_source.is_none() {
            Some(url_origin(&current).ok_or_else(|| source_unavailable("M3U 地址无效"))?)
        } else {
            None
        };
        for _ in 0..=MAX_REDIRECTS {
            if let Some(source) = fixed_source {
                // Re-check the complete URL after every join. This preserves
                // the fixed provider authority even if a redirect is an
                // otherwise valid public HTTP(S) URL.
                validate_fixed_metadata_url(current.as_str(), source)?;
            }
            let target = resolve_public_http_target(current.as_str(), policy)
                .await
                .map_err(|error| {
                    source_unavailable(match error {
                        HttpTargetError::Invalid => "来源地址格式无效",
                        HttpTargetError::ResolveFailed => "来源域名暂时无法解析",
                        HttpTargetError::UnsafeAddress => "来源域名解析到了不允许的地址",
                    })
                })?;
            let builder = reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                // Keep the source client predictable on Windows while
                // preserving HTTPS certificate validation where used.
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
                } else {
                    validate_user_m3u_url(current.as_str())?;
                    if url_origin(&current).as_deref() != user_source_origin.as_deref() {
                        return Err(source_unavailable("M3U 重定向来源与登记地址不一致"));
                    }
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
        is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<Vec<WorkCardDto>, AppError> {
        let drafts = match source {
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
        }?;
        // Artwork 只登记同一次响应中的字段；取消后不再创建无消费者的映射。
        Ok(artwork::register_cards(self.image_proxy.as_deref(), drafts, is_cancelled).await)
    }

    async fn search_tvmaze(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<artwork::CardDraft>, AppError> {
        let url = format!("{TVMAZE_URL}?q={}", urlencode(query));
        let value = self.get_json(&url, MetadataSourceKind::Tvmaze).await?;
        let items = value
            .as_array()
            .ok_or_else(|| source_unavailable("TVMaze 返回结构异常"))?;
        let mut drafts = Vec::new();
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
            drafts.push(
                artwork::CardDraft::new(card(
                    "tvmaze",
                    &id,
                    title,
                    string_field(show, "summary").map(|v| clean_text(&v)),
                    year_from_value(show.get("premiered")),
                    (ContentCategory::Video, MediaTypeDto::Series),
                    Some(ExternalIdProviderDto::Tvmaze),
                ))
                .with_poster(artwork::tvmaze_poster(show)),
            );
        }
        Ok(drafts)
    }

    async fn search_bangumi(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<artwork::CardDraft>, AppError> {
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
        let mut drafts = Vec::new();
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
            drafts.push(
                artwork::CardDraft::new(card(
                    "bangumi",
                    &id.to_string(),
                    title,
                    string_field(item, "summary").map(|v| clean_text(&v)),
                    year_from_value(item.get("date")),
                    (category, media_type),
                    Some(ExternalIdProviderDto::Bangumi),
                ))
                .with_poster(artwork::bangumi_poster(item)),
            );
        }
        Ok(drafts)
    }

    async fn search_anilist(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<artwork::CardDraft>, AppError> {
        let mut drafts = Vec::new();
        let mut succeeded = false;
        for media_type in ["ANIME", "MANGA"] {
            let payload = serde_json::json!({
                "query": ANILIST_SEARCH_QUERY,
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
                drafts.push(
                    artwork::CardDraft::new(card(
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
                    ))
                    .with_poster(artwork::anilist_poster(item)),
                );
            }
        }
        if !succeeded {
            return Err(source_unavailable("AniList 暂时不可用"));
        }
        Ok(drafts)
    }

    async fn search_itunes(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<artwork::CardDraft>, AppError> {
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
                Some(
                    artwork::CardDraft::new(card(
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
                    .with_poster(artwork::itunes_poster(item)),
                )
            })
            .collect())
    }

    async fn search_gutenberg(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<artwork::CardDraft>, AppError> {
        let url = format!("{GUTENBERG_OPDS_URL}/?query={}", urlencode(query));
        let xml = self.get_text(&url, MetadataSourceKind::Gutenberg).await?;
        Ok(parse_atom_entries(&xml)
            .iter()
            .filter_map(|entry| {
                let id = entry
                    .id
                    .split("/ebooks/")
                    .nth(1)
                    .and_then(|value| value.split(|ch: char| !ch.is_ascii_digit()).next())
                    .filter(|value| !value.is_empty())?;
                Some(
                    artwork::CardDraft::new(card(
                        "gutenberg",
                        id,
                        entry.title.clone(),
                        None,
                        None,
                        (ContentCategory::Book, MediaTypeDto::Book),
                        Some(ExternalIdProviderDto::Gutenberg),
                    ))
                    .with_poster(entry.image.as_deref().and_then(artwork::gutenberg_poster)),
                )
            })
            .take(limit as usize)
            .collect())
    }

    async fn search_internet_archive(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<artwork::CardDraft>, AppError> {
        // Advanced Search 是 Internet Archive 的公开 JSON API。限定 mediatype:texts，
        // 避免把视频、音频和软件条目投影成图书；该端点不需要 API Key。
        //
        // 该响应不携带任何图片字段，因此这里保持空海报：IA 的派生封面端点会
        // 跳到另一个主机，受控出站策略按已登记主机作用域会拒绝该跳转，登记它
        // 只会得到一张永远加载不出来的海报。
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
                Some(artwork::CardDraft::new(card(
                    "archive",
                    &external_id,
                    title,
                    stringish_field(item, "description").map(|v| clean_text(&v)),
                    year_from_value(item.get("year")),
                    (ContentCategory::Book, MediaTypeDto::Book),
                    None,
                )))
            })
            .collect())
    }

    async fn search_mangadex(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<artwork::CardDraft>, AppError> {
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
            .filter_map(mangadex_draft)
            .collect())
    }

    async fn search_arxiv(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<artwork::CardDraft>, AppError> {
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
                artwork::CardDraft::new(card(
                    "arxiv",
                    &external_id,
                    entry.title,
                    Some(entry.summary),
                    entry.year,
                    (ContentCategory::Periodical, MediaTypeDto::Article),
                    None,
                ))
            })
            .collect())
    }

    async fn search_europe_pmc(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<artwork::CardDraft>, AppError> {
        // 只选择开放获取文章，并要求返回 PMCID；没有 PMCID 的记录无法在本地
        // 获取全文，因此不把它们投影为可导入候选。该来源没有海报字段。
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
                Some(artwork::CardDraft::new(card(
                    "europepmc",
                    &pmcid,
                    title,
                    string_field(item, "abstractText").map(|v| clean_text(&v)),
                    year,
                    (ContentCategory::Periodical, MediaTypeDto::Article),
                    None,
                )))
            })
            .collect())
    }

    async fn search_wikisource(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<artwork::CardDraft>, AppError> {
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
                Some(artwork::CardDraft::new(card(
                    "wikisource",
                    &title,
                    title.clone(),
                    string_field(item, "snippet").map(|v| clean_text(&v)),
                    None,
                    (ContentCategory::Periodical, MediaTypeDto::Article),
                    None,
                )))
            })
            .collect())
    }

    async fn search_crossref(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<artwork::CardDraft>, AppError> {
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
                Some(artwork::CardDraft::new(card(
                    "crossref",
                    &id,
                    title,
                    None,
                    year_from_crossref(item),
                    (ContentCategory::Periodical, MediaTypeDto::Article),
                    None,
                )))
            })
            .collect())
    }

    async fn search_openalex(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<artwork::CardDraft>, AppError> {
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
                Some(artwork::CardDraft::new(card(
                    "openalex",
                    &id,
                    title,
                    None,
                    item.get("publication_year")
                        .and_then(Value::as_i64)
                        .and_then(|v| i32::try_from(v).ok()),
                    (ContentCategory::Periodical, MediaTypeDto::Article),
                    None,
                )))
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

/// M3U endpoints are user supplied, so local/private hosts and service ports
/// are valid. Query parameters may contain a token and therefore require HTTPS.
fn validate_user_m3u_url(raw: &str) -> Result<reqwest::Url, AppError> {
    let parsed = parse_http_url(raw, HttpUrlPolicy::UserProvidedSource)
        .map_err(|_| source_unavailable("M3U 地址不安全"))?;
    if parsed.as_url().query().is_some() && parsed.as_url().scheme() != "https" {
        return Err(source_unavailable("带查询参数的 M3U 地址必须使用 HTTPS"));
    }
    Ok(parsed.into_url())
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
        self.client
            .search(self.source, query, limit, is_cancelled)
            .await
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
            let title = extinf_title(line)
                .filter(|title| !title.is_empty())
                .map(str::to_owned)
                .or_else(|| attribute(line, "tvg-name"))
                .unwrap_or_else(|| "未命名频道".to_owned());
            pending = Some((title, attribute(line, "group-title")));
        } else if !line.starts_with('#') {
            if let Some((title, group)) = pending.take() {
                // M3U is a user-configured source, so a LAN channel URL must
                // survive the parse filter. The strict default media policy
                // would drop exactly the personal/self-hosted entries the user
                // registered; link-local, reserved and metadata targets stay
                // denied by the shared policy itself.
                if parse_http_url(line, HttpUrlPolicy::UserProvidedSource).is_ok() {
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

/// `#EXTINF` 行里标题的起点：**引号外的第一个逗号**之后。
///
/// 属性值可能自带逗号（`group-title="News, International"`），按第一个逗号盲切会把
/// 属性内容粘进标题；最后一个逗号也不对，标题本身可以含逗号。
fn extinf_title(line: &str) -> Option<&str> {
    let mut quoted = false;
    for (index, ch) in line.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            ',' if !quoted => return Some(line[index + 1..].trim()),
            _ => {}
        }
    }
    None
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
                    let value = crate::unescape_xml_text(&text).unwrap_or_default();
                    if let Some(entry) = current.as_mut() {
                        match target {
                            "id" => entry.id.push_str(&value),
                            "title" => entry.title.push_str(&value),
                            "summary" => entry.summary.push_str(&value),
                            "published" => {
                                entry.year = value.get(0..4).and_then(|v| v.parse().ok())
                            }
                            _ => {}
                        }
                    }
                }
            }
            Ok(Event::GeneralRef(reference)) if entry_depth == 1 => {
                if let (Some(target), Some(entry)) = (field, current.as_mut()) {
                    let value = crate::unescape_xml_reference(&reference).unwrap_or_default();
                    match target {
                        "id" => entry.id.push_str(&value),
                        "title" => entry.title.push_str(&value),
                        "summary" => entry.summary.push_str(&value),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(event)) => {
                let name = local_name(event.name().as_ref());
                if name == "entry" && entry_depth > 0 {
                    entry_depth -= 1;
                    if entry_depth == 0 {
                        if let Some(mut entry) = current.take() {
                            entry.id = entry.id.trim().to_owned();
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

/// OPDS/Atom 条目。`image` 是 Provider 响应里真实带回的封面 `link`（相对或
/// 绝对地址），是否可用由 Provider 图片主机白名单裁决。
#[derive(Debug, Clone, PartialEq, Eq)]
struct AtomEntry {
    id: String,
    title: String,
    image: Option<String>,
    image_rank: u8,
}

impl AtomEntry {
    fn new() -> Self {
        Self {
            id: String::new(),
            title: String::new(),
            image: None,
            image_rank: 0,
        }
    }
}

fn parse_atom_entries(xml: &str) -> Vec<AtomEntry> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut entries = Vec::new();
    let mut depth = 0usize;
    let mut current: Option<AtomEntry> = None;
    let mut field: Option<&'static str> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = local_name(event.name().as_ref());
                if name == "entry" {
                    depth += 1;
                    if depth == 1 {
                        current = Some(AtomEntry::new());
                    }
                } else if depth == 1 {
                    if name == "link" {
                        record_atom_image(current.as_mut(), &event);
                    }
                    field = match name.as_str() {
                        "id" => Some("id"),
                        "title" => Some("title"),
                        _ => None,
                    };
                }
            }
            // OPDS 的封面 link 通常是自闭合元素。
            Ok(Event::Empty(event)) if depth == 1 => {
                if local_name(event.name().as_ref()) == "link" {
                    record_atom_image(current.as_mut(), &event);
                }
            }
            Ok(Event::Text(text)) if depth == 1 => {
                let value = crate::unescape_xml_text(&text).unwrap_or_default();
                if let Some(entry) = current.as_mut() {
                    match field {
                        Some("id") => entry.id.push_str(&value),
                        Some("title") => entry.title.push_str(&value),
                        _ => {}
                    }
                }
            }
            Ok(Event::GeneralRef(reference)) if depth == 1 => {
                let value = crate::unescape_xml_reference(&reference).unwrap_or_default();
                if let Some(entry) = current.as_mut() {
                    match field {
                        Some("id") => entry.id.push_str(&value),
                        Some("title") => entry.title.push_str(&value),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(event)) => {
                let name = local_name(event.name().as_ref());
                if name == "entry" && depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        if let Some(mut entry) = current.take() {
                            entry.id = entry.id.trim().to_owned();
                            entry.title = clean_text(&entry.title);
                            if !entry.id.is_empty() && !entry.title.is_empty() {
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

/// 记录 OPDS 封面 `link`：原图优先于缩略图，其余关系（如 `alternate`）忽略。
/// href 是否真的可用由 `artwork::gutenberg_poster` 的主机白名单裁决。
fn record_atom_image(entry: Option<&mut AtomEntry>, event: &BytesStart<'_>) {
    let Some(entry) = entry else { return };
    let Some(href) = atom_link_attribute(event, "href") else {
        return;
    };
    let rel = atom_link_attribute(event, "rel").unwrap_or_default();
    let rank = match rel.as_str() {
        "http://opds-spec.org/image" | "http://opds-spec.org/cover" => 2,
        "http://opds-spec.org/image/thumbnail" | "http://opds-spec.org/thumbnail" => 1,
        _ => return,
    };
    if rank > entry.image_rank {
        entry.image_rank = rank;
        entry.image = Some(href);
    }
}

fn atom_link_attribute(event: &BytesStart<'_>, key: &str) -> Option<String> {
    event
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.as_ref() == key.as_bytes())
        .and_then(|attribute| {
            attribute
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .ok()
                .map(|value| value.into_owned())
        })
}

fn local_name(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw)
        .rsplit(':')
        .next()
        .unwrap_or("")
        .to_owned()
}

/// MangaDex 条目 → 卡片草稿。
///
/// `includes[]=cover_art` 带回的 relationship 里含封面 `fileName`；卡片映射、
/// ID 形状与文件名边界在这里收口，便于用固定 fixture 验证整条路径。
fn mangadex_draft(item: &Value) -> Option<artwork::CardDraft> {
    let id = string_field(item, "id")?;
    let attrs = item.get("attributes")?;
    let title = localized_title(attrs.get("title")?)?;
    Some(
        artwork::CardDraft::new(card(
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
        .with_poster(artwork::mangadex_poster(&id, item)),
    )
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
    crate::lower_hex(&hasher.finalize())[..24].to_owned()
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
    use std::sync::Mutex;

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
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "News");
        assert_eq!(entries[0].group.as_deref(), Some("Live"));
        assert_eq!(entries[1].title, "LAN");
        assert_eq!(entries[1].url, "http://127.0.0.1:8080/live.m3u8");
    }

    #[test]
    fn m3u_extinf_title_starts_after_the_first_unquoted_comma() {
        let entries = parse_m3u_entries(
            "#EXTM3U\n\
             #EXTINF:-1 group-title=\"News, International\",BBC World\nhttps://cdn.example/world.m3u8\n\
             #EXTINF:-1 group-title=\"A, B\",Comma, In Title\nhttps://cdn.example/comma.m3u8\n\
             #EXTINF:-1 tvg-name=\"Fallback Channel\" group-title=\"News, International\",\nhttps://cdn.example/fallback.m3u8\n\
             #EXTINF:-1 group-title=\"News, International\",\nhttps://cdn.example/unnamed.m3u8\n",
        );
        assert_eq!(entries.len(), 4);
        // 属性里的逗号不能进入标题；标题里的逗号必须保留。
        assert_eq!(entries[0].title, "BBC World");
        assert_eq!(entries[0].group.as_deref(), Some("News, International"));
        assert_eq!(entries[1].title, "Comma, In Title");
        assert_eq!(entries[1].group.as_deref(), Some("A, B"));
        // 逗号后没有标题时，仍回退到 tvg-name，最后才是「未命名频道」。
        assert_eq!(entries[2].title, "Fallback Channel");
        assert_eq!(entries[2].group.as_deref(), Some("News, International"));
        assert_eq!(entries[3].title, "未命名频道");
    }

    #[test]
    fn user_m3u_playlist_accepts_local_http_but_requires_https_for_query_tokens() {
        for raw in [
            "http://127.0.0.1:9978/list.m3u",
            "http://playlist:8096/list.m3u",
            "https://192.168.1.10:5001/list.m3u",
            "https://public.example.invalid:12345/list.m3u?token=opaque",
        ] {
            assert!(
                validate_user_m3u_url(raw).is_ok(),
                "explicitly configured M3U playlist should be accepted: {raw}"
            );
        }
        for raw in [
            "http://public.example.invalid:12345/list.m3u?token=opaque",
            "http://169.254.169.254:9978/latest/meta-data",
            "http://198.18.0.1:9978/list.m3u",
            "ftp://public.example.invalid/list.m3u",
        ] {
            assert!(
                validate_user_m3u_url(raw).is_err(),
                "unsafe M3U endpoint should be rejected: {raw}"
            );
        }
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
    fn m3u_playlist_keeps_personal_streams_and_rejects_reserved_targets() {
        let text = "#EXTM3U\n#EXTINF:-1,Home\nhttp://192.168.1.10:8096/live.m3u8\n#EXTINF:-1,Local\nhttp://127.0.0.1:9973/live.m3u8\n#EXTINF:-1,Reserved\nhttp://169.254.169.254/metadata\n";
        let entries = parse_m3u_entries(text);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Home", "Local"]
        );
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
    fn parses_gutenberg_opds_entries_with_cover_links() {
        let xml = r#"<feed xmlns="http://www.w3.org/2005/Atom"><entry><id>https://www.gutenberg.org/ebooks/84</id><title> Frankenstein </title><link rel="http://opds-spec.org/image/thumbnail" type="image/jpeg" href="https://www.gutenberg.org/cache/epub/84/pg84.cover.small.jpg"/><link rel="http://opds-spec.org/image" type="image/jpeg" href="https://www.gutenberg.org/cache/epub/84/pg84.cover.medium.jpg"/><link rel="alternate" type="text/html" href="https://www.gutenberg.org/ebooks/84"/></entry><entry><id>https://www.gutenberg.org/ebooks/authors/1</id><title>Author</title></entry></feed>"#;
        let entries = parse_atom_entries(xml);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "Frankenstein");
        assert_eq!(
            entries[0].image.as_deref(),
            Some("https://www.gutenberg.org/cache/epub/84/pg84.cover.medium.jpg"),
            "封面原图必须优先于缩略图"
        );
        assert_eq!(entries[1].image, None, "没有封面 link 的条目保持空海报");
        assert_eq!(
            entries[0]
                .id
                .split("/ebooks/")
                .nth(1)
                .unwrap()
                .split(|ch: char| !ch.is_ascii_digit())
                .next(),
            Some("84")
        );
    }

    #[test]
    fn anilist_search_query_requests_the_cover_image_field() {
        assert!(ANILIST_SEARCH_QUERY.contains("coverImage"));
        for field in ["extraLarge", "large", "medium"] {
            assert!(
                ANILIST_SEARCH_QUERY.contains(field),
                "AniList 查询必须请求封面字段 {field}，否则搜索响应不会带回海报地址"
            );
        }
    }

    #[test]
    fn mangadex_fixture_maps_cover_relationship_to_a_bounded_address() {
        let id = "0f7a4a1e-0f2f-4a4f-9c4f-3c1b0f0a4b6c";
        let item = serde_json::json!({
            "id": id,
            "type": "manga",
            "attributes": {
                "title": { "en": "Synthetic Comic" },
                "description": { "en": "synthetic" },
                "year": 2024
            },
            "relationships": [
                { "id": "author-1", "type": "author", "attributes": {} },
                {
                    "id": "cover-1",
                    "type": "cover_art",
                    "attributes": { "fileName": "synthetic-cover.jpg", "volume": "1" }
                }
            ]
        });
        let draft = mangadex_draft(&item).expect("固定 fixture 必须映射成卡片");
        assert_eq!(draft.card.title, "Synthetic Comic");
        assert_eq!(draft.card.release_year, Some(2024));
        let poster = draft.poster.expect("cover_art 关系必须被提取");
        assert_eq!(poster.source_id(), "mangadex");
        assert_eq!(
            poster.url(),
            format!("https://uploads.mangadex.org/covers/{id}/synthetic-cover.jpg")
        );

        // 没有 cover_art 关系时仍返回卡片，只是没有海报。
        let without_cover = serde_json::json!({
            "id": id,
            "attributes": { "title": { "en": "Synthetic Comic" } },
            "relationships": []
        });
        let draft = mangadex_draft(&without_cover).expect("无封面条目仍必须可用");
        assert!(draft.poster.is_none());
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

    /// 记录调用参数的受控图片代理仓库替身：断言登记确实经过
    /// `ImageProxyRepository`，并且拿到的是 Provider 边界校验后的地址。
    #[derive(Default)]
    struct RecordingImageProxy {
        calls: Mutex<Vec<(String, String)>>,
        reject: bool,
    }

    impl RecordingImageProxy {
        fn calls(&self) -> Vec<(String, String)> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl ImageProxyRepository for RecordingImageProxy {
        async fn register(&self, source_id: &str, target_url: &str) -> Result<String, AppError> {
            self.calls
                .lock()
                .unwrap()
                .push((source_id.to_owned(), target_url.to_owned()));
            if self.reject {
                return Err(source_unavailable("测试用登记拒绝"));
            }
            Ok("0f7a4a1e-0f2f-4a4f-9c4f-3c1b0f0a4b6c".to_owned())
        }

        async fn resolve(&self, _id: &str) -> Result<Option<String>, AppError> {
            Ok(None)
        }
    }

    /// TVMaze fixture：上游字段 → 投影 → 登记的唯一合成数据源。
    fn synthetic_show() -> Value {
        serde_json::json!({
            "name": "Synthetic Show",
            "summary": "synthetic summary",
            "premiered": "2024-01-01",
            "image": {
                "medium": "https://static.tvmaze.com/uploads/images/medium_portrait/synthetic.jpg"
            }
        })
    }

    fn synthetic_draft() -> artwork::CardDraft {
        let show = synthetic_show();
        artwork::CardDraft::new(card(
            "tvmaze",
            "1",
            "Synthetic Show".to_owned(),
            Some("synthetic summary".to_owned()),
            Some(2024),
            (ContentCategory::Video, MediaTypeDto::Series),
            Some(ExternalIdProviderDto::Tvmaze),
        ))
        .with_poster(artwork::tvmaze_poster(&show))
    }

    #[tokio::test]
    async fn provider_artwork_is_registered_as_an_opaque_uri() {
        let proxy = Arc::new(RecordingImageProxy::default());
        let client = MetadataClient::with_image_proxy(proxy.clone()).expect("注入必须成功");

        let dto = artwork::register_card(client.image_proxy.as_deref(), synthetic_draft()).await;

        assert_eq!(
            dto.poster_uri.as_deref(),
            Some("haven://artwork/0f7a4a1e-0f2f-4a4f-9c4f-3c1b0f0a4b6c")
        );
        assert_eq!(dto.backdrop_uri, None);
        assert_eq!(dto.title, "Synthetic Show");
        let calls = proxy.calls();
        assert_eq!(calls.len(), 1, "每张卡片只登记一次，不额外发图片请求");
        assert_eq!(calls[0].0, "tvmaze");
        assert_eq!(
            calls[0].1,
            "https://static.tvmaze.com/uploads/images/medium_portrait/synthetic.jpg"
        );
    }

    #[tokio::test]
    async fn cancelled_metadata_does_not_register_unused_artwork() {
        let proxy = RecordingImageProxy::default();
        let cards = artwork::register_cards(Some(&proxy), vec![synthetic_draft()], &|| true).await;
        assert!(cards.is_empty());
        assert!(proxy.calls().is_empty());
    }

    #[tokio::test]
    async fn cancellation_between_cards_stops_artwork_registration() {
        let proxy = RecordingImageProxy::default();
        let cards = artwork::register_cards(
            Some(&proxy),
            vec![synthetic_draft(), synthetic_draft()],
            &|| !proxy.calls().is_empty(),
        )
        .await;
        assert!(cards.is_empty());
        assert_eq!(proxy.calls().len(), 1);
    }

    #[tokio::test]
    async fn wire_projection_never_carries_the_remote_artwork_url() {
        let proxy = Arc::new(RecordingImageProxy::default());
        let client = MetadataClient::with_image_proxy(proxy).expect("注入必须成功");
        let dto = artwork::register_card(client.image_proxy.as_deref(), synthetic_draft()).await;

        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("haven://artwork/"));
        assert!(!json.contains("tvmaze.com"), "远端地址不得进入 Wire");
        assert!(!json.contains("https://"));
    }

    #[tokio::test]
    async fn metadata_stays_usable_when_artwork_is_absent_or_rejected() {
        // 未注入 Repository：不登记，也不失败。
        let client = MetadataClient::new().expect("无 Artwork 的客户端必须可构造");
        let dto = artwork::register_card(client.image_proxy.as_deref(), synthetic_draft()).await;
        assert_eq!(dto.poster_uri, None);

        // Provider 没给图片：完全不登记。
        let recording = Arc::new(RecordingImageProxy::default());
        let client = MetadataClient::with_image_proxy(recording.clone()).expect("注入必须成功");
        let mut draft = synthetic_draft();
        draft.poster = None;
        let dto = artwork::register_card(client.image_proxy.as_deref(), draft).await;
        assert_eq!(dto.poster_uri, None);
        assert!(recording.calls().is_empty());

        // 登记被策略/存储拒绝：元数据仍然可用，只是没有海报。
        let rejecting = Arc::new(RecordingImageProxy {
            reject: true,
            ..Default::default()
        });
        let client = MetadataClient::with_image_proxy(rejecting.clone()).expect("注入必须成功");
        let dto = artwork::register_card(client.image_proxy.as_deref(), synthetic_draft()).await;
        assert_eq!(dto.poster_uri, None);
        assert_eq!(dto.title, "Synthetic Show");
        assert_eq!(dto.release_year, Some(2024));
        assert_eq!(rejecting.calls().len(), 1);

        // 上游地址不在 Provider 图片主机白名单：不会进入登记路径。
        let recording = Arc::new(RecordingImageProxy::default());
        let client = MetadataClient::with_image_proxy(recording.clone()).expect("注入必须成功");
        let hostile = serde_json::json!({
            "name": "Synthetic Show",
            "image": { "medium": "https://evil.example/synthetic.jpg" }
        });
        let draft = artwork::CardDraft::new(card(
            "tvmaze",
            "1",
            "Synthetic Show".to_owned(),
            None,
            None,
            (ContentCategory::Video, MediaTypeDto::Series),
            None,
        ))
        .with_poster(artwork::tvmaze_poster(&hostile));
        let dto = artwork::register_card(client.image_proxy.as_deref(), draft).await;
        assert_eq!(dto.poster_uri, None);
        assert!(recording.calls().is_empty());
    }
}
