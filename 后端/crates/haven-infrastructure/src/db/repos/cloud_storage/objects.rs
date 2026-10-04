//! 云盘对象绑定（Sqlite）：读取 / 快照 / 复核 / PDF 导入。
//!
//! 边界（契约见 `haven_application::services::cloud_storage::state`）：
//! - 每个公开函数 = 一个短事务，读→校验→写原子；stat / 下载等网络 IO 由调用方在事务外
//!   完成，事务内不接触任何 Provider 凭据。
//! - 快照只在「目录绑定 + 位置 + 资源 locator + 账户」内部完全自洽时给出；malformed /
//!   foreign（root_ref、provider、locator 任一不匹配）一律拒绝，绝不返回半个绑定。
//! - 导入只写内部 UUID：`ResourceLocator::StorageObject.object_id` 是内部对象 ID，Provider
//!   file ID 只落在 `cloud_object_bindings`，绝不进入 locator 或 Wire。
//! - 错误文案不含 SQL、Provider ID 或本地路径。

use rusqlite::Connection;

use haven_application::services::cloud_storage::ports::{
    CLOUD_DRIVE_PROVIDER_ID, validate_drive_object_id,
};
use haven_application::services::cloud_storage::state::{
    CLOUD_OBJECT_MAX_BYTES, CloudAccount, CloudCasOutcome, CloudFolderBinding, CloudObjectBinding,
    CloudObjectSnapshot, CloudPdfCandidate, CloudPdfImportOutcome, MAX_CLOUD_OBJECTS_PER_FOLDER,
    cloud_binding_stale,
};
use haven_common::{AppError, ErrorKind, UtcMillis, validation};
use haven_domain::entities::{
    Edition, MediaIndex, MediaItem, Resource, ResourceLocator, StorageLocation, Work,
};
use haven_domain::enums::{
    Availability, AvailabilitySource, MediaItemStatus, MediaType, ResourceType,
    StorageProviderType, StorageStatus, WorkStatus, WorkType,
};
use haven_domain::ids::{EditionId, MediaItemId, ResourceId, StorageLocationId, WorkId};

use super::accounts::{get_account_on_conn, require_current_account_on_conn};
use super::folders::{get_folder_on_conn, validate_internal_uuid};
use crate::db::Db;
use crate::db::repos::resource::row_to_resource;
use crate::db::repos::storage_location::row_to_storage_location;
use crate::db::repos::{id_from_row, map_db_error};

const SELECT_OBJECT_COLUMNS: &str = "id, location_id, provider_file_id, display_name, size_bytes, resource_id, media_item_id, created_at";

fn row_to_object(row: &rusqlite::Row<'_>) -> rusqlite::Result<CloudObjectBinding> {
    let id: String = row.get("id")?;
    // 内部对象 UUID：malformed 行必须拒绝，不能当成合法绑定返回。
    uuid::Uuid::parse_str(&id).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "云盘对象内部 ID 非法",
            )),
        )
    })?;
    Ok(CloudObjectBinding {
        id,
        location_id: id_from_row::<StorageLocationId>(row.get("location_id")?)?,
        provider_file_id: row.get("provider_file_id")?,
        display_name: row.get("display_name")?,
        size_bytes: row.get::<_, i64>("size_bytes")? as u64,
        resource_id: id_from_row::<ResourceId>(row.get("resource_id")?)?,
        media_item_id: id_from_row::<MediaItemId>(row.get("media_item_id")?)?,
    })
}

fn load_location_on_conn(
    conn: &Connection,
    id: StorageLocationId,
) -> Result<Option<StorageLocation>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, provider_type, display_name, root_ref, credential_ref, status,
                    created_at, updated_at
             FROM storage_locations WHERE id = ?1",
        )
        .map_err(map_db_error("查询云盘存储位置失败"))?;
    let mut rows = stmt
        .query_map(rusqlite::params![id.to_string()], row_to_storage_location)
        .map_err(map_db_error("查询云盘存储位置失败"))?;
    rows.next()
        .transpose()
        .map_err(map_db_error("查询云盘存储位置失败"))
}

fn load_resource_on_conn(conn: &Connection, id: ResourceId) -> Result<Option<Resource>, AppError> {
    let mut stmt = conn
        .prepare("SELECT * FROM resources WHERE id = ?1")
        .map_err(map_db_error("查询云盘资源失败"))?;
    let mut rows = stmt
        .query_map(rusqlite::params![id.to_string()], row_to_resource)
        .map_err(map_db_error("查询云盘资源失败"))?;
    rows.next()
        .transpose()
        .map_err(map_db_error("查询云盘资源失败"))
}

/// 事务内按内部对象 UUID 读取绑定（供对象链校验 join 复用）；malformed ID 直接拒绝。
pub(super) fn get_object_on_conn(
    conn: &Connection,
    object_id: &str,
) -> Result<Option<CloudObjectBinding>, AppError> {
    validate_internal_uuid(object_id, "云盘对象内部 ID 非法")?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {SELECT_OBJECT_COLUMNS} FROM cloud_object_bindings WHERE id = ?1"
        ))
        .map_err(map_db_error("查询云盘对象绑定失败"))?;
    let mut rows = stmt
        .query_map(rusqlite::params![object_id], row_to_object)
        .map_err(map_db_error("查询云盘对象绑定失败"))?;
    rows.next()
        .transpose()
        .map_err(map_db_error("查询云盘对象绑定失败"))
}

/// 按 `(location, provider_file_id)` 精确查找：Provider ID 大小写敏感（`COLLATE BINARY`）。
fn get_object_by_identity_on_conn(
    conn: &Connection,
    location_id: StorageLocationId,
    provider_file_id: &str,
) -> Result<Option<CloudObjectBinding>, AppError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {SELECT_OBJECT_COLUMNS} FROM cloud_object_bindings
             WHERE location_id = ?1 AND provider_file_id = ?2 COLLATE BINARY"
        ))
        .map_err(map_db_error("查询云盘对象绑定失败"))?;
    let mut rows = stmt
        .query_map(
            rusqlite::params![location_id.to_string(), provider_file_id],
            row_to_object,
        )
        .map_err(map_db_error("查询云盘对象绑定失败"))?;
    rows.next()
        .transpose()
        .map_err(map_db_error("查询云盘对象绑定失败"))
}

pub(super) fn get_object(db: &Db, object_id: &str) -> Result<Option<CloudObjectBinding>, AppError> {
    let conn = db.lock();
    get_object_on_conn(&conn, object_id)
}

/// 读取对象依赖的目录绑定 / 位置 / 资源 / 账户并校验内部一致性。`Ok(None)` = 引用缺失或
/// malformed / foreign（root_ref、provider、locator 任一不匹配）；`Err` 只表示真正的查询 /
/// 反序列化失败。
fn load_bound_context_on_conn(
    conn: &Connection,
    object: &CloudObjectBinding,
) -> Result<Option<(CloudFolderBinding, StorageLocation, CloudAccount)>, AppError> {
    let Some(folder) = get_folder_on_conn(conn, object.location_id)? else {
        return Ok(None);
    };
    if validate_internal_uuid(&folder.root_ref, "云盘目录内部 ID 非法").is_err() {
        return Ok(None);
    }
    let Some(location) = load_location_on_conn(conn, object.location_id)? else {
        return Ok(None);
    };
    if location.provider_type != StorageProviderType::GoogleDrive
        || location.root_ref != folder.root_ref
    {
        return Ok(None);
    }
    let Some(account) = get_account_on_conn(conn, &folder.account_id)? else {
        return Ok(None);
    };
    if account.provider != CLOUD_DRIVE_PROVIDER_ID {
        return Ok(None);
    }
    let Some(resource) = load_resource_on_conn(conn, object.resource_id)? else {
        return Ok(None);
    };
    if resource.media_item_id != object.media_item_id
        || resource.storage_location_id != Some(object.location_id)
        || resource.mime_type.as_deref() != Some("application/pdf")
        || resource.resource_type != ResourceType::PublicationFile
    {
        return Ok(None);
    }
    match &resource.locator {
        ResourceLocator::StorageObject {
            provider_id,
            object_id,
            path_hint: None,
        } if *provider_id == object.location_id && object_id == &object.id => {}
        _ => return Ok(None),
    }
    Ok(Some((folder, location, account)))
}

pub(super) fn object_snapshot(
    db: &Db,
    object_id: &str,
) -> Result<Option<CloudObjectSnapshot>, AppError> {
    super::with_cloud_read_tx(db, |conn| {
        let Some(object) = get_object_on_conn(conn, object_id)? else {
            return Ok(None);
        };
        let Some((folder, location, account)) = load_bound_context_on_conn(conn, &object)? else {
            return Err(invalid_binding());
        };
        Ok(Some(CloudObjectSnapshot {
            object,
            account_id: folder.account_id.clone(),
            folder,
            account_generation: account.generation,
            location_status: location.status,
        }))
    })
}

/// IO 后复核：账户仍 connected 且代际一致、对象行与快照逐字段相同、目录 / 位置 / 资源链
/// 仍自洽且位置仍 `Connected`。任何不一致 → [`cloud_binding_stale`]（旧 IO 结果必须丢弃，
/// 不得凭旧凭据重试）。
pub(super) fn assert_current(
    db: &Db,
    expected_account_generation: i64,
    snapshot: &CloudObjectSnapshot,
) -> Result<(), AppError> {
    if expected_account_generation != snapshot.account_generation {
        return Err(cloud_binding_stale());
    }
    if validate_internal_uuid(&snapshot.object.id, "云盘对象内部 ID 非法").is_err() {
        return Err(cloud_binding_stale());
    }
    super::with_cloud_read_tx(db, |conn| {
        match require_current_account_on_conn(
            conn,
            &snapshot.account_id,
            expected_account_generation,
        )? {
            CloudCasOutcome::Applied(account)
                if account.provider == CLOUD_DRIVE_PROVIDER_ID
                    && account.generation == snapshot.account_generation => {}
            _ => return Err(cloud_binding_stale()),
        }
        let Some(current) = get_object_on_conn(&conn, &snapshot.object.id)? else {
            return Err(cloud_binding_stale());
        };
        if current != snapshot.object {
            return Err(cloud_binding_stale());
        }
        let Some((folder, location, _)) = load_bound_context_on_conn(&conn, &current)? else {
            return Err(cloud_binding_stale());
        };
        if folder != snapshot.folder
            || folder.account_id != snapshot.account_id
            || location.status != StorageStatus::Connected
            || location.status != snapshot.location_status
        {
            return Err(cloud_binding_stale());
        }
        Ok(())
    })
}

/// 把已通过 stat 的 PDF 导入统一内容模型：同一事务写真实 Work / Edition / MediaItem /
/// Resource + 对象绑定。类型走既有扫描器语义（`MediaType::Document`、
/// `ResourceType::PublicationFile`、`application/pdf`；`category` 由 `media_item::save_on_conn`
/// 从 media_type 推导）。幂等：同一 `(location, provider_file_id)` 已存在 → `Existing`，
/// 不重复建内容；重新导入只刷新 Provider 文件名 / 大小及 Resource 大小，保留内容链身份、
/// 用户标题与可用性来源。完全一致时不写任何行。
pub(super) fn import_pdf(
    db: &Db,
    binding: &CloudFolderBinding,
    expected_generation: i64,
    file: &CloudPdfCandidate,
) -> Result<CloudCasOutcome<CloudPdfImportOutcome>, AppError> {
    validate_drive_object_id(&file.provider_file_id)?;
    if file.display_name.trim().is_empty()
        || file.display_name.chars().count() > 300
        || file.display_name.chars().any(char::is_control)
    {
        return Err(validation("云盘文件名非法"));
    }
    if file.size_bytes == 0 || file.size_bytes > CLOUD_OBJECT_MAX_BYTES {
        return Err(validation("云盘 PDF 超出只读大小上限"));
    }
    validate_internal_uuid(&binding.account_id, "云盘账户内部 ID 非法")?;
    validate_internal_uuid(&binding.root_ref, "云盘目录内部 ID 非法")?;
    validate_drive_object_id(&binding.provider_folder_id)?;

    super::with_cloud_tx(db, |tx| {
        // 1. 传入绑定必须与当前目录绑定逐字段一致（目录被移除 / 重登记 → Missing / Stale）。
        let Some(current) = get_folder_on_conn(tx, binding.location_id)? else {
            return Ok(CloudCasOutcome::Missing);
        };
        if current != *binding {
            return Ok(CloudCasOutcome::Stale);
        }
        // 2. 账户仍 connected 且代际匹配（同一事务内 CAS）。
        let account =
            match require_current_account_on_conn(tx, &binding.account_id, expected_generation)? {
                CloudCasOutcome::Applied(account) => account,
                CloudCasOutcome::Stale => return Ok(CloudCasOutcome::Stale),
                CloudCasOutcome::Missing => return Ok(CloudCasOutcome::Missing),
            };
        if account.provider != CLOUD_DRIVE_PROVIDER_ID {
            return Ok(CloudCasOutcome::Missing);
        }
        // 3. 位置必须仍是该账户的 google_drive 位置、Connected 且 root_ref 未变。
        let Some(location) = load_location_on_conn(tx, binding.location_id)? else {
            return Ok(CloudCasOutcome::Missing);
        };
        if location.provider_type != StorageProviderType::GoogleDrive {
            return Err(invalid_binding());
        }
        if location.root_ref != binding.root_ref {
            return Ok(CloudCasOutcome::Stale);
        }
        if location.status != StorageStatus::Connected {
            return Ok(CloudCasOutcome::Stale);
        }
        // 4. 幂等：不重复建内容，只在 Provider 文件名 / 大小变化时刷新绑定与 Resource。
        if let Some(existing) =
            get_object_by_identity_on_conn(tx, binding.location_id, &file.provider_file_id)?
        {
            return Ok(CloudCasOutcome::Applied(CloudPdfImportOutcome::Existing(
                refresh_object_metadata_on_conn(tx, existing, file)?,
            )));
        }
        // 5. 容量门禁：同一事务内计数（不接受无界表）。
        let count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM cloud_object_bindings WHERE location_id = ?1",
                rusqlite::params![binding.location_id.to_string()],
                |row| row.get(0),
            )
            .map_err(map_db_error("统计云盘对象数量失败"))?;
        if count >= i64::from(MAX_CLOUD_OBJECTS_PER_FOLDER) {
            return Err(object_limit_reached());
        }

        let now = UtcMillis::now();
        // 与 scanner 的 media_index_for 对非影音媒介一致；category 不接受调用方传入。
        let work = Work {
            id: WorkId::new(),
            canonical_title: file.display_name.clone(),
            original_title: None,
            sort_title: Some(file.display_name.clone()),
            description: None,
            work_type: WorkType::Standalone,
            release_year: None,
            language: None,
            director: None,
            actor: None,
            status: WorkStatus::Unknown,
            rating_value: None,
            rating_scale: None,
            artwork: Default::default(),
            created_at: now,
            updated_at: now,
        };
        let edition = Edition {
            id: EditionId::new(),
            work_id: work.id,
            title: file.display_name.clone(),
            subtitle: None,
            edition_type: MediaType::Document,
            release_date: None,
            language: None,
            region: None,
            publisher_or_studio: None,
            description: None,
            artwork: Default::default(),
            created_at: now,
            updated_at: now,
        };
        let media_item = MediaItem {
            id: MediaItemId::new(),
            edition_id: edition.id,
            parent_id: None,
            media_type: MediaType::Document,
            title: file.display_name.clone(),
            index: MediaIndex::Custom {
                label: "file".to_owned(),
                ordinal: None,
            },
            duration_ms: None,
            page_count: None,
            chapter_count: None,
            published_at: None,
            status: MediaItemStatus::Available,
            created_at: now,
            updated_at: now,
        };
        let object_binding = CloudObjectBinding {
            id: uuid::Uuid::new_v4().to_string(),
            location_id: binding.location_id,
            provider_file_id: file.provider_file_id.clone(),
            display_name: file.display_name.clone(),
            size_bytes: file.size_bytes,
            resource_id: ResourceId::new(),
            media_item_id: media_item.id,
        };
        // locator 只含内部 object UUID / location UUID；Provider file ID 绝不进入 locator。
        let resource = Resource {
            id: object_binding.resource_id,
            media_item_id: object_binding.media_item_id,
            resource_type: ResourceType::PublicationFile,
            source_id: None,
            storage_location_id: Some(binding.location_id),
            locator: ResourceLocator::StorageObject {
                provider_id: binding.location_id,
                object_id: object_binding.id.clone(),
                path_hint: None,
            },
            mime_type: Some("application/pdf".to_owned()),
            size: Some(file.size_bytes),
            hash: None,
            availability: Availability::Available,
            availability_source: AvailabilitySource::Storage,
            modified_ms: None,
            fingerprint_first: None,
            fingerprint_last: None,
            created_at: now,
            updated_at: now,
        };

        crate::db::repos::work::save_on_conn(tx, &work)?;
        crate::db::repos::edition::save_on_conn(tx, &edition)?;
        crate::db::repos::media_item::save_on_conn(tx, &media_item)?;
        crate::db::repos::resource::save_on_conn(tx, &resource)?;
        tx.execute(
            "INSERT INTO cloud_object_bindings
                (id, location_id, provider_file_id, display_name, size_bytes,
                 resource_id, media_item_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                object_binding.id,
                binding.location_id.to_string(),
                object_binding.provider_file_id,
                object_binding.display_name,
                file.size_bytes as i64,
                object_binding.resource_id.to_string(),
                object_binding.media_item_id.to_string(),
                now.0,
            ],
        )
        .map_err(map_db_error("保存云盘对象绑定失败"))?;
        Ok(CloudCasOutcome::Applied(CloudPdfImportOutcome::Created(
            object_binding,
        )))
    })
}

/// 显式重导入的元数据刷新，仍处于 import_pdf 的目录 / 账户 CAS 事务内。
/// 不改内部身份、locator、用户标题或 availability；原样命中不改时间戳。
fn refresh_object_metadata_on_conn(
    conn: &Connection,
    existing: CloudObjectBinding,
    file: &CloudPdfCandidate,
) -> Result<CloudObjectBinding, AppError> {
    let name_changed = existing.display_name != file.display_name;
    let size_changed = existing.size_bytes != file.size_bytes;
    if !name_changed && !size_changed {
        return Ok(existing);
    }
    conn.execute(
        "UPDATE cloud_object_bindings SET display_name = ?1, size_bytes = ?2 WHERE id = ?3",
        rusqlite::params![file.display_name, file.size_bytes as i64, existing.id],
    )
    .map_err(map_db_error("刷新云盘对象绑定失败"))?;
    // 仅改名无需改写 Resource；大小与绑定必须在同一事务同步。
    if size_changed {
        conn.execute(
            "UPDATE resources SET size = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![
                file.size_bytes as i64,
                UtcMillis::now().0,
                existing.resource_id.to_string()
            ],
        )
        .map_err(map_db_error("刷新云盘资源大小失败"))?;
    }
    Ok(CloudObjectBinding {
        display_name: file.display_name.clone(),
        size_bytes: file.size_bytes,
        ..existing
    })
}

fn invalid_binding() -> AppError {
    AppError::new(
        "CLOUD_BINDING_INVALID",
        ErrorKind::Internal,
        "云盘绑定数据不一致，已拒绝本次读取",
        false,
    )
}

fn object_limit_reached() -> AppError {
    AppError::new(
        "CLOUD_OBJECT_LIMIT",
        ErrorKind::Conflict,
        "云盘目录对象数量已达上限",
        false,
    )
}
