//! SQLite Repository 实现（Repository Pattern，见 TECHNICAL_ARCHITECTURE §26）。
//!
//! 说明：
//! - 每个 Repository 持 `Arc<Db>`，方法内通过 `Db::lock()` 串行访问连接。
//! - 当前查询均为短事务级操作；Tauri 接线后重查询由外层 `spawn_blocking` 承接。
//! - Locator 序列化走 `Locator` 自身的 version + kind + data envelope（未知版本拒绝）。

pub mod download;
pub mod edition;
pub mod enrichment;
pub mod favorite;
pub mod hierarchy;
pub mod history;
pub mod image_proxy;
pub mod marker;
pub mod media_item;
pub mod progress;
pub mod resource;
pub mod resource_preferences;
pub mod search_history;
pub mod settings;
pub mod storage_location;
pub mod trending_cache;
pub mod work;
pub mod work_relation;

use haven_common::AppError;
use haven_domain::entities::ArtworkSet;
use haven_domain::locator::Locator;

pub use download::{SqliteDownloadBatchRepository, SqliteDownloadRepository};
pub use edition::SqliteEditionRepository;
pub use enrichment::SqliteEnrichmentRepository;
pub use favorite::SqliteFavoriteRepository;
pub use history::SqliteHistoryRepository;
pub use image_proxy::SqliteImageProxyRepository;
pub use marker::SqliteMarkerRepository;
pub use media_item::SqliteMediaItemRepository;
pub use progress::SqliteProgressRepository;
pub use resource::SqliteResourceRepository;
pub use resource_preferences::SqliteResourcePreferenceRepository;
pub use search_history::SqliteSearchHistoryRepository;
pub use settings::{SqliteSettingsRepository, SqliteSettingsUoW};
pub use storage_location::SqliteStorageLocationRepository;
pub use trending_cache::SqliteTrendingCacheRepository;
pub use work::SqliteWorkRepository;
pub use work_relation::SqliteWorkRelationRepository;

/// 一次持有全部已实现 Repository 的便捷容器。
pub struct SqliteRepositories {
    pub work: SqliteWorkRepository,
    pub edition: SqliteEditionRepository,
    pub download: SqliteDownloadRepository,
    pub download_batch: SqliteDownloadBatchRepository,
    pub media_item: SqliteMediaItemRepository,
    pub resource: SqliteResourceRepository,
    pub resource_preferences: SqliteResourcePreferenceRepository,
    pub progress: SqliteProgressRepository,
    pub marker: SqliteMarkerRepository,
    pub favorite: SqliteFavoriteRepository,
    pub history: SqliteHistoryRepository,
    pub settings: SqliteSettingsRepository,
    pub search_history: SqliteSearchHistoryRepository,
    pub image_proxy: SqliteImageProxyRepository,
    pub enrichment: SqliteEnrichmentRepository,
    pub storage_location: SqliteStorageLocationRepository,
    pub trending_cache: SqliteTrendingCacheRepository,
    pub work_relation: SqliteWorkRelationRepository,
}

impl SqliteRepositories {
    pub fn new(db: std::sync::Arc<crate::db::Db>) -> Self {
        Self {
            work: SqliteWorkRepository::new(db.clone()),
            edition: SqliteEditionRepository::new(db.clone()),
            download: SqliteDownloadRepository::new(db.clone()),
            download_batch: SqliteDownloadBatchRepository::new(db.clone()),
            media_item: SqliteMediaItemRepository::new(db.clone()),
            resource: SqliteResourceRepository::new(db.clone()),
            resource_preferences: SqliteResourcePreferenceRepository::new(db.clone()),
            progress: SqliteProgressRepository::new(db.clone()),
            marker: SqliteMarkerRepository::new(db.clone()),
            favorite: SqliteFavoriteRepository::new(db.clone()),
            history: SqliteHistoryRepository::new(db.clone()),
            settings: SqliteSettingsRepository::new(db.clone()),
            search_history: SqliteSearchHistoryRepository::new(db.clone()),
            image_proxy: SqliteImageProxyRepository::new(db.clone()),
            enrichment: SqliteEnrichmentRepository::new(db.clone()),
            storage_location: SqliteStorageLocationRepository::new(db.clone()),
            trending_cache: SqliteTrendingCacheRepository::new(db.clone()),
            work_relation: SqliteWorkRelationRepository::new(db),
        }
    }
}

// ---- 组合端口转发：SqliteRepositories 实现全部 Repository 契约 ----
// 使 `Arc<SqliteRepositories>` 可直接满足 Application 的组合端口
// （LibraryPorts / ProgressPorts / HistoryPorts / MarkerPorts / FavoritePorts）。

// 手写转发（trait 方法各不相同，逐个实现）。
#[async_trait::async_trait]
impl haven_domain::contracts::WorkRepository for SqliteRepositories {
    async fn get(
        &self,
        id: haven_domain::ids::WorkId,
    ) -> Result<Option<haven_domain::entities::Work>, haven_common::AppError> {
        self.work.get(id).await
    }
    async fn save(
        &self,
        work: &haven_domain::entities::Work,
    ) -> Result<(), haven_common::AppError> {
        self.work.save(work).await
    }
    async fn list(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<haven_domain::entities::Work>, haven_common::AppError> {
        self.work.list(limit, offset).await
    }
    async fn list_sorted(
        &self,
        order: haven_domain::contracts::WorkOrder,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<haven_domain::entities::Work>, haven_common::AppError> {
        self.work.list_sorted(order, limit, offset).await
    }
    async fn list_filtered(
        &self,
        order: haven_domain::contracts::WorkOrder,
        category: Option<haven_domain::enums::ContentCategory>,
        media_types: Option<&[haven_domain::enums::MediaType]>,
        query: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<haven_domain::entities::Work>, haven_common::AppError> {
        self.work
            .list_filtered(order, category, media_types, query, limit, offset)
            .await
    }
    async fn count_filtered(
        &self,
        category: Option<haven_domain::enums::ContentCategory>,
        media_types: Option<&[haven_domain::enums::MediaType]>,
        query: Option<&str>,
    ) -> Result<u64, haven_common::AppError> {
        self.work.count_filtered(category, media_types, query).await
    }
    async fn list_filtered_fts(
        &self,
        category: Option<haven_domain::enums::ContentCategory>,
        media_types: Option<&[haven_domain::enums::MediaType]>,
        query: &str,
        after_rank: Option<f64>,
        after_id: Option<haven_domain::ids::WorkId>,
        limit: u32,
    ) -> Result<Vec<(f64, haven_domain::entities::Work)>, haven_common::AppError> {
        self.work
            .list_filtered_fts(category, media_types, query, after_rank, after_id, limit)
            .await
    }
    async fn delete(&self, id: haven_domain::ids::WorkId) -> Result<bool, haven_common::AppError> {
        self.work.delete(id).await
    }
    async fn id_for_source_ref(
        &self,
        provider: &str,
        external_id: &str,
    ) -> Result<Option<haven_domain::ids::WorkId>, haven_common::AppError> {
        self.work.id_for_source_ref(provider, external_id).await
    }
    async fn has_any_source_ref(
        &self,
        id: haven_domain::ids::WorkId,
    ) -> Result<bool, haven_common::AppError> {
        self.work.has_any_source_ref(id).await
    }
    async fn save_source_ref(
        &self,
        provider: &str,
        external_id: &str,
        work_id: haven_domain::ids::WorkId,
    ) -> Result<(), haven_common::AppError> {
        self.work
            .save_source_ref(provider, external_id, work_id)
            .await
    }
}

#[async_trait::async_trait]
impl haven_domain::contracts::EditionRepository for SqliteRepositories {
    async fn get(
        &self,
        id: haven_domain::ids::EditionId,
    ) -> Result<Option<haven_domain::entities::Edition>, haven_common::AppError> {
        self.edition.get(id).await
    }
    async fn save(
        &self,
        edition: &haven_domain::entities::Edition,
    ) -> Result<(), haven_common::AppError> {
        self.edition.save(edition).await
    }
    async fn list_by_work(
        &self,
        work_id: haven_domain::ids::WorkId,
    ) -> Result<Vec<haven_domain::entities::Edition>, haven_common::AppError> {
        self.edition.list_by_work(work_id).await
    }
    async fn list_by_works(
        &self,
        work_ids: &[haven_domain::ids::WorkId],
    ) -> Result<Vec<haven_domain::entities::Edition>, haven_common::AppError> {
        self.edition.list_by_works(work_ids).await
    }
    async fn delete(
        &self,
        id: haven_domain::ids::EditionId,
    ) -> Result<bool, haven_common::AppError> {
        self.edition.delete(id).await
    }
}

#[async_trait::async_trait]
impl haven_domain::contracts::MediaItemRepository for SqliteRepositories {
    async fn get(
        &self,
        id: haven_domain::ids::MediaItemId,
    ) -> Result<Option<haven_domain::entities::MediaItem>, haven_common::AppError> {
        self.media_item.get(id).await
    }
    async fn save(
        &self,
        item: &haven_domain::entities::MediaItem,
    ) -> Result<(), haven_common::AppError> {
        self.media_item.save(item).await
    }
    async fn list_by_edition(
        &self,
        edition_id: haven_domain::ids::EditionId,
    ) -> Result<Vec<haven_domain::entities::MediaItem>, haven_common::AppError> {
        self.media_item.list_by_edition(edition_id).await
    }
    async fn list_by_editions(
        &self,
        edition_ids: &[haven_domain::ids::EditionId],
    ) -> Result<Vec<haven_domain::entities::MediaItem>, haven_common::AppError> {
        self.media_item.list_by_editions(edition_ids).await
    }
    async fn delete(
        &self,
        id: haven_domain::ids::MediaItemId,
    ) -> Result<bool, haven_common::AppError> {
        self.media_item.delete(id).await
    }
}

#[async_trait::async_trait]
impl haven_domain::contracts::ProgressRepository for SqliteRepositories {
    async fn get_for_media_item(
        &self,
        media_item_id: haven_domain::ids::MediaItemId,
    ) -> Result<Option<haven_domain::entities::Progress>, haven_common::AppError> {
        self.progress.get_for_media_item(media_item_id).await
    }
    async fn save(
        &self,
        progress: &haven_domain::entities::Progress,
    ) -> Result<(), haven_common::AppError> {
        self.progress.save(progress).await
    }
    async fn save_if_revision(
        &self,
        progress: &haven_domain::entities::Progress,
        expected_revision: Option<&str>,
    ) -> Result<Option<String>, haven_common::AppError> {
        self.progress
            .save_if_revision(progress, expected_revision)
            .await
    }
    async fn recent(
        &self,
        limit: u32,
    ) -> Result<Vec<haven_domain::entities::Progress>, haven_common::AppError> {
        self.progress.recent(limit).await
    }
    async fn get_for_media_items(
        &self,
        media_item_ids: &[haven_domain::ids::MediaItemId],
    ) -> Result<
        std::collections::HashMap<haven_domain::ids::MediaItemId, haven_domain::entities::Progress>,
        haven_common::AppError,
    > {
        self.progress.get_for_media_items(media_item_ids).await
    }
}

#[async_trait::async_trait]
impl haven_domain::contracts::HistoryRepository for SqliteRepositories {
    async fn get(
        &self,
        id: haven_domain::ids::HistoryEntryId,
    ) -> Result<Option<haven_domain::entities::HistoryEntry>, haven_common::AppError> {
        self.history.get(id).await
    }
    async fn save(
        &self,
        entry: &haven_domain::entities::HistoryEntry,
    ) -> Result<(), haven_common::AppError> {
        self.history.save(entry).await
    }
    async fn list_for_media_item(
        &self,
        media_item_id: haven_domain::ids::MediaItemId,
    ) -> Result<Vec<haven_domain::entities::HistoryEntry>, haven_common::AppError> {
        self.history.list_for_media_item(media_item_id).await
    }
    async fn recent(
        &self,
        limit: u32,
    ) -> Result<Vec<haven_domain::entities::HistoryEntry>, haven_common::AppError> {
        self.history.recent(limit).await
    }
    async fn clear_all(&self) -> Result<(), haven_common::AppError> {
        self.history.clear_all().await
    }
    async fn delete(
        &self,
        id: haven_domain::ids::HistoryEntryId,
    ) -> Result<bool, haven_common::AppError> {
        self.history.delete(id).await
    }
}

#[async_trait::async_trait]
impl haven_domain::contracts::SearchHistoryRepository for SqliteRepositories {
    async fn list(
        &self,
        limit: u32,
    ) -> Result<Vec<haven_domain::contracts::SearchHistoryEntry>, haven_common::AppError> {
        self.search_history.list(limit).await
    }

    async fn record(
        &self,
        term: &str,
        at: haven_common::UtcMillis,
    ) -> Result<(), haven_common::AppError> {
        self.search_history.record(term, at).await
    }

    async fn delete(&self, term: &str) -> Result<bool, haven_common::AppError> {
        self.search_history.delete(term).await
    }

    async fn clear_all(&self) -> Result<u64, haven_common::AppError> {
        self.search_history.clear_all().await
    }
}

#[async_trait::async_trait]
impl haven_domain::contracts::FavoriteRepository for SqliteRepositories {
    async fn set(
        &self,
        target: &haven_domain::entities::FavoriteTarget,
    ) -> Result<(), haven_common::AppError> {
        self.favorite.set(target).await
    }
    async fn unset(
        &self,
        target: &haven_domain::entities::FavoriteTarget,
    ) -> Result<bool, haven_common::AppError> {
        self.favorite.unset(target).await
    }
    async fn is_favorite(
        &self,
        target: &haven_domain::entities::FavoriteTarget,
    ) -> Result<bool, haven_common::AppError> {
        self.favorite.is_favorite(target).await
    }
    async fn list(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<haven_domain::entities::Favorite>, haven_common::AppError> {
        self.favorite.list(limit, offset).await
    }
    async fn is_favorite_many(
        &self,
        targets: &[haven_domain::entities::FavoriteTarget],
    ) -> Result<
        std::collections::HashSet<haven_domain::entities::FavoriteTarget>,
        haven_common::AppError,
    > {
        self.favorite.is_favorite_many(targets).await
    }
}

#[async_trait::async_trait]
impl haven_domain::contracts::MarkerRepository for SqliteRepositories {
    async fn list_for_media_item(
        &self,
        media_item_id: haven_domain::ids::MediaItemId,
    ) -> Result<Vec<haven_domain::entities::Marker>, haven_common::AppError> {
        self.marker.list_for_media_item(media_item_id).await
    }
    async fn list_all(
        &self,
        limit: u32,
    ) -> Result<Vec<haven_domain::entities::Marker>, haven_common::AppError> {
        self.marker.list_all(limit).await
    }
    async fn save(
        &self,
        marker: &haven_domain::entities::Marker,
    ) -> Result<(), haven_common::AppError> {
        self.marker.save(marker).await
    }
    async fn soft_delete(
        &self,
        id: haven_domain::ids::MarkerId,
    ) -> Result<bool, haven_common::AppError> {
        self.marker.soft_delete(id).await
    }
}

#[async_trait::async_trait]
impl haven_domain::contracts::ResourceRepository for SqliteRepositories {
    async fn get(
        &self,
        id: haven_domain::ids::ResourceId,
    ) -> Result<Option<haven_domain::entities::Resource>, haven_common::AppError> {
        self.resource.get(id).await
    }
    async fn save(
        &self,
        resource: &haven_domain::entities::Resource,
    ) -> Result<(), haven_common::AppError> {
        self.resource.save(resource).await
    }
    async fn list_by_media_item(
        &self,
        media_item_id: haven_domain::ids::MediaItemId,
    ) -> Result<Vec<haven_domain::entities::Resource>, haven_common::AppError> {
        self.resource.list_by_media_item(media_item_id).await
    }
    async fn delete(
        &self,
        id: haven_domain::ids::ResourceId,
    ) -> Result<bool, haven_common::AppError> {
        self.resource.delete(id).await
    }
    async fn mark_unavailable_by_storage(
        &self,
        storage_location_id: haven_domain::ids::StorageLocationId,
        availability: haven_domain::enums::Availability,
    ) -> Result<u64, haven_common::AppError> {
        self.resource
            .mark_unavailable_by_storage(storage_location_id, availability)
            .await
    }
    async fn delete_by_storage(
        &self,
        storage_location_id: haven_domain::ids::StorageLocationId,
    ) -> Result<u64, haven_common::AppError> {
        self.resource.delete_by_storage(storage_location_id).await
    }
}

#[async_trait::async_trait]
impl haven_domain::contracts::SettingsRepository for SqliteRepositories {
    async fn get(
        &self,
        section: &str,
    ) -> Result<Option<haven_domain::contracts::SettingsRow>, haven_common::AppError> {
        self.settings.get(section).await
    }
    async fn upsert(
        &self,
        row: &haven_domain::contracts::SettingsRow,
    ) -> Result<(), haven_common::AppError> {
        self.settings.upsert(row).await
    }
}

#[async_trait::async_trait]
impl haven_domain::contracts::ResourcePreferenceRepository for SqliteRepositories {
    async fn get_edition(
        &self,
        edition_id: haven_domain::ids::EditionId,
    ) -> Result<Option<haven_domain::contracts::EditionPreference>, haven_common::AppError> {
        self.resource_preferences.get_edition(edition_id).await
    }
    async fn get_media_item(
        &self,
        media_item_id: haven_domain::ids::MediaItemId,
    ) -> Result<Option<haven_domain::contracts::MediaItemPreference>, haven_common::AppError> {
        self.resource_preferences
            .get_media_item(media_item_id)
            .await
    }
    async fn cas_upsert_edition(
        &self,
        preference: &haven_domain::contracts::EditionPreference,
        expected_revision: Option<&str>,
    ) -> Result<bool, haven_common::AppError> {
        self.resource_preferences
            .cas_upsert_edition(preference, expected_revision)
            .await
    }
    async fn cas_upsert_media_item(
        &self,
        preference: &haven_domain::contracts::MediaItemPreference,
        expected_revision: Option<&str>,
    ) -> Result<bool, haven_common::AppError> {
        self.resource_preferences
            .cas_upsert_media_item(preference, expected_revision)
            .await
    }
}

#[async_trait::async_trait]
impl haven_domain::contracts::ImageProxyRepository for SqliteRepositories {
    async fn register(
        &self,
        source_id: &str,
        target_url: &str,
    ) -> Result<String, haven_common::AppError> {
        self.image_proxy.register(source_id, target_url).await
    }
    async fn resolve(&self, id: &str) -> Result<Option<String>, haven_common::AppError> {
        self.image_proxy.resolve(id).await
    }
}

#[async_trait::async_trait]
impl haven_application::services::trending::TrendingCachePort for SqliteRepositories {
    async fn list(
        &self,
    ) -> Result<Vec<haven_application::services::trending::TrendingBoardCacheEntry>, AppError> {
        self.trending_cache.list().await
    }

    async fn upsert(
        &self,
        entry: &haven_application::services::trending::TrendingBoardCacheEntry,
    ) -> Result<(), AppError> {
        self.trending_cache.upsert(entry).await
    }
}

#[async_trait::async_trait]
impl haven_application::services::trending::ArtworkCachePort for SqliteRepositories {
    async fn register(&self, source_id: &str, target_url: &str) -> Result<String, AppError> {
        haven_domain::contracts::ImageProxyRepository::register(self, source_id, target_url).await
    }
}

#[async_trait::async_trait]
impl haven_domain::contracts::StorageLocationRepository for SqliteRepositories {
    async fn get(
        &self,
        id: haven_domain::ids::StorageLocationId,
    ) -> Result<Option<haven_domain::entities::StorageLocation>, haven_common::AppError> {
        self.storage_location.get(id).await
    }
    async fn save(
        &self,
        location: &haven_domain::entities::StorageLocation,
    ) -> Result<(), haven_common::AppError> {
        self.storage_location.save(location).await
    }
    async fn list(
        &self,
    ) -> Result<Vec<haven_domain::entities::StorageLocation>, haven_common::AppError> {
        self.storage_location.list().await
    }
    async fn delete(
        &self,
        id: haven_domain::ids::StorageLocationId,
    ) -> Result<bool, haven_common::AppError> {
        self.storage_location.delete(id).await
    }
    async fn clear_credential_ref(
        &self,
        id: haven_domain::ids::StorageLocationId,
    ) -> Result<bool, haven_common::AppError> {
        self.storage_location.clear_credential_ref(id).await
    }
}

#[async_trait::async_trait]
impl haven_domain::contracts::DownloadRepository for SqliteRepositories {
    async fn get(
        &self,
        id: haven_domain::ids::DownloadTaskId,
    ) -> Result<Option<haven_domain::entities::DownloadTask>, haven_common::AppError> {
        self.download.get(id).await
    }
    async fn save(
        &self,
        task: &haven_domain::entities::DownloadTask,
    ) -> Result<(), haven_common::AppError> {
        self.download.save(task).await
    }
    async fn list(
        &self,
        limit: u32,
    ) -> Result<Vec<haven_domain::entities::DownloadTask>, haven_common::AppError> {
        self.download.list(limit).await
    }
    async fn find_active(
        &self,
        source_resource_id: haven_domain::ids::ResourceId,
        target_storage_id: haven_domain::ids::StorageLocationId,
    ) -> Result<Option<haven_domain::entities::DownloadTask>, haven_common::AppError> {
        self.download
            .find_active(source_resource_id, target_storage_id)
            .await
    }
    async fn delete_terminal(
        &self,
        id: haven_domain::ids::DownloadTaskId,
    ) -> Result<bool, haven_common::AppError> {
        self.download.delete_terminal(id).await
    }
    async fn associate_offline_resource(
        &self,
        id: haven_domain::ids::DownloadTaskId,
        expected: haven_domain::enums::DownloadState,
        resource_id: haven_domain::ids::ResourceId,
    ) -> Result<bool, haven_common::AppError> {
        self.download
            .associate_offline_resource(id, expected, resource_id)
            .await
    }
    async fn compare_and_set_state(
        &self,
        id: haven_domain::ids::DownloadTaskId,
        expected: haven_domain::enums::DownloadState,
        next: haven_domain::enums::DownloadState,
    ) -> Result<bool, haven_common::AppError> {
        self.download
            .compare_and_set_state(id, expected, next)
            .await
    }
    async fn update_progress(
        &self,
        id: haven_domain::ids::DownloadTaskId,
        expected: haven_domain::enums::DownloadState,
        bytes_total: Option<u64>,
        bytes_downloaded: u64,
        speed_bps: Option<u64>,
        eta_seconds: Option<u64>,
    ) -> Result<bool, haven_common::AppError> {
        self.download
            .update_progress(
                id,
                expected,
                bytes_total,
                bytes_downloaded,
                speed_bps,
                eta_seconds,
            )
            .await
    }
    async fn mark_active_interrupted(&self) -> Result<u64, haven_common::AppError> {
        self.download.mark_active_interrupted().await
    }
    async fn list_by_batch(
        &self,
        batch_id: haven_domain::ids::DownloadBatchId,
    ) -> Result<Vec<haven_domain::entities::DownloadTask>, haven_common::AppError> {
        self.download.list_by_batch(batch_id).await
    }
    async fn list_schedulable(
        &self,
        limit: u32,
        now: haven_common::UtcMillis,
    ) -> Result<Vec<haven_domain::entities::DownloadTask>, haven_common::AppError> {
        self.download.list_schedulable(limit, now).await
    }
}

#[async_trait::async_trait]
impl haven_domain::contracts::DownloadBatchRepository for SqliteRepositories {
    async fn get(
        &self,
        id: haven_domain::ids::DownloadBatchId,
    ) -> Result<Option<haven_domain::entities::DownloadBatch>, haven_common::AppError> {
        self.download_batch.get(id).await
    }
    async fn save(
        &self,
        batch: &haven_domain::entities::DownloadBatch,
    ) -> Result<(), haven_common::AppError> {
        self.download_batch.save(batch).await
    }
    async fn list(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<haven_domain::entities::DownloadBatch>, haven_common::AppError> {
        self.download_batch.list(limit, offset).await
    }
}

/// ArtworkSet → 四列（poster/cover/backdrop/thumbnail），每列存单个 ArtworkRef 的 JSON。
pub(crate) fn artwork_to_json(value: &ArtworkSet) -> Result<[Option<String>; 4], AppError> {
    let encode =
        |item: &Option<haven_domain::entities::ArtworkRef>| -> Result<Option<String>, AppError> {
            match item {
                Some(artwork) => serde_json::to_string(artwork)
                    .map(Some)
                    .map_err(|e| json_err("Artwork 序列化失败", e)),
                None => Ok(None),
            }
        };
    Ok([
        encode(&value.poster)?,
        encode(&value.cover)?,
        encode(&value.backdrop)?,
        encode(&value.thumbnail)?,
    ])
}

/// 四列（poster/cover/backdrop/thumbnail）→ ArtworkSet。
pub(crate) fn artwork_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtworkSet> {
    let decode = |col: &str| -> rusqlite::Result<Option<haven_domain::entities::ArtworkRef>> {
        let raw: Option<String> = row.get(col)?;
        match raw {
            Some(json) => serde_json::from_str(&json).map(Some).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            }),
            None => Ok(None),
        }
    };
    Ok(ArtworkSet {
        poster: decode("poster")?,
        cover: decode("cover")?,
        backdrop: decode("backdrop")?,
        thumbnail: decode("thumbnail")?,
    })
}

pub(crate) fn json_err(msg: &'static str, e: serde_json::Error) -> AppError {
    AppError::new(
        "SERIALIZE_FAILED",
        haven_common::ErrorKind::Parse,
        msg,
        false,
    )
    .with_source(e)
}

pub(crate) fn locator_to_json(locator: &Locator) -> Result<String, AppError> {
    serde_json::to_string(locator).map_err(|e| json_err("Locator 序列化失败", e))
}

pub(crate) fn locator_from_json(json: &str) -> Result<Locator, AppError> {
    serde_json::from_str(json).map_err(|e| {
        AppError::new(
            "LOCATOR_PARSE_FAILED",
            haven_common::ErrorKind::Parse,
            "Locator 反序列化失败（可能来自未知版本或损坏数据）",
            false,
        )
        .with_source(e)
    })
}

pub(crate) fn id_from_row<T>(value: String) -> Result<T, rusqlite::Error>
where
    T: std::str::FromStr,
{
    value.parse().map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "无法解析实体 ID",
            )),
        )
    })
}

pub(crate) fn map_db_error(context: &'static str) -> impl Fn(rusqlite::Error) -> AppError {
    move |e| {
        AppError::new(
            "DATABASE_ERROR",
            haven_common::ErrorKind::Database,
            context,
            true,
        )
        .with_source(e)
    }
}

/// 枚举 → DB 存储字符串（snake_case，不带 JSON 引号）。
/// schema 约定：枚举一律 TEXT（snake_case）。
pub(crate) fn enum_to_db_str<T: serde::Serialize>(value: &T) -> Result<String, AppError> {
    let json = serde_json::to_value(value).map_err(|e| {
        AppError::new(
            "SERIALIZE_FAILED",
            haven_common::ErrorKind::Parse,
            "枚举序列化失败",
            false,
        )
        .with_source(e)
    })?;
    json.as_str().map(str::to_owned).ok_or_else(|| {
        AppError::new(
            "SERIALIZE_FAILED",
            haven_common::ErrorKind::Parse,
            "枚举未序列化为字符串",
            false,
        )
    })
}

// 内容层级校验的唯一实现位于 `hierarchy` 模块（validate_content_chain，错误码
// CONTENT_CHAIN_INVALID），供 HistoryEntry / Progress / Marker 等复用。
// 注意：历史上曾存在重复实现 ensure_content_chain（INVALID_CONTENT_CHAIN），
// 已于 2026-08-13 统一移除，调用方以 hierarchy::validate_content_chain 为准。

#[async_trait::async_trait]
impl haven_domain::contracts::EnrichmentRepository for SqliteRepositories {
    async fn get(
        &self,
        work_id: haven_domain::ids::WorkId,
    ) -> Result<Option<haven_domain::contracts::EnrichmentState>, haven_common::AppError> {
        self.enrichment.get(work_id).await
    }
    async fn list(
        &self,
        work_id: Option<haven_domain::ids::WorkId>,
    ) -> Result<Vec<haven_domain::contracts::EnrichmentState>, haven_common::AppError> {
        self.enrichment.list(work_id).await
    }
    async fn upsert(
        &self,
        state: &haven_domain::contracts::EnrichmentState,
    ) -> Result<(), haven_common::AppError> {
        self.enrichment.upsert(state).await
    }
}

#[async_trait::async_trait]
impl haven_domain::contracts::WorkRelationRepository for SqliteRepositories {
    async fn list_relations_by_work(
        &self,
        work_id: haven_domain::ids::WorkId,
    ) -> Result<Vec<haven_domain::entities::WorkRelation>, haven_common::AppError> {
        self.work_relation.list_relations_by_work(work_id).await
    }
    async fn save_relation(
        &self,
        relation: &haven_domain::entities::WorkRelation,
    ) -> Result<(), haven_common::AppError> {
        self.work_relation.save_relation(relation).await
    }
    async fn delete_relation(&self, id: String) -> Result<bool, haven_common::AppError> {
        self.work_relation.delete_relation(id).await
    }
}
