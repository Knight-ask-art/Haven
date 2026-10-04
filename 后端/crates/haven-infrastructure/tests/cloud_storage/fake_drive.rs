use crate::support::{
    FOLDER_ID, FOLDER_NAME, PDF_ID, PDF_LEN, PDF_NAME, PROFILE_ACCOUNT_ID, TRASH_ID, TRASH_NAME,
    UNKNOWN_ID, UNKNOWN_NAME, ZIP_ID, ZIP_NAME, file, pdf_bytes, provider_error,
};
use async_trait::async_trait;
use haven_application::services::cloud_storage::ports::{
    CLOUD_DRIVE_ROOT_ID, CloudDriveAccount, CloudDriveCredential, CloudDriveFileMetadata,
    CloudDriveFolderPage, CloudDrivePort,
};
use haven_application::services::ports::{RemoteByteRange, RemoteContentRange, RemoteSessionBody};
use haven_common::AppError;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
const REAL_ROOT_ID: &str = "Real-Root-1";

// ---------- 假 Drive：root + 一个已登记目录，含 PDF 与不可选文件 ----------

pub struct FakeDrive {
    files: Mutex<HashMap<String, CloudDriveFileMetadata>>,
    listings: Mutex<HashMap<String, Vec<String>>>,
    pdf: Mutex<Vec<u8>>,
    fail_stat: Mutex<HashSet<String>>,
    fail_read: Mutex<HashSet<String>>,
    list_calls: AtomicUsize,
    read_calls: AtomicUsize,
}

impl FakeDrive {
    pub fn new() -> Arc<Self> {
        let folder_mime = "application/vnd.google-apps.folder";
        let mut files = HashMap::new();
        files.insert(
            CLOUD_DRIVE_ROOT_ID.to_owned(),
            file(
                REAL_ROOT_ID,
                "我的云端硬盘",
                folder_mime,
                None,
                true,
                &[],
                false,
            ),
        );
        files.insert(
            FOLDER_ID.to_owned(),
            file(
                FOLDER_ID,
                FOLDER_NAME,
                folder_mime,
                None,
                true,
                &[REAL_ROOT_ID],
                false,
            ),
        );
        let children: [(&str, &str, &str, Option<u64>, bool); 4] = [
            (
                PDF_ID,
                PDF_NAME,
                "application/pdf",
                Some(PDF_LEN as u64),
                false,
            ),
            (ZIP_ID, ZIP_NAME, "application/zip", Some(1024), false),
            (TRASH_ID, TRASH_NAME, "application/pdf", Some(128), true),
            (UNKNOWN_ID, UNKNOWN_NAME, "application/pdf", None, false),
        ];
        for (id, name, mime, size, trashed) in children {
            files.insert(
                id.to_owned(),
                file(id, name, mime, size, false, &[FOLDER_ID], trashed),
            );
        }
        let mut listings = HashMap::new();
        listings.insert(REAL_ROOT_ID.to_owned(), vec![FOLDER_ID.to_owned()]);
        listings.insert(
            FOLDER_ID.to_owned(),
            vec![
                PDF_ID.to_owned(),
                ZIP_ID.to_owned(),
                TRASH_ID.to_owned(),
                UNKNOWN_ID.to_owned(),
            ],
        );
        Arc::new(Self {
            files: Mutex::new(files),
            listings: Mutex::new(listings),
            pdf: Mutex::new(pdf_bytes()),
            fail_stat: Mutex::new(HashSet::new()),
            fail_read: Mutex::new(HashSet::new()),
            list_calls: AtomicUsize::new(0),
            read_calls: AtomicUsize::new(0),
        })
    }

    pub fn fail_stat(&self, file_id: &str) {
        self.fail_stat.lock().unwrap().insert(file_id.to_owned());
    }

    pub fn fail_read(&self, file_id: &str) {
        self.fail_read.lock().unwrap().insert(file_id.to_owned());
    }

    pub fn clear_failures(&self) {
        self.fail_stat.lock().unwrap().clear();
        self.fail_read.lock().unwrap().clear();
    }

    pub fn remote_file_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.files.lock().unwrap().keys().cloned().collect();
        ids.sort();
        ids
    }

    pub fn read_calls(&self) -> usize {
        self.read_calls.load(Ordering::SeqCst)
    }

    /// 模拟同 file ID 原地改写，元数据与字节一起更新，仅用于串行回归安排。
    pub fn replace_pdf_len(&self, len: usize) {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        bytes.resize(len, b'x');
        *self.pdf.lock().unwrap() = bytes;
        if let Some(meta) = self.files.lock().unwrap().get_mut(PDF_ID) {
            meta.size_bytes = Some(len as u64);
        }
    }
}

#[async_trait]
impl CloudDrivePort for FakeDrive {
    async fn list_folder(
        &self,
        _credential: &CloudDriveCredential,
        folder_id: &str,
        _page_token: Option<&str>,
    ) -> Result<CloudDriveFolderPage, AppError> {
        self.list_calls.fetch_add(1, Ordering::SeqCst);
        let ids = {
            let listings = self.listings.lock().unwrap();
            listings
                .get(folder_id)
                .cloned()
                .ok_or_else(provider_error)?
        };
        let files = self.files.lock().unwrap();
        let mut entries = Vec::with_capacity(ids.len());
        for id in ids {
            entries.push(files.get(&id).cloned().ok_or_else(provider_error)?);
        }
        Ok(CloudDriveFolderPage {
            entries,
            next_page_token: None,
        })
    }

    async fn stat_file(
        &self,
        _credential: &CloudDriveCredential,
        file_id: &str,
    ) -> Result<CloudDriveFileMetadata, AppError> {
        if self.fail_stat.lock().unwrap().contains(file_id) {
            return Err(provider_error());
        }
        self.files
            .lock()
            .unwrap()
            .get(file_id)
            .cloned()
            .ok_or_else(provider_error)
    }

    async fn read_pdf(
        &self,
        _credential: &CloudDriveCredential,
        file_id: &str,
        range: Option<RemoteByteRange>,
    ) -> Result<RemoteSessionBody, AppError> {
        self.read_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_read.lock().unwrap().contains(file_id)
            || !self.files.lock().unwrap().contains_key(file_id)
        {
            return Err(provider_error());
        }
        let pdf = self.pdf.lock().unwrap();
        let total = pdf.len() as u64;
        let (bytes, content_range) = match range {
            None => (pdf.to_vec(), None),
            Some(requested) => {
                let start = requested.start;
                let end = requested.end.unwrap_or(total - 1).min(total - 1);
                let slice = pdf
                    .get(start as usize..=end as usize)
                    .ok_or_else(provider_error)?
                    .to_vec();
                (slice, Some(RemoteContentRange { start, end, total }))
            }
        };
        Ok(RemoteSessionBody {
            mime_type: "application/pdf".to_owned(),
            bytes,
            total_size: total,
            content_range,
            accept_ranges: true,
        })
    }

    async fn about(
        &self,
        _credential: &CloudDriveCredential,
    ) -> Result<CloudDriveAccount, AppError> {
        Ok(CloudDriveAccount {
            provider_account_id: PROFILE_ACCOUNT_ID.to_owned(),
            display_name: "云盘用户".to_owned(),
        })
    }
}
