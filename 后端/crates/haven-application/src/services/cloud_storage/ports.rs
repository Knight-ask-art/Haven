//! Google Drive 服务端专用端口；Provider 身份和令牌不进入 Wire。

use async_trait::async_trait;
use haven_common::{AppError, UtcMillis, validation};
use haven_domain::credential::SecretString;
use haven_domain::ids::CredentialRef;

use crate::services::ports::{RemoteByteRange, RemoteSessionBody};

pub const CLOUD_DRIVE_PROVIDER_ID: &str = "google_drive";
pub const CLOUD_DRIVE_ROOT_ID: &str = "root";
pub const CLOUD_DRIVE_OBJECT_ID_MAX_LEN: usize = 128;
pub const CLOUD_DRIVE_PAGE_TOKEN_MAX_LEN: usize = 2048;

/// Provider ID is not a path, filename or caller-supplied URL.
pub fn validate_drive_object_id(value: &str) -> Result<&str, AppError> {
    if value.is_empty() {
        return Err(validation("云盘对象 ID 不能为空"));
    }
    if value.len() > CLOUD_DRIVE_OBJECT_ID_MAX_LEN {
        return Err(validation("云盘对象 ID 超长"));
    }
    if !value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(validation("云盘对象 ID 含不允许的字符"));
    }
    Ok(value)
}

/// Pagination tokens are opaque, not subject to the object-ID alphabet.
/// The transport always adds them via URL query-pair encoding.
pub fn validate_drive_page_token(value: &str) -> Result<&str, AppError> {
    if value.is_empty() || value.len() > CLOUD_DRIVE_PAGE_TOKEN_MAX_LEN {
        return Err(validation("云盘分页 token 长度非法"));
    }
    if !value.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(validation("云盘分页 token 含控制字符、空白或非 ASCII 字节"));
    }
    Ok(value)
}

pub fn credential_ref(profile_id: &str) -> Result<CredentialRef, AppError> {
    CredentialRef::new_scoped(CLOUD_DRIVE_PROVIDER_ID, profile_id)
}

/// Owned credentials are server-only and deliberately not Clone/Serialize.
#[derive(Debug)]
pub struct CloudDriveCredential {
    pub access_token: SecretString,
    pub refresh_token: SecretString,
    pub expires_at_ms: i64,
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthStatus {
    Pending,
    Authorized,
    Cancelled,
    Expired,
    Failed,
    Consumed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudDriveAuthAttempt {
    pub id: String,
    pub expires_at: UtcMillis,
}

impl CloudDriveAuthAttempt {
    pub fn is_expired_at(&self, now: UtcMillis) -> bool {
        now.0 >= self.expires_at.0
    }
}

/// Object and parent IDs remain behind the application boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudDriveFileMetadata {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: Option<u64>,
    pub is_folder: bool,
    pub parents: Vec<String>,
    pub trashed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudDriveFolderPage {
    pub entries: Vec<CloudDriveFileMetadata>,
    pub next_page_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudDriveAccount {
    /// Drive user.permissionId; no separate, invented account identifier.
    pub provider_account_id: String,
    pub display_name: String,
}

#[async_trait]
pub trait CloudDriveAuthPort: Send + Sync {
    /// Local configuration truth; must not open a browser or issue network requests.
    fn is_configured(&self) -> bool;
    /// Implementations accept only the fixed read-only scope, never a frontend scope.
    async fn start(&self, scopes: &[String]) -> Result<CloudDriveAuthAttempt, AppError>;
    async fn poll(&self, attempt_id: &str) -> Result<OAuthStatus, AppError>;
    async fn cancel(&self, attempt_id: &str) -> Result<(), AppError>;
    async fn take_result(&self, attempt_id: &str)
    -> Result<Option<CloudDriveCredential>, AppError>;
    /// Persisted refresh tokens suffice after restart; no stale access token required.
    async fn refresh(&self, refresh_token: &SecretString)
    -> Result<CloudDriveCredential, AppError>;
}

/// 打开系统浏览器完成 Google 授权；实现由桌面组合根注入，只接收本模块生成的固定授权
/// 地址。实现不得记录、回显或转发 URL 以外的授权码 / state，也不得接受调用方给的地址。
#[async_trait]
pub trait CloudOAuthBrowserPort: Send + Sync {
    async fn open_google_authorization(&self, url: &str) -> Result<(), AppError>;
}

#[async_trait]
pub trait CloudDrivePort: Send + Sync {
    async fn list_folder(
        &self,
        credential: &CloudDriveCredential,
        folder_id: &str,
        page_token: Option<&str>,
    ) -> Result<CloudDriveFolderPage, AppError>;

    async fn stat_file(
        &self,
        credential: &CloudDriveCredential,
        file_id: &str,
    ) -> Result<CloudDriveFileMetadata, AppError>;

    async fn read_pdf(
        &self,
        credential: &CloudDriveCredential,
        file_id: &str,
        range: Option<RemoteByteRange>,
    ) -> Result<RemoteSessionBody, AppError>;

    async fn about(&self, credential: &CloudDriveCredential)
    -> Result<CloudDriveAccount, AppError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_id_accepts_root_and_bounded_ids() {
        assert_eq!(
            validate_drive_object_id(CLOUD_DRIVE_ROOT_ID).unwrap(),
            "root"
        );
        assert_eq!(validate_drive_object_id("1A2b3C-_x").unwrap(), "1A2b3C-_x");
        assert!(validate_drive_object_id(&"a".repeat(CLOUD_DRIVE_OBJECT_ID_MAX_LEN)).is_ok());
    }

    #[test]
    fn object_id_rejects_slash_query_control_and_path_escape() {
        for bad in [
            "", "a/b", "a\\b", "a?q=1", "a#frag", "root/", "../root", "a b", "a\nb", "a\0b", "a:b",
            "a.b", "文件",
        ] {
            assert!(validate_drive_object_id(bad).is_err(), "应拒绝: {bad:?}");
        }
        assert!(validate_drive_object_id(&"a".repeat(CLOUD_DRIVE_OBJECT_ID_MAX_LEN + 1)).is_err());
    }

    #[test]
    fn page_token_is_bounded_opaque_not_filename_safe() {
        assert!(validate_drive_page_token("~!!~AI9FV7Tc.a-b_c+=/").is_ok());
        for bad in ["", "a b", "a\nb", "tok\r\nX", "a\u{7}b", "文件"] {
            assert!(validate_drive_page_token(bad).is_err(), "应拒绝: {bad:?}");
        }
        assert!(
            validate_drive_page_token(&"a".repeat(CLOUD_DRIVE_PAGE_TOKEN_MAX_LEN + 1)).is_err()
        );
    }

    #[test]
    fn credential_debug_redacts_tokens_and_ref_is_scoped() {
        let credential = CloudDriveCredential {
            access_token: SecretString::new("access-secret"),
            refresh_token: SecretString::new("refresh-secret"),
            expires_at_ms: 1_700_000_000_000,
            scope: Some("https://www.googleapis.com/auth/drive.readonly".into()),
        };
        let rendered = format!("{credential:?}");
        assert!(!rendered.contains("access-secret"));
        assert!(!rendered.contains("refresh-secret"));
        assert!(rendered.contains("[REDACTED]"));
        let reference = credential_ref("3f1c0b2e-0000-4000-8000-000000000000").unwrap();
        assert_eq!(
            reference.as_str(),
            "haven:google_drive:3f1c0b2e-0000-4000-8000-000000000000"
        );
        assert!(credential_ref("bad profile").is_err());
    }

    #[test]
    fn attempt_expiry_includes_the_exact_deadline() {
        let attempt = CloudDriveAuthAttempt {
            id: "attempt".into(),
            expires_at: UtcMillis(100),
        };
        assert!(!attempt.is_expired_at(UtcMillis(99)));
        assert!(attempt.is_expired_at(UtcMillis(100)));
        assert!(attempt.is_expired_at(UtcMillis(101)));
    }
}
