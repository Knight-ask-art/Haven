//! HomeService：`home_get`（契约 §14.1；G-0.1 首页 Continue / Recently Added 真实投影）。
//!
//! 规则：
//! - `continue_items`：`progress_recent` 联查 Work/Edition/MediaItem 组装 ContinueItemDto；
//!   每条携带 progress + primaryAction（labelHint=continue，与进度存在语义一致）。
//! - `recently_added`：`library_list`（sort=recently_added，category=all）首页投影。
//! - `shelves`：0.1 仅包含收藏 Work 的稳定 Shelf；无收藏时不返回空架子。
//! - 组装为批量查询（list_by_works/list_by_editions），与页大小无关（消除 N+1）。
//! - 0.1 不等远程 Source；首页不得为构造 Hero 响应而串行等待（契约 §14.1）。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use haven_common::AppError;
use haven_domain::contracts::{
    EditionRepository, FavoriteRepository, MediaItemRepository, ProgressRepository, WorkRepository,
};

use crate::mapper::progress::progress_summary;
use crate::mapper::work_card::{WorkCardInput, primary_action, work_card};
use crate::services::ports::LibraryPorts;
use crate::wire::{
    ContinueItemDto, HomeDto, LabelHint, LibraryListRequest, LibraryListSort, PrimaryActionDto,
    QueryCategory, ShelfDto, WorkCardDto,
};

/// 首页各分组上限（0.1 本地数据闭环；未来分页属 IPC-FE-002）。
const CONTINUE_LIMIT: u32 = 20;
const RECENTLY_ADDED_LIMIT: u32 = 20;
const FAVORITES_PREVIEW_LIMIT: u32 = 20;

#[derive(Clone)]
pub struct HomeService {
    ports: Arc<dyn LibraryPorts>,
}

impl HomeService {
    pub fn new(ports: Arc<dyn LibraryPorts>) -> Self {
        Self { ports }
    }

    /// `home_get`：聚合 Continue + RecentlyAdded + Shelves 为单次首页投影。
    pub async fn get(&self) -> Result<HomeDto, AppError> {
        let continue_items = self.build_continue_items().await?;
        let recently_added = self.build_recently_added().await?;
        let shelves = self.build_shelves().await?;
        Ok(HomeDto {
            schema_version: 1,
            continue_items,
            recently_added,
            shelves,
        })
    }

    /// Continue 分组：`progress_recent` 联查 Work/Edition/MediaItem 组装 ContinueItemDto。
    async fn build_continue_items(&self) -> Result<Vec<ContinueItemDto>, AppError> {
        let progress_items = ProgressRepository::recent(&*self.ports, CONTINUE_LIMIT).await?;
        if progress_items.is_empty() {
            return Ok(Vec::new());
        }
        // 进度已含 work_id/edition_id/media_item_id；据此批量取 Work/Edition/MediaItem。
        let work_ids: Vec<_> = progress_items.iter().map(|p| p.work_id).collect();
        let works: HashMap<_, _> = WorkRepository::list(&*self.ports, MAX_LIMIT, 0)
            .await?
            .into_iter()
            .map(|w| (w.id, w))
            .collect();
        let editions = EditionRepository::list_by_works(&*self.ports, &work_ids).await?;
        let editions_by_work: HashMap<_, Vec<_>> = {
            let mut map: HashMap<_, Vec<_>> = HashMap::new();
            for e in editions {
                map.entry(e.work_id).or_default().push(e);
            }
            map
        };
        let edition_ids: Vec<_> = editions_by_work.values().flatten().map(|e| e.id).collect();
        let media_items = MediaItemRepository::list_by_editions(&*self.ports, &edition_ids).await?;
        let media_by_edition: HashMap<_, Vec<_>> = {
            let mut map: HashMap<_, Vec<_>> = HashMap::new();
            for m in media_items {
                map.entry(m.edition_id).or_default().push(m);
            }
            map
        };

        let mut items = Vec::with_capacity(progress_items.len());
        for progress in &progress_items {
            let work = match works.get(&progress.work_id) {
                Some(w) => w,
                None => continue,
            };
            let editions = editions_by_work
                .get(&progress.work_id)
                .cloned()
                .unwrap_or_default();
            let media: Vec<_> = editions
                .iter()
                .flat_map(|e| media_by_edition.get(&e.id).cloned().unwrap_or_default())
                .collect();
            // primaryAction 取同一 Edition + 其下 MediaItem（进度所属 MediaItem 优先）。
            let action_item = media
                .iter()
                .find(|m| m.id == progress.media_item_id)
                .or_else(|| media.first());
            let action = match action_item {
                Some(item) => primary_action(&WorkCardInput {
                    work,
                    editions: &editions,
                    media_items: std::slice::from_ref(item),
                    progress: Some(progress),
                    favorite: false,
                })?,
                None => None,
            };
            let primary_action = action.unwrap_or_else(|| PrimaryActionDto {
                kind: crate::wire::PrimaryActionKind::OpenEdition,
                label_hint: LabelHint::Continue,
                edition_id: progress.edition_id.to_string(),
                media_item_id: Some(progress.media_item_id.to_string()),
                locator: None,
            });
            let progress_dto = progress_summary(progress)?;
            items.push(ContinueItemDto {
                work_id: progress.work_id.to_string(),
                media_item_id: progress.media_item_id.to_string(),
                progress: progress_dto,
                primary_action,
            });
        }
        Ok(items)
    }

    /// RecentlyAdded 分组：复用 `library_list`（sort=recently_added，首页投影）。
    async fn build_recently_added(&self) -> Result<Vec<WorkCardDto>, AppError> {
        let request = LibraryListRequest {
            category: QueryCategory::All,
            media_types: None,
            query: None,
            sort: LibraryListSort::RecentlyAdded,
            cursor: None,
            limit: RECENTLY_ADDED_LIMIT,
        };
        let limit = request.limit.clamp(1, MAX_LIMIT);
        let offset = 0u32;
        let works = self
            .ports
            .list_filtered(
                haven_domain::contracts::WorkOrder::RecentlyAdded,
                None,
                None,
                None,
                limit,
                offset,
            )
            .await?;
        self.build_cards(&works).await
    }

    /// 0.1 唯一首页 Shelf：按收藏时间顺序展示收藏的 Work；空收藏不返回空架子。
    async fn build_shelves(&self) -> Result<Vec<ShelfDto>, AppError> {
        use haven_domain::entities::FavoriteTarget;

        let favorite_work_ids: Vec<_> =
            FavoriteRepository::list(&*self.ports, FAVORITES_PREVIEW_LIMIT, 0)
                .await?
                .into_iter()
                .filter_map(|favorite| match favorite.target {
                    FavoriteTarget::Work(id) => Some(id),
                    FavoriteTarget::Edition(_) | FavoriteTarget::MediaItem(_) => None,
                })
                .collect();
        if favorite_work_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut remaining_ids: HashSet<_> = favorite_work_ids.iter().copied().collect();
        let mut works_by_id = HashMap::with_capacity(remaining_ids.len());
        let mut offset = 0;
        while !remaining_ids.is_empty() {
            let page = WorkRepository::list(&*self.ports, MAX_LIMIT, offset).await?;
            let page_len = page.len() as u32;
            for work in page {
                if remaining_ids.remove(&work.id) {
                    works_by_id.insert(work.id, work);
                }
            }
            if page_len < MAX_LIMIT {
                break;
            }
            let Some(next_offset) = offset.checked_add(page_len) else {
                break;
            };
            offset = next_offset;
        }
        let works: Vec<_> = favorite_work_ids
            .into_iter()
            .filter_map(|id| works_by_id.remove(&id))
            .collect();
        let preview = self.build_cards(&works).await?;
        if preview.is_empty() {
            return Ok(Vec::new());
        }

        Ok(vec![ShelfDto {
            shelf_id: "shelf-favorites".to_owned(),
            title_key: "shelf.favorites".to_owned(),
            preview,
            view_more: None,
        }])
    }

    /// 批量组装 WorkCard：每页固定 5 次查询（与 LibraryService 一致，消除 N+1）。
    async fn build_cards(
        &self,
        works: &[haven_domain::entities::Work],
    ) -> Result<Vec<WorkCardDto>, AppError> {
        use haven_domain::entities::FavoriteTarget;
        let work_ids: Vec<_> = works.iter().map(|w| w.id).collect();
        let editions = self.ports.list_by_works(&work_ids).await?;
        let mut editions_by_work: HashMap<_, Vec<_>> = HashMap::new();
        for e in editions {
            editions_by_work.entry(e.work_id).or_default().push(e);
        }
        let edition_ids: Vec<_> = editions_by_work.values().flatten().map(|e| e.id).collect();
        let media_items = self.ports.list_by_editions(&edition_ids).await?;
        let mut media_by_edition: HashMap<_, Vec<_>> = HashMap::new();
        for m in media_items {
            media_by_edition.entry(m.edition_id).or_default().push(m);
        }
        let first_media_ids: Vec<_> = editions_by_work
            .iter()
            .flat_map(|(_, es)| es.iter())
            .filter_map(|e| media_by_edition.get(&e.id)?.first().map(|m| m.id))
            .collect();
        let progress_map = self.ports.get_for_media_items(&first_media_ids).await?;
        let favorites = self
            .ports
            .is_favorite_many(
                &work_ids
                    .iter()
                    .copied()
                    .map(FavoriteTarget::Work)
                    .collect::<Vec<_>>(),
            )
            .await?;
        let mut items = Vec::with_capacity(works.len());
        for work in works {
            let work_editions = editions_by_work.get(&work.id).cloned().unwrap_or_default();
            let mut work_media = Vec::new();
            for edition in &work_editions {
                if let Some(ms) = media_by_edition.get(&edition.id) {
                    work_media.extend(ms.iter().cloned());
                }
            }
            let progress = work_media.first().and_then(|m| progress_map.get(&m.id));
            items.push(work_card(&WorkCardInput {
                work,
                editions: &work_editions,
                media_items: &work_media,
                progress,
                favorite: favorites.contains(&FavoriteTarget::Work(work.id)),
            })?);
        }
        Ok(items)
    }
}

/// 复用 LibraryService 的服务端上限（保持单一真源）。
use crate::services::library::MAX_LIMIT;

#[cfg(test)]
mod tests {
    use super::*;
    use haven_common::UtcMillis;
    use haven_domain::contracts::{
        EditionRepository, FavoriteRepository, MediaItemRepository, ProgressRepository,
        WorkRepository,
    };
    use haven_domain::entities::{
        Edition, Favorite, FavoriteTarget, MediaIndex, MediaItem, Progress, Work,
    };
    use haven_domain::enums::{CompletionState, MediaItemStatus, MediaType, WorkStatus, WorkType};
    use haven_domain::ids::{EditionId, MediaItemId, ProgressId, WorkId};
    use haven_domain::locator::{Locator, VideoLocator};

    struct MemPorts {
        works: Vec<Work>,
        editions: Vec<Edition>,
        items: Vec<MediaItem>,
        progress: Vec<Progress>,
        favorites: Vec<WorkId>,
    }

    impl MemPorts {
        fn new(works: Vec<Work>) -> Self {
            Self {
                works,
                editions: vec![],
                items: vec![],
                progress: vec![],
                favorites: vec![],
            }
        }
        fn with_edition(mut self, edition: Edition) -> Self {
            self.editions.push(edition);
            self
        }
        fn with_item(mut self, item: MediaItem) -> Self {
            self.items.push(item);
            self
        }
        fn with_progress(mut self, progress: Progress) -> Self {
            self.progress.push(progress);
            self
        }
        fn with_favorite(mut self, work_id: WorkId) -> Self {
            self.favorites.push(work_id);
            self
        }
    }

    #[async_trait::async_trait]
    impl WorkRepository for MemPorts {
        async fn get(&self, id: WorkId) -> Result<Option<Work>, AppError> {
            Ok(self.works.iter().find(|w| w.id == id).cloned())
        }
        async fn save(&self, _work: &Work) -> Result<(), AppError> {
            Ok(())
        }
        async fn list(&self, limit: u32, offset: u32) -> Result<Vec<Work>, AppError> {
            Ok(self
                .works
                .iter()
                .skip(offset as usize)
                .take(limit as usize)
                .cloned()
                .collect())
        }
        async fn list_sorted(
            &self,
            _order: haven_domain::contracts::WorkOrder,
            limit: u32,
            offset: u32,
        ) -> Result<Vec<Work>, AppError> {
            WorkRepository::list(self, limit, offset).await
        }
        async fn list_filtered(
            &self,
            _order: haven_domain::contracts::WorkOrder,
            _category: Option<haven_domain::enums::ContentCategory>,
            _media_types: Option<&[haven_domain::enums::MediaType]>,
            _query: Option<&str>,
            limit: u32,
            offset: u32,
        ) -> Result<Vec<Work>, AppError> {
            WorkRepository::list(self, limit, offset).await
        }
        async fn count_filtered(
            &self,
            _category: Option<haven_domain::enums::ContentCategory>,
            _media_types: Option<&[haven_domain::enums::MediaType]>,
            _query: Option<&str>,
        ) -> Result<u64, AppError> {
            Ok(self.works.len() as u64)
        }
        async fn id_for_source_ref(
            &self,
            _provider: &str,
            _external_id: &str,
        ) -> Result<Option<WorkId>, AppError> {
            Ok(None)
        }
        async fn save_source_ref(
            &self,
            _provider: &str,
            _external_id: &str,
            _work_id: WorkId,
        ) -> Result<(), AppError> {
            Ok(())
        }
        async fn has_any_source_ref(&self, _id: WorkId) -> Result<bool, AppError> {
            Ok(false)
        }
        async fn delete(&self, _id: WorkId) -> Result<bool, AppError> {
            Ok(false)
        }
    }

    #[async_trait::async_trait]
    impl EditionRepository for MemPorts {
        async fn get(&self, id: EditionId) -> Result<Option<Edition>, AppError> {
            Ok(self.editions.iter().find(|e| e.id == id).cloned())
        }
        async fn save(&self, _e: &Edition) -> Result<(), AppError> {
            Ok(())
        }
        async fn list_by_work(&self, work_id: WorkId) -> Result<Vec<Edition>, AppError> {
            Ok(self
                .editions
                .iter()
                .filter(|e| e.work_id == work_id)
                .cloned()
                .collect())
        }
        async fn delete(&self, _id: EditionId) -> Result<bool, AppError> {
            Ok(false)
        }
    }

    #[async_trait::async_trait]
    impl MediaItemRepository for MemPorts {
        async fn get(&self, id: MediaItemId) -> Result<Option<MediaItem>, AppError> {
            Ok(self.items.iter().find(|i| i.id == id).cloned())
        }
        async fn save(&self, _m: &MediaItem) -> Result<(), AppError> {
            Ok(())
        }
        async fn list_by_edition(&self, edition_id: EditionId) -> Result<Vec<MediaItem>, AppError> {
            Ok(self
                .items
                .iter()
                .filter(|i| i.edition_id == edition_id)
                .cloned()
                .collect())
        }
        async fn delete(&self, _id: MediaItemId) -> Result<bool, AppError> {
            Ok(false)
        }
    }

    #[async_trait::async_trait]
    impl ProgressRepository for MemPorts {
        async fn get_for_media_item(&self, id: MediaItemId) -> Result<Option<Progress>, AppError> {
            Ok(self
                .progress
                .iter()
                .find(|p| p.media_item_id == id)
                .cloned())
        }
        async fn save(&self, _p: &Progress) -> Result<(), AppError> {
            Ok(())
        }
        async fn save_if_revision(
            &self,
            _progress: &Progress,
            _expected_revision: Option<&str>,
        ) -> Result<Option<String>, AppError> {
            Ok(None)
        }
        async fn recent(&self, limit: u32) -> Result<Vec<Progress>, AppError> {
            Ok(self.progress.iter().take(limit as usize).cloned().collect())
        }
    }

    #[async_trait::async_trait]
    impl FavoriteRepository for MemPorts {
        async fn set(&self, _target: &FavoriteTarget) -> Result<(), AppError> {
            Ok(())
        }
        async fn unset(&self, _target: &FavoriteTarget) -> Result<bool, AppError> {
            Ok(false)
        }
        async fn is_favorite(&self, target: &FavoriteTarget) -> Result<bool, AppError> {
            Ok(match target {
                FavoriteTarget::Work(id) => self.favorites.contains(id),
                _ => false,
            })
        }
        async fn list(&self, limit: u32, offset: u32) -> Result<Vec<Favorite>, AppError> {
            Ok(self
                .favorites
                .iter()
                .skip(offset as usize)
                .take(limit as usize)
                .copied()
                .map(|id| Favorite {
                    target: FavoriteTarget::Work(id),
                    created_at: UtcMillis(1),
                })
                .collect())
        }
    }

    fn sample_work(id: WorkId, title: &str) -> Work {
        Work {
            id,
            canonical_title: title.into(),
            original_title: None,
            sort_title: Some(title.into()),
            description: None,
            work_type: WorkType::Fiction,
            release_year: Some(2024),
            language: None,
            director: None,
            actor: None,
            status: WorkStatus::Completed,
            rating_value: None,
            rating_scale: None,
            artwork: Default::default(),
            created_at: UtcMillis(1),
            updated_at: UtcMillis(1),
        }
    }

    fn sample_edition(work_id: WorkId) -> Edition {
        Edition {
            id: EditionId::new(),
            work_id,
            title: "版本".into(),
            subtitle: None,
            edition_type: MediaType::Movie,
            release_date: None,
            language: None,
            region: None,
            publisher_or_studio: None,
            description: None,
            artwork: Default::default(),
            created_at: UtcMillis(1),
            updated_at: UtcMillis(1),
        }
    }

    fn sample_item(edition_id: EditionId) -> MediaItem {
        MediaItem {
            id: MediaItemId::new(),
            edition_id,
            parent_id: None,
            media_type: MediaType::Movie,
            title: "电影".into(),
            index: MediaIndex::Movie,
            duration_ms: None,
            page_count: None,
            chapter_count: None,
            published_at: None,
            status: MediaItemStatus::Available,
            created_at: UtcMillis(1),
            updated_at: UtcMillis(1),
        }
    }

    fn sample_progress(
        work_id: WorkId,
        edition_id: EditionId,
        media_item_id: MediaItemId,
    ) -> Progress {
        Progress {
            id: ProgressId::new(),
            work_id,
            edition_id,
            media_item_id,
            locator: Locator::Video(VideoLocator {
                media_item_id,
                position_ms: 60_000,
            }),
            completion: CompletionState::InProgress,
            percentage: Some(0.5),
            last_active_at: UtcMillis(1_700_000_000_000),
            updated_at: UtcMillis(1_700_000_000_000),
            keyframe_uri: None,
        }
    }

    #[tokio::test]
    async fn home_get_empty_db_returns_empty_groups() {
        let ports = Arc::new(MemPorts::new(vec![]));
        let service = HomeService::new(ports);
        let home = service.get().await.unwrap();
        assert_eq!(home.schema_version, 1);
        assert!(home.continue_items.is_empty());
        assert!(home.recently_added.is_empty());
        assert!(home.shelves.is_empty());
    }

    #[tokio::test]
    async fn home_get_assembles_continue_and_recently_added() {
        let work = sample_work(WorkId::new(), "沙丘2");
        let edition = sample_edition(work.id);
        let item = sample_item(edition.id);
        let progress = sample_progress(work.id, edition.id, item.id);
        let ports = Arc::new(
            MemPorts::new(vec![work.clone()])
                .with_edition(edition)
                .with_item(item)
                .with_progress(progress),
        );
        let service = HomeService::new(ports);
        let home = service.get().await.unwrap();
        assert_eq!(home.continue_items.len(), 1);
        let continue_item = &home.continue_items[0];
        assert_eq!(continue_item.work_id, work.id.to_string());
        assert!(continue_item.progress.progress_ratio.is_some());
        assert_eq!(home.recently_added.len(), 1);
        assert_eq!(home.recently_added[0].work_id, work.id.to_string());
    }

    #[tokio::test]
    async fn home_get_returns_stable_favorites_shelf_with_work_cards() {
        let work = sample_work(WorkId::new(), "沙丘2");
        let edition = sample_edition(work.id);
        let item = sample_item(edition.id);
        let ports = Arc::new(
            MemPorts::new(vec![work.clone()])
                .with_edition(edition)
                .with_item(item)
                .with_favorite(work.id),
        );
        let service = HomeService::new(ports);

        let first = service.get().await.unwrap();
        let second = service.get().await.unwrap();

        assert_eq!(first.shelves.len(), 1);
        let shelf = &first.shelves[0];
        assert_eq!(shelf.shelf_id, "shelf-favorites");
        assert_eq!(shelf.title_key, "shelf.favorites");
        assert!(shelf.view_more.is_none());
        assert_eq!(shelf.preview.len(), 1);
        assert_eq!(shelf.preview[0].work_id, work.id.to_string());
        assert!(shelf.preview[0].favorite);
        assert_eq!(second.shelves[0].shelf_id, shelf.shelf_id);
    }

    #[tokio::test]
    async fn home_get_omits_favorites_shelf_without_favorites() {
        let work = sample_work(WorkId::new(), "未收藏作品");
        let edition = sample_edition(work.id);
        let item = sample_item(edition.id);
        let ports = Arc::new(
            MemPorts::new(vec![work])
                .with_edition(edition)
                .with_item(item),
        );
        let service = HomeService::new(ports);

        let home = service.get().await.unwrap();

        assert!(home.shelves.is_empty());
    }

    #[tokio::test]
    async fn home_get_finds_favorite_work_beyond_first_work_page() {
        let favorite_work = sample_work(WorkId::new(), "跨页收藏作品");
        let edition = sample_edition(favorite_work.id);
        let item = sample_item(edition.id);
        let mut works: Vec<_> = (0..MAX_LIMIT)
            .map(|index| sample_work(WorkId::new(), &format!("普通作品 {index}")))
            .collect();
        works.push(favorite_work.clone());
        let ports = Arc::new(
            MemPorts::new(works)
                .with_edition(edition)
                .with_item(item)
                .with_favorite(favorite_work.id),
        );
        let service = HomeService::new(ports);

        let home = service.get().await.unwrap();

        assert_eq!(home.shelves.len(), 1);
        assert_eq!(home.shelves[0].preview.len(), 1);
        assert_eq!(
            home.shelves[0].preview[0].work_id,
            favorite_work.id.to_string()
        );
    }
}
