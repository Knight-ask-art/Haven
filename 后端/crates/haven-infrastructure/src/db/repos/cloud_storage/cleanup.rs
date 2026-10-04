//! 凭据清理 outbox（stage → keystore delete → finish）的 SQLite 同步助手。
//!
//! 边界：
//! - outbox 只有不透明 `haven:google_drive:<profile-id>` 引用与两个标量列，没有令牌、
//!   scope、授权码或 Provider ID。
//! - 事务入口是 [`Db`] 的同步方法（写走 `Db::with_tx`，读走 `Db::lock`）；`*_on_conn`
//!   只接收 `&Connection`，由调用方保证已在同一事务 / 同一把锁内。
//! - staged 行属于「store.set 之前」的调用方：普通清理（pending / finish）**从不**消费
//!   staged，只有 TTL 恢复或显式 retire 才把它转成 ready。
//! - 错误消息是固定文本，不含 SQL、凭据引用或账户 ID。

use rusqlite::{Connection, OptionalExtension};

use haven_application::services::cloud_storage::ports::CLOUD_DRIVE_PROVIDER_ID;
use haven_application::services::cloud_storage::state::{
    CLOUD_CREDENTIAL_CLEANUP_BATCH, CLOUD_CREDENTIAL_STAGE_TTL_MS, CloudCredentialCleanupOutcome,
    MAX_CLOUD_STAGED_CREDENTIALS,
};
use haven_common::{AppError, ErrorKind, UtcMillis, validation};
use haven_domain::ids::CredentialRef;

use super::with_cloud_tx;
use crate::db::Db;
use crate::db::repos::map_db_error;

/// outbox 与账户行只接受这一作用域的不透明引用（与迁移 052 的 CHECK 同源）。
pub(super) fn is_scoped_credential(credential: &CredentialRef) -> bool {
    match credential.as_str().strip_prefix("haven:") {
        Some(rest) => matches!(
            rest.split_once(':'),
            Some((provider, _)) if provider == CLOUD_DRIVE_PROVIDER_ID
        ),
        None => false,
    }
}

fn corrupt_cleanup_row() -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "云盘凭据清理行损坏",
        )),
    )
}

fn credential_in_use() -> AppError {
    AppError::new(
        "CLOUD_CREDENTIAL_IN_USE",
        ErrorKind::Conflict,
        "凭据引用仍在使用中",
        false,
    )
}

fn credential_already_staged() -> AppError {
    AppError::new(
        "CLOUD_CREDENTIAL_ALREADY_STAGED",
        ErrorKind::Conflict,
        "凭据引用已在清理队列中",
        false,
    )
}

fn outbox_full() -> AppError {
    AppError::new(
        "CLOUD_CREDENTIAL_OUTBOX_FULL",
        ErrorKind::Conflict,
        "凭据清理队列已满",
        true,
    )
}

/// 读取 `(cleanup_ready, staged_at)`；不存在返回 `None`。
fn staged_row_on_conn(
    conn: &Connection,
    credential: &CredentialRef,
) -> Result<Option<(bool, i64)>, AppError> {
    conn.query_row(
        "SELECT cleanup_ready, staged_at FROM cloud_credential_cleanup WHERE credential_ref = ?1",
        rusqlite::params![credential.as_str()],
        |row| Ok((row.get::<_, i64>(0)? != 0, row.get::<_, i64>(1)?)),
    )
    .optional()
    .map_err(map_db_error("查询待清理凭据失败"))
}

/// 任何账户（含断开墓碑）当前登记的凭据引用都算「有主」。
fn account_owns_credential_on_conn(
    conn: &Connection,
    credential: &CredentialRef,
) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM cloud_accounts WHERE credential_ref = ?1)",
        rusqlite::params![credential.as_str()],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
    .map_err(map_db_error("查询云盘账户失败"))
}

/// connected 账户正在使用的活动引用。
fn active_credential_on_conn(
    conn: &Connection,
    credential: &CredentialRef,
) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM cloud_accounts WHERE credential_ref = ?1 AND connected = 1
         )",
        rusqlite::params![credential.as_str()],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
    .map_err(map_db_error("查询云盘账户失败"))
}

fn outbox_len_on_conn(conn: &Connection) -> Result<i64, AppError> {
    conn.query_row("SELECT COUNT(*) FROM cloud_credential_cleanup", [], |row| {
        row.get(0)
    })
    .map_err(map_db_error("统计待清理凭据失败"))
}

// ---------- connect / replace 的暂存前置条件 ----------

/// staged、未 ready、未超固定 TTL 三者同时成立才允许被 connect / replace 消耗。
pub(super) fn fresh_staged_credential_on_conn(
    conn: &Connection,
    credential: &CredentialRef,
    now: UtcMillis,
) -> Result<bool, AppError> {
    let cutoff = now.0.saturating_sub(CLOUD_CREDENTIAL_STAGE_TTL_MS);
    match staged_row_on_conn(conn, credential)? {
        Some((ready, staged_at)) => Ok(!ready && staged_at > cutoff && staged_at <= now.0),
        None => Ok(false),
    }
}

pub(super) fn consume_staged_credential_on_conn(
    conn: &Connection,
    credential: &CredentialRef,
) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM cloud_credential_cleanup
          WHERE credential_ref = ?1 AND cleanup_ready = 0",
        rusqlite::params![credential.as_str()],
    )
    .map_err(map_db_error("消费暂存凭据失败"))?;
    Ok(())
}

/// 登记 ready 清理行：不存在则新建，已存在只置 ready —— **绝不**改写 staged_at，
/// 因此不会覆盖原清理所有者，也不会因后来的调用延长 TTL。
pub(super) fn queue_ready_credential_on_conn(
    conn: &Connection,
    credential: &CredentialRef,
    now: UtcMillis,
) -> Result<(), AppError> {
    if !is_scoped_credential(credential) {
        return Err(validation("仅支持 google_drive 作用域的凭据引用"));
    }
    conn.execute(
        "INSERT INTO cloud_credential_cleanup (credential_ref, staged_at, cleanup_ready)
         VALUES (?1, ?2, 1)
         ON CONFLICT(credential_ref) DO UPDATE SET cleanup_ready = 1",
        rusqlite::params![credential.as_str(), now.0],
    )
    .map_err(map_db_error("登记待清理凭据失败"))?;
    Ok(())
}

// ---------- outbox 操作 ----------

pub(super) fn stage_credential_on_conn(
    conn: &Connection,
    credential: &CredentialRef,
    now: UtcMillis,
) -> Result<(), AppError> {
    if !is_scoped_credential(credential) {
        return Err(validation("仅支持 google_drive 作用域的凭据引用"));
    }
    if account_owns_credential_on_conn(conn, credential)? {
        return Err(credential_in_use());
    }
    if staged_row_on_conn(conn, credential)?.is_some() {
        return Err(credential_already_staged());
    }
    if outbox_len_on_conn(conn)? >= i64::from(MAX_CLOUD_STAGED_CREDENTIALS) {
        return Err(outbox_full());
    }
    conn.execute(
        "INSERT INTO cloud_credential_cleanup (credential_ref, staged_at, cleanup_ready)
         VALUES (?1, ?2, 0)",
        rusqlite::params![credential.as_str(), now.0],
    )
    .map_err(map_db_error("登记待清理凭据失败"))?;
    Ok(())
}

/// 失败的 store.set / 迟到的写入补偿：staged → ready，缺失则重建 ready 行；
/// 但**绝不**退休 connected 账户正在使用的活动引用。
pub(super) fn retire_staged_credential_on_conn(
    conn: &Connection,
    credential: &CredentialRef,
    now: UtcMillis,
) -> Result<(), AppError> {
    if active_credential_on_conn(conn, credential)? {
        return Err(credential_in_use());
    }
    queue_ready_credential_on_conn(conn, credential, now)
}

/// 超过固定 staging TTL 且**不是**任何 connected 账户活动引用的 staged 行转 ready，
/// 一批最多 [`CLOUD_CREDENTIAL_CLEANUP_BATCH`] 条。
pub(super) fn recover_staged_credentials_on_conn(
    conn: &Connection,
    now: UtcMillis,
) -> Result<u32, AppError> {
    let cutoff = now.0.saturating_sub(CLOUD_CREDENTIAL_STAGE_TTL_MS);
    let affected = conn
        .execute(
            "UPDATE cloud_credential_cleanup
                SET cleanup_ready = 1
              WHERE cleanup_ready = 0
                AND staged_at <= ?1
                AND credential_ref NOT IN (
                    SELECT credential_ref FROM cloud_accounts
                     WHERE connected = 1 AND credential_ref IS NOT NULL
                )
                AND rowid IN (
                    SELECT rowid FROM cloud_credential_cleanup
                     WHERE cleanup_ready = 0 AND staged_at <= ?1
                     ORDER BY staged_at ASC, credential_ref ASC
                     LIMIT ?2
                )",
            rusqlite::params![cutoff, i64::from(CLOUD_CREDENTIAL_CLEANUP_BATCH)],
        )
        .map_err(map_db_error("恢复超时待清理凭据失败"))?;
    Ok(affected as u32)
}

/// 只列 ready 引用（登记顺序），staged 永不进入普通删除列表；limit 有界。
pub(super) fn pending_credentials_on_conn(
    conn: &Connection,
    limit: u32,
) -> Result<Vec<CredentialRef>, AppError> {
    if limit > CLOUD_CREDENTIAL_CLEANUP_BATCH {
        return Err(validation("凭据清理批量超过上限"));
    }
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare(
            "SELECT credential_ref FROM cloud_credential_cleanup
              WHERE cleanup_ready = 1
                AND credential_ref NOT IN (
                    SELECT credential_ref FROM cloud_accounts
                    WHERE connected = 1 AND credential_ref IS NOT NULL
                )
              ORDER BY staged_at ASC, credential_ref ASC
              LIMIT ?1",
        )
        .map_err(map_db_error("查询待清理凭据失败"))?;
    let rows = stmt
        .query_map(rusqlite::params![i64::from(limit)], |row| {
            let raw: String = row.get(0)?;
            let parsed = raw
                .parse::<CredentialRef>()
                .map_err(|_| corrupt_cleanup_row())?;
            if !is_scoped_credential(&parsed) {
                return Err(corrupt_cleanup_row());
            }
            Ok(parsed)
        })
        .map_err(map_db_error("查询待清理凭据失败"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(map_db_error("查询待清理凭据失败"))
}

/// keystore 删除成功后的收尾：活动引用直接拒绝；只删 ready 行；仅当账户仍
/// `connected = 0` 且 `credential_ref` 仍等于该引用时才清空它。
pub(super) fn finish_credential_cleanup_on_conn(
    conn: &Connection,
    credential: &CredentialRef,
    now: UtcMillis,
) -> Result<CloudCredentialCleanupOutcome, AppError> {
    if active_credential_on_conn(conn, credential)? {
        return Err(credential_in_use());
    }
    let removed = conn
        .execute(
            "DELETE FROM cloud_credential_cleanup
              WHERE credential_ref = ?1 AND cleanup_ready = 1",
            rusqlite::params![credential.as_str()],
        )
        .map_err(map_db_error("清理待清理凭据失败"))?;
    if removed == 0 {
        return Ok(CloudCredentialCleanupOutcome::AlreadyClean);
    }
    let cleared = conn
        .execute(
            "UPDATE cloud_accounts
                SET credential_ref = NULL, updated_at = ?1
              WHERE credential_ref = ?2 AND connected = 0",
            rusqlite::params![now.0, credential.as_str()],
        )
        .map_err(map_db_error("清除已断开账户凭据引用失败"))?;
    Ok(if cleared > 0 {
        CloudCredentialCleanupOutcome::Cleared
    } else {
        CloudCredentialCleanupOutcome::RefRetained
    })
}

// ---------- Db 入口（写走 with_tx，读走 lock） ----------

impl Db {
    pub(super) fn stage_credential(&self, credential: &CredentialRef) -> Result<(), AppError> {
        with_cloud_tx(self, |tx| {
            stage_credential_on_conn(tx, credential, UtcMillis::now())
        })
    }

    pub(super) fn retire_staged_credential(
        &self,
        credential: &CredentialRef,
    ) -> Result<(), AppError> {
        with_cloud_tx(self, |tx| {
            retire_staged_credential_on_conn(tx, credential, UtcMillis::now())
        })
    }

    pub(super) fn recover_staged_credentials(&self, now: UtcMillis) -> Result<u32, AppError> {
        with_cloud_tx(self, |tx| recover_staged_credentials_on_conn(tx, now))
    }

    pub(super) fn pending_credentials(&self, limit: u32) -> Result<Vec<CredentialRef>, AppError> {
        let conn = self.lock();
        pending_credentials_on_conn(&conn, limit)
    }

    pub(super) fn finish_credential_cleanup(
        &self,
        credential: &CredentialRef,
    ) -> Result<CloudCredentialCleanupOutcome, AppError> {
        with_cloud_tx(self, |tx| {
            finish_credential_cleanup_on_conn(tx, credential, UtcMillis::now())
        })
    }
}
