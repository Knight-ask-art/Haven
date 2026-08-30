//! 凭据存储实现。
//!
//! 规范：ADR-001。Windows 使用 Windows Credential Manager Generic Credential
//! （keyring-core 0.7.2 + windows-native-keyring-store 0.5.1，显式 persistence=Local）。
//! 非 Windows 平台返回明确 unsupported，不静默使用文件后端。

#[cfg(windows)]
mod windows;

#[cfg(not(windows))]
mod unsupported;

use std::sync::Arc;

use haven_domain::credential::CredentialStore;

#[cfg(not(windows))]
pub use self::unsupported::UnsupportedCredentialStore;
#[cfg(windows)]
pub use self::windows::WindowsCredentialStore;

/// 平台工厂：返回当前平台可用的 CredentialStore 实现。
pub fn credential_store() -> Result<Arc<dyn CredentialStore>, haven_common::AppError> {
    #[cfg(windows)]
    {
        Ok(Arc::new(WindowsCredentialStore::new()?))
    }
    #[cfg(not(windows))]
    {
        Ok(Arc::new(UnsupportedCredentialStore))
    }
}
