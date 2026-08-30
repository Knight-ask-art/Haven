//! 受控视频截图 Use Case（V02-PLAYBACK-HARDWARE-SCREENSHOT-001）。
//!
//! 截图不是播放进度、Marker 或 Artwork Cache 的一种。前端只上传当前帧的
//! 有界 JPEG 分块，Application 负责窗口归属、顺序、TTL 和协议校验，最后
//! 通过 Infrastructure Port 打开系统保存对话框并原子保存。图片字节不会
//! 进入 SQLite、Settings、Wire 响应或日志。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use haven_common::{AppError, ErrorKind};
use uuid::Uuid;

use crate::wire::{
    VideoScreenshotBeginResultDto, VideoScreenshotChunkRequest, VideoScreenshotResultDto,
    VideoScreenshotStatusDto,
};

/// 单个分块的硬上限，和前端保持一致；Command 与 Application 都会校验。
pub const VIDEO_SCREENSHOT_MAX_CHUNK_BYTES: usize = 64 * 1024;
/// 单次截图上传的总字节上限。常规浏览器 JPEG 远小于该值，避免无限制内存增长。
pub const VIDEO_SCREENSHOT_MAX_TOTAL_BYTES: usize = 8 * 1024 * 1024;
/// 上传状态的短 TTL；窗口切换、取消和提交都会更早清理。
pub const VIDEO_SCREENSHOT_UPLOAD_TTL: Duration = Duration::from_secs(30);

/// Infrastructure 的保存结果。`Cancelled` 表示用户关闭系统保存对话框，
/// 不是错误，也不会留下临时文件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoScreenshotSaveOutcome {
    Saved,
    Cancelled,
}

/// 截图保存端口。实现方拥有临时文件、JPEG 解码/尺寸校验、默认目录和
/// Native Save Dialog；Application 不依赖操作系统 API。
pub trait VideoScreenshotStoragePort: Send + Sync {
    fn save_jpeg(&self, bytes: Vec<u8>) -> Result<VideoScreenshotSaveOutcome, AppError>;
}

struct PendingUpload {
    owner_webview_label: String,
    next_sequence: u32,
    bytes: Vec<u8>,
    expires_at: Instant,
}

struct Inner {
    uploads: Mutex<HashMap<Uuid, PendingUpload>>,
    storage: Arc<dyn VideoScreenshotStoragePort>,
    ttl: Duration,
}

/// 截图 Application Service。`Clone` 只复制 Arc，不复制上传内容。
#[derive(Clone)]
pub struct VideoScreenshotService {
    inner: Arc<Inner>,
}

impl VideoScreenshotService {
    pub fn new(storage: Arc<dyn VideoScreenshotStoragePort>) -> Self {
        Self::with_ttl(storage, VIDEO_SCREENSHOT_UPLOAD_TTL)
    }

    fn with_ttl(storage: Arc<dyn VideoScreenshotStoragePort>, ttl: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                uploads: Mutex::new(HashMap::new()),
                storage,
                ttl,
            }),
        }
    }

    /// 开始一次绑定到当前 WebView 的上传。
    pub fn begin(
        &self,
        owner_webview_label: &str,
    ) -> Result<VideoScreenshotBeginResultDto, AppError> {
        if owner_webview_label.trim().is_empty() {
            return Err(invalid_argument("截图窗口无效"));
        }
        let upload_id = Uuid::new_v4();
        let now = Instant::now();
        let mut uploads = self.inner.uploads.lock().unwrap_or_else(|e| e.into_inner());
        Self::remove_expired(&mut uploads, now);
        uploads.insert(
            upload_id,
            PendingUpload {
                owner_webview_label: owner_webview_label.to_owned(),
                next_sequence: 0,
                bytes: Vec::new(),
                expires_at: now + self.inner.ttl,
            },
        );
        Ok(VideoScreenshotBeginResultDto {
            schema_version: 1,
            upload_id: upload_id.to_string(),
            max_chunk_bytes: VIDEO_SCREENSHOT_MAX_CHUNK_BYTES as u32,
            max_total_bytes: VIDEO_SCREENSHOT_MAX_TOTAL_BYTES as u64,
        })
    }

    /// 接收一个严格递增 sequence 的分块。
    pub fn chunk(
        &self,
        owner_webview_label: &str,
        request: VideoScreenshotChunkRequest,
    ) -> Result<(), AppError> {
        let upload_id = parse_upload_id(&request.upload_id)?;
        if request.bytes.len() > VIDEO_SCREENSHOT_MAX_CHUNK_BYTES {
            return Err(AppError::new(
                "SCREENSHOT_TOO_LARGE",
                ErrorKind::Validation,
                "截图数据过大",
                false,
            ));
        }
        let now = Instant::now();
        let mut uploads = self.inner.uploads.lock().unwrap_or_else(|e| e.into_inner());
        Self::remove_expired(&mut uploads, now);
        let Some(upload) = uploads.get_mut(&upload_id) else {
            return Err(upload_expired());
        };
        if upload.owner_webview_label != owner_webview_label {
            return Err(AppError::new(
                "SCREENSHOT_UPLOAD_EXPIRED",
                ErrorKind::Unauthorized,
                "截图上传已失效",
                false,
            ));
        }
        if upload.next_sequence != request.sequence {
            return Err(invalid_argument("截图分块顺序无效"));
        }
        let next_len = upload
            .bytes
            .len()
            .checked_add(request.bytes.len())
            .ok_or_else(too_large)?;
        if next_len > VIDEO_SCREENSHOT_MAX_TOTAL_BYTES {
            return Err(too_large());
        }
        upload.bytes.extend_from_slice(&request.bytes);
        upload.next_sequence = upload.next_sequence.saturating_add(1);
        upload.expires_at = now + self.inner.ttl;
        Ok(())
    }

    /// 提交上传。先从内存中移除，保证保存失败也不会留下可重复提交状态。
    pub fn commit(
        &self,
        owner_webview_label: &str,
        upload_id: &str,
    ) -> Result<VideoScreenshotResultDto, AppError> {
        let upload_id = parse_upload_id(upload_id)?;
        let now = Instant::now();
        // 先在锁内完成归属校验并移除上传，再释放锁后调用保存端口。
        // Native Save Dialog 是阻塞操作，不能让它持有 uploads Mutex，
        // 否则其他窗口的 begin/chunk/cancel 会被系统对话框长时间阻塞。
        let upload = {
            let mut uploads = self.inner.uploads.lock().unwrap_or_else(|e| e.into_inner());
            Self::remove_expired(&mut uploads, now);
            let Some(upload) = uploads.get(&upload_id) else {
                return Err(upload_expired());
            };
            if upload.owner_webview_label != owner_webview_label {
                return Err(AppError::new(
                    "SCREENSHOT_UPLOAD_EXPIRED",
                    ErrorKind::Unauthorized,
                    "截图上传已失效",
                    false,
                ));
            }
            uploads
                .remove(&upload_id)
                .expect("截图上传在锁内应仍然存在")
        };
        if !is_jpeg_payload(&upload.bytes) {
            return Err(AppError::new(
                "SCREENSHOT_PAYLOAD_INVALID",
                ErrorKind::Validation,
                "截图数据无效",
                false,
            ));
        }
        let outcome = self.inner.storage.save_jpeg(upload.bytes)?;
        Ok(VideoScreenshotResultDto {
            schema_version: 1,
            status: match outcome {
                VideoScreenshotSaveOutcome::Saved => VideoScreenshotStatusDto::Saved,
                VideoScreenshotSaveOutcome::Cancelled => VideoScreenshotStatusDto::Cancelled,
            },
        })
    }

    /// 取消一次上传。窗口关闭时由 Tauri 生命周期调用 `cancel_owner`。
    pub fn cancel(&self, owner_webview_label: &str, upload_id: &str) -> Result<(), AppError> {
        let upload_id = parse_upload_id(upload_id)?;
        let now = Instant::now();
        let mut uploads = self.inner.uploads.lock().unwrap_or_else(|e| e.into_inner());
        Self::remove_expired(&mut uploads, now);
        let Some(upload) = uploads.get(&upload_id) else {
            return Err(upload_expired());
        };
        if upload.owner_webview_label != owner_webview_label {
            return Err(AppError::new(
                "SCREENSHOT_UPLOAD_EXPIRED",
                ErrorKind::Unauthorized,
                "截图上传已失效",
                false,
            ));
        }
        uploads.remove(&upload_id);
        Ok(())
    }

    /// 清理一个 WebView 拥有的所有上传，防止窗口销毁后保留图片字节。
    pub fn cancel_owner(&self, owner_webview_label: &str) {
        let mut uploads = self.inner.uploads.lock().unwrap_or_else(|e| e.into_inner());
        uploads.retain(|_, upload| upload.owner_webview_label != owner_webview_label);
    }

    fn remove_expired(uploads: &mut HashMap<Uuid, PendingUpload>, now: Instant) {
        uploads.retain(|_, upload| upload.expires_at > now);
    }
}

fn parse_upload_id(value: &str) -> Result<Uuid, AppError> {
    let id = Uuid::parse_str(value).map_err(|_| invalid_argument("截图上传标识无效"))?;
    if id.to_string() != value {
        return Err(invalid_argument("截图上传标识无效"));
    }
    Ok(id)
}

fn is_jpeg_payload(bytes: &[u8]) -> bool {
    bytes.len() >= 4
        && bytes.first() == Some(&0xff)
        && bytes.get(1) == Some(&0xd8)
        && bytes.get(bytes.len() - 2) == Some(&0xff)
        && bytes.last() == Some(&0xd9)
}

fn invalid_argument(message: &'static str) -> AppError {
    AppError::new(
        "SCREENSHOT_PAYLOAD_INVALID",
        ErrorKind::Validation,
        message,
        false,
    )
}

fn too_large() -> AppError {
    AppError::new(
        "SCREENSHOT_TOO_LARGE",
        ErrorKind::Validation,
        "截图数据过大",
        false,
    )
}

fn upload_expired() -> AppError {
    AppError::new(
        "SCREENSHOT_UPLOAD_EXPIRED",
        ErrorKind::NotFound,
        "截图上传已失效，请重试",
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeStorage {
        calls: AtomicUsize,
        outcome: VideoScreenshotSaveOutcome,
    }

    impl VideoScreenshotStoragePort for FakeStorage {
        fn save_jpeg(&self, _bytes: Vec<u8>) -> Result<VideoScreenshotSaveOutcome, AppError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.outcome)
        }
    }

    struct FailingStorage;

    impl VideoScreenshotStoragePort for FailingStorage {
        fn save_jpeg(&self, _bytes: Vec<u8>) -> Result<VideoScreenshotSaveOutcome, AppError> {
            Err(AppError::new(
                "SCREENSHOT_DIALOG_START_FAILED",
                ErrorKind::Io,
                "保存对话框无法启动，请重试。",
                true,
            ))
        }
    }

    fn service(outcome: VideoScreenshotSaveOutcome) -> (VideoScreenshotService, Arc<FakeStorage>) {
        let storage = Arc::new(FakeStorage {
            calls: AtomicUsize::new(0),
            outcome,
        });
        (VideoScreenshotService::new(storage.clone()), storage)
    }

    #[test]
    fn enforces_owner_sequence_and_jpeg_payload() {
        let (service, storage) = service(VideoScreenshotSaveOutcome::Saved);
        let begin = service.begin("main").unwrap();
        let upload_id = begin.upload_id.clone();
        service
            .chunk(
                "main",
                VideoScreenshotChunkRequest {
                    upload_id: upload_id.clone(),
                    sequence: 0,
                    bytes: vec![0xff, 0xd8, 1, 2, 0xff, 0xd9],
                },
            )
            .unwrap();
        let error = service
            .chunk(
                "main",
                VideoScreenshotChunkRequest {
                    upload_id: upload_id.clone(),
                    sequence: 0,
                    bytes: vec![0xff, 0xd8, 0xff, 0xd9],
                },
            )
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SCREENSHOT_PAYLOAD_INVALID");
        let error = service.commit("other", &upload_id).unwrap_err();
        assert_eq!(error.code().as_str(), "SCREENSHOT_UPLOAD_EXPIRED");
        let result = service.commit("main", &upload_id).unwrap();
        assert_eq!(result.status, VideoScreenshotStatusDto::Saved);
        assert_eq!(storage.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn rejects_oversized_chunk_and_cleans_owner_uploads() {
        let (service, storage) = service(VideoScreenshotSaveOutcome::Saved);
        let begin = service.begin("main").unwrap();
        let error = service
            .chunk(
                "main",
                VideoScreenshotChunkRequest {
                    upload_id: begin.upload_id,
                    sequence: 0,
                    bytes: vec![0u8; VIDEO_SCREENSHOT_MAX_CHUNK_BYTES + 1],
                },
            )
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SCREENSHOT_TOO_LARGE");
        service.cancel_owner("main");
        assert_eq!(storage.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn rejects_payload_after_total_upload_limit() {
        let (service, storage) = service(VideoScreenshotSaveOutcome::Saved);
        let begin = service.begin("main").unwrap();
        let chunk = vec![0u8; VIDEO_SCREENSHOT_MAX_CHUNK_BYTES];
        let full_chunks = VIDEO_SCREENSHOT_MAX_TOTAL_BYTES / VIDEO_SCREENSHOT_MAX_CHUNK_BYTES;

        for sequence in 0..full_chunks as u32 {
            service
                .chunk(
                    "main",
                    VideoScreenshotChunkRequest {
                        upload_id: begin.upload_id.clone(),
                        sequence,
                        bytes: chunk.clone(),
                    },
                )
                .unwrap();
        }

        let error = service
            .chunk(
                "main",
                VideoScreenshotChunkRequest {
                    upload_id: begin.upload_id,
                    sequence: full_chunks as u32,
                    bytes: vec![0],
                },
            )
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SCREENSHOT_TOO_LARGE");
        assert_eq!(storage.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn returns_cancelled_without_treating_it_as_failure() {
        let (service, _storage) = service(VideoScreenshotSaveOutcome::Cancelled);
        let begin = service.begin("main").unwrap();
        service
            .chunk(
                "main",
                VideoScreenshotChunkRequest {
                    upload_id: begin.upload_id.clone(),
                    sequence: 0,
                    bytes: vec![0xff, 0xd8, 0xff, 0xd9],
                },
            )
            .unwrap();
        let result = service.commit("main", &begin.upload_id).unwrap();
        assert_eq!(result.status, VideoScreenshotStatusDto::Cancelled);
    }

    #[test]
    fn propagates_dialog_start_failure_without_turning_it_into_cancelled() {
        let service = VideoScreenshotService::new(Arc::new(FailingStorage));
        let begin = service.begin("main").unwrap();
        service
            .chunk(
                "main",
                VideoScreenshotChunkRequest {
                    upload_id: begin.upload_id.clone(),
                    sequence: 0,
                    bytes: vec![0xff, 0xd8, 0xff, 0xd9],
                },
            )
            .unwrap();

        let error = service.commit("main", &begin.upload_id).unwrap_err();

        assert_eq!(error.code().as_str(), "SCREENSHOT_DIALOG_START_FAILED");
        assert!(error.retryable());
    }

    #[test]
    fn expires_uploads_after_ttl() {
        let storage = Arc::new(FakeStorage {
            calls: AtomicUsize::new(0),
            outcome: VideoScreenshotSaveOutcome::Saved,
        });
        let service = VideoScreenshotService::with_ttl(storage, Duration::ZERO);
        let begin = service.begin("main").unwrap();
        let error = service.commit("main", &begin.upload_id).unwrap_err();
        assert_eq!(error.code().as_str(), "SCREENSHOT_UPLOAD_EXPIRED");
    }
}
