//! Progress Repository（Sqlite）。
//!
//! 规范：DOMAIN_MODEL §28–§30（Locator 是事实来源；percentage 仅 UI 派生值）。
//! `revision` 是独立的 opaque CAS token，`updated_at` 只负责时间展示与排序。
//! media_item_id 唯一：同一 MediaItem 只保留一行 Progress，upsert 语义。

use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::OptionalExtension;

use haven_common::AppError;
use haven_domain::contracts::ProgressRepository;
use haven_domain::entities::Progress;
use haven_domain::ids::{EditionId, MediaItemId, ProgressId, WorkId};
use uuid::Uuid;

use crate::db::Db;
use crate::db::repos::hierarchy::validate_content_chain;
use crate::db::repos::{
    enum_to_db_str, id_from_row, locator_from_json, locator_to_json, map_db_error,
};

pub struct SqliteProgressRepository {
    db: Arc<Db>,
}

impl SqliteProgressRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

pub(crate) fn row_to_progress(row: &rusqlite::Row<'_>) -> rusqlite::Result<Progress> {
    let media_item_id: String = row.get("media_item_id")?;
    let completion: String = row.get("completion")?;
    let percentage: Option<f64> = row.get("percentage")?;
    let locator_json: String = row.get("locator_json")?;
    let locator_version: u32 = row.get("locator_version")?;
    let locator = locator_from_json(&locator_json).map_err(rusqlite_err)?;
    if locator.version() != locator_version {
        return Err(rusqlite_err(AppError::new(
            "LOCATOR_VERSION_MISMATCH",
            haven_common::ErrorKind::Parse,
            "Locator envelope 与数据库版本列不一致",
            false,
        )));
    }

    // 018 新增列，老库无此列时兼容为 None；034 之后 revision 必须存在。
    let keyframe_uri: Option<String> = row.get::<_, Option<String>>("keyframe_uri").unwrap_or(None);
    let revision: String = row.get("revision")?;
    if revision.trim().is_empty() {
        return Err(rusqlite_err(AppError::new(
            "PROGRESS_REVISION_MISSING",
            haven_common::ErrorKind::Database,
            "Progress 缺少持久化 revision",
            false,
        )));
    }

    Ok(Progress {
        id: id_from_row::<ProgressId>(row.get("id")?)?,
        work_id: id_from_row::<WorkId>(row.get("work_id")?)?,
        edition_id: id_from_row::<EditionId>(row.get("edition_id")?)?,
        media_item_id: id_from_row::<MediaItemId>(media_item_id)?,
        locator,
        completion: parse_enum(&completion)?,
        percentage: percentage.map(|v| v as f32),
        last_active_at: haven_common::UtcMillis(row.get("last_active_at")?),
        updated_at: haven_common::UtcMillis(row.get("updated_at")?),
        revision: Some(revision),
        keyframe_uri,
    })
}

fn parse_enum<T: serde::de::DeserializeOwned>(s: &str) -> rusqlite::Result<T> {
    serde_json::from_str(&format!("\"{s}\"")).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "无法解析枚举",
            )),
        )
    })
}

fn rusqlite_err(e: AppError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.user_message(),
        )),
    )
}

pub(crate) const SELECT_COLUMNS: &str = "id, work_id, edition_id, media_item_id, locator_json, locator_version, completion, percentage, last_active_at, updated_at, revision, keyframe_uri";

/// 生成不可由展示字段推导的 Progress CAS token。
pub(crate) fn new_revision() -> String {
    Uuid::new_v4().to_string()
}

#[async_trait]
impl ProgressRepository for SqliteProgressRepository {
    async fn get_for_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Option<Progress>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM progress WHERE media_item_id = ?1"
            ))
            .map_err(map_db_error("查询 Progress 失败"))?;
        let mut rows = stmt
            .query_map(
                rusqlite::params![media_item_id.to_string()],
                row_to_progress,
            )
            .map_err(map_db_error("查询 Progress 失败"))?;
        rows.next()
            .transpose()
            .map_err(map_db_error("查询 Progress 失败"))
    }

    async fn get_for_media_items(
        &self,
        media_item_ids: &[MediaItemId],
    ) -> Result<std::collections::HashMap<MediaItemId, Progress>, AppError> {
        let mut out = std::collections::HashMap::new();
        if media_item_ids.is_empty() {
            return Ok(out);
        }
        let conn = self.db.lock();
        let placeholders: Vec<String> = (1..=media_item_ids.len())
            .map(|i| format!("?{i}"))
            .collect();
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM progress WHERE media_item_id IN ({})",
            placeholders.join(",")
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(map_db_error("批量查询 Progress 失败"))?;
        let params: Vec<String> = media_item_ids.iter().map(|id| id.to_string()).collect();
        let params: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt
            .query_map(params.as_slice(), row_to_progress)
            .map_err(map_db_error("批量查询 Progress 失败"))?;
        for row in rows {
            let p = row.map_err(map_db_error("批量查询 Progress 失败"))?;
            out.insert(p.media_item_id, p);
        }
        Ok(out)
    }

    async fn save(&self, progress: &Progress) -> Result<(), AppError> {
        let conn = self.db.lock();
        validate_progress(&conn, progress)?;
        // R-PROGRESS-CAS-REV3：`save` 是 reset/无返回写路径——**保留传入 last_active_at**
        // （max 不回退），仅 updated_at 单调推进；不得把 reset 推到 recent/LastActive 首位。
        let _ = save_unconditional(&conn, progress, true)?;
        Ok(())
    }

    async fn save_if_revision(
        &self,
        progress: &Progress,
        expected_revision: Option<&str>,
    ) -> Result<Option<String>, AppError> {
        let conn = self.db.lock();
        validate_progress(&conn, progress)?;
        let locator_json = locator_to_json(&progress.locator)?;
        let completion = enum_to_db_str(&progress.completion)?;
        let percentage = progress.percentage.map(|v| v as f64);
        let revision = new_revision();
        let revision = if let Some(expected) = expected_revision {
            conn.query_row(
                "UPDATE progress SET
                     work_id = ?1,
                     edition_id = ?2,
                     locator_json = ?4,
                     locator_version = ?5,
                     completion = ?6,
                     percentage = ?7,
                     keyframe_uri = ?8,
                     revision = ?9,
                     last_active_at = CASE
                         WHEN ?10 <= updated_at THEN updated_at + 1
                         ELSE ?10
                     END,
                     updated_at = CASE
                         WHEN ?10 <= updated_at THEN updated_at + 1
                         ELSE ?10
                     END
                 WHERE media_item_id = ?3 AND revision = ?11
                 RETURNING revision",
                rusqlite::params![
                    progress.work_id.to_string(),
                    progress.edition_id.to_string(),
                    progress.media_item_id.to_string(),
                    locator_json,
                    progress.locator.version(),
                    completion,
                    percentage,
                    progress.keyframe_uri,
                    revision,
                    progress.updated_at.0,
                    expected,
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_db_error("条件保存 Progress 失败"))?
        } else {
            // 普通保存（save_if_revision(None)）：last_active_at 与 authoritative
            // updated_at 同步单调推进（新活动），保持 REV2/REV2B 语义。
            Some(save_unconditional(&conn, progress, false)?)
        };
        Ok(revision)
    }

    async fn recent(&self, limit: u32) -> Result<Vec<Progress>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM progress ORDER BY last_active_at DESC, id DESC LIMIT ?1"
            ))
            .map_err(map_db_error("查询最近进度失败"))?;
        let rows = stmt
            .query_map(rusqlite::params![limit], row_to_progress)
            .map_err(map_db_error("查询最近进度失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询最近进度失败"))
    }
}

/// 校验 progress 内容链与 percentage（save 与 save_if_revision 共用，避免复制）。
pub(crate) fn validate_progress(
    conn: &rusqlite::Connection,
    progress: &Progress,
) -> Result<(), AppError> {
    validate_content_chain(
        conn,
        progress.work_id,
        progress.edition_id,
        progress.media_item_id,
    )?;
    if progress
        .percentage
        .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
    {
        return Err(AppError::new(
            "INVALID_PROGRESS_PERCENTAGE",
            haven_common::ErrorKind::Validation,
            "Progress percentage 必须是 0..=1 的有限数",
            false,
        ));
    }
    Ok(())
}

/// 无条件 upsert（INSERT ... ON CONFLICT(media_item_id) DO UPDATE）并返回 authoritative
/// opaque `revision`；`updated_at` 仍单调推进且不回退。`preserve_last_active`：
/// - `true`  → 保留传入 `last_active_at`（max 不回退）——reset/无返回写路径；
/// - `false` → `last_active_at` 与 authoritative `updated_at` 同步推进（普通保存，新活动）。
fn save_unconditional(
    conn: &rusqlite::Connection,
    progress: &Progress,
    preserve_last_active: bool,
) -> Result<String, AppError> {
    let locator_json = locator_to_json(&progress.locator)?;
    let completion = enum_to_db_str(&progress.completion)?;
    let percentage = progress.percentage.map(|v| v as f64);
    // last_active_at 赋值表达式（固定字面量，非注入）。
    let last_active_expr = if preserve_last_active {
        "CASE WHEN excluded.last_active_at <= progress.last_active_at
              THEN progress.last_active_at
              ELSE excluded.last_active_at END"
    } else {
        "CASE WHEN excluded.updated_at <= progress.updated_at
              THEN progress.updated_at + 1
              ELSE excluded.updated_at END"
    };
    let revision = new_revision();
    let sql = format!(
        "INSERT INTO progress
            (id, work_id, edition_id, media_item_id, locator_json, locator_version,
             completion, percentage, last_active_at, updated_at, revision, keyframe_uri)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(media_item_id) DO UPDATE SET
             work_id = excluded.work_id,
             edition_id = excluded.edition_id,
             locator_json = excluded.locator_json,
             locator_version = excluded.locator_version,
             completion = excluded.completion,
             percentage = excluded.percentage,
             keyframe_uri = excluded.keyframe_uri,
             revision = excluded.revision,
             last_active_at = {last_active_expr},
             updated_at = CASE
                 WHEN excluded.updated_at <= progress.updated_at THEN progress.updated_at + 1
                 ELSE excluded.updated_at
             END
         RETURNING revision"
    );
    conn.query_row(
        &sql,
        rusqlite::params![
            progress.id.to_string(),
            progress.work_id.to_string(),
            progress.edition_id.to_string(),
            progress.media_item_id.to_string(),
            locator_json,
            progress.locator.version(),
            completion,
            percentage,
            progress.last_active_at.0,
            progress.updated_at.0,
            revision,
            progress.keyframe_uri,
        ],
        |row| row.get::<_, String>(0),
    )
    .map_err(map_db_error("保存 Progress 失败"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use haven_domain::enums::CompletionState;
    use haven_domain::locator::{Locator, VideoLocator};

    fn seed_content(db: &Db) -> (WorkId, EditionId, MediaItemId) {
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let now = haven_common::UtcMillis::now().0;
        let conn = db.lock();
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, '测试作品', 'fiction', 'completed', ?2, ?2)",
            rusqlite::params![work_id.to_string(), now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
             VALUES (?1, ?2, '测试版本', 'movie', ?3, ?3)",
            rusqlite::params![edition_id.to_string(), work_id.to_string(), now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_items (id, edition_id, media_type, title, status, created_at, updated_at)
             VALUES (?1, ?2, 'episode', 'S01E01', 'available', ?3, ?3)",
            rusqlite::params![media_item_id.to_string(), edition_id.to_string(), now],
        )
        .unwrap();
        (work_id, edition_id, media_item_id)
    }

    fn sample_progress(
        work_id: WorkId,
        edition_id: EditionId,
        media_item_id: MediaItemId,
    ) -> Progress {
        Progress {
            id: ProgressId::new(),
            work_id,
            edition_id,
            media_item_id,
            locator: Locator::Video(VideoLocator {
                media_item_id,
                position_ms: 3_600_000,
            }),
            completion: CompletionState::InProgress,
            percentage: Some(0.25),
            last_active_at: haven_common::UtcMillis::now(),
            updated_at: haven_common::UtcMillis::now(),
            revision: None,
            keyframe_uri: None,
        }
    }

    #[tokio::test]
    async fn save_get_roundtrip_preserves_locator() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (w, e, m) = seed_content(&db);
        let repo = SqliteProgressRepository::new(db);
        let progress = sample_progress(w, e, m);

        repo.save(&progress).await.unwrap();
        let read = repo.get_for_media_item(m).await.unwrap().expect("存在");
        assert_eq!(read.locator, progress.locator);
        assert_eq!(read.completion, CompletionState::InProgress);
        assert_eq!(read.percentage, Some(0.25));
        assert_eq!(read.work_id, w);
    }

    #[tokio::test]
    async fn save_twice_upserts_single_row() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (w, e, m) = seed_content(&db);
        let repo = SqliteProgressRepository::new(db.clone());
        let mut progress = sample_progress(w, e, m);
        repo.save(&progress).await.unwrap();

        progress.percentage = Some(0.9);
        progress.locator = Locator::Video(VideoLocator {
            media_item_id: m,
            position_ms: 9_999_999,
        });
        repo.save(&progress).await.unwrap();

        let count: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM progress", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "同一 media_item 只能一行");
        let read = repo.get_for_media_item(m).await.unwrap().unwrap();
        assert_eq!(read.percentage, Some(0.9));
        match read.locator {
            Locator::Video(v) => assert_eq!(v.position_ms, 9_999_999),
            _ => panic!("locator 类型应保留"),
        }
    }

    #[tokio::test]
    async fn conditional_save_allows_exactly_one_writer_per_revision() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (w, e, m) = seed_content(&db);
        let repo = SqliteProgressRepository::new(db);
        let mut progress = sample_progress(w, e, m);
        progress.updated_at = haven_common::UtcMillis(1_000);

        let first = repo
            .save_if_revision(&progress, None)
            .await
            .unwrap()
            .expect("首次写入返回 revision");
        assert_ne!(first, "1000");

        let mut winner = progress.clone();
        winner.updated_at = haven_common::UtcMillis(1_000);
        winner.percentage = Some(0.5);
        let next = repo
            .save_if_revision(&winner, Some(&first))
            .await
            .unwrap()
            .expect("当前 revision 条件写成功");
        assert_ne!(next, first, "每次成功写入都必须生成新的 opaque revision");

        let mut stale = progress;
        stale.updated_at = haven_common::UtcMillis(2_000);
        stale.percentage = Some(0.9);
        assert_eq!(
            repo.save_if_revision(&stale, Some(&first)).await.unwrap(),
            None,
            "同一 expected revision 的第二个写入必须原子冲突"
        );
        let stored = repo.get_for_media_item(m).await.unwrap().unwrap();
        assert_eq!(stored.percentage, Some(0.5), "冲突写不得覆盖胜者");
        assert_eq!(stored.updated_at.0, 1_001);
        // R-PROGRESS-CAS-REV2 Important 1：authoritative last_active_at 必须与
        // updated_at 同值单调推进（recent / LastActive 排序依据）。
        assert_eq!(stored.last_active_at.0, stored.updated_at.0);
    }

    #[tokio::test]
    async fn revision_is_opaque_and_not_the_display_timestamp() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (w, e, m) = seed_content(&db);
        let repo = SqliteProgressRepository::new(db);
        let mut progress = sample_progress(w, e, m);
        progress.updated_at = haven_common::UtcMillis(1_000);

        let revision = repo
            .save_if_revision(&progress, None)
            .await
            .unwrap()
            .expect("首次写入返回 revision");

        assert_ne!(revision, "1000", "CAS token 不能复用展示时间戳");
    }

    #[tokio::test]
    async fn unconditional_upserts_return_distinct_authoritative_revisions() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (w, e, m) = seed_content(&db);
        let repo = SqliteProgressRepository::new(db);
        let mut progress = sample_progress(w, e, m);
        progress.updated_at = haven_common::UtcMillis(5_000);

        let first = repo
            .save_if_revision(&progress, None)
            .await
            .unwrap()
            .unwrap();
        let second = repo
            .save_if_revision(&progress, None)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(first, "5000");
        assert_ne!(second, first);
        let stored = repo.get_for_media_item(m).await.unwrap().unwrap();
        assert_eq!(
            stored.updated_at.0, 5_001,
            "无条件 upsert 单调推进 updated_at"
        );
        assert_eq!(
            stored.last_active_at.0, stored.updated_at.0,
            "authoritative last_active_at 必须与 updated_at 一致"
        );
    }

    /// R-PROGRESS-CAS-REV2 Important 1：候选时间戳回拨（< 当前持久化值）时，
    /// updated_at 与 last_active_at 都被推进到 当前+1，recent 排序保持最新活动优先。
    #[tokio::test]
    async fn clock_rollback_advances_both_timestamps_and_keeps_recent_order() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (w, e, m1) = seed_content(&db);
        let (w2, e2, m2) = seed_content(&db);
        let repo = SqliteProgressRepository::new(db);

        let mut a = sample_progress(w, e, m1);
        a.updated_at = haven_common::UtcMillis(10_000);
        a.last_active_at = a.updated_at;
        repo.save_if_revision(&a, None).await.unwrap().unwrap();

        let mut b = sample_progress(w2, e2, m2);
        b.updated_at = haven_common::UtcMillis(9_000);
        b.last_active_at = b.updated_at;
        repo.save_if_revision(&b, None).await.unwrap().unwrap();

        // 时钟回拨写 **已有行** m1（候选 1000 < 当前 10000）→ ON CONFLICT 推进到 10001。
        let mut rollback = sample_progress(w, e, m1);
        rollback.updated_at = haven_common::UtcMillis(1_000);
        rollback.last_active_at = rollback.updated_at;
        rollback.percentage = Some(0.5);
        let rev = repo
            .save_if_revision(&rollback, None)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(rev, "10000", "CAS token 不得退化为展示时间戳");
        let stored = repo.get_for_media_item(m1).await.unwrap().unwrap();
        assert_eq!(stored.updated_at.0, 10_001);
        assert_eq!(stored.last_active_at.0, stored.updated_at.0);
        assert_eq!(stored.percentage, Some(0.5));

        // recent 按 last_active_at DESC：m1(10001) 在 m2(9000) 前。
        let recent = repo.recent(10).await.unwrap();
        assert_eq!(recent[0].media_item_id, m1, "回拨写入仍是最近活动");
        assert_eq!(recent[1].media_item_id, m2);
    }

    /// R-PROGRESS-CAS-REV2 Important 2：真实 barrier 并发——同一文件库两个独立连接，
    /// 各自 Repository 以同一 expected revision 写不同字段；断言恰好一个 Some revision、
    /// 一个 None；**最终行精确绑定到返回 Some 的 winner**（percentage 精确等于该 writer、
    /// stored revision 精确等于 Some revision），loser 字段未落库。
    #[tokio::test]
    async fn concurrent_conditional_writes_two_connections_exactly_one_wins() {
        use std::sync::Barrier;

        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("progress.db");
        let (w, e, m, first_rev) = {
            let db = Arc::new(Db::open(&db_path).unwrap());
            let (w, e, m) = seed_content(&db);
            let repo = SqliteProgressRepository::new(db.clone());
            let mut p = sample_progress(w, e, m);
            p.updated_at = haven_common::UtcMillis(1_000);
            let rev = repo
                .save_if_revision(&p, None)
                .await
                .unwrap()
                .expect("初始 revision");
            (w, e, m, rev)
        };

        let db_a = Arc::new(Db::open(&db_path).unwrap());
        let db_b = Arc::new(Db::open(&db_path).unwrap());
        let repo_a = SqliteProgressRepository::new(db_a);
        let repo_b = SqliteProgressRepository::new(db_b);

        let mut pa = sample_progress(w, e, m);
        pa.updated_at = haven_common::UtcMillis(2_000);
        pa.percentage = Some(0.5);
        let mut pb = sample_progress(w, e, m);
        pb.updated_at = haven_common::UtcMillis(3_000);
        pb.percentage = Some(0.9);

        let barrier = Arc::new(Barrier::new(2));

        let barrier_a = barrier.clone();
        let exp_a = first_rev.clone();
        let ta = std::thread::spawn(move || {
            barrier_a.wait();
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            rt.block_on(async move { repo_a.save_if_revision(&pa, Some(&exp_a)).await })
        });
        let barrier_b = barrier;
        let exp_b = first_rev.clone();
        let tb = std::thread::spawn(move || {
            barrier_b.wait();
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            rt.block_on(async move { repo_b.save_if_revision(&pb, Some(&exp_b)).await })
        });

        let (ra, rb) = (ta.join().unwrap(), tb.join().unwrap());
        let (ra, rb) = (ra.unwrap(), rb.unwrap());

        // 由 ra/rb 哪个是 Some 明确推导 winner revision 与 winner percentage。
        let (winner_rev, winner_percentage) = match (ra.as_deref(), rb.as_deref()) {
            (Some(rev), None) => (rev.to_owned(), 0.5f32),
            (None, Some(rev)) => (rev.to_owned(), 0.9f32),
            _ => panic!("并发同 expected 必须恰好一个 Some、一个 None，实际 ra={ra:?} rb={rb:?}"),
        };
        assert!(!winner_rev.is_empty(), "胜者必须返回 opaque revision");

        // 最终行精确绑定到 winner（db_check 连接在线程/repo 释放后打开）。
        let db_check = Arc::new(Db::open(&db_path).unwrap());
        let repo_check = SqliteProgressRepository::new(db_check);
        let stored = repo_check
            .get_for_media_item(m)
            .await
            .unwrap()
            .expect("胜者行存在");
        assert_eq!(
            stored.percentage,
            Some(winner_percentage),
            "最终行 percentage 必须精确等于 winner（loser 字段不得落库）"
        );
        assert_eq!(stored.revision.as_deref(), Some(winner_rev.as_str()));
        assert!(stored.updated_at.0 > 1_000);
        assert_eq!(stored.last_active_at.0, stored.updated_at.0);
        // 释放检查连接后 TempDir 由正常作用域自动清理。
        drop(repo_check);
    }

    /// R-PROGRESS-CAS-REV3：`save`（reset/无返回写路径）**保留传入 last_active_at**，
    /// 仅 completion/percentage/updated_at 重置；同毫秒候选时 updated_at 单调推进而
    /// last_active_at 不变；recent/LastActive 排序不被 reset 重排。
    #[tokio::test]
    async fn reset_save_preserves_last_active_and_advances_updated() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (w, e, m1) = seed_content(&db);
        let (w2, e2, m2) = seed_content(&db);
        let repo = SqliteProgressRepository::new(db);

        // 已存 progress A（m1）last_active=5000；B（m2）last_active=7000（更新活动）。
        let mut a = sample_progress(w, e, m1);
        a.updated_at = haven_common::UtcMillis(5_000);
        a.last_active_at = a.updated_at;
        repo.save_if_revision(&a, None).await.unwrap().unwrap();
        let mut b = sample_progress(w2, e2, m2);
        b.updated_at = haven_common::UtcMillis(7_000);
        b.last_active_at = b.updated_at;
        repo.save_if_revision(&b, None).await.unwrap().unwrap();

        // reset-like 写 A：completion=NotStarted、percentage=None、last_active 保留原值 5000、
        // updated 更新为 6000。
        let mut reset = sample_progress(w, e, m1);
        reset.updated_at = haven_common::UtcMillis(6_000);
        reset.last_active_at = haven_common::UtcMillis(5_000);
        reset.completion = CompletionState::NotStarted;
        reset.percentage = None;
        repo.save(&reset).await.unwrap();

        let stored = repo.get_for_media_item(m1).await.unwrap().unwrap();
        assert_eq!(stored.completion, CompletionState::NotStarted, "已重置");
        assert_eq!(stored.percentage, None, "已重置");
        assert_eq!(
            stored.last_active_at.0, 5_000,
            "reset 必须保留 last_active_at"
        );
        assert_eq!(stored.updated_at.0, 6_000, "updated_at 前移到候选");

        // 同毫秒候选再次 reset（updated=6000 不变）→ updated_at 单调推进 6001，
        // last_active_at 仍 5000 不变。
        let mut reset_same = reset;
        reset_same.updated_at = haven_common::UtcMillis(6_000);
        repo.save(&reset_same).await.unwrap();
        let stored = repo.get_for_media_item(m1).await.unwrap().unwrap();
        assert_eq!(stored.updated_at.0, 6_001, "同毫秒候选必须单调推进");
        assert_eq!(
            stored.last_active_at.0, 5_000,
            "同毫秒 reset 不得改变 last_active_at"
        );

        // recent 按 last_active_at 排序：reset 保留 A 的 last_active=5000，B(7000) 仍在 A(5000) 前。
        let recent = repo.recent(10).await.unwrap();
        assert_eq!(
            recent[0].media_item_id, m2,
            "reset 不得把自身推到 recent 首位"
        );
        assert_eq!(recent[1].media_item_id, m1);
    }

    #[tokio::test]
    async fn recent_orders_by_last_active_desc() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (w, e, m1) = seed_content(&db);
        let (w2, e2, m2) = seed_content(&db);
        let repo = SqliteProgressRepository::new(db);

        let mut p1 = sample_progress(w, e, m1);
        p1.last_active_at = haven_common::UtcMillis(1_000);
        let mut p2 = sample_progress(w2, e2, m2);
        p2.last_active_at = haven_common::UtcMillis(2_000);
        repo.save(&p1).await.unwrap();
        repo.save(&p2).await.unwrap();

        let recent = repo.recent(10).await.unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].media_item_id, m2, "最近活跃在前");
        assert_eq!(recent[1].media_item_id, m1);
    }

    #[tokio::test]
    async fn save_rejects_mismatched_content_chain() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (w1, e1, _) = seed_content(&db);
        let (_, _, m2) = seed_content(&db);
        let repo = SqliteProgressRepository::new(db);
        let progress = sample_progress(w1, e1, m2);

        let error = repo.save(&progress).await.unwrap_err();
        assert_eq!(error.code().as_str(), "CONTENT_CHAIN_INVALID");
    }

    #[tokio::test]
    async fn save_rejects_invalid_percentage() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (w, e, m) = seed_content(&db);
        let repo = SqliteProgressRepository::new(db);
        let mut progress = sample_progress(w, e, m);
        progress.percentage = Some(1.1);

        let error = repo.save(&progress).await.unwrap_err();
        assert_eq!(error.code().as_str(), "INVALID_PROGRESS_PERCENTAGE");
    }
}
