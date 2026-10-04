//! 来源原始配置缓存端口（Application 边界）。
//!
//! 这个模块只回答一件事：**某个来源最后一次成功获取到的原始配置是什么**。它不
//! 注册来源、不改启用状态、不解释配置内容，也不承担刷新/ETag 语义——本切片没有
//! 任何自动重新获取路径，写入只发生在用户显式导入并且取回与解析都成功之后。
//!
//! 边界：
//! - 缓存按 `sourceId` 寻址。端点、凭据引用与启用状态属于来源身份，仍由
//!   `SourceRegistryService` 唯一拥有；缓存里没有这些字段，也不参与身份判定。
//! - [`CachedSourceConfig`] 承载的是**不可信原始字节**（可能含站点端点、header 与
//!   令牌），因此刻意不实现 `Debug` / `Serialize`：它不允许出现在日志、错误文案或
//!   IPC 载荷里。需要离开后端时，只能先经解析器变成安全摘要。
//! - 读取端不因缺失而报错：没有缓存就是 `None`，不是「空配置」。
//!
//! 删除语义（与 ADR-001 的关系）：来源被移除时必须一并清掉它的缓存，否则会在库里
//! 留下一份没有任何身份指向的原始配置。删除顺序由 `SourceRegistryService` 统一编排
//! ——先删系统凭据，再删缓存，最后清理持久化记录。

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use haven_common::{AppError, UtcMillis};

/// 一次成功获取后保留下来的原始配置。
///
/// 刻意不实现 `Debug`：`body` 是不可信原文，任何 `{:?}` 都可能把它写进日志。
#[derive(Clone, PartialEq, Eq)]
pub struct CachedSourceConfig {
    body: Vec<u8>,
    fetched_at: UtcMillis,
}

impl CachedSourceConfig {
    pub fn new(body: Vec<u8>, fetched_at: UtcMillis) -> Self {
        Self { body, fetched_at }
    }

    /// 原始配置字节。仅后端使用；不得写入日志、错误或摘要。
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub fn fetched_at(&self) -> UtcMillis {
        self.fetched_at
    }
}

/// 按来源保存「最后一次成功获取的原始配置」的端口。
///
/// 实现负责持久化与并发；失败必须返回带稳定错误码的 [`AppError`]，且错误里不能
/// 出现配置原文、端点或任何配置值。
#[async_trait]
pub trait SourceConfigCache: Send + Sync {
    /// 原子幂等覆盖写入。`sourceId` 由调用方给出（来源身份属于注册表，不由缓存生成）；
    /// 返回 `Err` 时必须保持该 ID 原有的缓存值不变。
    async fn put(
        &self,
        source_id: &str,
        body: &[u8],
        fetched_at: UtcMillis,
    ) -> Result<(), AppError>;

    /// 读取；没有缓存返回 `None`（不是空配置，也不是错误）。
    async fn get(&self, source_id: &str) -> Result<Option<CachedSourceConfig>, AppError>;

    /// 删除；返回是否实际删除（不存在返回 `false`，删除本身幂等）。
    async fn delete(&self, source_id: &str) -> Result<bool, AppError>;
}

/// 进程内实现，供 Application 单元测试与不落盘的装配使用。
///
/// 它不做任何持久化：重启即丢失。生产装配必须使用 SQLite 实现，否则「最后一次
/// 成功获取的配置」在重启后消失，会变成一个看起来可用、实际不成立的能力。
#[derive(Default)]
pub struct InMemorySourceConfigCache {
    entries: Mutex<HashMap<String, CachedSourceConfig>>,
}

impl InMemorySourceConfigCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, CachedSourceConfig>> {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

#[async_trait]
impl SourceConfigCache for InMemorySourceConfigCache {
    async fn put(
        &self,
        source_id: &str,
        body: &[u8],
        fetched_at: UtcMillis,
    ) -> Result<(), AppError> {
        self.lock().insert(
            source_id.to_owned(),
            CachedSourceConfig::new(body.to_vec(), fetched_at),
        );
        Ok(())
    }

    async fn get(&self, source_id: &str) -> Result<Option<CachedSourceConfig>, AppError> {
        Ok(self.lock().get(source_id).cloned())
    }

    async fn delete(&self, source_id: &str) -> Result<bool, AppError> {
        Ok(self.lock().remove(source_id).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_entry_is_none_and_delete_is_idempotent() {
        let cache = InMemorySourceConfigCache::new();
        assert!(cache.get("custom_tvbox_0001").await.unwrap().is_none());
        assert!(!cache.delete("custom_tvbox_0001").await.unwrap());
    }

    #[tokio::test]
    async fn put_then_get_round_trips_the_raw_bytes() {
        let cache = InMemorySourceConfigCache::new();
        cache
            .put("custom_tvbox_0001", br#"{"sites":[]}"#, UtcMillis(1_000))
            .await
            .unwrap();
        let cached = cache
            .get("custom_tvbox_0001")
            .await
            .unwrap()
            .expect("写入后必须可读回");
        assert_eq!(cached.body(), br#"{"sites":[]}"#);
        assert_eq!(cached.fetched_at(), UtcMillis(1_000));
        assert!(cache.delete("custom_tvbox_0001").await.unwrap());
        assert!(cache.get("custom_tvbox_0001").await.unwrap().is_none());
    }
}
