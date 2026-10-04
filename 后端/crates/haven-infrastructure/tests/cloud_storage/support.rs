//! 四端口 CloudStorageService / CloudBrowseService 的离线测试替身。
//!
//! 真实 SQLite 内存库（完整迁移）+ 只保存 refresh token 的内存 keystore + 固定只读
//! scope 的假 OAuth + 无网络无浏览器的假 Google Drive；全部确定性，不睡眠。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub use crate::fake_drive::FakeDrive;
use async_trait::async_trait;
use haven_application::services::cloud_storage::browse::{
    CloudBrowseEntryView, CloudBrowsePage, CloudBrowseService,
};
use haven_application::services::cloud_storage::ports::{
    CloudDriveAuthAttempt, CloudDriveAuthPort, CloudDriveCredential, CloudDriveFileMetadata,
    OAuthStatus,
};
use haven_application::services::cloud_storage::service::CloudStorageService;
use haven_application::services::cloud_storage::views::CloudObjectView;
use haven_application::services::cloud_storage::views::{
    CloudAccountView, CloudConnectBeginRequest, CloudConnectStatus,
};
use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::credential::{CredentialStore, SecretString};
use haven_domain::ids::{CredentialRef, StorageLocationId};
use haven_infrastructure::Db;
use haven_infrastructure::db::repos::SqliteCloudStorageRepository;

/// 应用层只读 scope 常量是 crate 私有，这里按握手实现使用的协议字面量固定。
pub const READONLY_SCOPE: &str = "https://www.googleapis.com/auth/drive.readonly";
pub const PROFILE_ACCOUNT_ID: &str = "Acct-42";
pub const FOLDER_ID: &str = "Fld-9";
pub const FOLDER_NAME: &str = "文献";
pub const LOCATION_NAME: &str = "云端资料";
pub const PDF_ID: &str = "Pdf-7";
pub const PDF_NAME: &str = "报告.pdf";
pub const PDF_LEN: usize = 512;
pub const ZIP_ID: &str = "Zip-3";
pub const ZIP_NAME: &str = "资料.zip";
pub const TRASH_ID: &str = "Trash-5";
pub const TRASH_NAME: &str = "旧.pdf";
pub const UNKNOWN_ID: &str = "Nul-1";
pub const UNKNOWN_NAME: &str = "未知.pdf";

/// 恰好 512 字节、以 `%PDF-` 开头的假 PDF。
pub fn pdf_bytes() -> Vec<u8> {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    bytes.resize(PDF_LEN, b'x');
    bytes
}

/// Provider 侧失败：服务层必须原样向上报错，绝不伪造成功。
pub fn provider_error() -> AppError {
    AppError::new(
        "FAKE_DRIVE_ERROR",
        ErrorKind::Network,
        "模拟云盘请求失败",
        true,
    )
}

pub fn credential(access: &str, refresh: &str) -> CloudDriveCredential {
    CloudDriveCredential {
        access_token: SecretString::new(access),
        refresh_token: SecretString::new(refresh),
        expires_at_ms: UtcMillis::now().0 + 3_600_000,
        scope: Some(READONLY_SCOPE.to_owned()),
    }
}

pub(crate) fn file(
    id: &str,
    name: &str,
    mime: &str,
    size: Option<u64>,
    is_folder: bool,
    parents: &[&str],
    trashed: bool,
) -> CloudDriveFileMetadata {
    let parents = parents.iter().map(|parent| (*parent).to_owned()).collect();
    CloudDriveFileMetadata {
        id: id.to_owned(),
        name: name.to_owned(),
        mime_type: mime.to_owned(),
        size_bytes: size,
        is_folder,
        parents,
        trashed,
    }
}

// ---------- 内存 keystore：只保存 refresh token 字符串 ----------

pub struct MemoryStore {
    secrets: Mutex<HashMap<String, String>>,
}

impl MemoryStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            secrets: Mutex::new(HashMap::new()),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.secrets.lock().unwrap().is_empty()
    }

    pub fn values(&self) -> Vec<String> {
        self.secrets.lock().unwrap().values().cloned().collect()
    }
}

#[async_trait]
impl CredentialStore for MemoryStore {
    async fn set(&self, target: &CredentialRef, secret: &SecretString) -> Result<(), AppError> {
        assert!(
            !secret.expose().contains("access-token"),
            "keystore 只允许保存 refresh token"
        );
        self.secrets
            .lock()
            .unwrap()
            .insert(target.as_str().to_owned(), secret.expose().to_owned());
        Ok(())
    }

    async fn get(&self, target: &CredentialRef) -> Result<Option<SecretString>, AppError> {
        let value = self.secrets.lock().unwrap().get(target.as_str()).cloned();
        Ok(value.map(SecretString::new))
    }

    async fn delete(&self, target: &CredentialRef) -> Result<bool, AppError> {
        Ok(self
            .secrets
            .lock()
            .unwrap()
            .remove(target.as_str())
            .is_some())
    }
}

// ---------- 假 OAuth：固定只读 scope，凭据只交出一次 ----------

pub struct FakeAuth {
    results: Mutex<VecDeque<CloudDriveCredential>>,
    issued_scopes: Mutex<Vec<String>>,
    refresh_calls: AtomicUsize,
    last_refresh_token: Mutex<Option<String>>,
}

impl FakeAuth {
    pub fn new(credentials: Vec<CloudDriveCredential>) -> Arc<Self> {
        Arc::new(Self {
            results: Mutex::new(credentials.into()),
            issued_scopes: Mutex::new(Vec::new()),
            refresh_calls: AtomicUsize::new(0),
            last_refresh_token: Mutex::new(None),
        })
    }

    pub fn issued_scopes(&self) -> Vec<String> {
        self.issued_scopes.lock().unwrap().clone()
    }

    pub fn refresh_calls(&self) -> usize {
        self.refresh_calls.load(Ordering::SeqCst)
    }

    pub fn last_refresh_token(&self) -> Option<String> {
        self.last_refresh_token.lock().unwrap().clone()
    }
}

#[async_trait]
impl CloudDriveAuthPort for FakeAuth {
    fn is_configured(&self) -> bool {
        true
    }
    async fn start(&self, scopes: &[String]) -> Result<CloudDriveAuthAttempt, AppError> {
        self.issued_scopes
            .lock()
            .unwrap()
            .extend(scopes.iter().cloned());
        Ok(CloudDriveAuthAttempt {
            id: uuid::Uuid::new_v4().to_string(),
            expires_at: UtcMillis(UtcMillis::now().0 + 600_000),
        })
    }

    async fn poll(&self, _attempt_id: &str) -> Result<OAuthStatus, AppError> {
        Ok(OAuthStatus::Authorized)
    }

    async fn cancel(&self, _attempt_id: &str) -> Result<(), AppError> {
        Ok(())
    }

    async fn take_result(
        &self,
        _attempt_id: &str,
    ) -> Result<Option<CloudDriveCredential>, AppError> {
        Ok(self.results.lock().unwrap().pop_front())
    }

    async fn refresh(
        &self,
        refresh_token: &SecretString,
    ) -> Result<CloudDriveCredential, AppError> {
        self.refresh_calls.fetch_add(1, Ordering::SeqCst);
        *self.last_refresh_token.lock().unwrap() = Some(refresh_token.expose().to_owned());
        Ok(credential("access-token-refreshed", refresh_token.expose()))
    }
}

// ---------- 组合根替身 ----------

/// (账户视图, 根页, 目录句柄, 位置 ID, 已登记目录页, 导入绑定)
pub type Opened = (
    CloudAccountView,
    CloudBrowsePage,
    String,
    StorageLocationId,
    CloudBrowsePage,
    CloudObjectView,
);

pub struct Harness {
    pub db: Arc<Db>,
    pub repo: Arc<SqliteCloudStorageRepository>,
    pub store: Arc<MemoryStore>,
    pub auth: Arc<FakeAuth>,
    pub drive: Arc<FakeDrive>,
    pub core: Arc<CloudStorageService>,
    pub browse: CloudBrowseService,
}

impl Harness {
    pub fn new(credentials: Vec<CloudDriveCredential>) -> Self {
        let db = Arc::new(Db::open_in_memory().expect("打开内存库"));
        let repo = Arc::new(SqliteCloudStorageRepository::new(db.clone()));
        let store = MemoryStore::new();
        let auth = FakeAuth::new(credentials);
        let drive = FakeDrive::new();
        let core = Arc::new(CloudStorageService::new(
            repo.clone(),
            drive.clone(),
            auth.clone(),
            store.clone(),
        ));
        let browse = CloudBrowseService::new(core.clone());
        Self {
            db,
            repo,
            store,
            auth,
            drive,
            core,
            browse,
        }
    }

    /// 重启：同一持久层与同一 keystore 上重新构造服务（内存缓存必须清零）。
    pub fn restart(&self) -> Arc<CloudStorageService> {
        Arc::new(CloudStorageService::new(
            self.repo.clone(),
            self.drive.clone(),
            self.auth.clone(),
            self.store.clone(),
        ))
    }

    pub async fn connect_new(&self) -> CloudAccountView {
        let attempt = self
            .core
            .begin_connect(CloudConnectBeginRequest { account_id: None })
            .await
            .expect("发起授权");
        let polled = self
            .core
            .poll_connect(&attempt.attempt_id)
            .await
            .expect("轮询授权");
        assert_eq!(polled.status, CloudConnectStatus::Authorized);
        self.core
            .complete_connect(&attempt.attempt_id)
            .await
            .expect("完成授权")
    }

    pub async fn reconnect(&self, account_id: &str) -> CloudAccountView {
        let attempt = self
            .core
            .begin_connect(CloudConnectBeginRequest {
                account_id: Some(account_id.to_owned()),
            })
            .await
            .expect("发起重新授权");
        self.core
            .complete_connect(&attempt.attempt_id)
            .await
            .expect("完成重新授权")
    }

    pub async fn connect_register_import(&self) -> Opened {
        let account = self.connect_new().await;
        let root = self
            .browse
            .browse_root(&account.id)
            .await
            .expect("浏览根目录");
        let folder_handle = find_entry(&root, FOLDER_NAME)
            .handle
            .clone()
            .expect("目录句柄");
        let location_id = self
            .browse
            .register_folder(&folder_handle, LOCATION_NAME)
            .await
            .expect("登记目录");
        let page = self
            .browse
            .browse_location(location_id)
            .await
            .expect("浏览已登记目录");
        let pdf_handle = find_entry(&page, PDF_NAME)
            .handle
            .clone()
            .expect("PDF 句柄");
        let binding = self
            .browse
            .import_pdf(location_id, &pdf_handle)
            .await
            .expect("导入 PDF");
        (account, root, folder_handle, location_id, page, binding)
    }
}

pub fn find_entry<'a>(page: &'a CloudBrowsePage, display_name: &str) -> &'a CloudBrowseEntryView {
    page.entries
        .iter()
        .find(|entry| entry.display_name == display_name)
        .unwrap_or_else(|| panic!("页面缺少条目 {display_name}"))
}
