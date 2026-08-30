//! 受控技术缓存清理（V02-SETTINGS-PRIVACY-DATA-007）。
//!
//! 这是窄范围的 Artwork Cache 用例，不是通用 Cache Manager。清理只接受显式
//! 技术缓存 scope，永远不触碰 Offline Resource、原始媒体或业务事实。

use std::sync::Arc;

use crate::wire::{CacheClearResultDto, CacheScopeDto};
use async_trait::async_trait;
use haven_common::AppError;

#[async_trait]
pub trait ArtworkCacheClearPort: Send + Sync {
    async fn clear_all(&self) -> Result<u64, AppError>;
}

#[derive(Clone)]
pub struct CacheService {
    artwork: Arc<dyn ArtworkCacheClearPort>,
}

impl CacheService {
    pub fn new(artwork: Arc<dyn ArtworkCacheClearPort>) -> Self {
        Self { artwork }
    }

    pub async fn clear(&self, scope: CacheScopeDto) -> Result<CacheClearResultDto, AppError> {
        match scope {
            CacheScopeDto::Artwork => Ok(CacheClearResultDto {
                scope,
                removed_entries: self.artwork.clear_all().await?,
            }),
            CacheScopeDto::Thumbnails | CacheScopeDto::ProviderResponseCache => Err(AppError::new(
                "CACHE_SCOPE_UNAVAILABLE",
                haven_common::ErrorKind::Unsupported,
                "当前版本没有可清理的该类技术缓存",
                false,
            )),
        }
    }
}
