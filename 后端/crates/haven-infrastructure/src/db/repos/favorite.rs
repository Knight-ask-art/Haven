//! Favorite Repository（Sqlite）。
//!
//! 规范：DOMAIN_MODEL §26（Work / Edition / MediaItem 三选一，互斥表达）。
//! schema：CHECK 保证每行恰好一个 target 非空；三个部分唯一索引保证同 target 只一行。

use std::sync::Arc;

use async_trait::async_trait;

use haven_common::AppError;
use haven_domain::contracts::FavoriteRepository;
use haven_domain::entities::{Favorite, FavoriteTarget};

use crate::db::Db;
use crate::db::repos::{id_from_row, map_db_error};

pub struct SqliteFavoriteRepository {
    db: Arc<Db>,
}

impl SqliteFavoriteRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

fn target_to_columns(target: &FavoriteTarget) -> (Option<String>, Option<String>, Option<String>) {
    match target {
        FavoriteTarget::Work(id) => (Some(id.to_string()), None, None),
        FavoriteTarget::Edition(id) => (None, Some(id.to_string()), None),
        FavoriteTarget::MediaItem(id) => (None, None, Some(id.to_string())),
    }
}

fn row_to_favorite(row: &rusqlite::Row<'_>) -> rusqlite::Result<Favorite> {
    let work_id: Option<String> = row.get("work_id")?;
    let edition_id: Option<String> = row.get("edition_id")?;
    let media_item_id: Option<String> = row.get("media_item_id")?;

    let target = match (work_id, edition_id, media_item_id) {
        (Some(id), None, None) => FavoriteTarget::Work(id_from_row(id)?),
        (None, Some(id), None) => FavoriteTarget::Edition(id_from_row(id)?),
        (None, None, Some(id)) => FavoriteTarget::MediaItem(id_from_row(id)?),
        _ => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "favorites 行违反三选一约束",
                )),
            ));
        }
    };

    Ok(Favorite {
        target,
        created_at: haven_common::UtcMillis(row.get("created_at")?),
    })
}

#[async_trait]
impl FavoriteRepository for SqliteFavoriteRepository {
    async fn set(&self, target: &FavoriteTarget) -> Result<(), AppError> {
        let (work_id, edition_id, media_item_id) = target_to_columns(target);
        let conn = self.db.lock();
        conn.execute(
            "INSERT INTO favorites (work_id, edition_id, media_item_id, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT DO NOTHING",
            rusqlite::params![
                work_id,
                edition_id,
                media_item_id,
                haven_common::UtcMillis::now().0
            ],
        )
        .map_err(map_db_error("收藏失败"))?;
        Ok(())
    }

    async fn unset(&self, target: &FavoriteTarget) -> Result<bool, AppError> {
        let conn = self.db.lock();
        let affected = match target {
            FavoriteTarget::Work(id) => conn
                .execute(
                    "DELETE FROM favorites WHERE work_id = ?1",
                    rusqlite::params![id.to_string()],
                )
                .map_err(map_db_error("取消收藏失败"))?,
            FavoriteTarget::Edition(id) => conn
                .execute(
                    "DELETE FROM favorites WHERE edition_id = ?1",
                    rusqlite::params![id.to_string()],
                )
                .map_err(map_db_error("取消收藏失败"))?,
            FavoriteTarget::MediaItem(id) => conn
                .execute(
                    "DELETE FROM favorites WHERE media_item_id = ?1",
                    rusqlite::params![id.to_string()],
                )
                .map_err(map_db_error("取消收藏失败"))?,
        };
        Ok(affected > 0)
    }

    async fn is_favorite(&self, target: &FavoriteTarget) -> Result<bool, AppError> {
        let conn = self.db.lock();
        let exists = match target {
            FavoriteTarget::Work(id) => conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM favorites WHERE work_id = ?1)",
                    rusqlite::params![id.to_string()],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(map_db_error("查询收藏失败"))?,
            FavoriteTarget::Edition(id) => conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM favorites WHERE edition_id = ?1)",
                    rusqlite::params![id.to_string()],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(map_db_error("查询收藏失败"))?,
            FavoriteTarget::MediaItem(id) => conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM favorites WHERE media_item_id = ?1)",
                    rusqlite::params![id.to_string()],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(map_db_error("查询收藏失败"))?,
        };
        Ok(exists > 0)
    }

    async fn is_favorite_many(
        &self,
        targets: &[FavoriteTarget],
    ) -> Result<std::collections::HashSet<FavoriteTarget>, AppError> {
        let mut out = std::collections::HashSet::new();
        let mut work_ids = Vec::new();
        let mut edition_ids = Vec::new();
        let mut media_item_ids = Vec::new();
        for target in targets {
            match target {
                FavoriteTarget::Work(id) => work_ids.push(id.to_string()),
                FavoriteTarget::Edition(id) => edition_ids.push(id.to_string()),
                FavoriteTarget::MediaItem(id) => media_item_ids.push(id.to_string()),
            }
        }

        let conn = self.db.lock();
        // 每个分组的占位符与参数严格一一对应；三种 target 都遵守 trait 的
        // 批量语义，不再把 Edition/MediaItem 静默当成未收藏。
        for (column, values) in [
            ("work_id", work_ids),
            ("edition_id", edition_ids),
            ("media_item_id", media_item_ids),
        ] {
            if values.is_empty() {
                continue;
            }
            let placeholders: Vec<String> = (1..=values.len()).map(|i| format!("?{i}")).collect();
            let sql = format!(
                "SELECT work_id, edition_id, media_item_id, created_at
                 FROM favorites WHERE {column} IN ({})",
                placeholders.join(",")
            );
            let mut stmt = conn
                .prepare(&sql)
                .map_err(map_db_error("批量查询收藏失败"))?;
            let params: Vec<&dyn rusqlite::ToSql> =
                values.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
            let rows = stmt
                .query_map(params.as_slice(), row_to_favorite)
                .map_err(map_db_error("批量查询收藏失败"))?;
            for row in rows {
                out.insert(row.map_err(map_db_error("批量查询收藏失败"))?.target);
            }
        }
        Ok(out)
    }

    async fn list(&self, limit: u32, offset: u32) -> Result<Vec<Favorite>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare("SELECT work_id, edition_id, media_item_id, created_at FROM favorites ORDER BY created_at DESC LIMIT ?1 OFFSET ?2")
            .map_err(map_db_error("查询收藏列表失败"))?;
        let rows = stmt
            .query_map(rusqlite::params![limit, offset], row_to_favorite)
            .map_err(map_db_error("查询收藏列表失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询收藏列表失败"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::ids::{EditionId, MediaItemId, WorkId};

    fn seed_work(db: &Db) -> WorkId {
        let work_id = WorkId::new();
        let now = haven_common::UtcMillis::now().0;
        db.lock()
            .execute(
                "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
                 VALUES (?1, '收藏测试作品', 'fiction', 'completed', ?2, ?2)",
                rusqlite::params![work_id.to_string(), now],
            )
            .unwrap();
        work_id
    }

    fn seed_edition(db: &Db, work_id: WorkId) -> EditionId {
        let edition_id = EditionId::new();
        let now = haven_common::UtcMillis::now().0;
        db.lock()
            .execute(
                "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
                 VALUES (?1, ?2, '测试版本', 'movie', ?3, ?3)",
                rusqlite::params![edition_id.to_string(), work_id.to_string(), now],
            )
            .unwrap();
        edition_id
    }

    fn seed_media_item(db: &Db, edition_id: EditionId) -> MediaItemId {
        let id = MediaItemId::new();
        let now = haven_common::UtcMillis::now().0;
        db.lock()
            .execute(
                "INSERT INTO media_items (id, edition_id, media_type, title, status, created_at, updated_at)
                 VALUES (?1, ?2, 'episode', 'S01E01', 'available', ?3, ?3)",
                rusqlite::params![id.to_string(), edition_id.to_string(), now],
            )
            .unwrap();
        id
    }

    #[tokio::test]
    async fn set_and_is_favorite_roundtrip() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let work_id = seed_work(&db);
        let repo = SqliteFavoriteRepository::new(db);

        let target = FavoriteTarget::Work(work_id);
        assert!(!repo.is_favorite(&target).await.unwrap());
        repo.set(&target).await.unwrap();
        assert!(repo.is_favorite(&target).await.unwrap());
    }

    #[tokio::test]
    async fn set_is_idempotent_and_unset_removes() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let work_id = seed_work(&db);
        let repo = SqliteFavoriteRepository::new(db.clone());

        let target = FavoriteTarget::Work(work_id);
        repo.set(&target).await.unwrap();
        let created_at_before: i64 = db
            .lock()
            .query_row(
                "SELECT created_at FROM favorites WHERE work_id = ?1",
                rusqlite::params![work_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        repo.set(&target).await.unwrap();
        let count: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM favorites", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "重复收藏应幂等");
        let created_at_after: i64 = db
            .lock()
            .query_row(
                "SELECT created_at FROM favorites WHERE work_id = ?1",
                rusqlite::params![work_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            created_at_after, created_at_before,
            "幂等 set 不应刷新收藏时间或改变列表顺序"
        );

        assert!(repo.unset(&target).await.unwrap());
        assert!(!repo.unset(&target).await.unwrap(), "重复取消 false");
        assert!(!repo.is_favorite(&target).await.unwrap());
    }

    #[tokio::test]
    async fn edition_and_media_item_targets_work_independently() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let work_id = seed_work(&db);
        let edition_id = seed_edition(&db, work_id);
        let media_item_id = seed_media_item(&db, edition_id);
        let repo = SqliteFavoriteRepository::new(db);

        let work = FavoriteTarget::Work(work_id);
        let edition = FavoriteTarget::Edition(edition_id);
        let item = FavoriteTarget::MediaItem(media_item_id);

        repo.set(&work).await.unwrap();
        repo.set(&edition).await.unwrap();
        repo.set(&item).await.unwrap();

        assert!(repo.is_favorite(&work).await.unwrap());
        assert!(repo.is_favorite(&edition).await.unwrap());
        assert!(repo.is_favorite(&item).await.unwrap());
    }

    #[tokio::test]
    async fn batch_lookup_supports_mixed_target_types() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let work_id = seed_work(&db);
        let edition_id = seed_edition(&db, work_id);
        let media_item_id = seed_media_item(&db, edition_id);
        let repo = SqliteFavoriteRepository::new(db);

        let work = FavoriteTarget::Work(work_id);
        let edition = FavoriteTarget::Edition(edition_id);
        let item = FavoriteTarget::MediaItem(media_item_id);
        repo.set(&work).await.unwrap();
        repo.set(&item).await.unwrap();

        let result = repo.is_favorite_many(&[work, edition, item]).await.unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.contains(&work));
        assert!(!result.contains(&edition));
        assert!(result.contains(&item));
    }

    #[tokio::test]
    async fn list_returns_newest_first() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let work_a = seed_work(&db);
        let work_b = seed_work(&db);
        let repo = SqliteFavoriteRepository::new(db);

        repo.set(&FavoriteTarget::Work(work_a)).await.unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        repo.set(&FavoriteTarget::Work(work_b)).await.unwrap();

        let listed = repo.list(10, 0).await.unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].target, FavoriteTarget::Work(work_b), "最新在前");
        assert_eq!(listed[1].target, FavoriteTarget::Work(work_a));
    }
}
