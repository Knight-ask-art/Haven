//! 云盘 PDF 的校验与只读读取（Provider 字节、ID 与令牌不进入错误文案）。
//!
//! 不变量：
//! - 读取以**完整绑定快照**为准：账户代际、目录绑定、位置状态与对象行在使用前后各复核一次，
//!   任何一次不一致都丢弃本次 IO 结果。
//! - 分片窗口由本模块**先夹紧到 EOF 再判断**：开口范围也会被封闭成有界窗口；超出单次响应上限
//!   的窗口（含开口过大）一律显式拒绝，绝不把夹紧交给传输层（那会得到与请求不一致的分片）。
//! - 字节返回后再 stat 一次，复核 MIME / 大小 / trashed / 父目录；大小与绑定不一致返回
//!   CLOUD_OBJECT_CHANGED，允许用户显式重新导入并重开，而不是追随新绑定。

use haven_common::{AppError, ErrorKind};

use crate::services::ports::{RemoteByteRange, RemoteSessionBody};

use super::credential_session::CredentialSessions;
use super::ports::{CloudDriveFileMetadata, CloudDrivePort};
use super::state::{
    CLOUD_OBJECT_MAX_BYTES, CloudFolderBinding, CloudObjectSnapshot, CloudPdfCandidate,
    CloudStorageRepository, cloud_binding_stale,
};

/// 单次响应允许的字节上限（与传输层媒体上限一致）。
pub(crate) const MAX_PDF_SINGLE_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_PDF_NAME_CHARS: usize = 300;
const PDF_MIME: &str = "application/pdf";
const PDF_MAGIC: &[u8] = b"%PDF-";

/// 请求窗口的规范化结果：总是有界，且总是已经夹紧到 EOF。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PdfReadPlan {
    /// 整份读取（仅当整份不超过单次响应上限）。
    Full { total: u64 },
    /// 闭区间分片；`end` 已夹紧到 `total - 1`。
    Range { start: u64, end: u64, total: u64 },
}

impl PdfReadPlan {
    /// 传给传输层的范围：整份为 `None`，分片一律为**显式闭合**区间。
    pub(crate) fn requested(self) -> Option<RemoteByteRange> {
        match self {
            PdfReadPlan::Full { .. } => None,
            PdfReadPlan::Range { start, end, .. } => Some(RemoteByteRange {
                start,
                end: Some(end),
            }),
        }
    }
}

/// 依据权威大小与请求范围算出可执行的读取计划。
pub(crate) fn plan_pdf_read(
    total: u64,
    requested: Option<RemoteByteRange>,
) -> Result<PdfReadPlan, AppError> {
    if total == 0 || total > CLOUD_OBJECT_MAX_BYTES {
        return Err(pdf_unsupported());
    }
    let Some(range) = requested else {
        if total > MAX_PDF_SINGLE_RESPONSE_BYTES {
            return Err(pdf_window_too_large());
        }
        return Ok(PdfReadPlan::Full { total });
    };
    if range.start >= total {
        return Err(pdf_range_invalid());
    }
    // 先夹紧到 EOF，再判断窗口大小：开口范围不得变成传输层的隐式截断。
    let end = range.end.unwrap_or(total - 1).min(total - 1);
    if end < range.start {
        return Err(pdf_range_invalid());
    }
    if end - range.start + 1 > MAX_PDF_SINGLE_RESPONSE_BYTES {
        return Err(pdf_window_too_large());
    }
    Ok(PdfReadPlan::Range {
        start: range.start,
        end,
        total,
    })
}

/// stat 结果 → 导入候选：必须是未回收、非目录、MIME 正确、大小有界的 PDF。
pub(crate) fn validate_pdf_candidate(
    metadata: &CloudDriveFileMetadata,
    provider_file_id: &str,
) -> Result<CloudPdfCandidate, AppError> {
    let size = validate_pdf_metadata(metadata, provider_file_id)?;
    let name = metadata.name.trim();
    if name.is_empty()
        || name.chars().count() > MAX_PDF_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return Err(pdf_unsupported());
    }
    Ok(CloudPdfCandidate {
        provider_file_id: provider_file_id.to_owned(),
        display_name: name.to_owned(),
        size_bytes: size,
    })
}

fn validate_pdf_metadata(
    metadata: &CloudDriveFileMetadata,
    provider_file_id: &str,
) -> Result<u64, AppError> {
    if metadata.id != provider_file_id
        || metadata.trashed
        || metadata.is_folder
        || metadata.mime_type != PDF_MIME
    {
        return Err(pdf_unsupported());
    }
    let size = metadata.size_bytes.ok_or_else(pdf_unsupported)?;
    if size == 0 || size > CLOUD_OBJECT_MAX_BYTES {
        return Err(pdf_unsupported());
    }
    Ok(size)
}

/// 字节返回后的形状校验：整份与分片的形状都必须与计划完全一致。
fn validate_pdf_body(body: &RemoteSessionBody, plan: PdfReadPlan) -> Result<(), AppError> {
    if body.mime_type != PDF_MIME
        || body.bytes.is_empty()
        || body.total_size == 0
        || body.total_size > CLOUD_OBJECT_MAX_BYTES
        || body.bytes.len() as u64 > MAX_PDF_SINGLE_RESPONSE_BYTES
    {
        return Err(pdf_unsupported());
    }
    match plan {
        PdfReadPlan::Full { total } => {
            if body.content_range.is_some()
                || body.total_size != total
                || body.bytes.len() as u64 != total
                || !body.bytes.starts_with(PDF_MAGIC)
            {
                return Err(pdf_unsupported());
            }
        }
        PdfReadPlan::Range { start, end, total } => {
            let range = body.content_range.ok_or_else(pdf_unsupported)?;
            if range.start != start
                || range.end != end
                || range.total != body.total_size
                || body.total_size != total
            {
                return Err(pdf_unsupported());
            }
            let length = range
                .end
                .checked_sub(range.start)
                .and_then(|length| length.checked_add(1))
                .ok_or_else(pdf_unsupported)?;
            if length != body.bytes.len() as u64 {
                return Err(pdf_unsupported());
            }
            if range.start == 0
                && !body
                    .bytes
                    .starts_with(&PDF_MAGIC[..PDF_MAGIC.len().min(body.bytes.len())])
            {
                return Err(pdf_unsupported());
            }
        }
    }
    Ok(())
}

/// 读取前后 stat：文件仍在绑定目录里，且大小与导入记录一致；不一致为对象变更，不是非 PDF。
fn validate_pdf_stat(
    metadata: &CloudDriveFileMetadata,
    provider_file_id: &str,
    folder: &CloudFolderBinding,
    expected_size: u64,
) -> Result<(), AppError> {
    let size = validate_pdf_metadata(metadata, provider_file_id)?;
    if size != expected_size {
        return Err(cloud_object_changed());
    }
    if !metadata
        .parents
        .iter()
        .any(|parent| parent == &folder.provider_folder_id)
    {
        return Err(cloud_binding_stale());
    }
    Ok(())
}

/// 只读读取一个已导入的 PDF 对象。
pub(crate) async fn read_pdf_object(
    repo: &dyn CloudStorageRepository,
    drive: &dyn CloudDrivePort,
    credentials: &CredentialSessions,
    object_id: &str,
    expected: &CloudObjectSnapshot,
    requested: Option<RemoteByteRange>,
) -> Result<RemoteSessionBody, AppError> {
    let snapshot = repo
        .object_snapshot(object_id)
        .await?
        .ok_or_else(object_missing)?;
    if &snapshot != expected || snapshot.object.id != object_id {
        return Err(cloud_binding_stale());
    }
    let plan = plan_pdf_read(snapshot.object.size_bytes, requested)?;
    repo.assert_current(snapshot.account_generation, &snapshot)
        .await?;
    let session = credentials.acquire(&snapshot.account_id).await?;
    if session.generation != snapshot.account_generation {
        return Err(cloud_binding_stale());
    }
    let credential = std::sync::Arc::clone(&session.credential);
    let before = drive
        .stat_file(&credential, &snapshot.object.provider_file_id)
        .await?;
    validate_pdf_stat(
        &before,
        &snapshot.object.provider_file_id,
        &snapshot.folder,
        snapshot.object.size_bytes,
    )?;
    credentials.recheck_current(&session).await?;
    repo.assert_current(snapshot.account_generation, &snapshot)
        .await?;
    let body = drive
        .read_pdf(
            &credential,
            &snapshot.object.provider_file_id,
            plan.requested(),
        )
        .await?;
    validate_pdf_body(&body, plan)?;
    let metadata = drive
        .stat_file(&credential, &snapshot.object.provider_file_id)
        .await?;
    validate_pdf_stat(
        &metadata,
        &snapshot.object.provider_file_id,
        &snapshot.folder,
        snapshot.object.size_bytes,
    )?;
    credentials.recheck_current(&session).await?;
    repo.assert_current(snapshot.account_generation, &snapshot)
        .await?;
    Ok(body)
}

fn object_missing() -> AppError {
    AppError::new(
        "CLOUD_OBJECT_NOT_FOUND",
        ErrorKind::NotFound,
        "云盘对象不存在",
        false,
    )
}

fn pdf_unsupported() -> AppError {
    AppError::new(
        "CLOUD_PDF_UNSUPPORTED",
        ErrorKind::Unsupported,
        "该文件不是可读取的云盘 PDF",
        false,
    )
}

/// 同 ID 远端文件大小变化需要显式重新导入；无 hash/revision，不声称能识别同大小改写。
fn cloud_object_changed() -> AppError {
    AppError::new(
        "CLOUD_OBJECT_CHANGED",
        ErrorKind::Conflict,
        "云盘文件已在远端变更，请重新导入后再打开",
        true,
    )
}

fn pdf_window_too_large() -> AppError {
    AppError::new(
        "CLOUD_PDF_WINDOW_TOO_LARGE",
        ErrorKind::Unsupported,
        "该 PDF 分片超出单次读取上限，请缩小读取范围",
        false,
    )
}

fn pdf_range_invalid() -> AppError {
    AppError::new(
        "CLOUD_PDF_RANGE_INVALID",
        ErrorKind::Validation,
        "云盘 PDF 读取范围非法",
        false,
    )
}
