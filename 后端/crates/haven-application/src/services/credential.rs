//! CredentialDeletionService：跨存储删除编排（S-04）。
//!
//! 规范：ADR-001——"先删 Credential Manager 条目，再清 DB credential_ref"。
//! 顺序保证：
//! - API **只接收 `location_id`**（R-S04-1 修复）：系统凭据与 DB 引用之间的
//!   绑定由服务从 Repository 读取，调用者无法把 A 的 location 与 B 的
//!   credential_ref 交叉错配。
//! - location 不存在 → `RESOURCE_NOT_FOUND`（不触碰任何系统凭据）。
//! - credential_ref 为 null → 幂等成功（无凭据可删，无引用可清）。
//! - `store.delete` 失败 → 直接返回错误，**DB ref 保持不变**；retryable 由底层分类。
//! - DB clear 失败 → `DATABASE_ERROR`（retryable=true）；恢复语义：重试幂等
//!   （系统凭据已删时 delete 返回 NoEntry → 仍会清 ref）。

use std::sync::Arc;

use haven_common::AppError;
use haven_domain::contracts::StorageLocationRepository;
use haven_domain::credential::CredentialStore;
use haven_domain::ids::StorageLocationId;

/// 删除结果（幂等策略下的两个成功终态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialDeleteOutcome {
    /// 系统凭据实际删除，DB ref 已清。
    Deleted,
    /// 系统凭据本就不存在（NoEntry）或本来无凭据（null ref），按幂等策略清 DB ref。
    RefCleared,
}

/// 删除编排所需端口（访问方法由 blanket impl 提供，MSRV 1.85 无 trait upcasting）。
pub trait CredentialDeletePorts: StorageLocationRepository {
    fn as_storage_location(&self) -> &dyn StorageLocationRepository;
}
impl<T> CredentialDeletePorts for T
where
    T: StorageLocationRepository,
{
    fn as_storage_location(&self) -> &dyn StorageLocationRepository {
        self
    }
}

pub struct CredentialDeletionService {
    store: Arc<dyn CredentialStore>,
    ports: Arc<dyn CredentialDeletePorts>,
}

impl CredentialDeletionService {
    pub fn new(store: Arc<dyn CredentialStore>, ports: Arc<dyn CredentialDeletePorts>) -> Self {
        Self { store, ports }
    }

    /// 删除 StorageLocation 绑定的系统凭据并清除 DB 引用。
    /// 凭据 target 来自 DB 绑定（调用者不提供），杜绝交叉错配。
    pub async fn delete(
        &self,
        location_id: StorageLocationId,
    ) -> Result<CredentialDeleteOutcome, AppError> {
        let location = self
            .ports
            .as_storage_location()
            .get(location_id)
            .await?
            .ok_or_else(|| {
                AppError::new(
                    "RESOURCE_NOT_FOUND",
                    haven_common::ErrorKind::NotFound,
                    "存储位置不存在",
                    false,
                )
            })?;
        let Some(credential_ref) = location.credential_ref else {
            return Ok(CredentialDeleteOutcome::RefCleared);
        };

        let deleted = self.store.delete(&credential_ref).await?;
        self.ports
            .as_storage_location()
            .clear_credential_ref(location_id)
            .await
            .map_err(|e| {
                AppError::new(
                    "DATABASE_ERROR",
                    haven_common::ErrorKind::Database,
                    "清除凭据引用失败（系统凭据已删除；重试幂等）",
                    true,
                )
                .with_source(e)
            })?;
        Ok(if deleted {
            CredentialDeleteOutcome::Deleted
        } else {
            CredentialDeleteOutcome::RefCleared
        })
    }
}
