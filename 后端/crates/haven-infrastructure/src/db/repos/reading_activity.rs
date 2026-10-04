//! Reading Activity Repository（SQLite，047）。
//!
//! 只做两件事：把已闭合的会话写进窄表，以及按开始时刻的 UTC 半开区间读回来。
//! 本地日历换算（本地日、星期、小时）全部属于领域聚合，本层不做时区判断，
//! 因此同一份数据在任何时区下都读得出同一批行。

use std::sync::Arc;

use async_trait::async_trait;
use haven_common::{AppError, UtcMillis};
use haven_domain::contracts::ReadingActivityRepository;
use haven_domain::ids::{MediaItemId, ReadingSessionId};
use haven_domain::reading_activity::{ReadingSession, ReadingSessionCategory};
use rusqlite::Row;

use crate::db::Db;
use crate::db::repos::map_db_error;

pub struct SqliteReadingActivityRepository {
    db: Arc<Db>,
}

impl SqliteReadingActivityRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ReadingActivityRepository for SqliteReadingActivityRepository {
    /// `id` 是幂等键：同一次会话重复上报（重试、重复 close）只会命中同一行，
    /// 同一段时间不会被统计两次。
    async fn record_session(&self, session: &ReadingSession) -> Result<(), AppError> {
        let conn = self.db.lock();
        conn.execute(
            "INSERT INTO reading_sessions
                (id, media_item_id, category, started_at, ended_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO NOTHING",
            rusqlite::params![
                session.id.to_string(),
                session.media_item_id.to_string(),
                session.category.as_str(),
                session.started_at.0,
                session.ended_at.0,
                UtcMillis::now().0,
            ],
        )
        .map_err(map_db_error("记录阅读会话失败"))?;
        Ok(())
    }

    async fn list_sessions_between(
        &self,
        from: UtcMillis,
        to: UtcMillis,
    ) -> Result<Vec<ReadingSession>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, media_item_id, category, started_at, ended_at
                 FROM reading_sessions
                 WHERE started_at >= ?1 AND started_at < ?2
                 ORDER BY started_at ASC, id ASC",
            )
            .map_err(map_db_error("查询阅读会话失败"))?;
        let rows = stmt
            .query_map(rusqlite::params![from.0, to.0], session_from_row)
            .map_err(map_db_error("查询阅读会话失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("读取阅读会话失败"))
    }
}

/// 行 → 领域事实。任何一行读不成合法会话都是数据损坏（而不是「跳过这一条」）：
/// 静默丢行会让统计悄悄少算，比报错更难发现。
fn session_from_row(row: &Row<'_>) -> rusqlite::Result<ReadingSession> {
    let id_text: String = row.get("id")?;
    let id = uuid::Uuid::parse_str(&id_text)
        .ok()
        .filter(|parsed| parsed.to_string() == id_text)
        .map(ReadingSessionId::from_uuid)
        .ok_or_else(|| corrupt(0, rusqlite::types::Type::Text, "阅读会话 ID 非规范 UUID"))?;

    let media_item_text: String = row.get("media_item_id")?;
    let media_item_id = uuid::Uuid::parse_str(&media_item_text)
        .ok()
        .filter(|parsed| parsed.to_string() == media_item_text)
        .map(MediaItemId::from_uuid)
        .ok_or_else(|| corrupt(1, rusqlite::types::Type::Text, "阅读会话的媒体条目 ID 非法"))?;

    let category_text: String = row.get("category")?;
    let category = ReadingSessionCategory::parse(&category_text)
        .ok_or_else(|| corrupt(2, rusqlite::types::Type::Text, "未知的阅读会话分类"))?;

    let started_at: i64 = row.get("started_at")?;
    let ended_at: i64 = row.get("ended_at")?;
    ReadingSession::new(
        id,
        media_item_id,
        category,
        UtcMillis(started_at),
        UtcMillis(ended_at),
    )
    .map_err(|error| corrupt(3, rusqlite::types::Type::Integer, error.user_message()))
}

fn corrupt(
    index: usize,
    column: rusqlite::types::Type,
    message: impl Into<String>,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        column,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message.into(),
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(
        id_byte: u8,
        start: i64,
        duration: i64,
        category: ReadingSessionCategory,
    ) -> ReadingSession {
        ReadingSession::new(
            ReadingSessionId::from_uuid(
                uuid::Uuid::parse_str(&format!("0196f0d2-0000-7000-8000-0000000000{id_byte:02x}"))
                    .unwrap(),
            ),
            MediaItemId::from_uuid(
                uuid::Uuid::parse_str("0196f0d2-0000-7000-8000-00000000ffff").unwrap(),
            ),
            category,
            UtcMillis(start),
            UtcMillis(start + duration),
        )
        .unwrap()
    }

    fn repo() -> (Arc<Db>, SqliteReadingActivityRepository) {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteReadingActivityRepository::new(db.clone());
        (db, repo)
    }

    /// 区间是半开的 `[from, to)`：左端点入、右端点出。
    #[tokio::test]
    async fn sessions_are_read_back_in_the_half_open_window() {
        let (_db, repo) = repo();
        let first = session(1, 1_000, 60_000, ReadingSessionCategory::Book);
        let second = session(2, 61_000, 30_000, ReadingSessionCategory::Comic);
        let outside = session(3, 200_000, 10_000, ReadingSessionCategory::Video);
        for item in [&first, &second, &outside] {
            repo.record_session(item).await.unwrap();
        }

        let listed = repo
            .list_sessions_between(UtcMillis(1_000), UtcMillis(200_000))
            .await
            .unwrap();
        assert_eq!(listed, vec![first, second], "窗口内按开始时刻升序");
    }

    /// 幂等键：同一次会话重复上报只留一行。
    #[tokio::test]
    async fn recording_the_same_session_twice_keeps_one_row() {
        let (_db, repo) = repo();
        let item = session(1, 5_000, 60_000, ReadingSessionCategory::Book);
        repo.record_session(&item).await.unwrap();
        repo.record_session(&item).await.unwrap();

        let listed = repo
            .list_sessions_between(UtcMillis(0), UtcMillis(1_000_000))
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], item);
    }

    /// 空库读出空集，而不是一条 0 时长的假记录。
    #[tokio::test]
    async fn an_untouched_database_reads_back_empty() {
        let (_db, repo) = repo();
        assert!(
            repo.list_sessions_between(UtcMillis(0), UtcMillis(1_000_000))
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// 数据库层的时长上限与领域一致：超长会话写不进去（不能被绕过领域构造器注入）。
    #[tokio::test]
    async fn the_database_rejects_a_session_longer_than_the_domain_limit() {
        let (db, repo) = repo();
        let started_at = 0i64;
        let ended_at = haven_domain::reading_activity::MAX_SESSION_DURATION_MS + 1;
        let write = {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO reading_sessions
                    (id, media_item_id, category, started_at, ended_at, created_at)
                 VALUES ('0196f0d2-0000-7000-8000-0000000000ff',
                         '0196f0d2-0000-7000-8000-00000000ffff',
                         'book', ?1, ?2, 0)",
                rusqlite::params![started_at, ended_at],
            )
        };
        assert!(write.is_err(), "超出领域上限的会话不得落库");
        assert!(
            repo.list_sessions_between(UtcMillis(0), UtcMillis(1_000_000_000))
                .await
                .unwrap()
                .is_empty()
        );
    }
}
