//! Resource Repository（Sqlite）。
//!
//! 规范：DOMAIN_MODEL §9–§11。
//! ResourceLocator 存储：locator_kind（判别值，便于索引）+ locator_json（完整 JSON）。

use std::sync::Arc;

use async_trait::async_trait;

use haven_common::AppError;
use haven_domain::contracts::ResourceRepository;
use haven_domain::entities::{ContentHash, Resource, ResourceLocator};
use haven_domain::ids::{MediaItemId, ResourceId, StorageLocationId};

use crate::db::Db;
use crate::db::repos::{enum_to_db_str, id_from_row, json_err, map_db_error};

pub struct SqliteResourceRepository {
    db: Arc<Db>,
}

impl SqliteResourceRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

fn locator_kind(locator: &ResourceLocator) -> &'static str {
    match locator {
        ResourceLocator::LocalPath { .. } => "local_path",
        ResourceLocator::StorageObject { .. } => "storage_object",
        ResourceLocator::Http { .. } => "http",
        ResourceLocator::SourceObject { .. } => "source_object",
    }
}

pub(crate) fn row_to_resource(row: &rusqlite::Row<'_>) -> rusqlite::Result<Resource> {
    let resource_type: String = row.get("resource_type")?;
    let availability: String = row.get("availability")?;
    let availability_source: String = row.get("availability_source")?;
    let locator_json: String = row.get("locator_json")?;
    let hash_algorithm: Option<String> = row.get("hash_algorithm")?;
    let hash_digest: Option<String> = row.get("hash_digest")?;

    Ok(Resource {
        id: id_from_row::<ResourceId>(row.get("id")?)?,
        media_item_id: id_from_row::<MediaItemId>(row.get("media_item_id")?)?,
        resource_type: parse_enum(&resource_type)?,
        source_id: row
            .get::<_, Option<String>>("source_id")?
            .map(id_from_row)
            .transpose()?,
        storage_location_id: row
            .get::<_, Option<String>>("storage_location_id")?
            .map(id_from_row)
            .transpose()?,
        locator: serde_json::from_str(&locator_json).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?,
        mime_type: row.get("mime_type")?,
        size: row.get::<_, Option<i64>>("size")?.map(|v| v as u64),
        hash: match (hash_algorithm, hash_digest) {
            (Some(algorithm), Some(digest)) => Some(ContentHash {
                algorithm: parse_enum(&algorithm)?,
                digest,
            }),
            (None, None) => None,
            _ => {
                return Err(rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "hash_algorithm 与 hash_digest 必须同时存在",
                    )),
                ));
            }
        },
        availability: parse_enum(&availability)?,
        availability_source: parse_enum(&availability_source)?,
        modified_ms: row.get::<_, Option<i64>>("modified_ms")?.map(|v| v as u64),
        fingerprint_first: row.get("fingerprint_first")?,
        fingerprint_last: row.get("fingerprint_last")?,
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

const SELECT_COLUMNS: &str = "id, media_item_id, resource_type, source_id, storage_location_id, locator_json, mime_type, size, hash_algorithm, hash_digest, availability, availability_source, modified_ms, fingerprint_first, fingerprint_last, created_at, updated_at";

/// 在指定连接上保存 Resource（普通连接或事务连接复用；scanner 原子写入用）。
pub(crate) fn save_on_conn(
    conn: &rusqlite::Connection,
    resource: &Resource,
) -> Result<(), AppError> {
    let locator_json =
        serde_json::to_string(&resource.locator).map_err(|e| json_err("资源定位序列化失败", e))?;
    conn.execute(
        "INSERT INTO resources
            (id, media_item_id, resource_type, source_id, storage_location_id,
             locator_kind, locator_json, mime_type, size, hash_algorithm, hash_digest,
             availability, availability_source, modified_ms, fingerprint_first, fingerprint_last,
             created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                 ?14, ?15, ?16, ?17, ?18)
         ON CONFLICT(id) DO UPDATE SET
             media_item_id = excluded.media_item_id,
             resource_type = excluded.resource_type,
             source_id = excluded.source_id,
             storage_location_id = excluded.storage_location_id,
             locator_kind = excluded.locator_kind,
             locator_json = excluded.locator_json,
             mime_type = excluded.mime_type,
             size = excluded.size,
             hash_algorithm = excluded.hash_algorithm,
             hash_digest = excluded.hash_digest,
             availability = excluded.availability,
             availability_source = excluded.availability_source,
             modified_ms = excluded.modified_ms,
             fingerprint_first = excluded.fingerprint_first,
             fingerprint_last = excluded.fingerprint_last,
             updated_at = excluded.updated_at",
        rusqlite::params![
            resource.id.to_string(),
            resource.media_item_id.to_string(),
            enum_to_db_str(&resource.resource_type)?,
            resource.source_id.map(|id| id.to_string()),
            resource.storage_location_id.map(|id| id.to_string()),
            locator_kind(&resource.locator),
            locator_json,
            resource.mime_type,
            resource.size.map(|v| v as i64),
            resource
                .hash
                .as_ref()
                .map(|h| enum_to_db_str(&h.algorithm))
                .transpose()?,
            resource.hash.as_ref().map(|h| h.digest.clone()),
            enum_to_db_str(&resource.availability)?,
            enum_to_db_str(&resource.availability_source)?,
            resource.modified_ms.map(|v| v as i64),
            resource.fingerprint_first,
            resource.fingerprint_last,
            resource.created_at.0,
            resource.updated_at.0,
        ],
    )
    .map_err(map_db_error("保存资源失败"))?;
    Ok(())
}

#[async_trait]
impl ResourceRepository for SqliteResourceRepository {
    async fn get(&self, id: ResourceId) -> Result<Option<Resource>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM resources WHERE id = ?1"
            ))
            .map_err(map_db_error("查询资源失败"))?;
        let mut rows = stmt
            .query_map(rusqlite::params![id.to_string()], row_to_resource)
            .map_err(map_db_error("查询资源失败"))?;
        rows.next()
            .transpose()
            .map_err(map_db_error("查询资源失败"))
    }

    async fn save(&self, resource: &Resource) -> Result<(), AppError> {
        let conn = self.db.lock();
        save_on_conn(&conn, resource)
    }

    async fn list_by_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Vec<Resource>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM resources WHERE media_item_id = ?1 ORDER BY created_at"
            ))
            .map_err(map_db_error("查询资源列表失败"))?;
        let rows = stmt
            .query_map(
                rusqlite::params![media_item_id.to_string()],
                row_to_resource,
            )
            .map_err(map_db_error("查询资源列表失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询资源列表失败"))
    }

    async fn delete(&self, id: ResourceId) -> Result<bool, AppError> {
        let conn = self.db.lock();
        let affected = conn
            .execute(
                "DELETE FROM resources WHERE id = ?1",
                rusqlite::params![id.to_string()],
            )
            .map_err(map_db_error("删除资源失败"))?;
        Ok(affected > 0)
    }

    async fn mark_unavailable_by_storage(
        &self,
        storage_location_id: StorageLocationId,
        availability: haven_domain::enums::Availability,
    ) -> Result<u64, AppError> {
        let conn = self.db.lock();
        let affected = conn
            .execute(
                "UPDATE resources
                 SET availability = ?1, updated_at = ?2
                 WHERE storage_location_id = ?3",
                rusqlite::params![
                    serde_json::to_string(&availability)
                        .map_err(|e| map_db_error("序列化可用性失败")(
                            rusqlite::Error::ToSqlConversionFailure(Box::new(e))
                        ))?
                        .trim_matches('"'),
                    haven_common::UtcMillis::now().0,
                    storage_location_id.to_string()
                ],
            )
            .map_err(map_db_error("批量标记资源不可用失败"))?;
        Ok(affected as u64)
    }

    async fn delete_by_storage(
        &self,
        storage_location_id: StorageLocationId,
    ) -> Result<u64, AppError> {
        let conn = self.db.lock();
        let affected = conn
            .execute(
                "DELETE FROM resources WHERE storage_location_id = ?1",
                rusqlite::params![storage_location_id.to_string()],
            )
            .map_err(map_db_error("删除位置索引资源失败"))?;
        Ok(affected as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use haven_domain::enums::{Availability, AvailabilitySource, HashAlgorithm, ResourceType};
    use haven_domain::ids::{EditionId, SourceId, StorageLocationId, WorkId};

    fn seed_chain(db: &Db) -> MediaItemId {
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let now = haven_common::UtcMillis::now().0;
        let conn = db.lock();
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, '资源测试作品', 'fiction', 'completed', ?2, ?2)",
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
             VALUES (?1, ?2, 'movie', '测试电影', 'available', ?3, ?3)",
            rusqlite::params![media_item_id.to_string(), edition_id.to_string(), now],
        )
        .unwrap();
        media_item_id
    }

    fn sample_resource(media_item_id: MediaItemId) -> Resource {
        Resource {
            id: ResourceId::new(),
            media_item_id,
            resource_type: ResourceType::LocalFile,
            source_id: None,
            storage_location_id: None,
            locator: ResourceLocator::LocalPath {
                path: "D:\\Movies\\test.mkv".into(),
            },
            mime_type: Some("video/x-matroska".into()),
            size: Some(2_147_483_648),
            hash: Some(ContentHash {
                algorithm: HashAlgorithm::Sha256,
                digest: "abc123".into(),
            }),
            availability: Availability::Available,
            availability_source: AvailabilitySource::Unknown,
            modified_ms: None,
            fingerprint_first: None,
            fingerprint_last: None,
            created_at: haven_common::UtcMillis(1_000),
            updated_at: haven_common::UtcMillis(1_000),
        }
    }

    #[tokio::test]
    async fn local_file_roundtrip() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let media_item_id = seed_chain(&db);
        let repo = SqliteResourceRepository::new(db);
        let resource = sample_resource(media_item_id);

        repo.save(&resource).await.unwrap();
        let read = repo.get(resource.id).await.unwrap().expect("存在");
        assert_eq!(read, resource);
        assert_eq!(
            read.locator,
            ResourceLocator::LocalPath {
                path: "D:\\Movies\\test.mkv".into()
            }
        );
        assert_eq!(read.hash.expect("hash").algorithm, HashAlgorithm::Sha256);
    }

    #[tokio::test]
    async fn http_and_source_object_locators_roundtrip() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let media_item_id = seed_chain(&db);
        let repo = SqliteResourceRepository::new(db);
        let mut resource = sample_resource(media_item_id);

        resource.locator = ResourceLocator::Http {
            url: "https://example.com/v.m3u8".into(),
        };
        repo.save(&resource).await.unwrap();
        let read = repo.get(resource.id).await.unwrap().unwrap();
        assert_eq!(
            read.locator,
            ResourceLocator::Http {
                url: "https://example.com/v.m3u8".into()
            }
        );

        resource.locator = ResourceLocator::SourceObject {
            source_id: SourceId::new(),
            remote_id: "chapter-42".into(),
        };
        repo.save(&resource).await.unwrap();
        let read = repo.get(resource.id).await.unwrap().unwrap();
        match read.locator {
            ResourceLocator::SourceObject { remote_id, .. } => assert_eq!(remote_id, "chapter-42"),
            _ => panic!("locator 类型应保留"),
        }
    }

    #[tokio::test]
    async fn storage_object_roundtrip() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let media_item_id = seed_chain(&db);
        let repo = SqliteResourceRepository::new(db);
        let mut resource = sample_resource(media_item_id);
        resource.locator = ResourceLocator::StorageObject {
            provider_id: StorageLocationId::new(),
            object_id: "obj-1".into(),
            path_hint: Some("dir/file.pdf".into()),
        };
        repo.save(&resource).await.unwrap();
        let read = repo.get(resource.id).await.unwrap().unwrap();
        match read.locator {
            ResourceLocator::StorageObject {
                object_id,
                path_hint,
                ..
            } => {
                assert_eq!(object_id, "obj-1");
                assert_eq!(path_hint.as_deref(), Some("dir/file.pdf"));
            }
            _ => panic!("locator 类型应保留"),
        }
    }

    #[tokio::test]
    async fn list_by_media_item_returns_all() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let media_item_id = seed_chain(&db);
        let repo = SqliteResourceRepository::new(db);
        let mut a = sample_resource(media_item_id);
        a.resource_type = ResourceType::LocalFile;
        let mut b = sample_resource(media_item_id);
        b.id = ResourceId::new();
        b.resource_type = ResourceType::HttpFile;
        b.locator = ResourceLocator::Http {
            url: "https://example.com/v2.mp4".into(),
        };
        repo.save(&a).await.unwrap();
        repo.save(&b).await.unwrap();

        let listed = repo.list_by_media_item(media_item_id).await.unwrap();
        assert_eq!(listed.len(), 2);
    }
}
