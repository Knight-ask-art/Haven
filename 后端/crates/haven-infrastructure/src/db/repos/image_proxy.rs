//! Image Proxy Repository（Sqlite）：受控图片代理映射（migration 015）。
//!
//! - `register` 幂等：同 URL 返回同一稳定 UUID id（先查后插，UNIQUE 兜底并发）。
//! - `resolve` 只放行 http(s) 目标；库内脏数据一律拒绝。真正出站前由
//!   `ArtworkCache` 再执行来源 Host、DNS/IP、Redirect 和 signed-query 策略，
//!   因此登记本身不被当作 SSRF 防护。
//! - 显式登记的映射可以携带 CDN 签名查询串（见 [`allows_signed_query`]）；
//!   旧记录补齐与豆瓣自动来源策略仍然拒绝，签名串只存在于映射表与出站请求。
//! - 登记侧的 URL 形状与主机判定与共享出站策略同源（`haven_common::network`）：
//!   畸形 authority 与不可路由的字面地址在写入映射表之前就拒绝。端口策略不在
//!   这里收窄，公共非零端口保持既有行为。

use std::sync::Arc;

use async_trait::async_trait;
use haven_common::AppError;
use haven_common::network::{is_publicly_routable, validate_host};
use haven_domain::contracts::ImageProxyRepository;
use rusqlite::OptionalExtension;

/// 受控 Artwork 的登记信息。数据库 Row 不穿过 IPC；该结构只供
/// Infrastructure 内部的 Artwork Cache 使用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageProxyRecord {
    pub id: String,
    pub source_id: Option<String>,
    pub target_url: String,
    pub normalized_host: Option<String>,
}

/// Artwork 目标地址的硬上限。签名 CDN 地址必然带查询串，但仍然只允许落进
/// 受控的后端登记映射；长度上限防止任意长字符串进入映射表与缓存索引。
pub const MAX_ARTWORK_TARGET_URL_LEN: usize = 2_048;

use crate::db::Db;
use crate::db::repos::map_db_error;

pub struct SqliteImageProxyRepository {
    db: Arc<Db>,
}

impl SqliteImageProxyRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ImageProxyRepository for SqliteImageProxyRepository {
    async fn register(&self, source_id: &str, target_url: &str) -> Result<String, AppError> {
        let normalized_host = validate_registration(source_id, target_url)?;
        let conn = self.db.lock();
        let existing: Option<(String, Option<String>, Option<String>)> = conn
            .query_row(
                "SELECT id, source_id, normalized_host FROM image_proxy WHERE target_url = ?1",
                rusqlite::params![target_url],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(map_db_error("查询图片代理失败"))?;
        if let Some((id, existing_source_id, existing_host)) = existing {
            // 021 之前的记录只有 target_url。新的显式 register 调用已经
            // 通过来源策略校验，因此可以安全地补齐缺失的策略元数据，
            // 同时保留既有 opaque id 和已经登记的来源事实。
            if existing_source_id.is_none() {
                conn.execute(
                    "UPDATE image_proxy
                     SET source_id = ?2, normalized_host = ?3
                     WHERE id = ?1 AND source_id IS NULL",
                    rusqlite::params![id, source_id, normalized_host],
                )
                .map_err(map_db_error("补齐图片来源策略失败"))?;
            } else if existing_host.is_none() {
                conn.execute(
                    "UPDATE image_proxy SET normalized_host = ?2
                     WHERE id = ?1 AND normalized_host IS NULL",
                    rusqlite::params![id, normalized_host],
                )
                .map_err(map_db_error("补齐图片主机策略失败"))?;
            }
            return Ok(id);
        }
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO image_proxy
                (id, target_url, created_at, source_id, normalized_host, last_fetch_status)
             VALUES (?1, ?2, ?3, ?4, ?5, 'unknown')",
            rusqlite::params![
                id,
                target_url,
                haven_common::UtcMillis::now().0,
                source_id,
                normalized_host,
            ],
        )
        .map_err(map_db_error("注册图片代理失败"))?;
        Ok(id)
    }

    async fn resolve(&self, id: &str) -> Result<Option<String>, AppError> {
        let conn = self.db.lock();
        let url: Option<String> = conn
            .query_row(
                "SELECT target_url FROM image_proxy WHERE id = ?1",
                rusqlite::params![id],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_db_error("解析图片代理失败"))?;
        drop(conn);
        // 只放行 http(s)；脏数据视为未注册。
        Ok(url.filter(|value| value.starts_with("http://") || value.starts_with("https://")))
    }
}

impl SqliteImageProxyRepository {
    /// 解析登记信息，旧版本中 `source_id IS NULL` 的行仍可被读取，但
    /// Artwork Cache 不会为它们重新发起未知来源的网络请求。
    pub fn resolve_record(&self, id: &str) -> Result<Option<ImageProxyRecord>, AppError> {
        let conn = self.db.lock();
        conn.query_row(
            "SELECT id, source_id, target_url, normalized_host
             FROM image_proxy WHERE id = ?1",
            rusqlite::params![id],
            |row| {
                Ok(ImageProxyRecord {
                    id: row.get(0)?,
                    source_id: row.get(1)?,
                    target_url: row.get(2)?,
                    normalized_host: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(map_db_error("解析图片来源失败"))
    }

    pub fn mark_fetch_state(
        &self,
        id: &str,
        status: &str,
        error_code: Option<&str>,
    ) -> Result<(), AppError> {
        let conn = self.db.lock();
        conn.execute(
            "UPDATE image_proxy
             SET last_fetch_status = ?2,
                 last_fetched_at = ?3,
                 last_error_code = ?4
             WHERE id = ?1",
            rusqlite::params![id, status, haven_common::UtcMillis::now().0, error_code],
        )
        .map_err(map_db_error("更新图片抓取状态失败"))?;
        Ok(())
    }

    /// 为 021 之前的已知旧来源补齐策略元数据。
    ///
    /// 调用方必须先通过 [`legacy_source_for_url`] 得到 source id；本方法
    /// 仍会再次执行 URL 形状和来源策略校验，避免把这个兼容入口变成
    /// 任意 URL 的登记旁路。已有非空 source_id 永远不会被覆盖。
    pub fn backfill_known_source_if_missing(
        &self,
        id: &str,
        source_id: &str,
        target_url: &str,
    ) -> Result<(), AppError> {
        // 旧记录补齐是按主机推断来源的兼容路径，不是显式登记：签名查询串在这
        // 里必须继续 fail closed，不能因为换了一个调用方就放宽。
        let normalized_host = validate_artwork_target(source_id, target_url, false)?;
        let conn = self.db.lock();
        conn.execute(
            "UPDATE image_proxy
             SET source_id = ?2, normalized_host = ?3
             WHERE id = ?1 AND source_id IS NULL",
            rusqlite::params![id, source_id, normalized_host],
        )
        .map_err(map_db_error("补齐图片来源策略失败"))?;
        Ok(())
    }
}

/// 注册阶段只做不依赖 DNS 的 URL 形状和来源域校验；实际连接前还必须
/// 重新解析 DNS 并检查 IP，避免把“已登记”误当作 SSRF 防护。
///
/// 允许签名类查询参数：后端显式登记的映射（来源 + 主机双重作用域，写入
/// `source_id`/`normalized_host`）可以携带 CDN 签名串，登记表本身不外泄。
pub fn validate_registration(source_id: &str, target_url: &str) -> Result<String, AppError> {
    validate_artwork_target(source_id, target_url, allows_signed_query(source_id))
}

/// 显式登记的来源是否接受签名类查询参数。
///
/// 仅用户配置的 CMS/OPDS/Feed 图片路径接受签名串。固定元数据来源、未知来源
/// 与未来新增来源默认拒绝，不能仅因为调用了 `register` 就获得此能力。
/// 旧记录补齐（[`SqliteImageProxyRepository::backfill_known_source_if_missing`]）
/// 同样不放行；真正出站仍须验证已登记主机及公共 DNS 地址。
pub fn allows_signed_query(source_id: &str) -> bool {
    if matches!(source_id, "cms10" | "opds") {
        return true;
    }
    // 只接受注册表生成的 OPDS/Feed 身份形状；其它 custom 家族（例如 TVBox）
    // 不继承图片权限。它不替代调用方的来源归属校验或请求时的网络检查。
    ["custom_feed_", "custom_"].iter().any(|prefix| {
        source_id.strip_prefix(prefix).is_some_and(|id| {
            id.len() == 12
                && id
                    .bytes()
                    .all(|ch| ch.is_ascii_digit() || (b'a'..=b'f').contains(&ch))
        })
    })
}

fn validate_artwork_target(
    source_id: &str,
    target_url: &str,
    allow_signed_query: bool,
) -> Result<String, AppError> {
    if source_id.trim().is_empty() || source_id.len() > 64 {
        return Err(haven_common::validation("图片来源标识无效"));
    }
    if target_url.len() > MAX_ARTWORK_TARGET_URL_LEN {
        return Err(haven_common::validation("图片地址超出长度上限"));
    }
    // 归一化之前的原始形状：WHATWG 解析会把反斜杠当成路径分隔符，也会把
    // `https:///path` 的空 authority 重新解释成主机。授权边界必须看调用方
    // 真正写下（以及将写进映射表）的文本，而不是“解析之后的样子”。
    if !has_safe_authority_shape(target_url) {
        return Err(haven_common::validation(
            "图片代理只接受合法 http/https 地址",
        ));
    }
    let url = target_url
        .parse::<reqwest::Url>()
        .map_err(|_| haven_common::validation("图片代理只接受合法 http/https 地址"))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(haven_common::validation("图片地址包含不允许的 URL 组件"));
    }
    if !allow_signed_query && has_sensitive_query(&url) {
        return Err(haven_common::validation(
            "图片地址不能包含签名或 Secret 查询参数",
        ));
    }
    let raw_host = url
        .host_str()
        .ok_or_else(|| haven_common::validation("图片地址缺少主机"))?;
    // 主机判定与共享出站策略同一份：localhost、单标签主机、`.local`/`.internal`
    // 后缀都在这里 fail closed。端口策略刻意不在这里收窄——登记侧继续接受
    // 此前已允许的公共非零端口，端口兼容策略是另一项独立任务。
    let host = validate_host(raw_host)
        .map_err(|_| haven_common::validation("图片地址主机不在公共网络范围"))?;
    // 字面地址是连接目标本身，必须直接按公共路由判定；不能依赖来源登记，
    // 也不能靠 Fake-IP 分类器获得豁免。
    if let Ok(ip) = host.parse::<std::net::IpAddr>()
        && !is_publicly_routable(ip)
    {
        return Err(haven_common::validation("图片地址主机不在公共网络范围"));
    }
    if source_id == "douban" && host != "doubanio.com" && !host.ends_with(".doubanio.com") {
        return Err(haven_common::validation("豆瓣图片主机不在允许列表"));
    }
    Ok(host)
}

/// 原始文本层面的 authority 形状检查。
///
/// `url::Url` 会规范化若干畸形写法（空端口等同于没有端口、反斜杠等同于 `/`、
/// 空 authority 会被当成主机），这些形式一旦写进映射表，就会与调用方看到的
/// 文本含义不一致。这里只拒绝明确畸形的写法，不引入任何新的端口或主机策略。
fn has_safe_authority_shape(raw: &str) -> bool {
    if raw.chars().any(char::is_control) || raw.chars().any(char::is_whitespace) {
        return false;
    }
    if raw.contains('\\') {
        return false;
    }
    let Some((scheme, rest)) = raw.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    // `://` 之后必须紧跟非空的 authority；`https:///path` 这类写法在解析后
    // 会变成另一种目标。
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() {
        return false;
    }
    // `https://host:/path` 的显式空端口在解析后会被当成“没有端口”。
    !authority.ends_with(':')
}

/// 返回 021 之前能够从精确 Host 推断出的来源策略。
///
/// 这是兼容桥接的窄 allowlist，不是通用 URL 分类器。未知 Host、userinfo、
/// fragment 或 signed query 均返回 None，并继续由 Artwork Cache fail closed。
pub fn legacy_source_for_url(target_url: &str) -> Option<&'static str> {
    let url = target_url.parse::<reqwest::Url>().ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.port().is_some()
        || has_sensitive_query(&url)
    {
        return None;
    }
    match url.host_str()?.to_ascii_lowercase().as_str() {
        "img.picbf.com" => Some("cms10"),
        "www.gutenberg.org" | "gutenberg.org" => Some("opds"),
        _ => None,
    }
}

/// 地址是否带签名/凭据类查询参数。
///
/// 这是拒绝判定，不是分类器：显式登记的后端映射允许签名 CDN 地址（见
/// [`allows_signed_query`]），只有旧记录推断与自动授权来源用它 fail closed。
pub fn has_sensitive_query(url: &reqwest::Url) -> bool {
    const SENSITIVE: &[&str] = &[
        "token",
        "sig",
        "signature",
        "secret",
        "key",
        "auth",
        "expires",
        "expiry",
    ];
    url.query_pairs().any(|(key, _)| {
        let key = key.to_ascii_lowercase();
        SENSITIVE
            .iter()
            .any(|item| key == *item || key.contains(item))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn setup() -> (SqliteImageProxyRepository, Arc<Db>) {
        let db = Arc::new(Db::open_in_memory().unwrap());
        (SqliteImageProxyRepository::new(db.clone()), db)
    }

    #[tokio::test]
    async fn register_is_idempotent_and_resolve_roundtrips() {
        let (repo, _db) = setup();
        let id1 = repo
            .register("test", "https://img.example.com/p.jpg")
            .await
            .unwrap();
        let id2 = repo
            .register("test", "https://img.example.com/p.jpg")
            .await
            .unwrap();
        assert_eq!(id1, id2, "同 URL 必须返回同一稳定 id");
        assert_eq!(id1.len(), 36);

        let resolved = repo.resolve(&id1).await.unwrap();
        assert_eq!(resolved.as_deref(), Some("https://img.example.com/p.jpg"));

        assert_eq!(
            repo.resolve(&uuid::Uuid::new_v4().to_string())
                .await
                .unwrap(),
            None,
            "未注册 id 解析为 None"
        );
    }

    #[tokio::test]
    async fn register_backfills_policy_for_legacy_row_without_changing_id() {
        let (repo, db) = setup();
        let legacy_id = "legacy-artwork";
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO image_proxy (id, target_url, created_at)
                 VALUES (?1, ?2, 0)",
                rusqlite::params![legacy_id, "https://img.picbf.com/poster.jpg"],
            )
            .unwrap();
        }

        let id = repo
            .register("cms10", "https://img.picbf.com/poster.jpg")
            .await
            .unwrap();
        assert_eq!(id, legacy_id, "旧 Artwork 身份必须稳定");

        let record = repo.resolve_record(legacy_id).unwrap().unwrap();
        assert_eq!(record.source_id.as_deref(), Some("cms10"));
        assert_eq!(record.normalized_host.as_deref(), Some("img.picbf.com"));
    }

    #[tokio::test]
    async fn register_does_not_override_existing_source_policy() {
        let (repo, db) = setup();
        let id = repo
            .register("cms10", "https://img.picbf.com/poster.jpg")
            .await
            .unwrap();

        let same_id = repo
            .register("opds", "https://img.picbf.com/poster.jpg")
            .await
            .unwrap();
        assert_eq!(same_id, id);
        let record = repo.resolve_record(&id).unwrap().unwrap();
        assert_eq!(record.source_id.as_deref(), Some("cms10"));

        let conn = db.lock();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM image_proxy", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn non_http_targets_are_rejected_and_dirty_rows_never_served() {
        let (repo, db) = setup();
        assert!(
            repo.register("test", "file:///etc/passwd").await.is_err(),
            "非 http(s) 注册必须拒绝"
        );

        // 模拟库内脏数据：绕过端口直接 SQL 写入非 http 目标。
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO image_proxy (id, target_url, created_at) VALUES ('dirty', 'javascript:alert(1)', 0)",
                [],
            )
            .unwrap();
        }
        assert_eq!(repo.resolve("dirty").await.unwrap(), None, "脏行不得被代理");
    }

    #[test]
    fn registration_rejects_cross_source_and_private_targets() {
        assert!(validate_registration("douban", "https://evil.example/p.jpg").is_err());
        assert!(validate_registration("douban", "https://img.doubanio.com/p.jpg").is_ok());
        assert!(validate_registration("cms10", "http://127.0.0.1/p.jpg").is_err());
        assert!(validate_registration("cms10", "file:///etc/passwd").is_err());
        assert!(validate_registration("cms10", "https://user:pass@img.example.com/p.jpg").is_err());
        assert!(validate_registration("cms10", "https://img.example.com/p.jpg#frag").is_err());
        assert!(validate_registration("", "https://img.example.com/p.jpg").is_err());
    }

    #[test]
    fn signed_query_is_accepted_only_for_explicitly_registered_sources() {
        // 后端显式登记的映射可以携带 CDN 签名串：来源 + 主机作用域仍然成立。
        assert!(
            validate_registration(
                "custom_0123456789ab",
                "https://img.example.com/p.jpg?token=opaque&expires=1&sig=opaque"
            )
            .is_ok()
        );
        assert!(validate_registration("cms10", "https://img.picbf.com/p.jpg?key=opaque").is_ok());
        assert!(
            validate_registration(
                "custom_feed_0123456789ab",
                "https://img.example.com/p.jpg?sig=opaque"
            )
            .is_ok()
        );
        // 固定来源、未知来源和其它 custom 家族不因登记而获得签名图片权限。
        for source in [
            "douban",
            "tvmaze",
            "anilist",
            "bangumi",
            "itunes",
            "gutenberg",
            "mangadex",
            "future",
            "custom_tvbox_0123456789ab",
            "custom_not-an-id",
        ] {
            assert!(!allows_signed_query(source));
            assert!(
                validate_registration(source, "https://img.example.com/p.jpg?signature=opaque")
                    .is_err()
            );
        }
    }

    #[test]
    fn registration_bounds_the_target_url_length() {
        let oversized = format!(
            "https://img.example.com/p.jpg?token={}",
            "a".repeat(MAX_ARTWORK_TARGET_URL_LEN)
        );
        assert!(validate_registration("cms10", &oversized).is_err());
        let allowed = format!("https://img.example.com/p.jpg?token={}", "a".repeat(1_024));
        assert!(validate_registration("cms10", &allowed).is_ok());
    }

    #[test]
    fn registration_errors_never_echo_the_rejected_target() {
        // 被拒绝的签名地址只留下稳定错误，不得把 fixture 查询串或主机带进
        // 错误文案（错误会出现在日志与 IPC 结果里）。
        let secret = "fixture-signature-opaque";
        let error = validate_registration(
            "douban",
            &format!("https://img.doubanio.com/p.jpg?signature={secret}"),
        )
        .unwrap_err();
        let rendered = format!(
            "{} {} {error} {error:?}",
            error.code().as_str(),
            error.user_message()
        );
        assert!(!rendered.contains(secret), "错误文案不得包含查询串");
        assert!(
            !rendered.contains("doubanio.com"),
            "错误文案不得包含目标主机"
        );
    }

    #[tokio::test]
    async fn legacy_backfill_never_accepts_signed_query() {
        let (repo, db) = setup();
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO image_proxy (id, target_url, created_at)
                 VALUES ('legacy-signed', ?1, 0)",
                rusqlite::params!["https://img.picbf.com/poster.jpg?token=opaque"],
            )
            .unwrap();
        }

        assert!(
            repo.backfill_known_source_if_missing(
                "legacy-signed",
                "cms10",
                "https://img.picbf.com/poster.jpg?token=opaque"
            )
            .is_err(),
            "旧记录补齐不得为签名地址补出来源策略"
        );
        let record = repo.resolve_record("legacy-signed").unwrap().unwrap();
        assert_eq!(record.source_id, None, "补齐失败必须保持 fail closed");
        assert_eq!(record.normalized_host, None);
    }

    #[test]
    fn legacy_source_inference_is_exact_and_fail_closed() {
        assert_eq!(
            legacy_source_for_url("https://img.picbf.com/poster.jpg"),
            Some("cms10")
        );
        assert_eq!(
            legacy_source_for_url("https://WWW.GUTENBERG.ORG/book.png"),
            Some("opds")
        );
        assert_eq!(
            legacy_source_for_url("https://img.picbf.com.evil.example/poster.jpg"),
            None
        );
        assert_eq!(
            legacy_source_for_url("https://img.picbf.com/poster.jpg?signature=old"),
            None
        );
        assert_eq!(
            legacy_source_for_url("https://img.picbf.com:8443/poster.jpg"),
            None
        );
    }

    #[test]
    fn registration_rejects_addresses_outside_the_public_ranges() {
        // 与共享出站策略同一份判定：云元数据、CGNAT、基准测试/文档网段、
        // 保留段与 IPv6 ULA/link-local 都不能靠“登记”获得网络授权。
        for raw in [
            "http://169.254.169.254/latest/meta-data",
            "http://10.0.0.1/p.jpg",
            "http://127.0.0.1/p.jpg",
            "http://100.100.100.200/p.jpg",
            "http://198.18.0.1/p.jpg",
            "http://192.0.0.9/p.jpg",
            "http://[::1]/p.jpg",
            "http://[fc00::1]/p.jpg",
            "http://[fe80::1]/p.jpg",
            "http://[2001:db8::1]/p.jpg",
        ] {
            assert!(
                validate_registration("cms10", raw).is_err(),
                "不得登记非公网地址: {raw}"
            );
        }
        assert!(validate_registration("cms10", "https://img.example.com/p.jpg").is_ok());
        assert!(validate_registration("cms10", "https://1.1.1.1/p.jpg").is_ok());
        assert!(is_publicly_routable("1.1.1.1".parse().unwrap()));
        assert!(!is_publicly_routable("100.100.100.200".parse().unwrap()));
    }

    #[test]
    fn registration_rejects_malformed_authority_before_normalization() {
        // 反斜杠、控制字符与空白会在 WHATWG 归一化里改变 authority 的含义；
        // 空 authority 与显式空端口也会被解析成另一种目标。全部在解析前拒绝。
        for raw in [
            "https://img.example.com\\evil/p.jpg",
            "https://img.example.com/p.jpg\n",
            "https://img.example.com/p .jpg",
            "https://img.example.com\r\nX-Injected: 1/p.jpg",
            "https:///p.jpg",
            "https://img.example.com:/p.jpg",
        ] {
            assert!(
                validate_registration("cms10", raw).is_err(),
                "不得登记畸形地址: {raw:?}"
            );
        }
        // 端口兼容：登记侧继续接受此前已允许的公网非零端口。
        for raw in [
            "https://img.example.com/p.jpg",
            "https://img.example.com:8443/p.jpg",
            "https://img.example.com:9200/p.jpg",
            "http://img.example.com:8080/p.jpg?token=opaque",
        ] {
            assert!(
                validate_registration("cms10", raw).is_ok(),
                "不得回归已允许的公网端口: {raw}"
            );
        }
    }
}
