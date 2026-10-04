//! 来源原始配置缓存的 SQLite Repository（迁移 051）。
//!
//! 边界：
//! - 只读写 `source_config_cache`。没有 URL 列、没有 header 列、没有凭据列：端点与
//!   凭据引用属于来源身份，仍在 `settings` 域；缓存只认 `source_id`。
//! - `put` 是幂等覆盖：一源一行，写进去的就是该来源最后一次成功获取的原文。
//! - 「正文不能为空」由表上的 CHECK 约束在执行层保证，读取端因此不需要再做一次
//!   不可测的降级判断；空正文根本进不了库。
//! - 错误文案固定，不含配置原文、端点或任何配置值。

use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::OptionalExtension;

use haven_application::services::source_config_cache::{CachedSourceConfig, SourceConfigCache};
use haven_common::{AppError, UtcMillis};

use crate::db::Db;
use crate::db::repos::map_db_error;

pub struct SqliteSourceConfigCache {
    db: Arc<Db>,
}

impl SqliteSourceConfigCache {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl SourceConfigCache for SqliteSourceConfigCache {
    async fn put(
        &self,
        source_id: &str,
        body: &[u8],
        fetched_at: UtcMillis,
    ) -> Result<(), AppError> {
        let conn = self.db.lock();
        conn.execute(
            "INSERT INTO source_config_cache (source_id, body, fetched_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(source_id) DO UPDATE SET
                 body = excluded.body,
                 fetched_at = excluded.fetched_at",
            rusqlite::params![source_id, body, fetched_at.0],
        )
        .map_err(map_db_error("写入来源配置缓存失败"))?;
        Ok(())
    }

    async fn get(&self, source_id: &str) -> Result<Option<CachedSourceConfig>, AppError> {
        let conn = self.db.lock();
        let stored = conn
            .query_row(
                "SELECT body, fetched_at FROM source_config_cache WHERE source_id = ?1",
                rusqlite::params![source_id],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>("body")?,
                        row.get::<_, i64>("fetched_at")?,
                    ))
                },
            )
            .optional()
            .map_err(map_db_error("查询来源配置缓存失败"))?;
        Ok(stored.map(|(body, fetched_at)| CachedSourceConfig::new(body, UtcMillis(fetched_at))))
    }

    async fn delete(&self, source_id: &str) -> Result<bool, AppError> {
        let conn = self.db.lock();
        let affected = conn
            .execute(
                "DELETE FROM source_config_cache WHERE source_id = ?1",
                rusqlite::params![source_id],
            )
            .map_err(map_db_error("删除来源配置缓存失败"))?;
        Ok(affected > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> SqliteSourceConfigCache {
        let db = Arc::new(Db::open_in_memory().expect("内存库必须可迁移"));
        SqliteSourceConfigCache::new(db)
    }

    #[tokio::test]
    async fn missing_entry_is_none_and_delete_reports_nothing_removed() {
        let repo = repo();
        assert!(repo.get("custom_tvbox_0001").await.unwrap().is_none());
        assert!(!repo.delete("custom_tvbox_0001").await.unwrap());
    }

    #[tokio::test]
    async fn put_is_an_idempotent_overwrite_of_the_same_row() {
        let repo = repo();
        repo.put("custom_tvbox_0001", b"{\"sites\":[]}", UtcMillis(1_000))
            .await
            .unwrap();
        repo.put("custom_tvbox_0001", b"{\"sites\":[1]}", UtcMillis(2_000))
            .await
            .unwrap();

        let cached = repo
            .get("custom_tvbox_0001")
            .await
            .unwrap()
            .expect("写入后必须可读回");
        assert_eq!(cached.body(), b"{\"sites\":[1]}");
        assert_eq!(cached.fetched_at(), UtcMillis(2_000));
        assert!(repo.delete("custom_tvbox_0001").await.unwrap());
        assert!(repo.get("custom_tvbox_0001").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn entries_are_addressed_by_source_id_only() {
        let repo = repo();
        repo.put("custom_tvbox_a", b"{}", UtcMillis(1))
            .await
            .unwrap();
        repo.put("custom_tvbox_b", b"[]", UtcMillis(2))
            .await
            .unwrap();
        assert_eq!(
            repo.get("custom_tvbox_a").await.unwrap().unwrap().body(),
            b"{}"
        );
        assert_eq!(
            repo.get("custom_tvbox_b").await.unwrap().unwrap().body(),
            b"[]"
        );
        assert!(repo.delete("custom_tvbox_a").await.unwrap());
        assert!(repo.get("custom_tvbox_b").await.unwrap().is_some());
    }

    /// 空正文进不了库：表上的 CHECK 是执行层守卫，读取端因此不需要再猜一次。
    #[tokio::test]
    async fn an_empty_body_is_rejected_by_the_schema() {
        let repo = repo();
        repo.put("custom_tvbox_0001", b"{\"sites\":[]}", UtcMillis(500))
            .await
            .unwrap();
        let error = repo
            .put("custom_tvbox_0001", b"", UtcMillis(1_000))
            .await
            .expect_err("空正文必须被拒绝");
        assert_eq!(error.code().as_str(), "DATABASE_ERROR");
        let cached = repo
            .get("custom_tvbox_0001")
            .await
            .unwrap()
            .expect("失败的 upsert 必须保留旧的 last-known-good");
        assert_eq!(cached.body(), b"{\"sites\":[]}");
        assert_eq!(cached.fetched_at(), UtcMillis(500));
    }

    /// 重启保真：缓存落在磁盘上，进程重新打开数据库之后读到的是同一件事。
    #[tokio::test]
    async fn cached_config_survives_reopening_the_database() {
        let directory = tempfile::tempdir().expect("临时目录必须可建");
        let path = directory.path().join("haven.db");
        const BODY: &[u8] = br#"{"sites":[{"name":"x"}]}"#;

        {
            let repo = SqliteSourceConfigCache::new(Arc::new(
                Db::open(&path).expect("首次打开必须可迁移"),
            ));
            assert!(repo.get("custom_tvbox_0001").await.unwrap().is_none());
            repo.put("custom_tvbox_0001", BODY, UtcMillis(1_000))
                .await
                .unwrap();
            // 句柄在这里析构：下面的读取只能来自磁盘。
        }

        let repo =
            SqliteSourceConfigCache::new(Arc::new(Db::open(&path).expect("重新打开必须成功")));
        let cached = repo
            .get("custom_tvbox_0001")
            .await
            .unwrap()
            .expect("重新打开后缓存必须仍在");
        assert_eq!(cached.body(), BODY);
        assert_eq!(cached.fetched_at(), UtcMillis(1_000));
    }
}
