//! 云盘账户生命周期（连接 / 换凭据 / 断开）的 SQLite 同步助手。
//!
//! 边界：
//! - 只读写 `cloud_accounts` / `cloud_folder_bindings` / `storage_locations` /
//!   `resources` 与凭据清理 outbox 的行；不做网络 IO、不碰 keystore、不落令牌材料。
//! - 事务入口是 [`Db`] 的同步方法（写走 `Db::with_tx`，读走 `Db::lock`）；跨模块复用的
//!   `*_on_conn` 函数只接收 `&Connection`，由调用方保证已在同一事务 / 同一把锁内。
//! - CAS 前置条件（存在性、代际、connected、当前 ref、暂存引用新鲜度）在**任何写入之前**
//!   读完：不满足即 `Stale` / `Missing`，事务内不会留下半截副作用。
//! - 错误消息是固定文本，不含 SQL、账户 ID、Provider ID 或凭据引用。

use rusqlite::{Connection, OptionalExtension};

use haven_application::services::cloud_storage::ports::CLOUD_DRIVE_PROVIDER_ID;
use haven_application::services::cloud_storage::state::{
    CloudAccount, CloudCasOutcome, CloudConnectRequest, MAX_CLOUD_ACCOUNTS,
};
use haven_common::{AppError, ErrorKind, UtcMillis, validation};
use haven_domain::enums::{Availability, AvailabilitySource, StorageProviderType, StorageStatus};
use haven_domain::ids::{CredentialRef, StorageLocationId};

use crate::db::Db;
use crate::db::repos::{enum_to_db_str, id_from_row, map_db_error};
use crate::db::storage_content::set_resources_availability_on_conn;

use super::cleanup::{
    consume_staged_credential_on_conn, fresh_staged_credential_on_conn, is_scoped_credential,
    queue_ready_credential_on_conn,
};
use super::with_cloud_tx;

const ACCOUNT_COLUMNS: &str = "id, provider, provider_account_id, display_name, generation, \
     connected, credential_ref, updated_at";

/// 行级不变量失败：整行拒绝，消息固定且不回显列值。
fn corrupt_account_row() -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "云盘账户行损坏",
        )),
    )
}

fn is_valid_provider(value: &str) -> bool {
    (1..=32).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn is_valid_provider_account_id(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn is_valid_display_name(value: &str) -> bool {
    let characters = value.chars().count();
    (1..=120).contains(&characters) && !value.chars().any(char::is_control)
}

/// 行 → [`CloudAccount`]，逐列复核存储层不变量：内部 UUID、Provider 身份、名称、
/// 代际 > 0、连接位 ∈ {0,1}、凭据 ref 可解析且与 provider 同作用域。脏行绝不静默降级。
pub(super) fn row_to_cloud_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<CloudAccount> {
    let id: String = row.get("id")?;
    if id.len() != 36 || uuid::Uuid::parse_str(&id).is_err() {
        return Err(corrupt_account_row());
    }
    let provider: String = row.get("provider")?;
    if !is_valid_provider(&provider) || provider != CLOUD_DRIVE_PROVIDER_ID {
        return Err(corrupt_account_row());
    }
    let provider_account_id: String = row.get("provider_account_id")?;
    if !is_valid_provider_account_id(&provider_account_id) {
        return Err(corrupt_account_row());
    }
    let display_name: String = row.get("display_name")?;
    if !is_valid_display_name(&display_name) {
        return Err(corrupt_account_row());
    }
    let generation: i64 = row.get("generation")?;
    if generation <= 0 {
        return Err(corrupt_account_row());
    }
    let connected = match row.get::<_, i64>("connected")? {
        0 => false,
        1 => true,
        _ => return Err(corrupt_account_row()),
    };
    let credential_ref = match row.get::<_, Option<String>>("credential_ref")? {
        Some(raw) => {
            let parsed = raw
                .parse::<CredentialRef>()
                .map_err(|_| corrupt_account_row())?;
            let scoped = parsed
                .as_str()
                .strip_prefix("haven:")
                .and_then(|rest| rest.split_once(':'))
                .is_some_and(|(owner, _)| owner == provider);
            if !scoped {
                return Err(corrupt_account_row());
            }
            Some(parsed)
        }
        None => None,
    };
    if connected && credential_ref.is_none() {
        return Err(corrupt_account_row());
    }
    let updated_at: i64 = row.get("updated_at")?;
    if updated_at < 0 {
        return Err(corrupt_account_row());
    }
    Ok(CloudAccount {
        id,
        provider,
        provider_account_id,
        display_name,
        generation,
        connected,
        credential_ref,
        updated_at: UtcMillis(updated_at),
    })
}

// ---------- 读 ----------

pub(super) fn list_accounts_on_conn(conn: &Connection) -> Result<Vec<CloudAccount>, AppError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {ACCOUNT_COLUMNS} FROM cloud_accounts ORDER BY updated_at DESC, id ASC"
        ))
        .map_err(map_db_error("查询云盘账户列表失败"))?;
    let rows = stmt
        .query_map([], row_to_cloud_account)
        .map_err(map_db_error("查询云盘账户列表失败"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(map_db_error("查询云盘账户列表失败"))
}

pub(super) fn get_account_on_conn(
    conn: &Connection,
    account_id: &str,
) -> Result<Option<CloudAccount>, AppError> {
    conn.query_row(
        &format!("SELECT {ACCOUNT_COLUMNS} FROM cloud_accounts WHERE id = ?1"),
        rusqlite::params![account_id],
        row_to_cloud_account,
    )
    .optional()
    .map_err(map_db_error("查询云盘账户失败"))
}

pub(super) fn get_account_by_provider_on_conn(
    conn: &Connection,
    provider: &str,
    provider_account_id: &str,
) -> Result<Option<CloudAccount>, AppError> {
    conn.query_row(
        &format!(
            "SELECT {ACCOUNT_COLUMNS} FROM cloud_accounts
              WHERE provider = ?1 AND provider_account_id = ?2 COLLATE BINARY"
        ),
        rusqlite::params![provider, provider_account_id],
        row_to_cloud_account,
    )
    .optional()
    .map_err(map_db_error("查询云盘账户失败"))
}

/// 兄弟模块（目录 / 对象）共用的前置校验：账户必须**既存在、又 connected、且代际匹配**。
/// 三者缺一即 `Missing` / `Stale`，调用方据此拒绝写入。
pub(super) fn require_current_account_on_conn(
    conn: &Connection,
    account_id: &str,
    expected_generation: i64,
) -> Result<CloudCasOutcome<CloudAccount>, AppError> {
    match get_account_on_conn(conn, account_id)? {
        None => Ok(CloudCasOutcome::Missing),
        Some(account) if account.connected && account.generation == expected_generation => {
            Ok(CloudCasOutcome::Applied(account))
        }
        Some(_) => Ok(CloudCasOutcome::Stale),
    }
}

// ---------- 写：账户生命周期 ----------

fn account_limit_reached() -> AppError {
    AppError::new(
        "CLOUD_ACCOUNT_LIMIT_REACHED",
        ErrorKind::Conflict,
        "云盘账户数量已达上限",
        false,
    )
}

fn generation_overflow() -> AppError {
    AppError::new(
        "CLOUD_ACCOUNT_GENERATION_OVERFLOW",
        ErrorKind::Database,
        "云盘账户授权代际溢出",
        false,
    )
}

/// 已读到 / 已校验的行在条件写时消失：属于内部不变量破坏，必须报错回滚，
/// 绝不降级成 `Stale`（那会在事务里留下半截副作用）。
fn write_lost(message: &'static str) -> AppError {
    AppError::new(
        "CLOUD_ACCOUNT_WRITE_LOST",
        ErrorKind::Database,
        message,
        true,
    )
}

fn validate_connect_request(request: &CloudConnectRequest) -> Result<(), AppError> {
    if request.provider != CLOUD_DRIVE_PROVIDER_ID {
        return Err(validation("云盘账户仅支持 google_drive"));
    }
    if !is_valid_provider_account_id(&request.provider_account_id) {
        return Err(validation("云盘账户标识非法"));
    }
    if !is_valid_display_name(&request.display_name) {
        return Err(validation("云盘账户名称非法"));
    }
    Ok(())
}

fn validate_new_credential(credential: &CredentialRef) -> Result<(), AppError> {
    if !is_scoped_credential(credential) {
        return Err(validation("仅支持 google_drive 作用域的凭据引用"));
    }
    Ok(())
}

fn count_accounts_on_conn(conn: &Connection) -> Result<i64, AppError> {
    conn.query_row("SELECT COUNT(*) FROM cloud_accounts", [], |row| row.get(0))
        .map_err(map_db_error("统计云盘账户失败"))
}

/// 账户名下的目录位置状态迁移（连接恢复 / 断开都只碰自己的绑定行）。
fn set_account_locations_status_on_conn(
    conn: &Connection,
    account_id: &str,
    status: StorageStatus,
    now: UtcMillis,
) -> Result<u64, AppError> {
    let affected = conn
        .execute(
            "UPDATE storage_locations
                SET status = ?1, updated_at = ?2
              WHERE provider_type = ?3
                AND id IN (
                    SELECT location_id FROM cloud_folder_bindings WHERE account_id = ?4
                )",
            rusqlite::params![
                enum_to_db_str(&status)?,
                now.0,
                enum_to_db_str(&StorageProviderType::GoogleDrive)?,
                account_id,
            ],
        )
        .map_err(map_db_error("更新云盘位置状态失败"))?;
    Ok(affected as u64)
}

/// 账户名下全部云盘目录位置：断开时逐位置套用规范覆盖规则（`storage_content` 的
/// R-MAIN-08 规则 a/b），而不是在这里另写一套谓词。
fn account_location_ids_on_conn(
    conn: &Connection,
    account_id: &str,
) -> Result<Vec<StorageLocationId>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT location_id FROM cloud_folder_bindings
              WHERE account_id = ?1 ORDER BY location_id",
        )
        .map_err(map_db_error("查询云盘账户目录失败"))?;
    let rows = stmt
        .query_map(rusqlite::params![account_id], |row| {
            id_from_row::<StorageLocationId>(row.get(0)?)
        })
        .map_err(map_db_error("查询云盘账户目录失败"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(map_db_error("查询云盘账户目录失败"))
}

pub(super) fn connect_on_conn(
    conn: &Connection,
    request: &CloudConnectRequest,
    expected_prior_generation: Option<i64>,
    new_credential: &CredentialRef,
    now: UtcMillis,
) -> Result<CloudCasOutcome<CloudAccount>, AppError> {
    validate_connect_request(request)?;
    validate_new_credential(new_credential)?;

    let existing =
        get_account_by_provider_on_conn(conn, &request.provider, &request.provider_account_id)?;
    let (account_id, next_generation) = match existing.as_ref() {
        Some(account) => {
            // 期望「尚无该账户」或代际不符：已有行一律 Stale，绝不覆盖别人的连接。
            if expected_prior_generation != Some(account.generation) {
                return Ok(CloudCasOutcome::Stale);
            }
            let next = account
                .generation
                .checked_add(1)
                .ok_or_else(generation_overflow)?;
            (account.id.clone(), next)
        }
        None => {
            if expected_prior_generation.is_some() {
                return Ok(CloudCasOutcome::Missing);
            }
            if count_accounts_on_conn(conn)? >= i64::from(MAX_CLOUD_ACCOUNTS) {
                return Err(account_limit_reached());
            }
            (uuid::Uuid::new_v4().to_string(), 1)
        }
    };
    // 先 stage + store.set 才允许 connect：暂存行必须存在、未 ready、未超 TTL。
    if !fresh_staged_credential_on_conn(conn, new_credential, now)? {
        return Ok(CloudCasOutcome::Stale);
    }

    // ---- 以下为写入：全部前置条件已满足 ----
    if let Some(previous) = existing
        .as_ref()
        .and_then(|account| account.credential_ref.as_ref())
    {
        queue_ready_credential_on_conn(conn, previous, now)?;
    }
    consume_staged_credential_on_conn(conn, new_credential)?;
    conn.execute(
        "INSERT INTO cloud_accounts
            (id, provider, provider_account_id, display_name, generation, connected,
             credential_ref, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
             display_name = excluded.display_name,
             generation = excluded.generation,
             connected = 1,
             credential_ref = excluded.credential_ref,
             updated_at = excluded.updated_at",
        rusqlite::params![
            account_id,
            request.provider,
            request.provider_account_id,
            request.display_name,
            next_generation,
            new_credential.as_str(),
            now.0,
        ],
    )
    .map_err(map_db_error("保存云盘账户失败"))?;
    // Restore only the location-auth invalidation owned by this account. User/unknown failures
    // remain unchanged; every actual PDF read still re-stats its identity, MIME, size and parents.
    set_account_locations_status_on_conn(conn, &account_id, StorageStatus::Connected, now)?;
    conn.execute(
        "UPDATE resources SET availability = 'available', updated_at = ?1
         WHERE availability = 'storage_unavailable' AND availability_source = 'storage'
           AND storage_location_id IN (
               SELECT b.location_id FROM cloud_folder_bindings b
               JOIN storage_locations s ON s.id = b.location_id AND s.root_ref = b.root_ref
               WHERE b.account_id = ?2 AND s.provider_type = 'google_drive'
                 AND s.status = 'connected' AND s.credential_ref IS NULL
           )",
        rusqlite::params![now.0, account_id],
    )
    .map_err(map_db_error("恢复云盘授权可用性失败"))?;
    let stored =
        get_account_on_conn(conn, &account_id)?.ok_or_else(|| write_lost("保存云盘账户失败"))?;
    Ok(CloudCasOutcome::Applied(stored))
}

pub(super) fn replace_credential_on_conn(
    conn: &Connection,
    account_id: &str,
    expected_generation: i64,
    expected_credential: &CredentialRef,
    new_credential: &CredentialRef,
    now: UtcMillis,
) -> Result<CloudCasOutcome<CloudAccount>, AppError> {
    validate_new_credential(new_credential)?;
    if new_credential == expected_credential {
        return Err(validation("新旧凭据引用不能相同"));
    }
    let Some(account) = get_account_on_conn(conn, account_id)? else {
        return Ok(CloudCasOutcome::Missing);
    };
    if !account.connected
        || account.generation != expected_generation
        || account.credential_ref.as_ref() != Some(expected_credential)
    {
        return Ok(CloudCasOutcome::Stale);
    }
    if !fresh_staged_credential_on_conn(conn, new_credential, now)? {
        return Ok(CloudCasOutcome::Stale);
    }

    // ---- 以下为写入：旧 ref 转 ready、新 ref 消耗、账户行换代不换代 ----
    queue_ready_credential_on_conn(conn, expected_credential, now)?;
    consume_staged_credential_on_conn(conn, new_credential)?;
    let affected = conn
        .execute(
            "UPDATE cloud_accounts
                SET credential_ref = ?1, updated_at = ?2
              WHERE id = ?3 AND connected = 1 AND generation = ?4 AND credential_ref = ?5",
            rusqlite::params![
                new_credential.as_str(),
                now.0,
                account_id,
                expected_generation,
                expected_credential.as_str(),
            ],
        )
        .map_err(map_db_error("更新云盘账户凭据失败"))?;
    if affected == 0 {
        return Err(write_lost("更新云盘账户凭据失败"));
    }
    let stored =
        get_account_on_conn(conn, account_id)?.ok_or_else(|| write_lost("更新云盘账户凭据失败"))?;
    Ok(CloudCasOutcome::Applied(stored))
}

pub(super) fn disconnect_on_conn(
    conn: &Connection,
    account_id: &str,
    expected_generation: i64,
    now: UtcMillis,
) -> Result<CloudCasOutcome<CloudAccount>, AppError> {
    let Some(account) = get_account_on_conn(conn, account_id)? else {
        return Ok(CloudCasOutcome::Missing);
    };
    if account.generation != expected_generation {
        return Ok(CloudCasOutcome::Stale);
    }
    let next_generation = account
        .generation
        .checked_add(1)
        .ok_or_else(generation_overflow)?;

    // 墓碑 + 换代；credential_ref 原样保留到 keystore 删除收尾（ADR-001）。
    if let Some(previous) = account.credential_ref.as_ref() {
        queue_ready_credential_on_conn(conn, previous, now)?;
    }
    let affected = conn
        .execute(
            "UPDATE cloud_accounts
                SET connected = 0, generation = ?1, updated_at = ?2
              WHERE id = ?3",
            rusqlite::params![next_generation, now.0, account_id],
        )
        .map_err(map_db_error("断开云盘账户失败"))?;
    if affected == 0 {
        return Err(write_lost("断开云盘账户失败"));
    }
    set_account_locations_status_on_conn(conn, account_id, StorageStatus::Disconnected, now)?;
    // 断开语义与本地位置一致：按 R-MAIN-08 规则 a/b 逐位置标记不可用——覆盖 available
    // （即便 source=user）与 source=storage，绝不覆盖 user 显式标记的不可用状态与未知状态。
    for location_id in account_location_ids_on_conn(conn, account_id)? {
        set_resources_availability_on_conn(
            conn,
            location_id,
            Availability::StorageUnavailable,
            AvailabilitySource::Storage,
        )?;
    }
    let stored =
        get_account_on_conn(conn, account_id)?.ok_or_else(|| write_lost("断开云盘账户失败"))?;
    Ok(CloudCasOutcome::Applied(stored))
}

// ---------- Db 入口（写走 with_tx，读走 lock） ----------

impl Db {
    pub(super) fn list_accounts(&self) -> Result<Vec<CloudAccount>, AppError> {
        let conn = self.lock();
        list_accounts_on_conn(&conn)
    }

    pub(super) fn get_account(&self, account_id: &str) -> Result<Option<CloudAccount>, AppError> {
        let conn = self.lock();
        get_account_on_conn(&conn, account_id)
    }

    pub(super) fn get_account_by_provider(
        &self,
        provider: &str,
        provider_account_id: &str,
    ) -> Result<Option<CloudAccount>, AppError> {
        let conn = self.lock();
        get_account_by_provider_on_conn(&conn, provider, provider_account_id)
    }

    pub(super) fn connect(
        &self,
        request: &CloudConnectRequest,
        expected_prior_generation: Option<i64>,
        new_credential: &CredentialRef,
    ) -> Result<CloudCasOutcome<CloudAccount>, AppError> {
        with_cloud_tx(self, |tx| {
            connect_on_conn(
                tx,
                request,
                expected_prior_generation,
                new_credential,
                UtcMillis::now(),
            )
        })
    }

    pub(super) fn replace_credential(
        &self,
        account_id: &str,
        expected_generation: i64,
        expected_credential: &CredentialRef,
        new_credential: &CredentialRef,
    ) -> Result<CloudCasOutcome<CloudAccount>, AppError> {
        with_cloud_tx(self, |tx| {
            replace_credential_on_conn(
                tx,
                account_id,
                expected_generation,
                expected_credential,
                new_credential,
                UtcMillis::now(),
            )
        })
    }

    pub(super) fn disconnect(
        &self,
        account_id: &str,
        expected_generation: i64,
    ) -> Result<CloudCasOutcome<CloudAccount>, AppError> {
        with_cloud_tx(self, |tx| {
            disconnect_on_conn(tx, account_id, expected_generation, UtcMillis::now())
        })
    }
}
