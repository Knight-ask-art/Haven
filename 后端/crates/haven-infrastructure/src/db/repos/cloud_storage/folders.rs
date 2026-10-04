//! 云盘目录绑定（Sqlite）：读取 / 登记 / 移除。
//!
//! 边界（契约见 `haven_application::services::cloud_storage::state`）：
//! - 每个公开函数 = 一个短事务（`with_cloud_tx`；移除目录走 `with_cloud_purge_tx`，
//!   以便在事务边界外做 purge 的 TEMP 自清理），读→校验→写原子；无网络与文件系统 IO。
//! - 真实 Provider ID 只与自己的列比较，一律 `COLLATE BINARY`（Google ID 大小写敏感）；
//!   内部 location / root_ref 是内部 UUID，绝不写入 Provider ID。
//! - 移除目录只清理**应用内索引**：与本地位置 remove 共用同一份 purge 算法（该位置
//!   Resource + 仅由该位置派生的孤儿内容链与用户状态）；远端原始文件绝不删除；共享账户
//!   凭据、同账户其他目录与仍被其他位置引用的共享内容不受影响；非云盘位置一律不动。
//! - 错误文案不含 SQL、Provider ID 或本地路径。

use rusqlite::Connection;

use haven_application::services::cloud_storage::ports::{
    CLOUD_DRIVE_PROVIDER_ID, validate_drive_object_id,
};
use haven_application::services::cloud_storage::state::{
    CloudCasOutcome, CloudFolderBinding, CloudFolderRemoveOutcome, CloudFolderRequest,
    MAX_CLOUD_FOLDERS_PER_ACCOUNT,
};
use haven_common::{AppError, ErrorKind, UtcMillis, validation};
use haven_domain::enums::{StorageProviderType, StorageStatus};
use haven_domain::ids::StorageLocationId;

use super::accounts::{get_account_on_conn, require_current_account_on_conn};
use crate::db::Db;
use crate::db::repos::{enum_to_db_str, id_from_row, map_db_error};

const SELECT_FOLDER_COLUMNS: &str =
    "location_id, account_id, provider_folder_id, root_ref, created_at";

fn row_to_folder(row: &rusqlite::Row<'_>) -> rusqlite::Result<CloudFolderBinding> {
    Ok(CloudFolderBinding {
        location_id: id_from_row::<StorageLocationId>(row.get("location_id")?)?,
        account_id: row.get("account_id")?,
        provider_folder_id: row.get("provider_folder_id")?,
        root_ref: row.get("root_ref")?,
        created_at: UtcMillis(row.get("created_at")?),
    })
}

/// 内部 UUID（账户 / location / root_ref / 对象）与 Provider ID 分开校验：内部 ID 必须是
/// UUID；Provider ID 一律走 `validate_drive_object_id`（大小写敏感的外部字母表）。
pub(super) fn validate_internal_uuid(value: &str, message: &'static str) -> Result<(), AppError> {
    let parsed = uuid::Uuid::parse_str(value).map_err(|_| validation(message))?;
    if value.len() != 36 || parsed.hyphenated().to_string() != value {
        return Err(validation(message));
    }
    Ok(())
}

/// 事务内按 location 读取目录绑定；对象侧的目录 / 账户一致性校验复用它做 join。
pub(super) fn get_folder_on_conn(
    conn: &Connection,
    location_id: StorageLocationId,
) -> Result<Option<CloudFolderBinding>, AppError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {SELECT_FOLDER_COLUMNS} FROM cloud_folder_bindings WHERE location_id = ?1"
        ))
        .map_err(map_db_error("查询云盘目录绑定失败"))?;
    let mut rows = stmt
        .query_map(rusqlite::params![location_id.to_string()], row_to_folder)
        .map_err(map_db_error("查询云盘目录绑定失败"))?;
    rows.next()
        .transpose()
        .map_err(map_db_error("查询云盘目录绑定失败"))
}

/// 按 Provider 身份精确查找：`provider_folder_id` 大小写敏感（`COLLATE BINARY`）。
fn get_folder_by_identity_on_conn(
    conn: &Connection,
    account_id: &str,
    provider_folder_id: &str,
) -> Result<Option<CloudFolderBinding>, AppError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {SELECT_FOLDER_COLUMNS} FROM cloud_folder_bindings
             WHERE account_id = ?1 AND provider_folder_id = ?2 COLLATE BINARY"
        ))
        .map_err(map_db_error("查询云盘目录绑定失败"))?;
    let mut rows = stmt
        .query_map(
            rusqlite::params![account_id, provider_folder_id],
            row_to_folder,
        )
        .map_err(map_db_error("查询云盘目录绑定失败"))?;
    rows.next()
        .transpose()
        .map_err(map_db_error("查询云盘目录绑定失败"))
}

pub(super) fn get_folder(
    db: &Db,
    location_id: StorageLocationId,
) -> Result<Option<CloudFolderBinding>, AppError> {
    let conn = db.lock();
    get_folder_on_conn(&conn, location_id)
}

/// 登记目录（幂等）：账户必须存在、`connected` 且代际匹配（同一事务内 CAS）；同一
/// `(account, provider_folder_id)` 已存在时原样返回既有绑定，不新建位置、不覆盖。
pub(super) fn register_folder(
    db: &Db,
    request: &CloudFolderRequest,
    expected_generation: i64,
    display_name: &str,
) -> Result<CloudCasOutcome<CloudFolderBinding>, AppError> {
    validate_internal_uuid(&request.account_id, "云盘账户内部 ID 非法")?;
    validate_drive_object_id(&request.provider_folder_id)?;
    let display_name = display_name.trim();
    if display_name.is_empty()
        || display_name.chars().count() > 120
        || display_name.chars().any(char::is_control)
    {
        return Err(validation("云盘目录名称非法"));
    }

    super::with_cloud_tx(db, |tx| {
        let account =
            match require_current_account_on_conn(tx, &request.account_id, expected_generation)? {
                CloudCasOutcome::Applied(account) => account,
                CloudCasOutcome::Stale => return Ok(CloudCasOutcome::Stale),
                CloudCasOutcome::Missing => return Ok(CloudCasOutcome::Missing),
            };
        if account.provider != CLOUD_DRIVE_PROVIDER_ID {
            return Err(validation("该账户不是受支持的云盘提供方"));
        }
        if let Some(existing) =
            get_folder_by_identity_on_conn(tx, &request.account_id, &request.provider_folder_id)?
        {
            return Ok(CloudCasOutcome::Applied(existing));
        }
        let count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM cloud_folder_bindings WHERE account_id = ?1",
                rusqlite::params![request.account_id],
                |row| row.get(0),
            )
            .map_err(map_db_error("统计云盘目录数量失败"))?;
        if count >= i64::from(MAX_CLOUD_FOLDERS_PER_ACCOUNT) {
            return Err(folder_limit_reached());
        }

        let now = UtcMillis::now();
        let location_id = StorageLocationId::new();
        // root_ref 只写内部 UUID：Google folder ID 大小写敏感，会被 008 的
        // lower(root_ref) 唯一索引错误折叠成同一位置。
        let root_ref = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO storage_locations
                (id, provider_type, display_name, root_ref, credential_ref, status,
                 created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?6)",
            rusqlite::params![
                location_id.to_string(),
                enum_to_db_str(&StorageProviderType::GoogleDrive)?,
                display_name,
                root_ref,
                enum_to_db_str(&StorageStatus::Connected)?,
                now.0,
            ],
        )
        .map_err(map_db_error("保存云盘存储位置失败"))?;
        // 账户存在（上方 CAS 已确认），location 已先写入 → 外键一致。
        tx.execute(
            "INSERT INTO cloud_folder_bindings
                (location_id, account_id, provider_folder_id, root_ref, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                location_id.to_string(),
                request.account_id,
                request.provider_folder_id,
                root_ref,
                now.0,
            ],
        )
        .map_err(map_db_error("保存云盘目录绑定失败"))?;
        Ok(CloudCasOutcome::Applied(CloudFolderBinding {
            location_id,
            account_id: request.account_id.clone(),
            provider_folder_id: request.provider_folder_id.clone(),
            root_ref,
            created_at: now,
        }))
    })
}

/// 移除目录：只处理 `google_drive` 账户持有的云盘绑定；其余位置（本地 / 无绑定 / 外部
/// 账户）返回 `Absent` 且不做任何写入——本地数据绝不受影响。
///
/// 判定通过后，应用内清理与本地位置 remove **完全同一算法**
/// （`storage_content::purge_location_content`）：删除该位置 Resource 与仅由该位置派生的
/// 孤儿内容链及其用户状态，再清理该位置的下载元数据与云盘绑定行。远端（Provider）文件
/// 绝不删除；共享账户凭据、同账户其他目录、仍被其他位置引用的共享内容完整保留。
pub(super) fn remove_folder(
    db: &Db,
    location_id: StorageLocationId,
) -> Result<CloudFolderRemoveOutcome, AppError> {
    super::with_cloud_purge_tx(db, |conn| {
        let Some(binding) = get_folder_on_conn(conn, location_id)? else {
            return Ok(CloudFolderRemoveOutcome::Absent);
        };
        let is_google_location: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM storage_locations WHERE id = ?1 AND provider_type = ?2
                 )",
                rusqlite::params![
                    location_id.to_string(),
                    enum_to_db_str(&StorageProviderType::GoogleDrive)?
                ],
                |row| row.get(0),
            )
            .map_err(map_db_error("校验云盘存储位置失败"))?;
        if !is_google_location {
            return Ok(CloudFolderRemoveOutcome::Absent);
        }
        let Some(account) = get_account_on_conn(conn, &binding.account_id)? else {
            return Ok(CloudFolderRemoveOutcome::Absent);
        };
        if account.provider != CLOUD_DRIVE_PROVIDER_ID {
            return Ok(CloudFolderRemoveOutcome::Absent);
        }

        let location = location_id.to_string();
        // 1. 与本地 remove **同一算法**（同一份实现，非复制 SQL）：删除该位置 Resource 与仅由
        //    该位置派生的孤儿内容链及其用户状态；远端文件不受影响，共享内容完整保留。
        crate::db::storage_content::purge_location_content(conn, location_id)?;
        // 2. 该位置自己的下载批次（target_storage_id RESTRICT）随位置清理；其他位置的任务与
        //    批次保留不动。（下载任务已由 purge 的 RESTRICT 清理覆盖。）
        conn.execute(
            "DELETE FROM download_batches WHERE target_storage_id = ?1",
            rusqlite::params![location],
        )
        .map_err(map_db_error("清理云盘目录下载批次失败"))?;
        // 3. 只解绑云盘行：资源行已随 purge 清理，共享账户凭据与其他目录不受影响。
        conn.execute(
            "DELETE FROM cloud_object_bindings WHERE location_id = ?1",
            rusqlite::params![location],
        )
        .map_err(map_db_error("删除云盘对象绑定失败"))?;
        conn.execute(
            "DELETE FROM cloud_folder_bindings WHERE location_id = ?1",
            rusqlite::params![location],
        )
        .map_err(map_db_error("删除云盘目录绑定失败"))?;
        conn.execute(
            "DELETE FROM storage_locations WHERE id = ?1",
            rusqlite::params![location],
        )
        .map_err(map_db_error("删除云盘存储位置失败"))?;
        Ok(CloudFolderRemoveOutcome::Removed)
    })
}

fn folder_limit_reached() -> AppError {
    AppError::new(
        "CLOUD_FOLDER_LIMIT",
        ErrorKind::Conflict,
        "云盘目录数量已达上限",
        false,
    )
}
