//! SQLite 热榜技术缓存（migration 020）。

use std::sync::Arc;

use async_trait::async_trait;
use haven_application::services::trending::{TrendingBoardCacheEntry, TrendingCachePort};
use haven_application::wire::TrendingBoardDto;
use haven_common::AppError;

use crate::db::Db;
use crate::db::repos::map_db_error;

pub struct SqliteTrendingCacheRepository {
    db: Arc<Db>,
}

impl SqliteTrendingCacheRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl TrendingCachePort for SqliteTrendingCacheRepository {
    async fn list(&self) -> Result<Vec<TrendingBoardCacheEntry>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(
                "SELECT board_id, source_id, payload_json, revision, refreshed_at, expires_at
                 FROM trending_board_cache ORDER BY board_id ASC",
            )
            .map_err(map_db_error("读取热榜缓存失败"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(map_db_error("读取热榜缓存行失败"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_db_error("读取热榜缓存行失败"))?;
        drop(stmt);
        drop(conn);

        let mut entries = Vec::with_capacity(rows.len());
        for (board_id, source_id, payload_json, revision, refreshed_at, expires_at) in rows {
            let board: TrendingBoardDto = match serde_json::from_str(&payload_json) {
                Ok(board) => board,
                Err(_) => continue, // 技术缓存损坏按 miss 处理，可由下一次 Refresh 重建。
            };
            if board.board_id != board_id {
                continue;
            }
            entries.push(TrendingBoardCacheEntry {
                board,
                source_id,
                revision,
                refreshed_at,
                expires_at,
            });
        }
        Ok(entries)
    }

    async fn upsert(&self, entry: &TrendingBoardCacheEntry) -> Result<(), AppError> {
        if entry.board.board_id.is_empty()
            || !entry.board.items.iter().all(|item| {
                item.poster_uri
                    .as_deref()
                    .is_none_or(is_controlled_artwork_uri)
            })
        {
            return Err(haven_common::validation("热榜缓存包含未受控海报"));
        }
        let payload = serde_json::to_string(&entry.board).map_err(|error| {
            AppError::new(
                "TRENDING_CACHE_SERIALIZE_FAILED",
                haven_common::ErrorKind::Parse,
                "热榜缓存序列化失败",
                false,
            )
            .with_source(error)
        })?;
        let conn = self.db.lock();
        conn.execute(
            "INSERT INTO trending_board_cache
                (board_id, source_id, payload_json, revision, refreshed_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(board_id) DO UPDATE SET
                source_id = excluded.source_id,
                payload_json = excluded.payload_json,
                revision = excluded.revision,
                refreshed_at = excluded.refreshed_at,
                expires_at = excluded.expires_at",
            rusqlite::params![
                entry.board.board_id,
                entry.source_id,
                payload,
                entry.revision,
                entry.refreshed_at,
                entry.expires_at,
            ],
        )
        .map_err(map_db_error("写入热榜缓存失败"))?;
        Ok(())
    }
}

fn is_controlled_artwork_uri(value: &str) -> bool {
    let Some(id) = value.strip_prefix("haven://artwork/") else {
        return false;
    };
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}
