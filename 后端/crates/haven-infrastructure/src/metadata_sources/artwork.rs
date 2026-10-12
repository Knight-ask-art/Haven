//! 固定元数据 Provider 的 Artwork 提取与登记边界。
//!
//! 本模块只做三件事：
//! 1. 把 Provider 响应里真实存在的图片字段解析成**绝对 http(s) 地址**；
//! 2. 在 Provider 自己的图片主机白名单上校验地址（相对地址按 Provider 的固定
//!    图片基准解析，路径元素单独校验，例如 MangaDex 的封面 UUID 与 fileName）；
//! 3. 通过受控的 [`ImageProxyRepository`] 把地址登记成 opaque id。
//!
//! 两条边界必须同时成立：
//! - 这里不发图片网络请求。Provider 响应已经带回地址时，搜索阶段只登记，
//!   抓取留给渲染时的受控 Artwork 缓存。
//! - 远端 URL 永远不进入 Wire：`WorkCardDto` 只拿 `haven://artwork/<opaque>`，
//!   登记失败、地址缺失或地址不合法时卡片保持原样，只是没有 `posterUri`。

use haven_application::wire::WorkCardDto;
use haven_domain::contracts::ImageProxyRepository;
use serde_json::Value;

use crate::db::repos::image_proxy::{MAX_ARTWORK_TARGET_URL_LEN, has_sensitive_query};

/// 已校验的远端图片地址。只有 [`RemoteArtwork::resolve`] 能构造它，因此进入
/// 登记路径的地址一定已经过形状、主机白名单与长度校验。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RemoteArtwork {
    source_id: &'static str,
    url: String,
}

impl RemoteArtwork {
    /// 解析并校验一张 Provider 图片地址。
    ///
    /// `base` 是该 Provider 的固定图片基准（用于相对地址），`allowed_hosts`
    /// 是精确主机名，或以 `.` 开头的受控子域后缀（`".mzstatic.com"`）。
    fn resolve(
        source_id: &'static str,
        raw: &str,
        base: &str,
        allowed_hosts: &[&str],
    ) -> Option<Self> {
        let raw = raw.trim();
        if raw.is_empty()
            || raw.len() > MAX_ARTWORK_TARGET_URL_LEN
            || raw.contains('\\')
            || raw.chars().any(|ch| ch.is_whitespace() || ch.is_control())
        {
            return None;
        }
        let base = reqwest::Url::parse(base).ok()?;
        // 相对地址按 Provider 基准解析；绝对地址（含 `//host/...`）会解析成
        // 上游写死的主机，随后必须通过同一份主机白名单。
        let url = base.join(raw).ok()?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || has_sensitive_query(&url)
            || url.as_str().len() > MAX_ARTWORK_TARGET_URL_LEN
        {
            return None;
        }
        let host = url.host_str()?.to_ascii_lowercase();
        if !allowed_hosts
            .iter()
            .any(|pattern| host_matches(&host, pattern))
        {
            return None;
        }
        Some(Self {
            source_id,
            url: url.to_string(),
        })
    }

    pub(super) fn source_id(&self) -> &'static str {
        self.source_id
    }

    pub(super) fn url(&self) -> &str {
        &self.url
    }
}

fn host_matches(host: &str, pattern: &str) -> bool {
    match pattern.strip_prefix('.') {
        Some(suffix) => host
            .strip_suffix(suffix)
            .is_some_and(|prefix| prefix.len() > 1 && prefix.ends_with('.')),
        None => host == pattern,
    }
}

/// 一张搜索投影 + 该 Provider 响应里已校验、但尚未登记的图片地址。
pub(super) struct CardDraft {
    pub(super) card: WorkCardDto,
    pub(super) poster: Option<RemoteArtwork>,
}

impl CardDraft {
    pub(super) fn new(card: WorkCardDto) -> Self {
        Self { card, poster: None }
    }

    pub(super) fn with_poster(mut self, poster: Option<RemoteArtwork>) -> Self {
        self.poster = poster;
        self
    }
}

/// 登记草稿的 Artwork 并返回 Wire 卡片。
///
/// Artwork 是观感增强，不是搜索成功条件：没有注入 Repository、Provider 没给
/// 图片、或登记被策略/存储拒绝时都返回原始卡片。
pub(super) async fn register_card(
    proxy: Option<&dyn ImageProxyRepository>,
    draft: CardDraft,
) -> WorkCardDto {
    let CardDraft { mut card, poster } = draft;
    let (Some(poster), Some(proxy)) = (poster, proxy) else {
        return card;
    };
    if let Ok(id) = proxy.register(poster.source_id(), poster.url()).await
        && let Some(uri) = artwork_uri(&id)
    {
        card.poster_uri = Some(uri);
    }
    card
}

/// 图片登记发生在网络返回后；取消的查询不再新增未被消费的映射。
/// 每次异步登记前重新检查取消，保留现有 opaque 身份，不清理可能被资源引用的行。
pub(super) async fn register_cards(
    proxy: Option<&dyn ImageProxyRepository>,
    drafts: Vec<CardDraft>,
    is_cancelled: &(dyn Fn() -> bool + Send + Sync),
) -> Vec<WorkCardDto> {
    let mut cards = Vec::with_capacity(drafts.len());
    for draft in drafts {
        if is_cancelled() {
            return Vec::new();
        }
        cards.push(register_card(proxy, draft).await);
    }
    if is_cancelled() { Vec::new() } else { cards }
}

/// 受控 Artwork URI。opaque id 的字符集与 `mapper::work_card` 的
/// `controlled_artwork_uri` 以及前端的 `artworkRequestUri` 保持一致；
/// 形状不合法的 id 一律不进入 Wire。
fn artwork_uri(id: &str) -> Option<String> {
    let id = id.trim();
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return None;
    }
    Some(format!("haven://artwork/{id}"))
}

// ---------- Provider 图片字段提取 ----------
//
// 每个 Provider 只认识自己响应里的真实字段与自己的图片 CDN 主机；字段缺失或
// 主机不在白名单时返回 None（学术/无图记录本来就保持空海报）。

const TVMAZE_IMAGE_BASE: &str = "https://static.tvmaze.com/";
const TVMAZE_IMAGE_HOSTS: &[&str] = &["static.tvmaze.com"];

pub(super) fn tvmaze_poster(show: &Value) -> Option<RemoteArtwork> {
    let image = show.get("image")?;
    let raw = first_text(image, &["medium", "original"])?;
    RemoteArtwork::resolve("tvmaze", &raw, TVMAZE_IMAGE_BASE, TVMAZE_IMAGE_HOSTS)
}

const BANGUMI_IMAGE_BASE: &str = "https://lain.bgm.tv/";
const BANGUMI_IMAGE_HOSTS: &[&str] = &["lain.bgm.tv"];

pub(super) fn bangumi_poster(item: &Value) -> Option<RemoteArtwork> {
    let images = item.get("images")?;
    let raw = first_text(images, &["large", "common", "medium", "small", "grid"])?;
    RemoteArtwork::resolve("bangumi", &raw, BANGUMI_IMAGE_BASE, BANGUMI_IMAGE_HOSTS)
}

const ANILIST_IMAGE_BASE: &str = "https://s4.anilist.co/";
const ANILIST_IMAGE_HOSTS: &[&str] = &[
    "s4.anilist.co",
    "s1.anilist.co",
    "s2.anilist.co",
    "s3.anilist.co",
];

pub(super) fn anilist_poster(item: &Value) -> Option<RemoteArtwork> {
    let cover = item.get("coverImage")?;
    let raw = first_text(cover, &["extraLarge", "large", "medium"])?;
    RemoteArtwork::resolve("anilist", &raw, ANILIST_IMAGE_BASE, ANILIST_IMAGE_HOSTS)
}

const ITUNES_IMAGE_BASE: &str = "https://is1-ssl.mzstatic.com/";
const ITUNES_IMAGE_HOSTS: &[&str] = &[".mzstatic.com"];

pub(super) fn itunes_poster(item: &Value) -> Option<RemoteArtwork> {
    let raw = first_text(item, &["artworkUrl100", "artworkUrl60"])?;
    RemoteArtwork::resolve("itunes", &raw, ITUNES_IMAGE_BASE, ITUNES_IMAGE_HOSTS)
}

const MANGADEX_IMAGE_BASE: &str = "https://uploads.mangadex.org/";
const MANGADEX_IMAGE_HOSTS: &[&str] = &["uploads.mangadex.org"];

/// MangaDex 封面地址由 `manga id + cover_art.fileName` 组装：两个路径元素都来自
/// 上游 JSON，因此必须分别校验，不能把 `../`、分隔符或百分号编码带进路径。
pub(super) fn mangadex_poster(manga_id: &str, item: &Value) -> Option<RemoteArtwork> {
    if !is_uuid(manga_id) {
        return None;
    }
    let file_name = cover_file_name(item)?;
    RemoteArtwork::resolve(
        "mangadex",
        &format!("covers/{manga_id}/{file_name}"),
        MANGADEX_IMAGE_BASE,
        MANGADEX_IMAGE_HOSTS,
    )
}

fn cover_file_name(item: &Value) -> Option<String> {
    let name = item
        .get("relationships")?
        .as_array()?
        .iter()
        .find(|relation| relation.get("type").and_then(Value::as_str) == Some("cover_art"))?
        .get("attributes")?
        .get("fileName")?
        .as_str()?
        .trim();
    if name.is_empty() || name.len() > 128 || name.starts_with('.') || name.contains("..") {
        return None;
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return None;
    }
    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
    matches!(extension.as_str(), "jpg" | "jpeg" | "png" | "webp").then(|| name.to_owned())
}

fn is_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    bytes.iter().enumerate().all(|(index, byte)| {
        if matches!(index, 8 | 13 | 18 | 23) {
            *byte == b'-'
        } else {
            byte.is_ascii_hexdigit()
        }
    })
}

const GUTENBERG_IMAGE_BASE: &str = "https://www.gutenberg.org/";
const GUTENBERG_IMAGE_HOSTS: &[&str] = &["www.gutenberg.org", "gutenberg.org"];

/// Gutenberg OPDS 条目里的封面 `link`（`rel="http://opds-spec.org/image"`）。
pub(super) fn gutenberg_poster(href: &str) -> Option<RemoteArtwork> {
    RemoteArtwork::resolve(
        "gutenberg",
        href,
        GUTENBERG_IMAGE_BASE,
        GUTENBERG_IMAGE_HOSTS,
    )
}

/// 按优先级取第一个非空字符串字段，避免把 `null`/空串当作图片地址。
fn first_text(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider_artwork(fixture: &Value, source: &str) -> Option<RemoteArtwork> {
        match source {
            "tvmaze" => tvmaze_poster(fixture),
            "bangumi" => bangumi_poster(fixture),
            "anilist" => anilist_poster(fixture),
            "itunes" => itunes_poster(fixture),
            _ => None,
        }
    }

    #[test]
    fn fixed_provider_artwork_never_acquires_signed_query_rights() {
        for raw in [
            "https://static.tvmaze.com/poster.jpg?token=opaque",
            "https://static.tvmaze.com/poster.jpg?SIG=opaque",
            "https://static.tvmaze.com/poster.jpg?expires=1",
        ] {
            assert!(
                RemoteArtwork::resolve("tvmaze", raw, TVMAZE_IMAGE_BASE, TVMAZE_IMAGE_HOSTS)
                    .is_none()
            );
        }
        assert!(
            RemoteArtwork::resolve(
                "tvmaze",
                "https://static.tvmaze.com/poster.jpg?width=400",
                TVMAZE_IMAGE_BASE,
                TVMAZE_IMAGE_HOSTS,
            )
            .is_some()
        );
    }

    #[test]
    fn host_matching_is_exact_or_controlled_subdomain() {
        assert!(host_matches("is1-ssl.mzstatic.com", ".mzstatic.com"));
        assert!(!host_matches("mzstatic.com", ".mzstatic.com"));
        assert!(!host_matches("evil-mzstatic.com", ".mzstatic.com"));
        assert!(host_matches("static.tvmaze.com", "static.tvmaze.com"));
        assert!(!host_matches(
            "static.tvmaze.com.evil.example",
            "static.tvmaze.com"
        ));
    }

    #[test]
    fn extracts_artwork_fields_for_each_supported_provider() {
        let fixtures = [
            (
                "tvmaze",
                serde_json::json!({ "image": { "medium": "https://static.tvmaze.com/uploads/medium.jpg" } }),
                "https://static.tvmaze.com/uploads/medium.jpg",
            ),
            (
                "bangumi",
                serde_json::json!({ "images": { "large": "https://lain.bgm.tv/pic/cover/l/aa/bb/1.jpg" } }),
                "https://lain.bgm.tv/pic/cover/l/aa/bb/1.jpg",
            ),
            (
                "anilist",
                serde_json::json!({ "coverImage": { "extraLarge": "https://s4.anilist.co/file/anilistcdn/media/anime/cover/large/b1.jpg" } }),
                "https://s4.anilist.co/file/anilistcdn/media/anime/cover/large/b1.jpg",
            ),
            (
                "itunes",
                serde_json::json!({ "artworkUrl100": "https://is1-ssl.mzstatic.com/image/thumb/Podcasts1/v4/aa/bb/100x100bb.jpg" }),
                "https://is1-ssl.mzstatic.com/image/thumb/Podcasts1/v4/aa/bb/100x100bb.jpg",
            ),
        ];
        for (source, fixture, expected) in fixtures {
            let artwork = provider_artwork(&fixture, source)
                .unwrap_or_else(|| panic!("{source} 必须提取到海报地址"));
            assert_eq!(artwork.url(), expected, "{source}");
            assert_eq!(artwork.source_id(), source, "{source}");
        }
    }

    #[test]
    fn provider_artwork_is_empty_when_fields_are_missing_or_null() {
        assert!(tvmaze_poster(&serde_json::json!({ "image": serde_json::Value::Null })).is_none());
        assert!(tvmaze_poster(&serde_json::json!({ "name": "no image" })).is_none());
        assert!(bangumi_poster(&serde_json::json!({ "images": {} })).is_none());
        assert!(anilist_poster(&serde_json::json!({ "coverImage": { "medium": "  " } })).is_none());
        assert!(itunes_poster(&serde_json::json!({ "artworkUrl100": null })).is_none());
    }

    #[test]
    fn provider_artwork_rejects_hosts_outside_the_provider_cdn() {
        // 上游被改写或字段被塞入第三方地址时，必须留在 Provider 自己的主机上。
        assert!(
            tvmaze_poster(
                &serde_json::json!({ "image": { "medium": "https://evil.example/x.jpg" } })
            )
            .is_none()
        );
        assert!(
            anilist_poster(&serde_json::json!({ "coverImage": { "large": "https://s4.anilist.co.evil.example/x.jpg" } }))
                .is_none()
        );
        assert!(
            itunes_poster(&serde_json::json!({ "artworkUrl100": "http://evil.example/x.jpg" }))
                .is_none()
        );
        assert!(gutenberg_poster("https://evil.example/cover.jpg").is_none());
        // userinfo、fragment 与超长地址即使主机正确也不放行。
        assert!(
            bangumi_poster(
                &serde_json::json!({ "images": { "large": "https://user:pass@lain.bgm.tv/x.jpg" } })
            )
            .is_none()
        );
        assert!(
            bangumi_poster(
                &serde_json::json!({ "images": { "large": "https://lain.bgm.tv/x.jpg#frag" } })
            )
            .is_none()
        );
        let oversized = format!(
            "https://lain.bgm.tv/{}",
            "a".repeat(MAX_ARTWORK_TARGET_URL_LEN)
        );
        assert!(bangumi_poster(&serde_json::json!({ "images": { "large": oversized } })).is_none());
    }

    #[test]
    fn provider_artwork_resolves_relative_addresses_against_the_provider_base() {
        let artwork = gutenberg_poster("/cache/epub/84/pg84.cover.medium.jpg")
            .expect("相对封面地址必须按 Gutenberg 基准解析");
        assert_eq!(
            artwork.url(),
            "https://www.gutenberg.org/cache/epub/84/pg84.cover.medium.jpg"
        );
    }

    #[test]
    fn mangadex_cover_boundary_validates_uuid_and_file_name() {
        let id = "0f7a4a1e-0f2f-4a4f-9c4f-3c1b0f0a4b6c";
        let item = serde_json::json!({
            "relationships": [
                { "id": "rel", "type": "author", "attributes": { "fileName": "ignored.jpg" } },
                { "id": "cover", "type": "cover_art", "attributes": { "fileName": "a1b2c3.jpg" } }
            ]
        });
        let artwork = mangadex_poster(id, &item).expect("合法封面必须被接受");
        assert_eq!(
            artwork.url(),
            format!("https://uploads.mangadex.org/covers/{id}/a1b2c3.jpg")
        );
        assert_eq!(artwork.source_id(), "mangadex");

        for file_name in [
            "../../../etc/passwd",
            "sub/dir.jpg",
            "cover.jpg%2f..%2fsecret",
            "cover.svg",
            "cover",
            "..jpg",
            ".hidden.jpg",
            "cover name.jpg",
        ] {
            let item = serde_json::json!({
                "relationships": [
                    { "type": "cover_art", "attributes": { "fileName": file_name } }
                ]
            });
            assert!(
                mangadex_poster(id, &item).is_none(),
                "不安全的封面文件名必须拒绝: {file_name}"
            );
        }

        // 非 UUID 的 manga id 不得进入路径。
        assert!(mangadex_poster("../etc", &item).is_none());
        assert!(mangadex_poster("", &item).is_none());
        // 缺少 cover_art 关系（例如上游没有封面）时保持空海报。
        assert!(mangadex_poster(id, &serde_json::json!({ "relationships": [] })).is_none());
        assert!(
            mangadex_poster(
                id,
                &serde_json::json!({
                    "relationships": [{ "type": "cover_art", "attributes": {} }]
                })
            )
            .is_none()
        );
    }

    #[test]
    fn artwork_uri_accepts_only_opaque_ids() {
        assert_eq!(
            artwork_uri("0f7a4a1e-0f2f-4a4f-9c4f-3c1b0f0a4b6c").as_deref(),
            Some("haven://artwork/0f7a4a1e-0f2f-4a4f-9c4f-3c1b0f0a4b6c")
        );
        assert!(artwork_uri("").is_none());
        assert!(artwork_uri("../etc").is_none());
        assert!(artwork_uri("a b").is_none());
        assert!(artwork_uri("https://evil.example/x.jpg").is_none());
        assert!(artwork_uri(&"a".repeat(65)).is_none());
    }
}
