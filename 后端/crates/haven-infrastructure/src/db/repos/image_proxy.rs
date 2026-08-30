//! Image Proxy Repository（Sqlite）：受控图片代理映射（migration 015）。
//!
//! - `register` 幂等：同 URL 返回同一稳定 UUID id（先查后插，UNIQUE 兜底并发）。
//! - `resolve` 只放行 http(s) 目标；库内脏数据一律拒绝。真正出站前由
//!   `ArtworkCache` 再执行来源 Host、DNS/IP、Redirect 和 signed-query 策略，
//!   因此登记本身不被当作 SSRF 防护。

use std::sync::Arc;

use async_trait::async_trait;
use haven_common::AppError;
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
        let normalized_host = validate_registration(source_id, target_url)?;
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
pub fn validate_registration(source_id: &str, target_url: &str) -> Result<String, AppError> {
    if source_id.trim().is_empty() || source_id.len() > 64 {
        return Err(haven_common::validation("图片来源标识无效"));
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
    if has_sensitive_query(&url) {
        return Err(haven_common::validation(
            "图片地址不能包含签名或 Secret 查询参数",
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| haven_common::validation("图片地址缺少主机"))?
        .to_ascii_lowercase();
    if source_id == "douban" && host != "doubanio.com" && !host.ends_with(".doubanio.com") {
        return Err(haven_common::validation("豆瓣图片主机不在允许列表"));
    }
    if host == "localhost" || is_private_literal_host(&host) {
        return Err(haven_common::validation("图片地址主机不在公共网络范围"));
    }
    Ok(host)
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

fn is_private_literal_host(host: &str) -> bool {
    let Ok(ip) = host.parse::<std::net::IpAddr>() else {
        return false;
    };
    is_private_ip(ip)
}

pub fn is_private_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.octets() == [169, 254, 169, 254]
                || ip.octets()[0] == 0
        }
        std::net::IpAddr::V6(ip) => {
            if let Some(ipv4) = ip.to_ipv4() {
                return is_private_ip(std::net::IpAddr::V4(ipv4));
            }
            ip.is_loopback()
                || ip.is_unspecified()
                || (ip.segments()[0] & 0xfe00) == 0xfc00
                || (ip.segments()[0] & 0xffc0) == 0xfe80
        }
    }
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
    fn registration_rejects_signed_and_cross_source_urls() {
        assert!(validate_registration("douban", "https://evil.example/p.jpg").is_err());
        assert!(
            validate_registration("douban", "https://img.doubanio.com/p.jpg?signature=secret")
                .is_err()
        );
        assert!(validate_registration("douban", "https://img.doubanio.com/p.jpg").is_ok());
        assert!(validate_registration("cms10", "http://127.0.0.1/p.jpg").is_err());
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
    fn private_ip_policy_covers_metadata_and_ipv6_ranges() {
        assert!(is_private_ip("169.254.169.254".parse().unwrap()));
        assert!(is_private_ip("10.0.0.1".parse().unwrap()));
        assert!(is_private_ip("::1".parse().unwrap()));
        assert!(is_private_ip("::ffff:127.0.0.1".parse().unwrap()));
        assert!(!is_private_ip("1.1.1.1".parse().unwrap()));
    }
}
