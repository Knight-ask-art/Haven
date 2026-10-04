//! CloudBrowseService：Google Drive 有界、不透明的目录 / 文件选择与 PDF 导入编排。
//!
//! 边界：
//! - 调用方只能传**内部账户 UUID**、**不透明句柄**或**内部 `StorageLocationId`**；真实 Drive
//!   ID 与分页 token 只存在于本进程租约注册表内，永不进入 Wire，也不接受调用方提供的原始 ID。
//! - 视图只有 `handle / displayName / isFolder / pdfSupported / sizeBytes` 与 `nextCursor`
//!   （外加本页所属目录的 `folderHandle`）；不含 Drive ID、父 ID、`CredentialRef`、邮箱、令牌。
//! - `root` 别名先 stat 成真实 ID，再用该 ID 列举并逐条做父目录成员校验。
//! - 每次 IO 前后复核账户 `connected` / `generation`；已登记目录另复核绑定逐字段未变。
//! - 只读：不上传、不删除远端、不递归扫描。只有**已登记目录**下、`application/pdf`、未回收站、
//!   大小落在 `1..=`[`CLOUD_OBJECT_MAX_BYTES`] 的对象可导入；其余（含未登记目录里的 PDF）
//!   如实展示但不可选、不签发句柄。
//!
use std::sync::Arc;

use haven_common::{AppError, ErrorKind, UtcMillis, not_found, validation};
use haven_domain::ids::StorageLocationId;

use super::credential_session::{AccountCredential, is_canonical_uuid};
use super::ports::{
    CLOUD_DRIVE_ROOT_ID, CloudDriveFileMetadata, validate_drive_object_id,
    validate_drive_page_token,
};
use super::selection_handles::{BrowseHandleRegistry, CursorLease, FileLease, FolderLease};
use super::service::CloudStorageService;
use super::state::{
    CLOUD_OBJECT_MAX_BYTES, CloudAccount, CloudCasOutcome, CloudFolderBinding, CloudFolderRequest,
    CloudPdfImportOutcome, cloud_binding_stale,
};
use super::views::CloudObjectView;

/// 单页条目上限：防御 Provider 返回超大页（分页句柄上限见注册表）。
const CLOUD_BROWSE_MAX_PAGE_ENTRIES: usize = 1000;
/// 登记目录显示名上限。
const CLOUD_BROWSE_DISPLAY_NAME_MAX_CHARS: usize = 120;
/// 唯一可导入的 MIME。
const CLOUD_PDF_MIME: &str = "application/pdf";

/// 单个条目视图：只有不透明句柄与展示事实。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudBrowseEntryView {
    /// `None` 表示该条目当前不可选（不支持 / 已回收站 / 大小未知或越界 / 所在目录尚未登记）。
    pub handle: Option<String>,
    pub display_name: String,
    pub is_folder: bool,
    pub pdf_supported: bool,
    pub size_bytes: Option<u64>,
}

/// 一页浏览结果：条目 + 下一页光标句柄 + 本页所属目录句柄。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudBrowsePage {
    pub folder_handle: Option<String>,
    pub entries: Vec<CloudBrowseEntryView>,
    pub next_cursor: Option<String>,
}

/// 有界浏览编排：包装核心服务，持有自己的句柄租约注册表。
pub struct CloudBrowseService {
    core: Arc<CloudStorageService>,
    handles: Arc<BrowseHandleRegistry>,
}

/// 一次列举的身份锚点：账户 + 代际 + **真实**父目录 ID + 可选已登记绑定。
struct ListingAnchor {
    account_id: String,
    generation: i64,
    folder_id: String,
    registered: Option<CloudFolderBinding>,
}

impl CloudBrowseService {
    pub fn new(core: Arc<CloudStorageService>) -> Self {
        Self {
            core,
            handles: Arc::new(BrowseHandleRegistry::new()),
        }
    }

    /// 列出账户根目录：先 stat `root` 别名取真实 ID，再列举。
    pub async fn browse_root(&self, account_id: &str) -> Result<CloudBrowsePage, AppError> {
        let account = self.load_connected_account(account_id).await?;
        let credential = self.credential_for(&account).await?;
        let root = self
            .core
            .drive
            .stat_file(&credential.credential, CLOUD_DRIVE_ROOT_ID)
            .await?;
        if validate_drive_object_id(&root.id).is_err() || !root.is_folder || root.trashed {
            return Err(cloud_binding_stale());
        }
        self.core.credentials.recheck_current(&credential).await?;
        self.assert_account_unchanged(&account.id, account.generation)
            .await?;
        let now = UtcMillis::now();
        let anchor = ListingAnchor {
            account_id: account.id.clone(),
            generation: account.generation,
            folder_id: root.id.clone(),
            registered: None,
        };
        let handle = self.handles.insert_folder(
            FolderLease {
                account_id: account.id.clone(),
                generation: account.generation,
                folder_id: root.id.clone(),
                parent_id: None,
                display_name: root.name.clone(),
                registered: None,
            },
            now,
        )?;
        self.list_page(&credential, &anchor, handle, None, now)
            .await
    }

    /// 列出句柄指向的目录（未登记子目录或根）。
    pub async fn browse_folder(&self, folder_handle: &str) -> Result<CloudBrowsePage, AppError> {
        let now = UtcMillis::now();
        let lease = self.handles.folder(folder_handle, now)?;
        let account = self.load_connected_account(&lease.account_id).await?;
        if account.generation != lease.generation {
            return Err(cloud_binding_stale());
        }
        if let Some(binding) = &lease.registered {
            self.assert_binding_unchanged(binding).await?;
        }
        let credential = self.credential_for(&account).await?;
        let meta = self
            .core
            .drive
            .stat_file(&credential.credential, &lease.folder_id)
            .await?;
        if meta.id != lease.folder_id
            || !meta.is_folder
            || meta.trashed
            || lease
                .parent_id
                .as_ref()
                .is_some_and(|parent| !meta.parents.contains(parent))
        {
            return Err(cloud_binding_stale());
        }
        self.core.credentials.recheck_current(&credential).await?;
        let anchor = ListingAnchor {
            account_id: lease.account_id.clone(),
            generation: lease.generation,
            folder_id: lease.folder_id.clone(),
            registered: lease.registered.clone(),
        };
        self.list_page(&credential, &anchor, folder_handle.to_owned(), None, now)
            .await
    }

    /// 翻页：光标句柄一次性消费，原始 token 只从租约取得。
    pub async fn next_page(&self, cursor_handle: &str) -> Result<CloudBrowsePage, AppError> {
        let now = UtcMillis::now();
        let cursor = self.handles.take_cursor(cursor_handle, now)?;
        let account = self.load_connected_account(&cursor.account_id).await?;
        if account.generation != cursor.generation {
            return Err(cloud_binding_stale());
        }
        if let Some(binding) = &cursor.registered {
            self.assert_binding_unchanged(binding).await?;
        }
        let credential = self.credential_for(&account).await?;
        let anchor = ListingAnchor {
            account_id: cursor.account_id.clone(),
            generation: cursor.generation,
            folder_id: cursor.folder_id.clone(),
            registered: cursor.registered.clone(),
        };
        self.list_page(
            &credential,
            &anchor,
            cursor.folder_handle.clone(),
            Some(&cursor.page_token),
            now,
        )
        .await
    }

    /// 登记目录：重新 stat 确认仍是同一授权层级下的同一目录（按父关系与名称，不看 URL）。
    pub async fn register_folder(
        &self,
        folder_handle: &str,
        display_name: &str,
    ) -> Result<StorageLocationId, AppError> {
        let display_name = validate_display_name(display_name)?;
        let now = UtcMillis::now();
        let lease = self.handles.folder(folder_handle, now)?;
        let account = self.load_connected_account(&lease.account_id).await?;
        if account.generation != lease.generation {
            return Err(cloud_binding_stale());
        }
        let credential = self.credential_for(&account).await?;
        let meta = self
            .core
            .drive
            .stat_file(&credential.credential, &lease.folder_id)
            .await?;
        if meta.id != lease.folder_id
            || !meta.is_folder
            || meta.trashed
            || meta.name != lease.display_name
        {
            return Err(cloud_binding_stale());
        }
        if let Some(parent) = &lease.parent_id {
            if !meta.parents.iter().any(|value| value == parent) {
                return Err(cloud_binding_stale());
            }
        }
        self.core.credentials.recheck_current(&credential).await?;
        self.assert_account_unchanged(&account.id, account.generation)
            .await?;
        self.handles.folder(folder_handle, UtcMillis::now())?;
        if let Some(binding) = &lease.registered {
            self.assert_binding_unchanged(binding).await?;
        }
        let request = CloudFolderRequest {
            account_id: account.id.clone(),
            provider_folder_id: lease.folder_id.clone(),
        };
        match self
            .core
            .repo
            .register_folder(&request, account.generation, display_name)
            .await?
        {
            CloudCasOutcome::Applied(binding) => Ok(binding.location_id),
            CloudCasOutcome::Stale => Err(cloud_binding_stale()),
            CloudCasOutcome::Missing => Err(not_found("云盘账户不存在")),
        }
    }

    /// 列出**已登记**目录：先 stat 确认目录仍在，再列举。
    pub async fn browse_location(
        &self,
        location_id: StorageLocationId,
    ) -> Result<CloudBrowsePage, AppError> {
        let binding = self
            .core
            .repo
            .get_folder(location_id)
            .await?
            .ok_or_else(|| not_found("云盘目录不存在"))?;
        let account = self.load_connected_account(&binding.account_id).await?;
        let credential = self.credential_for(&account).await?;
        let meta = self
            .core
            .drive
            .stat_file(&credential.credential, &binding.provider_folder_id)
            .await?;
        if meta.id != binding.provider_folder_id || !meta.is_folder || meta.trashed {
            return Err(cloud_binding_stale());
        }
        self.core.credentials.recheck_current(&credential).await?;
        self.assert_account_unchanged(&account.id, account.generation)
            .await?;
        self.assert_binding_unchanged(&binding).await?;
        let now = UtcMillis::now();
        let anchor = ListingAnchor {
            account_id: account.id.clone(),
            generation: account.generation,
            folder_id: binding.provider_folder_id.clone(),
            registered: Some(binding.clone()),
        };
        let handle = self.handles.insert_folder(
            FolderLease {
                account_id: account.id.clone(),
                generation: account.generation,
                folder_id: binding.provider_folder_id.clone(),
                parent_id: None,
                display_name: meta.name.clone(),
                registered: Some(binding),
            },
            now,
        )?;
        self.list_page(&credential, &anchor, handle, None, now)
            .await
    }

    /// 导入 PDF：重新 stat 真实 MIME / 大小 / 成员关系，再走仓储 CAS 写入统一内容。
    pub async fn import_pdf(
        &self,
        location_id: StorageLocationId,
        file_handle: &str,
    ) -> Result<CloudObjectView, AppError> {
        let binding = self
            .core
            .repo
            .get_folder(location_id)
            .await?
            .ok_or_else(|| not_found("云盘目录不存在"))?;
        let now = UtcMillis::now();
        let lease = self.handles.file(file_handle, now)?;
        // 句柄必须来自该账户，且是在这个已登记目录下浏览得到的文件。
        if lease.account_id != binding.account_id || lease.folder_id != binding.provider_folder_id {
            return Err(cloud_binding_stale());
        }
        match &lease.registered {
            Some(registered) if registered == &binding => {}
            _ => return Err(cloud_binding_stale()),
        }
        let account = self.load_connected_account(&binding.account_id).await?;
        if account.generation != lease.generation {
            return Err(cloud_binding_stale());
        }
        let credential = self.credential_for(&account).await?;
        let meta = self
            .core
            .drive
            .stat_file(&credential.credential, &lease.file_id)
            .await?;
        if meta.id != lease.file_id
            || meta.is_folder
            || meta.trashed
            || meta.mime_type != CLOUD_PDF_MIME
            || !meta
                .parents
                .iter()
                .any(|value| value == &binding.provider_folder_id)
        {
            return Err(cloud_binding_stale());
        }
        let Some(size) = meta.size_bytes else {
            return Err(cloud_binding_stale());
        };
        if !(1..=CLOUD_OBJECT_MAX_BYTES).contains(&size) {
            return Err(cloud_binding_stale());
        }
        self.core.credentials.recheck_current(&credential).await?;
        self.assert_account_unchanged(&account.id, account.generation)
            .await?;
        self.assert_binding_unchanged(&binding).await?;
        self.handles.file(file_handle, UtcMillis::now())?;
        let candidate = super::pdf_read::validate_pdf_candidate(&meta, &lease.file_id)?;
        match self
            .core
            .repo
            .import_pdf(&binding, account.generation, &candidate)
            .await?
        {
            CloudCasOutcome::Applied(CloudPdfImportOutcome::Created(object))
            | CloudCasOutcome::Applied(CloudPdfImportOutcome::Existing(object)) => {
                Ok(CloudObjectView {
                    object_id: object.id,
                    media_item_id: object.media_item_id.to_string(),
                    resource_id: object.resource_id.to_string(),
                    display_name: object.display_name,
                    size_bytes: object.size_bytes,
                })
            }
            CloudCasOutcome::Stale => Err(cloud_binding_stale()),
            CloudCasOutcome::Missing => Err(not_found("云盘目录不存在")),
        }
    }

    async fn list_page(
        &self,
        credential: &AccountCredential,
        anchor: &ListingAnchor,
        folder_handle: String,
        page_token: Option<&str>,
        now: UtcMillis,
    ) -> Result<CloudBrowsePage, AppError> {
        if let Some(token) = page_token {
            validate_drive_page_token(token)?;
        }
        let page = self
            .core
            .drive
            .list_folder(&credential.credential, &anchor.folder_id, page_token)
            .await?;
        if page.entries.len() > CLOUD_BROWSE_MAX_PAGE_ENTRIES {
            return Err(AppError::new(
                "CLOUD_BROWSE_PAGE_TOO_LARGE",
                ErrorKind::Parse,
                "云盘返回的分页过大",
                false,
            ));
        }
        // 后置校验：账户代际 / 当前凭据 / 已登记绑定在 IO 期间未变化，否则丢弃整页。
        self.core.credentials.recheck_current(credential).await?;
        self.assert_account_unchanged(&anchor.account_id, anchor.generation)
            .await?;
        if let Some(binding) = &anchor.registered {
            self.assert_binding_unchanged(binding).await?;
        }
        let mut entries = Vec::with_capacity(page.entries.len());
        let mut owned_handles = Vec::with_capacity(page.entries.len());
        for entry in &page.entries {
            let projected = self.project_entry(anchor, entry, now)?;
            if let Some(handle) = &projected.handle {
                owned_handles.push(handle.clone());
            }
            entries.push(projected);
        }
        let next_cursor = match page.next_page_token {
            Some(token) => {
                validate_drive_page_token(&token)?;
                Some(self.handles.insert_cursor(
                    CursorLease {
                        account_id: anchor.account_id.clone(),
                        generation: anchor.generation,
                        folder_id: anchor.folder_id.clone(),
                        folder_handle: folder_handle.clone(),
                        registered: anchor.registered.clone(),
                        page_token: token,
                        // 本页签发的条目句柄由光标持有；光标被消费时只退休这些句柄。
                        owned_handles,
                    },
                    now,
                )?)
            }
            None => None,
        };
        Ok(CloudBrowsePage {
            folder_handle: Some(folder_handle),
            entries,
            next_cursor,
        })
    }

    /// 条目投影：ID 走端口守卫，父目录必须精确命中本次列举的真实父 ID。
    fn project_entry(
        &self,
        anchor: &ListingAnchor,
        entry: &CloudDriveFileMetadata,
        now: UtcMillis,
    ) -> Result<CloudBrowseEntryView, AppError> {
        let unsupported = || CloudBrowseEntryView {
            handle: None,
            display_name: entry.name.clone(),
            is_folder: entry.is_folder,
            pdf_supported: false,
            size_bytes: entry.size_bytes,
        };
        if validate_drive_object_id(&entry.id).is_err()
            || !entry.parents.iter().any(|value| value == &anchor.folder_id)
        {
            return Ok(unsupported());
        }
        if entry.is_folder {
            if entry.trashed {
                return Ok(unsupported());
            }
            let handle = self.handles.insert_folder(
                FolderLease {
                    account_id: anchor.account_id.clone(),
                    generation: anchor.generation,
                    folder_id: entry.id.clone(),
                    parent_id: Some(anchor.folder_id.clone()),
                    display_name: entry.name.clone(),
                    registered: None,
                },
                now,
            )?;
            return Ok(CloudBrowseEntryView {
                handle: Some(handle),
                display_name: entry.name.clone(),
                is_folder: true,
                pdf_supported: false,
                size_bytes: entry.size_bytes,
            });
        }
        if !is_importable_pdf(entry) {
            return Ok(unsupported());
        }
        // 只有已登记目录下的 PDF 才投影可导入能力并签发文件句柄；未登记目录如实展示
        // 名称 / 大小但不可选，避免 UI 把「尚未登记」误当可导入。
        let Some(registered) = anchor.registered.clone() else {
            return Ok(unsupported());
        };
        let handle = self.handles.insert_file(
            FileLease {
                account_id: anchor.account_id.clone(),
                generation: anchor.generation,
                folder_id: anchor.folder_id.clone(),
                file_id: entry.id.clone(),
                registered: Some(registered),
            },
            now,
        )?;
        Ok(CloudBrowseEntryView {
            handle: Some(handle),
            display_name: entry.name.clone(),
            is_folder: false,
            pdf_supported: true,
            size_bytes: entry.size_bytes,
        })
    }

    async fn load_connected_account(&self, account_id: &str) -> Result<CloudAccount, AppError> {
        if !is_canonical_uuid(account_id) {
            return Err(validation("云盘账户标识非法"));
        }
        let account = self
            .core
            .repo
            .get_account(account_id)
            .await?
            .ok_or_else(cloud_binding_stale)?;
        if !account.connected || account.generation <= 0 {
            return Err(cloud_binding_stale());
        }
        Ok(account)
    }

    async fn credential_for(&self, account: &CloudAccount) -> Result<AccountCredential, AppError> {
        let session = self.core.credentials.acquire(&account.id).await?;
        if session.generation != account.generation {
            return Err(cloud_binding_stale());
        }
        Ok(session)
    }

    async fn assert_account_unchanged(
        &self,
        account_id: &str,
        generation: i64,
    ) -> Result<(), AppError> {
        match self.core.repo.get_account(account_id).await? {
            Some(account) if account.connected && account.generation == generation => Ok(()),
            _ => Err(cloud_binding_stale()),
        }
    }

    async fn assert_binding_unchanged(
        &self,
        expected: &CloudFolderBinding,
    ) -> Result<(), AppError> {
        match self.core.repo.get_folder(expected.location_id).await? {
            Some(current) if &current == expected => Ok(()),
            _ => Err(cloud_binding_stale()),
        }
    }
}

/// 只有未回收站、大小已知且落在 `1..=`[`CLOUD_OBJECT_MAX_BYTES`] 的 PDF 可导入。
fn is_importable_pdf(entry: &CloudDriveFileMetadata) -> bool {
    entry.mime_type == CLOUD_PDF_MIME
        && !entry.is_folder
        && !entry.trashed
        && entry
            .size_bytes
            .is_some_and(|size| (1..=CLOUD_OBJECT_MAX_BYTES).contains(&size))
}

/// 登记显示名：去空白、非空、有界、无控制字符。
fn validate_display_name(value: &str) -> Result<&str, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(validation("目录名称不能为空"));
    }
    if trimmed.chars().count() > CLOUD_BROWSE_DISPLAY_NAME_MAX_CHARS {
        return Err(validation("目录名称过长"));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(validation("目录名称含控制字符"));
    }
    Ok(trimmed)
}
