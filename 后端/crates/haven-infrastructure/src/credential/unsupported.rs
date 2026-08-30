//! 非 Windows 平台：明确返回 unsupported，不静默使用文件后端（ADR-001 验证项 3）。

use haven_common::AppError;
use haven_domain::credential::{CredentialStore, SecretString};
use haven_domain::ids::CredentialRef;

const CREDENTIAL_UNSUPPORTED: &str = "CREDENTIAL_UNSUPPORTED";

/// 占位实现：所有操作返回 unsupported。
pub struct UnsupportedCredentialStore;

#[async_trait::async_trait]
impl CredentialStore for UnsupportedCredentialStore {
    async fn set(&self, _target: &CredentialRef, _secret: &SecretString) -> Result<(), AppError> {
        Err(unsupported())
    }

    async fn get(&self, _target: &CredentialRef) -> Result<Option<SecretString>, AppError> {
        Err(unsupported())
    }

    async fn delete(&self, _target: &CredentialRef) -> Result<bool, AppError> {
        Err(unsupported())
    }
}

fn unsupported() -> AppError {
    AppError::new(
        CREDENTIAL_UNSUPPORTED,
        haven_common::ErrorKind::Unsupported,
        "当前平台不支持系统凭据存储",
        false,
    )
}
