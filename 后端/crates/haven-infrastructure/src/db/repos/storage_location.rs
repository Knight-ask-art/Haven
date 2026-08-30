//! StorageLocation Repository（Sqlite）。
//!
//! 规范：LIBRARY_AND_STORAGE §14–§17。凭据只存 `credential_ref`，不存明文。

use std::sync::Arc;

use async_trait::async_trait;

use haven_common::AppError;
use haven_domain::contracts::StorageLocationRepository;
use haven_domain::entities::StorageLocation;
use haven_domain::ids::{CredentialRef, StorageLocationId};

use crate::db::Db;
use crate::db::repos::{enum_to_db_str, id_from_row, map_db_error};

pub struct SqliteStorageLocationRepository {
    db: Arc<Db>,
}

impl SqliteStorageLocationRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

pub(crate) fn row_to_storage_location(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StorageLocation> {
    let provider_type: String = row.get("provider_type")?;
    let status: String = row.get("status")?;
    let credential_ref = match row.get::<_, Option<String>>("credential_ref")? {
        Some(raw) => {
            // S-01 修复：脏 DB 值（非 haven: 命名空间）在进入实体前拒绝，
            // 不再允许绕过构造校验。
            Some(raw.parse::<CredentialRef>().map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("凭据引用非法：{}", e.user_message()),
                    )),
                )
            })?)
        }
        None => None,
    };
    Ok(StorageLocation {
        id: id_from_row::<StorageLocationId>(row.get("id")?)?,
        provider_type: parse_enum(&provider_type)?,
        display_name: row.get("display_name")?,
        root_ref: row.get("root_ref")?,
        credential_ref,
        status: parse_enum(&status)?,
        created_at: haven_common::UtcMillis(row.get("created_at")?),
        updated_at: haven_common::UtcMillis(row.get("updated_at")?),
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

const SELECT_COLUMNS: &str =
    "id, provider_type, display_name, root_ref, credential_ref, status, created_at, updated_at";

#[async_trait]
impl StorageLocationRepository for SqliteStorageLocationRepository {
    async fn get(&self, id: StorageLocationId) -> Result<Option<StorageLocation>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM storage_locations WHERE id = ?1"
            ))
            .map_err(map_db_error("查询存储位置失败"))?;
        let mut rows = stmt
            .query_map(rusqlite::params![id.to_string()], row_to_storage_location)
            .map_err(map_db_error("查询存储位置失败"))?;
        rows.next()
            .transpose()
            .map_err(map_db_error("查询存储位置失败"))
    }

    async fn save(&self, location: &StorageLocation) -> Result<(), AppError> {
        let conn = self.db.lock();
        conn.execute(
            "INSERT INTO storage_locations
                (id, provider_type, display_name, root_ref, credential_ref, status,
                 created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                 provider_type = excluded.provider_type,
                 display_name = excluded.display_name,
                 root_ref = excluded.root_ref,
                 credential_ref = excluded.credential_ref,
                 status = excluded.status,
                 updated_at = excluded.updated_at",
            rusqlite::params![
                location.id.to_string(),
                enum_to_db_str(&location.provider_type)?,
                location.display_name,
                location.root_ref,
                location
                    .credential_ref
                    .as_ref()
                    .map(|r| r.as_str().to_owned()),
                enum_to_db_str(&location.status)?,
                location.created_at.0,
                location.updated_at.0,
            ],
        )
        .map_err(map_db_error("保存存储位置失败"))?;
        Ok(())
    }

    async fn list(&self) -> Result<Vec<StorageLocation>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM storage_locations ORDER BY created_at"
            ))
            .map_err(map_db_error("查询存储位置列表失败"))?;
        let rows = stmt
            .query_map([], row_to_storage_location)
            .map_err(map_db_error("查询存储位置列表失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询存储位置列表失败"))
    }

    async fn delete(&self, id: StorageLocationId) -> Result<bool, AppError> {
        let conn = self.db.lock();
        let affected = conn
            .execute(
                "DELETE FROM storage_locations WHERE id = ?1",
                rusqlite::params![id.to_string()],
            )
            .map_err(map_db_error("删除存储位置失败"))?;
        Ok(affected > 0)
    }

    async fn clear_credential_ref(&self, id: StorageLocationId) -> Result<bool, AppError> {
        let conn = self.db.lock();
        let affected = conn
            .execute(
                "UPDATE storage_locations SET credential_ref = NULL, updated_at = ?1 WHERE id = ?2",
                rusqlite::params![haven_common::UtcMillis::now().0, id.to_string()],
            )
            .map_err(map_db_error("清除凭据引用失败"))?;
        Ok(affected > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use haven_domain::enums::{StorageProviderType, StorageStatus};

    fn sample_location() -> StorageLocation {
        StorageLocation {
            id: StorageLocationId::new(),
            provider_type: StorageProviderType::Local,
            display_name: "电影库".into(),
            // 唯一索引 lower(root_ref)（008）：每个样例唯一路径。
            root_ref: format!(
                "D:\\Movies\\{}",
                uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))
            ),
            credential_ref: None,
            status: StorageStatus::Connected,
            created_at: haven_common::UtcMillis(1_000),
            updated_at: haven_common::UtcMillis(1_000),
        }
    }

    #[tokio::test]
    async fn save_get_roundtrip() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteStorageLocationRepository::new(db);
        let location = sample_location();

        repo.save(&location).await.unwrap();
        let read = repo.get(location.id).await.unwrap().expect("存在");
        assert_eq!(read, location);
        assert_eq!(read.provider_type, StorageProviderType::Local);
    }

    #[tokio::test]
    async fn list_and_delete() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteStorageLocationRepository::new(db);
        let mut a = sample_location();
        a.display_name = "A".into();
        let mut b = sample_location();
        b.id = StorageLocationId::new();
        b.display_name = "B".into();
        repo.save(&a).await.unwrap();
        repo.save(&b).await.unwrap();

        let listed = repo.list().await.unwrap();
        assert_eq!(listed.len(), 2);

        assert!(repo.delete(a.id).await.unwrap());
        assert_eq!(repo.list().await.unwrap().len(), 1);
        assert!(!repo.delete(a.id).await.unwrap(), "重复删除 false");
    }
}
