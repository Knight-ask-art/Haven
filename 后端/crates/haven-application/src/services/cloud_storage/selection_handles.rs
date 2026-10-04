//! 浏览句柄租约注册表：进程内、有界、5 分钟 TTL。
//!
//! - 句柄是对外唯一选择标识（UUID v4 字符串）；真实 Drive ID / 分页 token 只存在于租约内。
//! - 容量固定 [`CLOUD_BROWSE_MAX_LEASES`]；**先清理过期再计数**，满员时显式拒绝，
//!   绝不静默淘汰仍在使用中的选择。
//! - 光标句柄一次性消费；目录 / 文件句柄可重复使用，但 TTL 固定不续期。
//! - 光标租约记录**本页**签发的条目句柄；光标被消费时只退休这些「上一页」条目句柄，
//!   父目录句柄与新一页的条目句柄都不受影响，因此翻多少页都保持有界。

use std::collections::HashMap;
use std::sync::Mutex;

use haven_common::{AppError, ErrorKind, UtcMillis};

use super::state::CloudFolderBinding;

/// 句柄租约 TTL（5 分钟）。
pub const CLOUD_BROWSE_HANDLE_TTL_MS: i64 = 300_000;
/// 同时存活的租约总数上限（目录 + 文件 + 光标）。
pub const CLOUD_BROWSE_MAX_LEASES: usize = 2048;

/// 句柄非本注册表签发（含格式非法、未知、类型不符）：稳定、脱敏、失败关闭。
pub fn cloud_browse_handle_invalid() -> AppError {
    AppError::new(
        "CLOUD_BROWSE_HANDLE_INVALID",
        ErrorKind::Validation,
        "浏览句柄无效，请重新浏览",
        false,
    )
}

/// 句柄已过期：稳定、脱敏、失败关闭。
pub fn cloud_browse_handle_expired() -> AppError {
    AppError::new(
        "CLOUD_BROWSE_HANDLE_EXPIRED",
        ErrorKind::NotFound,
        "浏览句柄已过期，请重新浏览",
        false,
    )
}

/// 容量拒绝：显式告知，不淘汰活动中的选择。
pub fn cloud_browse_busy() -> AppError {
    AppError::new(
        "CLOUD_BROWSE_BUSY",
        ErrorKind::Conflict,
        "浏览会话过多，请稍后重试",
        true,
    )
}

/// 句柄必须是不透明 UUID；任何真实 Provider ID / 原始 token 都会被拒绝。
pub(crate) fn is_opaque_handle(value: &str) -> bool {
    super::credential_session::is_canonical_uuid(value)
}

/// 目录句柄租约：账户代际 + 精确目录身份 + 可选已登记目录快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FolderLease {
    pub account_id: String,
    pub generation: i64,
    /// 真实 Drive 目录 ID（服务端专用）。
    pub folder_id: String,
    /// 列举出该目录的真实父目录 ID；`root` 解析结果与已登记目录为 `None`。
    pub parent_id: Option<String>,
    pub display_name: String,
    /// 该目录本身已是登记目录时的精确绑定快照。
    pub registered: Option<CloudFolderBinding>,
}

/// 文件句柄租约：账户代际 + 精确父目录 + 精确文件身份 + 已登记目录快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileLease {
    pub account_id: String,
    pub generation: i64,
    /// 列举出该文件的真实父目录 ID。
    pub folder_id: String,
    /// 真实 Drive 文件 ID（服务端专用）。
    pub file_id: String,
    /// 仅当该文件位于已登记目录下才为 `Some`；否则导入必须失败关闭。
    pub registered: Option<CloudFolderBinding>,
}

/// 光标句柄租约：分页 token 只在此处存在，一次性消费。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CursorLease {
    pub account_id: String,
    pub generation: i64,
    pub folder_id: String,
    /// 回显给 UI 的目录句柄（仍是句柄，不是 Drive ID）。
    pub folder_handle: String,
    pub registered: Option<CloudFolderBinding>,
    /// 原始 Provider 分页 token：绝不接受调用方提供的 token。
    pub page_token: String,
    /// 本页（即产生该光标的那一页）签发的条目句柄；光标被消费时只退休这些句柄。
    /// 父目录句柄与新一页新签发的句柄都不在此列表内。
    pub owned_handles: Vec<String>,
}

#[derive(Debug, Clone)]
enum Entry {
    Folder(FolderLease),
    File(FileLease),
    Cursor(CursorLease),
}

#[derive(Debug, Clone)]
struct Stored {
    expires_at_ms: i64,
    entry: Entry,
}

/// 有界句柄注册表。`Mutex` 只保护内存映射，任何网络 / DB IO 都在锁外。
pub(crate) struct BrowseHandleRegistry {
    inner: Mutex<HashMap<String, Stored>>,
}

impl BrowseHandleRegistry {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn insert_folder(
        &self,
        lease: FolderLease,
        now: UtcMillis,
    ) -> Result<String, AppError> {
        self.insert(now, Entry::Folder(lease))
    }

    pub(crate) fn insert_file(&self, lease: FileLease, now: UtcMillis) -> Result<String, AppError> {
        self.insert(now, Entry::File(lease))
    }

    pub(crate) fn insert_cursor(
        &self,
        lease: CursorLease,
        now: UtcMillis,
    ) -> Result<String, AppError> {
        self.insert(now, Entry::Cursor(lease))
    }

    pub(crate) fn folder(&self, handle: &str, now: UtcMillis) -> Result<FolderLease, AppError> {
        match self.lookup(handle, now)? {
            Entry::Folder(lease) => Ok(lease),
            _ => Err(cloud_browse_handle_invalid()),
        }
    }

    pub(crate) fn file(&self, handle: &str, now: UtcMillis) -> Result<FileLease, AppError> {
        match self.lookup(handle, now)? {
            Entry::File(lease) => Ok(lease),
            _ => Err(cloud_browse_handle_invalid()),
        }
    }

    /// 光标一次性消费：取出即失效，重复使用或过期都失败关闭。消费成功时只退休该光标
    /// 所属页面的条目句柄（目录 / 文件），绝不触碰父目录句柄、光标或其他页面的句柄。
    pub(crate) fn take_cursor(
        &self,
        handle: &str,
        now: UtcMillis,
    ) -> Result<CursorLease, AppError> {
        if !is_opaque_handle(handle) {
            return Err(cloud_browse_handle_invalid());
        }
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(stored) = map.remove(handle) else {
            return Err(cloud_browse_handle_invalid());
        };
        if stored.expires_at_ms <= now.0 {
            return Err(cloud_browse_handle_expired());
        }
        match stored.entry {
            Entry::Cursor(lease) => {
                for owned in &lease.owned_handles {
                    let is_page_entry = matches!(
                        map.get(owned).map(|stored| &stored.entry),
                        Some(Entry::Folder(_) | Entry::File(_))
                    );
                    if is_page_entry {
                        map.remove(owned);
                    }
                }
                Ok(lease)
            }
            _ => Err(cloud_browse_handle_invalid()),
        }
    }

    fn insert(&self, now: UtcMillis, entry: Entry) -> Result<String, AppError> {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // 先清理过期，再计数；满员显式拒绝，不淘汰活动选择。
        map.retain(|_, stored| stored.expires_at_ms > now.0);
        if map.len() >= CLOUD_BROWSE_MAX_LEASES {
            return Err(cloud_browse_busy());
        }
        let handle = uuid::Uuid::new_v4().to_string();
        map.insert(
            handle.clone(),
            Stored {
                expires_at_ms: now.0.saturating_add(CLOUD_BROWSE_HANDLE_TTL_MS),
                entry,
            },
        );
        Ok(handle)
    }

    fn lookup(&self, handle: &str, now: UtcMillis) -> Result<Entry, AppError> {
        if !is_opaque_handle(handle) {
            return Err(cloud_browse_handle_invalid());
        }
        let map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(stored) = map.get(handle) else {
            return Err(cloud_browse_handle_invalid());
        };
        if stored.expires_at_ms <= now.0 {
            return Err(cloud_browse_handle_expired());
        }
        Ok(stored.entry.clone())
    }
}

#[cfg(test)]
mod tests;
