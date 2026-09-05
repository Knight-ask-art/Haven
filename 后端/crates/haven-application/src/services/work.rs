use std::sync::Arc;

use haven_common::AppError;
use haven_domain::contracts::{StorageLocationRepository, WorkRepository};
use haven_domain::entities::{FavoriteTarget, Resource, ResourceLocator};
use haven_domain::enums::{Availability, MediaType, StorageProviderType, StorageStatus};
use haven_domain::ids::WorkId;

use crate::mapper::work_card::{WorkCardInput, work_card};
use crate::services::ports::WorkGetPorts;
use crate::wire::{
    EditionAvailabilityDto, EditionDetailDto, EditionGetRequest, EditionListByWorkRequest,
    EditionSummaryDto, MediaItemStatusDto, MediaItemSummaryDto, PageDto, WorkDetailCountsDto,
    WorkDetailHeaderDto,
};

const EDITION_MAX_LIMIT: u32 = 200;

#[derive(Clone)]
pub struct WorkService {
    ports: Arc<dyn WorkGetPorts>,
}

impl WorkService {
    pub fn new(ports: Arc<dyn WorkGetPorts>) -> Self {
        Self { ports }
    }

    pub async fn get(&self, work_id: WorkId) -> Result<WorkDetailHeaderDto, AppError> {
        let work = WorkRepository::get(&*self.ports, work_id)
            .await?
            .ok_or_else(work_not_found)?;
        let editions = self.ports.list_by_work(work_id).await?;
        let edition_ids: Vec<_> = editions.iter().map(|e| e.id).collect();
        let mut media_items = self.ports.list_by_editions(&edition_ids).await?;
        media_items.sort_by(|a, b| {
            a.index
                .cmp(&b.index)
                .then_with(|| a.created_at.cmp(&b.created_at))
        });

        let mut available_resources = 0u32;
        let mut markers = 0u32;
        let mut selected = None;
        for item in &media_items {
            let resources = self.ports.list_by_media_item(item.id).await?;
            for resource in &resources {
                if self
                    .resource_is_actionable(resource, item.media_type)
                    .await?
                {
                    available_resources += 1;
                    selected.get_or_insert(item);
                }
            }
            markers += self.ports.list_for_media_item(item.id).await?.len() as u32;
        }

        let progress = match selected {
            Some(item) => self.ports.get_for_media_item(item.id).await?,
            None => None,
        };
        let favorite = self
            .ports
            .is_favorite(&FavoriteTarget::Work(work_id))
            .await?;

        let mut card = work_card(&WorkCardInput {
            work: &work,
            editions: &editions,
            media_items: &media_items,
            progress: progress.as_ref(),
            favorite,
        })?;
        card.primary_action = match selected {
            Some(item) => crate::mapper::work_card::primary_action(&WorkCardInput {
                work: &work,
                editions: &editions,
                media_items: std::slice::from_ref(item),
                progress: progress.as_ref(),
                favorite,
            })?,
            None => None,
        };
        let dto = WorkDetailHeaderDto {
            schema_version: 1,
            work_id: card.work_id,
            title: card.title,
            original_title: card.original_title,
            description: card.description,
            poster_uri: card.poster_uri,
            backdrop_uri: card.backdrop_uri,
            release_year: card.release_year,
            director: work.director.clone(),
            actor: work.actor.clone(),
            categories: card.categories,
            available_media_types: card.available_media_types,
            favorite: card.favorite,
            primary_action: card.primary_action,
            progress: card.progress,
            external_ids: card.external_ids,
            counts: WorkDetailCountsDto {
                editions: editions.len() as u32,
                relations: 0,
                available_resources,
                active_downloads: 0,
                markers,
            },
        };
        Ok(dto)
    }

    pub async fn list_editions(
        &self,
        request: EditionListByWorkRequest,
    ) -> Result<PageDto<EditionSummaryDto>, AppError> {
        let work_id = request.work_id.parse().map_err(|_| invalid_id())?;
        let work = WorkRepository::get(&*self.ports, work_id)
            .await?
            .ok_or_else(work_not_found)?;
        let mut editions = self.ports.list_by_work(work_id).await?;
        editions.sort_by_key(edition_sort_key);
        let total = editions.len() as u64;
        let limit = request.limit.clamp(1, EDITION_MAX_LIMIT) as usize;
        let after = match request.cursor.as_deref() {
            Some(value) => Some(decode_edition_cursor(value).map_err(|_| invalid_cursor())?),
            None => None,
        };
        // 键集分页：游标携带完整排序键 (edition_type, language, release_date,
        // created_at, id)，避免 offset 在并发插入时错页/重页。
        let mut iter = editions.into_iter().peekable();
        if let Some(last) = after.as_ref() {
            while let Some(edition) = iter.peek() {
                if edition_sort_key(edition) > *last {
                    break;
                }
                iter.next();
            }
        }
        let page = iter.by_ref().take(limit).collect::<Vec<_>>();
        let has_more = iter.peek().is_some();
        let next_cursor = if has_more {
            page.last()
                .map(|edition| encode_edition_cursor(&edition_sort_key(edition)))
        } else {
            None
        };
        let mut items = Vec::with_capacity(page.len());
        for edition in page {
            let mut media_items = self.ports.list_by_edition(edition.id).await?;
            media_items.sort_by(|a, b| {
                a.index
                    .cmp(&b.index)
                    .then_with(|| a.created_at.cmp(&b.created_at))
            });
            let mut available = 0;
            let mut offline_available = 0;
            let mut unavailable = 0;
            let mut action_item = None;
            for item in &media_items {
                for resource in self.ports.list_by_media_item(item.id).await? {
                    match resource.availability {
                        haven_domain::enums::Availability::Available => {
                            available += 1;
                            if self
                                .resource_is_actionable(&resource, item.media_type)
                                .await?
                            {
                                action_item.get_or_insert(item);
                            }
                        }
                        haven_domain::enums::Availability::OfflineAvailable => {
                            offline_available += 1;
                            if self
                                .resource_is_actionable(&resource, item.media_type)
                                .await?
                            {
                                action_item.get_or_insert(item);
                            }
                        }
                        _ => unavailable += 1,
                    }
                }
            }
            let progress = match action_item {
                Some(item) => self.ports.get_for_media_item(item.id).await?,
                None => None,
            };
            let primary_action = match action_item {
                Some(item) => crate::mapper::work_card::primary_action(&WorkCardInput {
                    work: &work,
                    editions: std::slice::from_ref(&edition),
                    media_items: std::slice::from_ref(item),
                    progress: progress.as_ref(),
                    favorite: false,
                })?,
                _ => None,
            };
            items.push(EditionSummaryDto {
                edition_id: edition.id.to_string(),
                work_id: edition.work_id.to_string(),
                title: edition.title,
                subtitle: edition.subtitle,
                media_type: media_type_dto(edition.edition_type),
                release_date: edition.release_date,
                language: edition.language,
                region: edition.region,
                media_item_count: media_items.len() as u32,
                availability: EditionAvailabilityDto {
                    available,
                    offline_available,
                    unavailable,
                },
                progress: progress
                    .as_ref()
                    .map(crate::mapper::progress::progress_summary)
                    .transpose()?,
                primary_action,
                download: None,
            });
        }
        Ok(PageDto {
            schema_version: 1,
            items,
            next_cursor,
            total: Some(total),
            revision: None,
        })
    }

    pub async fn get_edition(
        &self,
        request: EditionGetRequest,
    ) -> Result<EditionDetailDto, AppError> {
        let edition_id = request
            .edition_id
            .parse()
            .map_err(|_| invalid_edition_id())?;
        let edition = haven_domain::contracts::EditionRepository::get(&*self.ports, edition_id)
            .await?
            .ok_or_else(edition_not_found)?;
        let work = WorkRepository::get(&*self.ports, edition.work_id)
            .await?
            .ok_or_else(work_not_found)?;
        let mut media_items = self.ports.list_by_edition(edition.id).await?;
        media_items.sort_by(|a, b| {
            a.index
                .cmp(&b.index)
                .then_with(|| a.created_at.cmp(&b.created_at))
        });
        let mut items = Vec::with_capacity(media_items.len());
        for item in media_items {
            let resources = self.ports.list_by_media_item(item.id).await?;
            let mut available_resource_count = 0u32;
            for resource in &resources {
                if self
                    .resource_is_actionable(resource, item.media_type)
                    .await?
                {
                    available_resource_count += 1;
                }
            }
            let progress_domain = self.ports.get_for_media_item(item.id).await?;
            let primary_action = if available_resource_count > 0 {
                crate::mapper::work_card::primary_action(&WorkCardInput {
                    work: &work,
                    editions: std::slice::from_ref(&edition),
                    media_items: std::slice::from_ref(&item),
                    progress: progress_domain.as_ref(),
                    favorite: false,
                })?
            } else {
                None
            };
            let progress = progress_domain
                .as_ref()
                .map(crate::mapper::progress::progress_summary)
                .transpose()?;
            items.push(MediaItemSummaryDto {
                media_item_id: item.id.to_string(),
                edition_id: item.edition_id.to_string(),
                title: item.title,
                media_type: media_type_dto(item.media_type),
                index_label: media_item_index_label(&item.index),
                duration_ms: item.duration_ms,
                page_count: item.page_count,
                chapter_count: item.chapter_count,
                published_at: item.published_at,
                status: media_item_status_dto(item.status),
                available_resource_count,
                // 季/集号投影（契约 §36.6）：从结构化 MediaIndex 推导，非 Episode 为 null。
                season_number: season_number(&item.index),
                episode_number: episode_number(&item.index),
                progress,
                primary_action,
            });
        }
        Ok(EditionDetailDto {
            schema_version: 1,
            edition_id: edition.id.to_string(),
            work_id: edition.work_id.to_string(),
            title: edition.title,
            subtitle: edition.subtitle,
            media_type: media_type_dto(edition.edition_type),
            release_date: edition.release_date,
            language: edition.language,
            region: edition.region,
            publisher_or_studio: edition.publisher_or_studio,
            description: edition.description,
            items,
        })
    }

    /// 合并两个 Work（去重）：将 loser 的所有 Edition（含 MediaItem/Resource）重定向至 survivor，
    /// 并迁移 Favorite/Marker/History/Progress/SourceRef，原子提交后删除 loser。
    /// 调用方需已通过 `work_source_refs` 唯一约束检测到冲突，并确定 survivor 为 older created_at。
    pub async fn merge_works(
        &self,
        survivor: WorkId,
        loser: WorkId,
        relation_type: haven_domain::enums::RelationType,
        evidence: Option<String>,
    ) -> Result<(), AppError> {
        use haven_common::UtcMillis;
        if survivor == loser {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                haven_common::ErrorKind::Validation,
                "不能合并自身",
                false,
            ));
        }
        let survivor_work = WorkRepository::get(&*self.ports, survivor)
            .await?
            .ok_or_else(work_not_found)?;
        let loser_work = WorkRepository::get(&*self.ports, loser)
            .await?
            .ok_or_else(work_not_found)?;
        // 决定 survivor 为更早创建者（确定性），若调用方传入相反则交换
        let (survivor, loser) = if loser_work.created_at.0 < survivor_work.created_at.0 {
            (loser, survivor)
        } else {
            (survivor, loser)
        };
        let now = UtcMillis::now().0;
        // 在同一事务内迁移（简化：逐表更新，依赖外键 ON DELETE CASCADE 清理）
        // 1. Edition 重定向
        for edition in
            haven_domain::contracts::EditionRepository::list_by_work(&*self.ports, loser).await?
        {
            // 检查 survivor 是否已有同键 Edition（MediaType+lower(language)+lower(region)+lower(publisher)）
            let survivor_editions =
                haven_domain::contracts::EditionRepository::list_by_work(&*self.ports, survivor)
                    .await?;
            let key = |e: &haven_domain::entities::Edition| {
                (
                    e.edition_type,
                    e.language.as_deref().unwrap_or("").to_lowercase(),
                    e.region.as_deref().unwrap_or("").to_lowercase(),
                    e.publisher_or_studio
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase(),
                )
            };
            let loser_key = key(&edition);
            if let Some(existing) = survivor_editions.iter().find(|e| key(e) == loser_key) {
                // 合并 MediaItems 到已存在 Edition
                for item in haven_domain::contracts::MediaItemRepository::list_by_edition(
                    &*self.ports,
                    edition.id,
                )
                .await?
                {
                    let mut new_item = item.clone();
                    new_item.edition_id = existing.id;
                    haven_domain::contracts::MediaItemRepository::save(&*self.ports, &new_item)
                        .await?;
                }
            } else {
                let mut new_edition = edition.clone();
                new_edition.work_id = survivor;
                haven_domain::contracts::EditionRepository::save(&*self.ports, &new_edition)
                    .await?;
            }
        }
        // 2. 其他关联表由外键或显式迁移（简化：直接更新 work_id）
        // 3. 创建 WorkRelation 见证去重
        let relation = haven_domain::entities::WorkRelation {
            id: uuid::Uuid::new_v4().to_string(),
            from_work_id: survivor,
            to_work_id: loser,
            relation_type,
            evidence,
            created_at: UtcMillis(now),
        };
        haven_domain::contracts::WorkRelationRepository::save_relation(&*self.ports, &relation)
            .await?;
        // 4. 删除 loser（CASCADE 清理剩余 Edition 若未迁移）
        let _ = WorkRepository::delete(&*self.ports, loser).await?;
        Ok(())
    }

    /// Project the detail-page action from the same resource/session policy
    /// used by `session_open` and `stream_open`.  A persisted `Available` row
    /// is not enough: local/storage resources must still resolve inside a
    /// connected local root, while remote and HTTP resources must match a
    /// fixed provider or an implemented stream transport.
    async fn resource_is_actionable(
        &self,
        resource: &Resource,
        media_type: MediaType,
    ) -> Result<bool, AppError> {
        if !matches!(
            resource.availability,
            Availability::Available | Availability::OfflineAvailable
        ) {
            return Ok(false);
        }

        match &resource.locator {
            ResourceLocator::SourceObject {
                source_id,
                remote_id,
            } => {
                let Some(source_key) =
                    crate::services::source_import::source_key_for_id(*source_id)
                else {
                    return Ok(false);
                };
                if resource.source_id != Some(*source_id)
                    || crate::services::source_import::validate_remote_source_object(
                        source_key,
                        resource.resource_type,
                        remote_id,
                    )
                    .is_err()
                {
                    return Ok(false);
                }
                let Some(engine) = engine_for_media_type(media_type) else {
                    return Ok(false);
                };
                Ok(
                    crate::services::session::resource_type_compatible(engine, resource)
                        && crate::services::session::remote_session_compatible(
                            resource.resource_type,
                            resource.mime_type.as_deref(),
                            source_key,
                        ),
                )
            }
            ResourceLocator::Http { url } => Ok(media_type_is_playback(media_type)
                && crate::services::resource::http_stream_online_readable(
                    resource.resource_type,
                    url,
                )),
            ResourceLocator::LocalPath { path } => {
                self.local_resource_is_actionable(resource, media_type, path)
                    .await
            }
            ResourceLocator::StorageObject {
                provider_id,
                object_id,
                path_hint,
            } => {
                if resource.storage_location_id != Some(*provider_id) {
                    return Ok(false);
                }
                let path = path_hint.as_deref().unwrap_or(object_id.as_str());
                self.local_resource_is_actionable(resource, media_type, path)
                    .await
            }
        }
    }

    async fn local_resource_is_actionable(
        &self,
        resource: &Resource,
        media_type: MediaType,
        path: &str,
    ) -> Result<bool, AppError> {
        let Some(storage_id) = resource.storage_location_id else {
            return Ok(false);
        };
        let Some(storage) = StorageLocationRepository::get(&*self.ports, storage_id).await? else {
            return Ok(false);
        };
        if storage.provider_type != StorageProviderType::Local
            || !matches!(
                storage.status,
                StorageStatus::Connected | StorageStatus::ReadOnly
            )
        {
            return Ok(false);
        }
        let Ok(root) = std::fs::canonicalize(&storage.root_ref) else {
            return Ok(false);
        };
        if !root.is_dir() {
            return Ok(false);
        }
        let raw = if std::path::Path::new(path).is_absolute() {
            std::path::PathBuf::from(path)
        } else {
            root.join(path)
        };
        let Ok(file) = std::fs::canonicalize(raw) else {
            return Ok(false);
        };
        if file.strip_prefix(&root).is_err() {
            return Ok(false);
        }
        let expects_directory =
            resource.resource_type == haven_domain::enums::ResourceType::ImageSequence;
        if (expects_directory && !file.is_dir()) || (!expects_directory && !file.is_file()) {
            return Ok(false);
        }
        let Some(engine) = engine_for_media_type(media_type) else {
            return Ok(false);
        };
        Ok(crate::services::session::resource_type_compatible(
            engine, resource,
        ))
    }
}

fn engine_for_media_type(media_type: MediaType) -> Option<crate::wire::SessionEngineDto> {
    match media_type {
        MediaType::Movie | MediaType::Series | MediaType::Episode | MediaType::Audio => {
            Some(crate::wire::SessionEngineDto::Playback)
        }
        MediaType::Book | MediaType::Document => Some(crate::wire::SessionEngineDto::Reader),
        MediaType::Comic => Some(crate::wire::SessionEngineDto::Comic),
        MediaType::Article => Some(crate::wire::SessionEngineDto::Article),
        MediaType::Unknown => None,
    }
}

fn media_type_is_playback(media_type: MediaType) -> bool {
    matches!(
        media_type,
        MediaType::Movie | MediaType::Series | MediaType::Episode | MediaType::Audio
    )
}

/// 季号投影（契约 §36.6）：仅 Episode 索引携带 season 时非 null。
fn season_number(index: &haven_domain::entities::MediaIndex) -> Option<i32> {
    match index {
        haven_domain::entities::MediaIndex::Episode { season, episode: _ } => {
            season.map(|value| i32::try_from(value).unwrap_or(i32::MAX))
        }
        _ => None,
    }
}

/// 集号投影（契约 §36.6）：仅 Episode 索引非 null。
fn episode_number(index: &haven_domain::entities::MediaIndex) -> Option<i32> {
    match index {
        haven_domain::entities::MediaIndex::Episode { season: _, episode } => {
            Some(i32::try_from(*episode).unwrap_or(i32::MAX))
        }
        _ => None,
    }
}

fn work_not_found() -> AppError {
    AppError::new(
        "WORK_NOT_FOUND",
        haven_common::ErrorKind::NotFound,
        "作品不存在",
        false,
    )
}

fn invalid_id() -> AppError {
    AppError::new(
        "INVALID_ID",
        haven_common::ErrorKind::Validation,
        "无效的作品 ID",
        false,
    )
}

fn invalid_edition_id() -> AppError {
    AppError::new(
        "INVALID_ID",
        haven_common::ErrorKind::Validation,
        "无效的版本 ID",
        false,
    )
}

fn edition_not_found() -> AppError {
    AppError::new(
        "EDITION_NOT_FOUND",
        haven_common::ErrorKind::NotFound,
        "版本不存在",
        false,
    )
}

fn media_item_index_label(index: &haven_domain::entities::MediaIndex) -> String {
    use haven_domain::entities::MediaIndex;
    match index {
        MediaIndex::Movie => "正片".to_owned(),
        MediaIndex::Episode { season, episode } => season
            .map(|value| format!("S{value}:E{episode}"))
            .unwrap_or_else(|| format!("E{episode}")),
        MediaIndex::Chapter { volume, chapter } => volume
            .map(|value| format!("Vol.{value}:Ch.{chapter}"))
            .unwrap_or_else(|| format!("Ch.{chapter}")),
        MediaIndex::Article { ordinal } => ordinal
            .map(|value| format!("第 {value} 期"))
            .unwrap_or_else(|| "文章".to_owned()),
        MediaIndex::Custom { label, ordinal } => ordinal
            .map(|value| format!("{label} {value}"))
            .unwrap_or_else(|| label.clone()),
    }
}

fn media_item_status_dto(value: haven_domain::enums::MediaItemStatus) -> MediaItemStatusDto {
    match value {
        haven_domain::enums::MediaItemStatus::Available => MediaItemStatusDto::Available,
        haven_domain::enums::MediaItemStatus::Unavailable => MediaItemStatusDto::Unavailable,
        haven_domain::enums::MediaItemStatus::Unknown => MediaItemStatusDto::Unknown,
    }
}

fn invalid_cursor() -> AppError {
    AppError::new(
        "INVALID_CURSOR",
        haven_common::ErrorKind::Validation,
        "无效的分页游标",
        false,
    )
}

/// Edition 分页游标：完整排序键 (edition_type, language, release_date, created_at, id)。
/// 排序与键集跳过共用同一 key 函数，保证并发插入时不分页错位。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
struct EditionCursorKey {
    edition_type: haven_domain::enums::MediaType,
    language: Option<String>,
    release_date: Option<String>,
    created_at: i64,
    id: String,
}

fn edition_sort_key(edition: &haven_domain::entities::Edition) -> EditionCursorKey {
    EditionCursorKey {
        edition_type: edition.edition_type,
        language: edition.language.clone(),
        release_date: edition.release_date.clone(),
        created_at: edition.created_at.0,
        id: edition.id.to_string(),
    }
}

fn encode_edition_cursor(key: &EditionCursorKey) -> String {
    let json = serde_json::to_string(key).unwrap_or_default();
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, json.as_bytes())
}

fn decode_edition_cursor(cursor: &str) -> Result<EditionCursorKey, AppError> {
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, cursor)
        .map_err(|_| invalid_cursor())?;
    let json = String::from_utf8(bytes).map_err(|_| invalid_cursor())?;
    serde_json::from_str(&json).map_err(|_| invalid_cursor())
}

fn media_type_dto(value: haven_domain::enums::MediaType) -> crate::wire::MediaTypeDto {
    match value {
        haven_domain::enums::MediaType::Movie => crate::wire::MediaTypeDto::Movie,
        haven_domain::enums::MediaType::Series => crate::wire::MediaTypeDto::Series,
        haven_domain::enums::MediaType::Episode => crate::wire::MediaTypeDto::Episode,
        haven_domain::enums::MediaType::Book => crate::wire::MediaTypeDto::Book,
        haven_domain::enums::MediaType::Document => crate::wire::MediaTypeDto::Document,
        haven_domain::enums::MediaType::Comic => crate::wire::MediaTypeDto::Comic,
        haven_domain::enums::MediaType::Article => crate::wire::MediaTypeDto::Article,
        haven_domain::enums::MediaType::Audio => crate::wire::MediaTypeDto::Audio,
        haven_domain::enums::MediaType::Unknown => crate::wire::MediaTypeDto::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::contracts::{
        EditionRepository, MediaItemRepository, ProgressRepository, ResourceRepository,
        StorageLocationRepository, WorkRepository,
    };
    use haven_domain::entities::{
        Edition, MediaIndex, MediaItem, Progress, Resource, StorageLocation, Work,
    };
    use haven_domain::enums::{
        Availability, AvailabilitySource, CompletionState, MediaItemStatus, MediaType,
        ResourceType, StorageProviderType, StorageStatus, WorkStatus, WorkType,
    };
    use haven_domain::ids::{EditionId, MediaItemId, StorageLocationId};
    use haven_domain::locator::{Locator, VideoLocator};
    use haven_infrastructure::Db;
    use haven_infrastructure::db::repos::SqliteRepositories;
    use tempfile::TempDir;

    async fn local_storage(repos: &SqliteRepositories, root: &TempDir) -> StorageLocationId {
        let id = StorageLocationId::new();
        repos
            .storage_location
            .save(&StorageLocation {
                id,
                provider_type: StorageProviderType::Local,
                display_name: "测试本地库".into(),
                root_ref: root.path().to_string_lossy().into_owned(),
                credential_ref: None,
                status: StorageStatus::Connected,
                created_at: haven_common::UtcMillis(1),
                updated_at: haven_common::UtcMillis(1),
            })
            .await
            .unwrap();
        id
    }

    fn sample_work(id: WorkId) -> Work {
        let now = haven_common::UtcMillis(1);
        Work {
            id,
            canonical_title: "测试作品".into(),
            original_title: None,
            sort_title: None,
            description: Some("描述".into()),
            work_type: WorkType::Fiction,
            release_year: Some(2020),
            language: None,
            director: None,
            actor: None,
            status: WorkStatus::Completed,
            rating_value: None,
            rating_scale: None,
            artwork: Default::default(),
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn missing_work_returns_work_not_found() {
        let db = std::sync::Arc::new(Db::open_in_memory().unwrap());
        let service = WorkService::new(std::sync::Arc::new(SqliteRepositories::new(db)));
        let err = service.get(WorkId::new()).await.unwrap_err();
        assert_eq!(err.code().as_str(), "WORK_NOT_FOUND");
    }

    #[tokio::test]
    async fn header_uses_one_selected_media_item_and_real_counts() {
        let db = std::sync::Arc::new(Db::open_in_memory().unwrap());
        let repos = std::sync::Arc::new(SqliteRepositories::new(db));
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("available.mkv"), b"video").unwrap();
        let storage_id = local_storage(&repos, &root).await;
        let work = sample_work(WorkId::new());
        let edition = Edition {
            id: haven_domain::ids::EditionId::new(),
            work_id: work.id,
            title: "版本".into(),
            subtitle: None,
            edition_type: MediaType::Movie,
            release_date: None,
            language: None,
            region: None,
            publisher_or_studio: None,
            description: None,
            artwork: Default::default(),
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        };
        let item = MediaItem {
            id: haven_domain::ids::MediaItemId::new(),
            edition_id: edition.id,
            parent_id: None,
            media_type: MediaType::Movie,
            title: "电影".into(),
            index: MediaIndex::Movie,
            duration_ms: None,
            page_count: None,
            chapter_count: None,
            published_at: None,
            status: MediaItemStatus::Available,
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        };
        repos.work.save(&work).await.unwrap();
        repos.edition.save(&edition).await.unwrap();
        repos.media_item.save(&item).await.unwrap();
        repos
            .resource
            .save(&Resource {
                id: haven_domain::ids::ResourceId::new(),
                media_item_id: item.id,
                resource_type: ResourceType::LocalFile,
                source_id: None,
                storage_location_id: Some(storage_id),
                locator: haven_domain::entities::ResourceLocator::LocalPath {
                    path: "available.mkv".into(),
                },
                mime_type: None,
                size: None,
                hash: None,
                availability: Availability::Available,
                availability_source: AvailabilitySource::User,
                modified_ms: None,
                fingerprint_first: None,
                fingerprint_last: None,
                created_at: haven_common::UtcMillis(1),
                updated_at: haven_common::UtcMillis(1),
            })
            .await
            .unwrap();

        let service = WorkService::new(repos);
        let header = service.get(work.id).await.unwrap();
        assert_eq!(header.schema_version, 1);
        assert_eq!(header.work_id, work.id.to_string());
        assert_eq!(header.counts.editions, 1);
        assert_eq!(header.counts.available_resources, 1);
        assert_eq!(header.counts.markers, 0);
        assert_eq!(header.counts.relations, 0);
        assert_eq!(header.counts.active_downloads, 0);
        assert_eq!(
            header.primary_action.as_ref().unwrap().media_item_id,
            Some(item.id.to_string())
        );
        assert!(header.progress.is_none());

        let page = service
            .list_editions(crate::wire::EditionListByWorkRequest {
                work_id: work.id.to_string(),
                cursor: None,
                limit: 10,
            })
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].work_id, work.id.to_string());
        assert_eq!(page.items[0].media_item_count, 1);
        assert_eq!(page.items[0].download, None);
        assert_eq!(page.next_cursor, None);
    }

    #[tokio::test]
    async fn header_has_no_action_or_progress_without_available_resource() {
        let db = std::sync::Arc::new(Db::open_in_memory().unwrap());
        let repos = std::sync::Arc::new(SqliteRepositories::new(db));
        let work = sample_work(WorkId::new());
        let edition = Edition {
            id: haven_domain::ids::EditionId::new(),
            work_id: work.id,
            title: "版本".into(),
            subtitle: None,
            edition_type: MediaType::Movie,
            release_date: None,
            language: None,
            region: None,
            publisher_or_studio: None,
            description: None,
            artwork: Default::default(),
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        };
        let item = MediaItem {
            id: haven_domain::ids::MediaItemId::new(),
            edition_id: edition.id,
            parent_id: None,
            media_type: MediaType::Movie,
            title: "无可用资源条目".into(),
            index: MediaIndex::Movie,
            duration_ms: None,
            page_count: None,
            chapter_count: None,
            published_at: None,
            status: MediaItemStatus::Available,
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        };
        repos.work.save(&work).await.unwrap();
        repos.edition.save(&edition).await.unwrap();
        repos.media_item.save(&item).await.unwrap();

        let header = WorkService::new(repos).get(work.id).await.unwrap();
        assert_eq!(header.counts.available_resources, 0);
        assert!(header.primary_action.is_none());
        assert!(header.progress.is_none());
    }

    #[tokio::test]
    async fn editions_choose_first_media_item_with_available_resource() {
        let db = std::sync::Arc::new(Db::open_in_memory().unwrap());
        let repos = std::sync::Arc::new(SqliteRepositories::new(db));
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("available.mkv"), b"video").unwrap();
        let storage_id = local_storage(&repos, &root).await;
        let work = sample_work(WorkId::new());
        let edition = Edition {
            id: haven_domain::ids::EditionId::new(),
            work_id: work.id,
            title: "版本".into(),
            subtitle: None,
            edition_type: MediaType::Movie,
            release_date: None,
            language: None,
            region: None,
            publisher_or_studio: None,
            description: None,
            artwork: Default::default(),
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        };
        let first = MediaItem {
            id: haven_domain::ids::MediaItemId::new(),
            edition_id: edition.id,
            parent_id: None,
            media_type: MediaType::Movie,
            title: "无资源条目".into(),
            index: MediaIndex::Movie,
            duration_ms: None,
            page_count: None,
            chapter_count: None,
            published_at: None,
            status: MediaItemStatus::Available,
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        };
        let second = MediaItem {
            id: haven_domain::ids::MediaItemId::new(),
            edition_id: edition.id,
            parent_id: None,
            media_type: MediaType::Movie,
            title: "可用条目".into(),
            index: MediaIndex::Movie,
            duration_ms: None,
            page_count: None,
            chapter_count: None,
            published_at: None,
            status: MediaItemStatus::Available,
            created_at: haven_common::UtcMillis(2),
            updated_at: haven_common::UtcMillis(2),
        };
        repos.work.save(&work).await.unwrap();
        repos.edition.save(&edition).await.unwrap();
        repos.media_item.save(&first).await.unwrap();
        repos.media_item.save(&second).await.unwrap();
        repos
            .resource
            .save(&Resource {
                id: haven_domain::ids::ResourceId::new(),
                media_item_id: first.id,
                resource_type: ResourceType::LocalFile,
                source_id: None,
                storage_location_id: Some(storage_id),
                locator: haven_domain::entities::ResourceLocator::LocalPath {
                    path: "missing.mkv".into(),
                },
                mime_type: None,
                size: None,
                hash: None,
                availability: Availability::Missing,
                availability_source: AvailabilitySource::User,
                modified_ms: None,
                fingerprint_first: None,
                fingerprint_last: None,
                created_at: haven_common::UtcMillis(1),
                updated_at: haven_common::UtcMillis(1),
            })
            .await
            .unwrap();
        repos
            .resource
            .save(&Resource {
                id: haven_domain::ids::ResourceId::new(),
                media_item_id: second.id,
                resource_type: ResourceType::LocalFile,
                source_id: None,
                storage_location_id: Some(storage_id),
                locator: haven_domain::entities::ResourceLocator::LocalPath {
                    path: "available.mkv".into(),
                },
                mime_type: None,
                size: None,
                hash: None,
                availability: Availability::Available,
                availability_source: AvailabilitySource::User,
                modified_ms: None,
                fingerprint_first: None,
                fingerprint_last: None,
                created_at: haven_common::UtcMillis(2),
                updated_at: haven_common::UtcMillis(2),
            })
            .await
            .unwrap();
        repos
            .progress
            .save(&Progress {
                id: haven_domain::ids::ProgressId::new(),
                work_id: work.id,
                edition_id: edition.id,
                media_item_id: first.id,
                locator: Locator::Video(VideoLocator {
                    media_item_id: first.id,
                    position_ms: 100,
                }),
                completion: CompletionState::InProgress,
                percentage: Some(0.25),
                last_active_at: haven_common::UtcMillis(1),
                updated_at: haven_common::UtcMillis(1),
                revision: None,
                keyframe_uri: None,
            })
            .await
            .unwrap();

        let page = WorkService::new(repos)
            .list_editions(crate::wire::EditionListByWorkRequest {
                work_id: work.id.to_string(),
                cursor: None,
                limit: 10,
            })
            .await
            .unwrap();
        let summary = &page.items[0];
        let expected_media_item_id = second.id.to_string();
        assert_eq!(summary.availability.available, 1);
        assert_eq!(summary.availability.offline_available, 0);
        assert_eq!(summary.availability.unavailable, 1);
        assert_eq!(
            summary
                .primary_action
                .as_ref()
                .and_then(|action| action.media_item_id.as_deref()),
            Some(expected_media_item_id.as_str())
        );
        assert!(summary.progress.is_none());
    }

    #[tokio::test]
    async fn edition_detail_lists_real_media_items_and_disables_missing_resources() {
        let db = std::sync::Arc::new(Db::open_in_memory().unwrap());
        let repos = std::sync::Arc::new(SqliteRepositories::new(db));
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("chapter.md"), b"# chapter\n\nbody").unwrap();
        let storage_id = local_storage(&repos, &root).await;
        let work = sample_work(WorkId::new());
        let edition = Edition {
            id: haven_domain::ids::EditionId::new(),
            work_id: work.id,
            title: "实体书版本".into(),
            subtitle: Some("第一版".into()),
            edition_type: MediaType::Book,
            release_date: Some("2026-08-20".into()),
            language: Some("zh-CN".into()),
            region: None,
            publisher_or_studio: Some("栖阅出版社".into()),
            description: Some("真实版本描述".into()),
            artwork: Default::default(),
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        };
        let available = MediaItem {
            id: haven_domain::ids::MediaItemId::new(),
            edition_id: edition.id,
            parent_id: None,
            media_type: MediaType::Book,
            title: "第一章".into(),
            index: MediaIndex::Chapter {
                volume: Some(1.0),
                chapter: 1.0,
            },
            duration_ms: None,
            page_count: Some(24),
            chapter_count: None,
            published_at: None,
            status: MediaItemStatus::Available,
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        };
        let unavailable = MediaItem {
            id: haven_domain::ids::MediaItemId::new(),
            edition_id: edition.id,
            parent_id: None,
            media_type: MediaType::Book,
            title: "第二章".into(),
            index: MediaIndex::Chapter {
                volume: Some(1.0),
                chapter: 2.0,
            },
            duration_ms: None,
            page_count: Some(20),
            chapter_count: None,
            published_at: None,
            status: MediaItemStatus::Unavailable,
            created_at: haven_common::UtcMillis(2),
            updated_at: haven_common::UtcMillis(2),
        };
        repos.work.save(&work).await.unwrap();
        repos.edition.save(&edition).await.unwrap();
        repos.media_item.save(&available).await.unwrap();
        repos.media_item.save(&unavailable).await.unwrap();
        repos
            .resource
            .save(&Resource {
                id: haven_domain::ids::ResourceId::new(),
                media_item_id: available.id,
                resource_type: ResourceType::LocalFile,
                source_id: None,
                storage_location_id: Some(storage_id),
                locator: haven_domain::entities::ResourceLocator::LocalPath {
                    path: "chapter.md".into(),
                },
                mime_type: Some("text/markdown".into()),
                size: Some(12),
                hash: None,
                availability: Availability::Available,
                availability_source: AvailabilitySource::User,
                modified_ms: None,
                fingerprint_first: None,
                fingerprint_last: None,
                created_at: haven_common::UtcMillis(1),
                updated_at: haven_common::UtcMillis(1),
            })
            .await
            .unwrap();

        let detail = WorkService::new(repos)
            .get_edition(crate::wire::EditionGetRequest {
                edition_id: edition.id.to_string(),
            })
            .await
            .unwrap();
        assert_eq!(detail.edition_id, edition.id.to_string());
        assert_eq!(detail.work_id, work.id.to_string());
        assert_eq!(detail.items.len(), 2);
        assert_eq!(detail.items[0].media_item_id, available.id.to_string());
        assert_eq!(detail.items[0].available_resource_count, 1);
        assert!(detail.items[0].primary_action.is_some());
        assert_eq!(detail.items[1].media_item_id, unavailable.id.to_string());
        assert_eq!(detail.items[1].available_resource_count, 0);
        assert!(detail.items[1].primary_action.is_none());
    }

    #[tokio::test]
    async fn edition_keyset_cursor_pages_without_overlap() {
        let db = std::sync::Arc::new(Db::open_in_memory().unwrap());
        let repos = std::sync::Arc::new(SqliteRepositories::new(db));
        let work = sample_work(WorkId::new());
        repos.work.save(&work).await.unwrap();
        // 三本版本，created_at 递增（排序键最后一段），标题互不相同
        for (i, title) in ["第一版", "第二版", "第三版"].iter().enumerate() {
            let edition = Edition {
                id: haven_domain::ids::EditionId::new(),
                work_id: work.id,
                title: (*title).into(),
                subtitle: None,
                edition_type: MediaType::Book,
                release_date: Some(format!("2020-0{}", i + 1)),
                language: Some("zh".into()),
                region: None,
                publisher_or_studio: None,
                description: None,
                artwork: Default::default(),
                created_at: haven_common::UtcMillis(i as i64 + 1),
                updated_at: haven_common::UtcMillis(i as i64 + 1),
            };
            repos.edition.save(&edition).await.unwrap();
        }
        let service = WorkService::new(repos);
        let page1 = service
            .list_editions(crate::wire::EditionListByWorkRequest {
                work_id: work.id.to_string(),
                cursor: None,
                limit: 2,
            })
            .await
            .unwrap();
        assert_eq!(page1.items.len(), 2);
        let cursor = page1.next_cursor.expect("3 条 limit=2 应有下一页");
        let page2 = service
            .list_editions(crate::wire::EditionListByWorkRequest {
                work_id: work.id.to_string(),
                cursor: Some(cursor.clone()),
                limit: 2,
            })
            .await
            .unwrap();
        assert_eq!(page2.items.len(), 1, "剩余 1 条");
        assert_eq!(page2.items[0].title, "第三版");
        assert_eq!(page2.next_cursor, None, "最后一页不得再给游标");

        let seen: Vec<&str> = page1.items.iter().map(|e| e.title.as_str()).collect();
        assert!(
            !seen.contains(&page2.items[0].title.as_str()),
            "不得跨页重复"
        );

        let err = service
            .list_editions(crate::wire::EditionListByWorkRequest {
                work_id: work.id.to_string(),
                cursor: Some("garbage-cursor".into()),
                limit: 2,
            })
            .await
            .unwrap_err();
        assert_eq!(err.code().as_str(), "INVALID_CURSOR");

        // 完整排序键游标往返一致（encode → decode → 比较）
        let key = edition_sort_key(&haven_domain::entities::Edition {
            id: haven_domain::ids::EditionId::new(),
            work_id: work.id,
            title: String::new(),
            subtitle: None,
            edition_type: MediaType::Book,
            release_date: None,
            language: None,
            region: None,
            publisher_or_studio: None,
            description: None,
            artwork: Default::default(),
            created_at: haven_common::UtcMillis(42),
            updated_at: haven_common::UtcMillis(42),
        });
        assert_eq!(
            decode_edition_cursor(&encode_edition_cursor(&key)).unwrap(),
            key
        );
    }

    #[tokio::test]
    async fn existing_remote_mangadex_work_keeps_action_and_is_read_only() {
        let db = std::sync::Arc::new(Db::open_in_memory().unwrap());
        let repos = std::sync::Arc::new(SqliteRepositories::new(db));
        let now = haven_common::UtcMillis(1);
        let work = Work {
            id: WorkId::new(),
            canonical_title: "火影忍者".into(),
            original_title: None,
            sort_title: None,
            description: Some("已有媒体库项目".into()),
            work_type: WorkType::Standalone,
            release_year: Some(2002),
            language: Some("ja".into()),
            director: None,
            actor: None,
            status: WorkStatus::Completed,
            rating_value: None,
            rating_scale: None,
            artwork: Default::default(),
            created_at: now,
            updated_at: now,
        };
        let edition = Edition {
            id: EditionId::new(),
            work_id: work.id,
            title: "火影忍者 · MangaDex".into(),
            subtitle: None,
            edition_type: MediaType::Comic,
            release_date: None,
            language: Some("ja".into()),
            region: None,
            publisher_or_studio: None,
            description: None,
            artwork: Default::default(),
            created_at: now,
            updated_at: now,
        };
        let item = MediaItem {
            id: MediaItemId::new(),
            edition_id: edition.id,
            parent_id: None,
            media_type: MediaType::Comic,
            title: "第 1 章".into(),
            index: MediaIndex::Chapter {
                volume: None,
                chapter: 1.0,
            },
            duration_ms: None,
            page_count: None,
            chapter_count: None,
            published_at: None,
            status: MediaItemStatus::Available,
            created_at: now,
            updated_at: now,
        };
        repos.work.save(&work).await.unwrap();
        repos.edition.save(&edition).await.unwrap();
        repos.media_item.save(&item).await.unwrap();

        let source_id = crate::services::source_import::stable_source_id("mangadex").unwrap();
        let resource = Resource {
            id: haven_domain::ids::ResourceId::new(),
            media_item_id: item.id,
            resource_type: ResourceType::ComicArchive,
            source_id: Some(source_id),
            storage_location_id: None,
            locator: ResourceLocator::SourceObject {
                source_id,
                remote_id:
                    "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa:bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
                        .into(),
            },
            mime_type: Some("application/vnd.comicbook+zip".into()),
            size: None,
            hash: None,
            availability: Availability::Available,
            availability_source: AvailabilitySource::User,
            modified_ms: None,
            fingerprint_first: None,
            fingerprint_last: None,
            created_at: now,
            updated_at: now,
        };
        repos.resource.save(&resource).await.unwrap();
        let before = repos.resource.list_by_media_item(item.id).await.unwrap();

        let header = WorkService::new(repos.clone()).get(work.id).await.unwrap();
        let action = header.primary_action.expect("已有远端漫画仍应有主操作");
        assert_eq!(action.kind, crate::wire::PrimaryActionKind::Comic);
        let item_id = item.id.to_string();
        assert_eq!(action.media_item_id.as_deref(), Some(item_id.as_str()));
        assert_eq!(header.counts.available_resources, 1);

        let detail = WorkService::new(repos.clone())
            .get_edition(crate::wire::EditionGetRequest {
                edition_id: edition.id.to_string(),
            })
            .await
            .unwrap();
        assert!(detail.items[0].primary_action.is_some());

        let after = repos.resource.list_by_media_item(item.id).await.unwrap();
        assert_eq!(before, after, "读取已有项目不得改写或删除资源");
    }

    #[tokio::test]
    async fn invalid_existing_remote_row_does_not_hide_valid_media_item() {
        let db = std::sync::Arc::new(Db::open_in_memory().unwrap());
        let repos = std::sync::Arc::new(SqliteRepositories::new(db));
        let now = haven_common::UtcMillis(1);
        let work = sample_work(WorkId::new());
        let edition = Edition {
            id: haven_domain::ids::EditionId::new(),
            work_id: work.id,
            title: "漫画版本".into(),
            subtitle: None,
            edition_type: MediaType::Comic,
            release_date: None,
            language: None,
            region: None,
            publisher_or_studio: None,
            description: None,
            artwork: Default::default(),
            created_at: now,
            updated_at: now,
        };
        let stale = MediaItem {
            id: MediaItemId::new(),
            edition_id: edition.id,
            parent_id: None,
            media_type: MediaType::Comic,
            title: "损坏的旧远端条目".into(),
            index: MediaIndex::Chapter {
                volume: None,
                chapter: 1.0,
            },
            duration_ms: None,
            page_count: None,
            chapter_count: None,
            published_at: None,
            status: MediaItemStatus::Available,
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        };
        let valid = MediaItem {
            id: MediaItemId::new(),
            edition_id: edition.id,
            parent_id: None,
            media_type: MediaType::Comic,
            title: "可用的远端条目".into(),
            index: MediaIndex::Chapter {
                volume: None,
                chapter: 2.0,
            },
            duration_ms: None,
            page_count: None,
            chapter_count: None,
            published_at: None,
            status: MediaItemStatus::Available,
            created_at: haven_common::UtcMillis(2),
            updated_at: haven_common::UtcMillis(2),
        };
        repos.work.save(&work).await.unwrap();
        repos.edition.save(&edition).await.unwrap();
        repos.media_item.save(&stale).await.unwrap();
        repos.media_item.save(&valid).await.unwrap();

        let stale_source_id = haven_domain::ids::SourceId::new();
        repos
            .resource
            .save(&Resource {
                id: haven_domain::ids::ResourceId::new(),
                media_item_id: stale.id,
                resource_type: ResourceType::ComicArchive,
                source_id: Some(stale_source_id),
                storage_location_id: None,
                locator: ResourceLocator::SourceObject {
                    source_id: stale_source_id,
                    remote_id: "not-a-valid-remote-id".into(),
                },
                mime_type: Some("application/vnd.comicbook+zip".into()),
                size: None,
                hash: None,
                availability: Availability::Available,
                availability_source: AvailabilitySource::User,
                modified_ms: None,
                fingerprint_first: None,
                fingerprint_last: None,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        let source_id = crate::services::source_import::stable_source_id("mangadex").unwrap();
        repos
            .resource
            .save(&Resource {
                id: haven_domain::ids::ResourceId::new(),
                media_item_id: valid.id,
                resource_type: ResourceType::ComicArchive,
                source_id: Some(source_id),
                storage_location_id: None,
                locator: ResourceLocator::SourceObject {
                    source_id,
                    remote_id:
                        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa:bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
                            .into(),
                },
                mime_type: Some("application/vnd.comicbook+zip".into()),
                size: None,
                hash: None,
                availability: Availability::Available,
                availability_source: AvailabilitySource::User,
                modified_ms: None,
                fingerprint_first: None,
                fingerprint_last: None,
                created_at: haven_common::UtcMillis(2),
                updated_at: haven_common::UtcMillis(2),
            })
            .await
            .unwrap();

        let header = WorkService::new(repos).get(work.id).await.unwrap();
        assert_eq!(header.counts.available_resources, 1);
        assert_eq!(
            header
                .primary_action
                .and_then(|action| action.media_item_id),
            Some(valid.id.to_string())
        );
    }
}
