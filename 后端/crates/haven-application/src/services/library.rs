//! LibraryService：`library_list`（SLICE-LIBRARY-001 后端）。
//!
//! 规则：
//! - limit 由后端钳制到 `MAX_LIMIT`（契约 §14.3：必须限制 limit）。
//! - WorkCardDto 组装：editions/media_items/progress/favorite 聚合后走 mapper。
//! - 组装为批量查询（list_by_works/list_by_editions/get_for_media_items/is_favorite_many），
//!   每页固定 5 次查询，与页大小无关（N+1 已清偿，2026-08-13）。
//! - progress 投影取"首个 media_item"（完整最近活跃语义为后续任务）。

use std::sync::Arc;

use haven_common::AppError;

use crate::mapper::work_card::{WorkCardInput, work_card};
use crate::services::ports::LibraryPorts;
use crate::wire::{LibraryListRequest, LibraryListSort, PageDto, WorkCardDto};

/// 服务端强制上限（契约要求限制 limit）。
pub const MAX_LIMIT: u32 = 200;

#[derive(Clone)]
pub struct LibraryService {
    ports: Arc<dyn LibraryPorts>,
}

impl LibraryService {
    pub fn new(ports: Arc<dyn LibraryPorts>) -> Self {
        Self { ports }
    }

    pub async fn list(
        &self,
        request: LibraryListRequest,
    ) -> Result<PageDto<WorkCardDto>, AppError> {
        let limit = request.limit.clamp(1, MAX_LIMIT);
        let has_query = request
            .query
            .as_deref()
            .map(|q| !q.trim().is_empty())
            .unwrap_or(false);
        let order = match request.sort {
            LibraryListSort::RecentlyAdded => haven_domain::contracts::WorkOrder::RecentlyAdded,
            LibraryListSort::Title => haven_domain::contracts::WorkOrder::Title,
            LibraryListSort::LastActive => haven_domain::contracts::WorkOrder::LastActive,
            LibraryListSort::ReleaseDate => haven_domain::contracts::WorkOrder::ReleaseDate,
            LibraryListSort::Rating => haven_domain::contracts::WorkOrder::Rating,
        };
        let category = domain_category(request.category);
        let media_types: Option<Vec<haven_domain::enums::MediaType>> = request
            .media_types
            .map(|types| types.into_iter().map(domain_media_type).collect());
        let query = request.query.filter(|q| !q.trim().is_empty());

        if has_query {
            // Relevance uses FTS keyset paging. Explicit user sorting must use
            // the same repository ordering for every page, so do not let the
            // FTS path silently override rating/year/recent sorting.
            if !matches!(request.sort, LibraryListSort::Title) {
                let offset = match request.cursor.as_deref() {
                    Some(cursor) => parse_offset(cursor)?,
                    None => 0,
                };
                let works = self
                    .ports
                    .list_filtered(
                        order,
                        category,
                        media_types.as_deref(),
                        query.as_deref(),
                        limit,
                        offset,
                    )
                    .await?;
                let total = self
                    .ports
                    .count_filtered(category, media_types.as_deref(), query.as_deref())
                    .await?;
                let items = self.build_cards(&works).await?;
                let next_cursor = if u64::from(offset + items.len() as u32) < total {
                    Some((offset + items.len() as u32).to_string())
                } else {
                    None
                };
                return Ok(PageDto {
                    schema_version: 1,
                    items,
                    next_cursor,
                    total: Some(total),
                    revision: None,
                });
            }
            return self
                .list_fts(
                    category,
                    media_types.as_deref(),
                    query.as_deref().unwrap_or_default(),
                    request.cursor.as_deref(),
                    limit,
                )
                .await;
        }

        let offset = match request.cursor.as_deref() {
            Some(cursor) => parse_offset(cursor)?,
            None => 0,
        };
        let works = self
            .ports
            .list_filtered(
                order,
                category,
                media_types.as_deref(),
                query.as_deref(),
                limit,
                offset,
            )
            .await?;
        let total = self
            .ports
            .count_filtered(category, media_types.as_deref(), query.as_deref())
            .await?;

        let items = self.build_cards(&works).await?;

        // 分页：有下一页时 next_cursor 携带下一个 offset（opaque string）。
        let next_cursor = if u64::from(offset + items.len() as u32) < total {
            Some((offset + items.len() as u32).to_string())
        } else {
            None
        };

        Ok(PageDto {
            schema_version: 1,
            items,
            next_cursor,
            total: Some(total),
            revision: None,
        })
    }

    /// FTS 检索：cursor 为 (rank,id) base64，键集分页（多取 1 条探测 has_more）。
    async fn list_fts(
        &self,
        category: Option<haven_domain::enums::ContentCategory>,
        media_types: Option<&[haven_domain::enums::MediaType]>,
        query: &str,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<PageDto<WorkCardDto>, AppError> {
        let (after_rank, after_id) = match cursor {
            Some(value) => {
                let fts = decode_fts_cursor(value).map_err(|_| invalid_cursor())?;
                let id = fts.id.parse().map_err(|_| invalid_cursor())?;
                (Some(fts.rank), Some(id))
            }
            None => (None, None),
        };
        let page = self
            .ports
            .list_filtered_fts(
                category,
                media_types,
                query,
                after_rank,
                after_id,
                limit + 1,
            )
            .await?;
        let has_more = page.len() as u32 > limit;
        let mut page = page;
        page.truncate(limit as usize);
        let next_cursor = if has_more {
            page.last()
                .map(|(rank, work)| encode_fts_cursor(*rank, &work.id.to_string()))
        } else {
            None
        };
        let works: Vec<_> = page.into_iter().map(|(_, work)| work).collect();
        let items = self.build_cards(&works).await?;
        let total = self
            .ports
            .count_filtered(category, media_types, Some(query))
            .await?;
        Ok(PageDto {
            schema_version: 1,
            items,
            next_cursor,
            total: Some(total),
            revision: None,
        })
    }

    /// 批量组装整页 WorkCard：每页固定 5 次查询（works / editions / media_items /
    /// progress / favorites），与页大小无关（消除 N+1，DEBT 已清偿）。
    async fn build_cards(
        &self,
        works: &[haven_domain::entities::Work],
    ) -> Result<Vec<WorkCardDto>, AppError> {
        use haven_domain::entities::FavoriteTarget;
        use std::collections::HashMap;

        let work_ids: Vec<_> = works.iter().map(|w| w.id).collect();

        let editions = self.ports.list_by_works(&work_ids).await?;
        let mut editions_by_work: HashMap<
            haven_domain::ids::WorkId,
            Vec<haven_domain::entities::Edition>,
        > = HashMap::new();
        for e in editions {
            editions_by_work.entry(e.work_id).or_default().push(e);
        }

        let edition_ids: Vec<_> = editions_by_work.values().flatten().map(|e| e.id).collect();
        let media_items = self.ports.list_by_editions(&edition_ids).await?;
        let mut media_by_edition: HashMap<
            haven_domain::ids::EditionId,
            Vec<haven_domain::entities::MediaItem>,
        > = HashMap::new();
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

    /// 单卡组装（Work Detail 等单对象场景；列表路径走 build_cards 批量版本）。
    #[allow(dead_code)]
    async fn build_card(
        &self,
        work: &haven_domain::entities::Work,
    ) -> Result<WorkCardDto, AppError> {
        let editions = self.ports.list_by_work(work.id).await?;
        let mut media_items = Vec::new();
        for edition in &editions {
            media_items.extend(self.ports.list_by_edition(edition.id).await?);
        }
        let progress = match media_items.first() {
            Some(item) => self.ports.get_for_media_item(item.id).await?,
            None => None,
        };
        let favorite = self
            .ports
            .is_favorite(&haven_domain::entities::FavoriteTarget::Work(work.id))
            .await?;

        work_card(&WorkCardInput {
            work,
            editions: &editions,
            media_items: &media_items,
            progress: progress.as_ref(),
            favorite,
        })
    }
}

/// cursor 为 opaque string；第一版解析为数字 offset（契约要求前端不得解析，
/// 服务端实现细节可演进；无效 cursor → 验证错误）。
fn parse_offset(cursor: &str) -> Result<u32, AppError> {
    cursor.parse::<u32>().map_err(|_| invalid_cursor())
}

fn invalid_cursor() -> AppError {
    AppError::new(
        "INVALID_CURSOR",
        haven_common::ErrorKind::Validation,
        "无效的分页游标",
        false,
    )
}

#[derive(serde::Serialize, serde::Deserialize)]
struct FtsCursor {
    rank: f64,
    id: String,
}

fn encode_fts_cursor(rank: f64, id: &str) -> String {
    let cursor = FtsCursor {
        rank,
        id: id.to_string(),
    };
    let json = serde_json::to_string(&cursor).unwrap_or_default();
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, json.as_bytes())
}

fn decode_fts_cursor(cursor: &str) -> Result<FtsCursor, AppError> {
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, cursor)
        .map_err(|_| invalid_cursor())?;
    let json = String::from_utf8(bytes).map_err(|_| invalid_cursor())?;
    serde_json::from_str(&json).map_err(|_| invalid_cursor())
}

/// wire QueryCategory → domain ContentCategory（`All` 是查询 sentinel → None 不过滤）。
fn domain_category(
    category: crate::wire::QueryCategory,
) -> Option<haven_domain::enums::ContentCategory> {
    match category {
        crate::wire::QueryCategory::All => None,
        crate::wire::QueryCategory::Video => Some(haven_domain::enums::ContentCategory::Video),
        crate::wire::QueryCategory::Book => Some(haven_domain::enums::ContentCategory::Book),
        crate::wire::QueryCategory::Comic => Some(haven_domain::enums::ContentCategory::Comic),
        crate::wire::QueryCategory::Periodical => {
            Some(haven_domain::enums::ContentCategory::Periodical)
        }
    }
}

fn domain_media_type(media_type: crate::wire::MediaTypeDto) -> haven_domain::enums::MediaType {
    match media_type {
        crate::wire::MediaTypeDto::Movie => haven_domain::enums::MediaType::Movie,
        crate::wire::MediaTypeDto::Series => haven_domain::enums::MediaType::Series,
        crate::wire::MediaTypeDto::Episode => haven_domain::enums::MediaType::Episode,
        crate::wire::MediaTypeDto::Book => haven_domain::enums::MediaType::Book,
        crate::wire::MediaTypeDto::Document => haven_domain::enums::MediaType::Document,
        crate::wire::MediaTypeDto::Comic => haven_domain::enums::MediaType::Comic,
        crate::wire::MediaTypeDto::Article => haven_domain::enums::MediaType::Article,
        crate::wire::MediaTypeDto::Audio => haven_domain::enums::MediaType::Audio,
        crate::wire::MediaTypeDto::Unknown => haven_domain::enums::MediaType::Unknown,
    }
}

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
    use haven_domain::enums::{MediaItemStatus, MediaType, WorkStatus, WorkType};
    use haven_domain::ids::{EditionId, MediaItemId, WorkId};

    /// 内存端口（测试替身）：满足 LibraryPorts。
    struct MemPorts {
        works: Vec<Work>,
        editions: Vec<Edition>,
        items: Vec<MediaItem>,
        progress: Vec<Progress>,
        favorites: Vec<WorkId>,
        /// 仅测试：若 Some，`list_filtered` 收到 limit 时必须精确等于该值（直接证明
        /// service 入参被钳制后传给 repo 的值）。
        expected_limit: Option<u32>,
    }

    impl MemPorts {
        fn new(works: Vec<Work>) -> Self {
            Self {
                works,
                editions: vec![],
                items: vec![],
                progress: vec![],
                favorites: vec![],
                expected_limit: None,
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
        fn expect_limit(mut self, limit: u32) -> Self {
            self.expected_limit = Some(limit);
            self
        }
    }

    #[async_trait::async_trait]
    impl WorkRepository for MemPorts {
        async fn get(&self, _id: WorkId) -> Result<Option<Work>, AppError> {
            Ok(self.works.first().cloned())
        }
        async fn save(&self, _work: &Work) -> Result<(), AppError> {
            Ok(())
        }
        async fn list(&self, limit: u32, offset: u32) -> Result<Vec<Work>, AppError> {
            self.list_sorted(haven_domain::contracts::WorkOrder::Title, limit, offset)
                .await
        }
        async fn list_sorted(
            &self,
            _order: haven_domain::contracts::WorkOrder,
            limit: u32,
            offset: u32,
        ) -> Result<Vec<Work>, AppError> {
            Ok(self
                .works
                .iter()
                .skip(offset as usize)
                .take(limit as usize)
                .cloned()
                .collect())
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
            if let Some(expected) = self.expected_limit {
                assert_eq!(
                    limit, expected,
                    "service 传入 repo 的 limit 必须精确等于钳制后的 expected"
                );
            }
            self.list_sorted(_order, limit, offset).await
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
        async fn get(&self, _id: EditionId) -> Result<Option<Edition>, AppError> {
            Ok(self.editions.first().cloned())
        }
        async fn save(&self, _e: &Edition) -> Result<(), AppError> {
            Ok(())
        }
        async fn list_by_work(&self, _work_id: WorkId) -> Result<Vec<Edition>, AppError> {
            Ok(self.editions.clone())
        }
        async fn delete(&self, _id: EditionId) -> Result<bool, AppError> {
            Ok(false)
        }
    }

    #[async_trait::async_trait]
    impl MediaItemRepository for MemPorts {
        async fn get(&self, _id: MediaItemId) -> Result<Option<MediaItem>, AppError> {
            Ok(self.items.first().cloned())
        }
        async fn save(&self, _m: &MediaItem) -> Result<(), AppError> {
            Ok(())
        }
        async fn list_by_edition(
            &self,
            _edition_id: EditionId,
        ) -> Result<Vec<MediaItem>, AppError> {
            Ok(self.items.clone())
        }
        async fn delete(&self, _id: MediaItemId) -> Result<bool, AppError> {
            Ok(false)
        }
    }

    #[async_trait::async_trait]
    impl ProgressRepository for MemPorts {
        async fn get_for_media_item(
            &self,
            media_item_id: MediaItemId,
        ) -> Result<Option<Progress>, AppError> {
            Ok(self
                .progress
                .iter()
                .find(|p| p.media_item_id == media_item_id)
                .cloned())
        }
        async fn save(&self, _p: &Progress) -> Result<(), AppError> {
            Ok(())
        }
        async fn save_if_revision(
            &self,
            progress: &Progress,
            _expected_revision: Option<&str>,
        ) -> Result<Option<String>, AppError> {
            Ok(progress.revision.clone())
        }
        async fn recent(&self, _limit: u32) -> Result<Vec<Progress>, AppError> {
            Ok(self.progress.clone())
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
        async fn list(&self, _limit: u32, _offset: u32) -> Result<Vec<Favorite>, AppError> {
            Ok(vec![])
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
            release_year: Some(2008),
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

    #[tokio::test]
    async fn list_assembles_full_cards() {
        let work = sample_work(WorkId::new(), "三体");
        let edition = sample_edition(work.id);
        let item = sample_item(edition.id);
        let ports = Arc::new(
            MemPorts::new(vec![work.clone()])
                .with_edition(edition)
                .with_item(item),
        );
        let service = LibraryService::new(ports);

        let page = service
            .list(LibraryListRequest {
                category: crate::wire::QueryCategory::All,
                media_types: None,
                query: None,
                sort: LibraryListSort::RecentlyAdded,
                cursor: None,
                limit: 50,
            })
            .await
            .unwrap();

        assert_eq!(page.items.len(), 1);
        let card = &page.items[0];
        assert_eq!(card.title, "三体");
        assert!(
            card.available_media_types
                .contains(&crate::wire::MediaTypeDto::Movie)
        );
        assert!(
            card.categories
                .contains(&crate::wire::ContentCategory::Video)
        );
        let action = card.primary_action.as_ref().unwrap();
        assert_eq!(action.kind, crate::wire::PrimaryActionKind::Playback);
        assert_eq!(action.label_hint, crate::wire::LabelHint::Start);
    }

    #[tokio::test]
    async fn limit_is_clamped_by_server() {
        // 空库 + 请求 u32::MAX：service 必须传给 repo 精确的 MAX_LIMIT（直接断言入参）。
        let ports = Arc::new(MemPorts::new(vec![]).expect_limit(MAX_LIMIT));
        let service = LibraryService::new(ports);
        let page = service
            .list(LibraryListRequest {
                category: crate::wire::QueryCategory::All,
                media_types: None,
                query: None,
                sort: LibraryListSort::Title,
                cursor: None,
                limit: u32::MAX,
            })
            .await
            .unwrap();
        assert!(page.items.is_empty(), "空库无条目");
    }

    #[tokio::test]
    async fn limit_zero_is_clamped_to_one() {
        // 单项库 + 请求 limit=0：service 传给 repo 的 limit 必须是 1（clamp 下界），
        // 返回恰好 1 条且分页不退化（单项库 next_cursor None）。
        let work = sample_work(haven_domain::ids::WorkId::new(), "测试");
        let edition = sample_edition(work.id);
        let ports = Arc::new(
            MemPorts::new(vec![work])
                .with_edition(edition)
                .expect_limit(1),
        );
        let service = LibraryService::new(ports);
        let page = service
            .list(LibraryListRequest {
                category: crate::wire::QueryCategory::All,
                media_types: None,
                query: None,
                sort: LibraryListSort::Title,
                cursor: None,
                limit: 0,
            })
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1, "limit=0 钳制为 1 后返回 1 条");
        assert_eq!(page.total, Some(1));
        assert!(page.next_cursor.is_none(), "单项库分页不退化");
    }

    #[tokio::test]
    async fn invalid_cursor_errors() {
        let ports = Arc::new(MemPorts::new(vec![]));
        let service = LibraryService::new(ports);
        let err = service
            .list(LibraryListRequest {
                category: crate::wire::QueryCategory::All,
                media_types: None,
                query: None,
                sort: LibraryListSort::Title,
                cursor: Some("not-a-number".into()),
                limit: 10,
            })
            .await
            .unwrap_err();
        assert_eq!(err.code().as_str(), "INVALID_CURSOR");
    }

    #[test]
    fn fts_cursor_roundtrips() {
        let encoded = encode_fts_cursor(-3.5, "work-1");
        let decoded = decode_fts_cursor(&encoded).unwrap();
        assert_eq!(decoded.rank, -3.5);
        assert_eq!(decoded.id, "work-1");
        // 负零也必须往返一致（SQLite bm25 并列时常出现 -0.0）
        let zero = encode_fts_cursor(-0.0, "work-2");
        assert_eq!(decode_fts_cursor(&zero).unwrap().rank, -0.0);
    }

    #[tokio::test]
    async fn fts_query_accepts_cursor_and_rejects_garbage() {
        let works: Vec<Work> = (0..3)
            .map(|i| sample_work(WorkId::new(), &format!("三体{i}")))
            .collect();
        let service = LibraryService::new(Arc::new(MemPorts::new(works)));
        let request = |cursor: Option<String>| LibraryListRequest {
            category: crate::wire::QueryCategory::All,
            media_types: None,
            query: Some("三体".into()),
            sort: LibraryListSort::Title,
            cursor,
            limit: 2,
        };
        let page = service.list(request(None)).await.unwrap();
        assert_eq!(page.items.len(), 2);
        let next = page.next_cursor.expect("3 条 limit=2 应有下一页");
        // 合法游标（FTS 键集编码）不得报错；无效游标必须稳定 INVALID_CURSOR
        let page2 = service.list(request(Some(next))).await.unwrap();
        assert_eq!(page2.items.len(), 2);
        let err = service
            .list(request(Some("not-a-valid-fts-cursor".into())))
            .await
            .unwrap_err();
        assert_eq!(err.code().as_str(), "INVALID_CURSOR");
    }

    #[tokio::test]
    async fn favorite_projection_reflected() {
        let work = sample_work(WorkId::new(), "收藏作");
        let mut ports = MemPorts::new(vec![work.clone()]);
        ports.favorites.push(work.id);
        let service = LibraryService::new(Arc::new(ports));
        let page = service
            .list(LibraryListRequest {
                category: crate::wire::QueryCategory::All,
                media_types: None,
                query: None,
                sort: LibraryListSort::Title,
                cursor: None,
                limit: 10,
            })
            .await
            .unwrap();
        assert!(page.items[0].favorite);
    }
}
