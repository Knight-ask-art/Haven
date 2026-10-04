//! 云盘存储 Repository（Sqlite）：账户 / 目录 / 对象绑定 + 凭据清理 outbox。
//!
//! 本模块是**薄封装**：全部读写与 CAS 逻辑在私有子模块（accounts / cleanup /
//! folders / objects）里，均为同步函数、首参 `&Db`、命名与签名对齐 trait 方法。
//! 这里的 async trait 方法只做委托（同步函数无 `.await`），每个方法仍是
//! `CloudStorageRepository` 契约规定的「一个短事务」；网络 IO 与 keystore 一律由
//! 调用方在事务之外完成。

use std::sync::Arc;

use async_trait::async_trait;

use haven_application::services::cloud_storage::state::{
    CloudAccount, CloudCasOutcome, CloudConnectRequest, CloudCredentialCleanupOutcome,
    CloudFolderBinding, CloudFolderRemoveOutcome, CloudFolderRequest, CloudObjectBinding,
    CloudObjectSnapshot, CloudPdfCandidate, CloudPdfImportOutcome, CloudStorageRepository,
};
use haven_common::{AppError, UtcMillis};
use haven_domain::ids::{CredentialRef, StorageLocationId};

use crate::db::Db;

mod accounts;
mod cleanup;
mod folders;
mod objects;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

/// SQLite 版云盘存储仓储；组合根只依赖本结构与 `CloudStorageRepository` 契约。
pub struct SqliteCloudStorageRepository {
    pub(super) db: Arc<Db>,
}

impl SqliteCloudStorageRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl CloudStorageRepository for SqliteCloudStorageRepository {
    async fn list_accounts(&self) -> Result<Vec<CloudAccount>, AppError> {
        self.db.list_accounts()
    }

    async fn get_account(&self, account_id: &str) -> Result<Option<CloudAccount>, AppError> {
        self.db.get_account(account_id)
    }

    async fn get_account_by_provider(
        &self,
        provider: &str,
        provider_account_id: &str,
    ) -> Result<Option<CloudAccount>, AppError> {
        self.db
            .get_account_by_provider(provider, provider_account_id)
    }

    async fn stage_credential(&self, credential: &CredentialRef) -> Result<(), AppError> {
        self.db.stage_credential(credential)
    }

    async fn retire_staged_credential(&self, credential: &CredentialRef) -> Result<(), AppError> {
        self.db.retire_staged_credential(credential)
    }

    async fn recover_staged_credentials(&self, now: UtcMillis) -> Result<u32, AppError> {
        self.db.recover_staged_credentials(now)
    }

    async fn pending_credentials(&self, limit: u32) -> Result<Vec<CredentialRef>, AppError> {
        self.db.pending_credentials(limit)
    }

    async fn finish_credential_cleanup(
        &self,
        credential: &CredentialRef,
    ) -> Result<CloudCredentialCleanupOutcome, AppError> {
        self.db.finish_credential_cleanup(credential)
    }

    async fn connect(
        &self,
        request: &CloudConnectRequest,
        expected_prior_generation: Option<i64>,
        new_credential: &CredentialRef,
    ) -> Result<CloudCasOutcome<CloudAccount>, AppError> {
        self.db
            .connect(request, expected_prior_generation, new_credential)
    }

    async fn replace_credential(
        &self,
        account_id: &str,
        expected_generation: i64,
        expected_credential: &CredentialRef,
        new_credential: &CredentialRef,
    ) -> Result<CloudCasOutcome<CloudAccount>, AppError> {
        self.db.replace_credential(
            account_id,
            expected_generation,
            expected_credential,
            new_credential,
        )
    }

    async fn disconnect(
        &self,
        account_id: &str,
        expected_generation: i64,
    ) -> Result<CloudCasOutcome<CloudAccount>, AppError> {
        self.db.disconnect(account_id, expected_generation)
    }

    async fn get_folder(
        &self,
        location_id: StorageLocationId,
    ) -> Result<Option<CloudFolderBinding>, AppError> {
        folders::get_folder(&self.db, location_id)
    }

    async fn register_folder(
        &self,
        request: &CloudFolderRequest,
        expected_generation: i64,
        display_name: &str,
    ) -> Result<CloudCasOutcome<CloudFolderBinding>, AppError> {
        folders::register_folder(&self.db, request, expected_generation, display_name)
    }

    async fn remove_folder(
        &self,
        location_id: StorageLocationId,
    ) -> Result<CloudFolderRemoveOutcome, AppError> {
        folders::remove_folder(&self.db, location_id)
    }

    async fn get_object(&self, object_id: &str) -> Result<Option<CloudObjectBinding>, AppError> {
        objects::get_object(&self.db, object_id)
    }

    async fn object_snapshot(
        &self,
        object_id: &str,
    ) -> Result<Option<CloudObjectSnapshot>, AppError> {
        objects::object_snapshot(&self.db, object_id)
    }

    async fn assert_current(
        &self,
        expected_account_generation: i64,
        snapshot: &CloudObjectSnapshot,
    ) -> Result<(), AppError> {
        objects::assert_current(&self.db, expected_account_generation, snapshot)
    }

    async fn import_pdf(
        &self,
        binding: &CloudFolderBinding,
        expected_generation: i64,
        file: &CloudPdfCandidate,
    ) -> Result<CloudCasOutcome<CloudPdfImportOutcome>, AppError> {
        objects::import_pdf(&self.db, binding, expected_generation, file)
    }
}

/// Cloud writes own an IMMEDIATE transaction: across independent SQLite handles the CAS
/// reads are serialized before any mutation, without changing Local transaction semantics.
fn with_cloud_tx<T>(
    db: &Db,
    f: impl FnOnce(&rusqlite::Connection) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let mut guard = db.lock();
    let tx = guard
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(crate::db::repos::map_db_error("开启云盘事务失败"))?;
    let value = f(&tx)?;
    tx.commit()
        .map_err(crate::db::repos::map_db_error("提交云盘事务失败"))?;
    Ok(value)
}

/// Cloud reads（快照 / IO 后复核）走 **DEFERRED** 事务：绑定链要跨 object → folder →
/// location → account → locator 多次查询，必须来自同一个一致快照（WAL 读快照 / 共享锁在
/// 首次读取时取得，直到提交），但读路径绝不能像 CAS 写那样先抢写锁。
fn with_cloud_read_tx<T>(
    db: &Db,
    f: impl FnOnce(&rusqlite::Connection) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let guard = db.lock();
    let tx = guard
        .unchecked_transaction()
        .map_err(crate::db::repos::map_db_error("开启云盘读事务失败"))?;
    let value = f(&tx)?;
    tx.commit()
        .map_err(crate::db::repos::map_db_error("提交云盘读事务失败"))?;
    Ok(value)
}

/// Cloud remove 复用 `storage_content::purge_location_content`，该算法持有连接级 TEMP
/// 中间表。这里保持与 `SqliteStorageUoW::run` 相同的防御顺序：事务前先清掉可能残留的旧
/// TEMP 表（否则回滚会把事务开始前已存在的旧表复活），IMMEDIATE 事务，事务后再清一次；
/// 主操作失败优先返回原错误。
fn with_cloud_purge_tx<T>(
    db: &Db,
    f: impl FnOnce(&rusqlite::Connection) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let mut guard = db.lock();
    guard
        .execute_batch(crate::db::storage_content::PURGE_TEMP_DROP_SQL)
        .map_err(crate::db::repos::map_db_error("清理 purge 临时表失败"))?;
    let tx = guard
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(crate::db::repos::map_db_error("开启云盘事务失败"))?;
    let result = match f(&tx) {
        Ok(value) => tx
            .commit()
            .map_err(crate::db::repos::map_db_error("提交云盘事务失败"))
            .map(|()| value),
        Err(error) => {
            drop(tx);
            Err(error)
        }
    };
    let cleanup = guard
        .execute_batch(crate::db::storage_content::PURGE_TEMP_DROP_SQL)
        .map_err(crate::db::repos::map_db_error("清理 purge 临时表失败"));
    match (result, cleanup) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(value), Ok(())) => Ok(value),
    }
}
