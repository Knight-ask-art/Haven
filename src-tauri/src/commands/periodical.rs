//! 报刊层级只读查询命令（`periodical_tree_get`）。
//!
//! Command 边界（ADR-002 / IPC-TAURI-001A）：
//! - 只做不可信输入校验（`workId` 必须是 UUID）→ 调用 [`PeriodicalQueryService`]
//!   → 把 `AppError` 映射为 `ErrorDto`；
//! - **不写 SQL**、不触碰 DB Row、不访问 Provider（查询路径无网络）；
//! - 层级与顺序完全由后端组装，前端只渲染。

use tauri::State;

use haven_application::wire::{ErrorDto, PeriodicalTreeDto, PeriodicalTreeGetRequest};

use crate::ipc::{invalid_id, run_blocking, to_error_dto};
use crate::state::AppState;

/// 命令核心逻辑（可直接测试，不依赖 Tauri runtime）。
pub async fn run_periodical_tree_get(
    state: &AppState,
    request: PeriodicalTreeGetRequest,
) -> Result<PeriodicalTreeDto, ErrorDto> {
    let work_id = request.work_id.parse().map_err(|_| invalid_id())?;
    state
        .periodical
        .periodical_tree(work_id)
        .await
        .map_err(|error| to_error_dto(&error))
}

#[tauri::command]
pub async fn periodical_tree_get(
    state: State<'_, AppState>,
    request: PeriodicalTreeGetRequest,
) -> Result<PeriodicalTreeDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_periodical_tree_get(&state, request).await }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use haven_domain::entities::{MediaIndex, MediaItem, Work};
    use haven_domain::enums::{MediaItemStatus, MediaType, WorkStatus, WorkType};
    use haven_domain::ids::{MediaItemId, PeriodicalId, WorkId};
    use haven_domain::periodical::{
        Issn, PageRange, Periodical, PeriodicalArticle, PeriodicalArticleAvailability,
        PeriodicalArticleSourceIdentity, PeriodicalIssue, PeriodicalVolume,
    };

    fn app_state() -> AppState {
        AppState::new(Arc::new(
            haven_infrastructure::Db::open_in_memory().unwrap(),
        ))
    }

    /// 通过共享 repos 种子一条 work → edition → media_item 链（MediaItem 是文章的
    /// 消费入口，ForeignKey 要求它先存在），返回该链的 Work 与 MediaItem。
    async fn seed_media_item(
        repos: &haven_infrastructure::db::repos::SqliteRepositories,
    ) -> (WorkId, MediaItemId) {
        use haven_domain::contracts::{EditionRepository, MediaItemRepository, WorkRepository};
        let work_id = WorkId::new();
        let edition_id = haven_domain::ids::EditionId::new();
        let media_item_id = MediaItemId::new();
        let now = haven_common::UtcMillis::now();
        repos
            .work
            .save(&Work {
                id: work_id,
                canonical_title: "报刊作品".into(),
                original_title: None,
                sort_title: None,
                description: None,
                work_type: WorkType::Standalone,
                release_year: None,
                language: None,
                director: None,
                actor: None,
                status: WorkStatus::Unknown,
                rating_value: None,
                rating_scale: None,
                artwork: Default::default(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .edition
            .save(&haven_domain::entities::Edition {
                id: edition_id,
                work_id,
                title: "报刊版本".into(),
                subtitle: None,
                edition_type: MediaType::Article,
                release_date: None,
                language: None,
                region: None,
                publisher_or_studio: None,
                description: None,
                artwork: Default::default(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .media_item
            .save(&MediaItem {
                id: media_item_id,
                edition_id,
                parent_id: None,
                media_type: MediaType::Article,
                title: "文章".into(),
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
            })
            .await
            .unwrap();
        (work_id, media_item_id)
    }

    /// 种子一个期刊 → 卷 → 期 → 文章层级，返回期刊所属的 Work。
    async fn seed_periodical_tree(
        repos: &haven_infrastructure::db::repos::SqliteRepositories,
    ) -> WorkId {
        use haven_domain::contracts::PeriodicalRepository;
        let (work_id, media_item_id) = seed_media_item(repos).await;
        let now = haven_common::UtcMillis::now();
        let periodical = Periodical {
            id: PeriodicalId::new(),
            work_id,
            title: "Nature Communications".into(),
            issn_print: None,
            issn_electronic: Some(Issn::parse("2041-1723").unwrap()),
            publisher: Some("Nature Portfolio".into()),
            created_at: now,
            updated_at: now,
        };
        repos.periodical.save(&periodical).await.unwrap();
        let volume = PeriodicalVolume {
            id: haven_domain::ids::PeriodicalVolumeId::new(),
            periodical_id: periodical.id,
            label: None,
            number: Some(12.0),
            year: Some(2024),
            ordinal: 1,
            created_at: now,
            updated_at: now,
        };
        repos.periodical.save_volume(&volume).await.unwrap();
        let issue = PeriodicalIssue {
            id: haven_domain::ids::PeriodicalIssueId::new(),
            volume_id: volume.id,
            label: Some("3-4".into()),
            number: None,
            publication_date: Some("2024-03".into()),
            ordinal: 1,
            created_at: now,
            updated_at: now,
        };
        repos.periodical.save_issue(&issue).await.unwrap();
        repos
            .periodical
            .save_article(&PeriodicalArticle {
                id: haven_domain::ids::PeriodicalArticleId::new(),
                issue_id: issue.id,
                media_item_id,
                ordinal: Some(1),
                title: "一篇期刊文章".into(),
                doi: None,
                page_range: PageRange::new("e12345", None),
                source: PeriodicalArticleSourceIdentity::new("europepmc", "PMC1").unwrap(),
                provider_content_availability: PeriodicalArticleAvailability::MetadataOnly,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        work_id
    }

    #[tokio::test]
    async fn invalid_work_id_returns_invalid_id() {
        let state = app_state();
        let error = run_periodical_tree_get(
            &state,
            PeriodicalTreeGetRequest {
                work_id: "not-a-uuid".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "INVALID_ID");
        assert!(!error.retryable);
    }

    /// 空树错误映射：Work 没有期刊归属时返回稳定的 `PERIODICAL_NOT_FOUND`，
    /// 而不是伪造一棵空树或按标题猜测期刊。
    #[tokio::test]
    async fn work_without_periodical_maps_to_periodical_not_found() {
        let state = app_state();
        let error = run_periodical_tree_get(
            &state,
            PeriodicalTreeGetRequest {
                work_id: WorkId::new().to_string(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "PERIODICAL_NOT_FOUND");
        assert!(!error.retryable);
    }

    #[tokio::test]
    async fn seeded_periodical_tree_round_trips_through_the_command() {
        let state = app_state();
        let work_id = seed_periodical_tree(&state.repos).await;

        let tree = run_periodical_tree_get(
            &state,
            PeriodicalTreeGetRequest {
                work_id: work_id.to_string(),
            },
        )
        .await
        .expect("已入库的期刊层级必须可读");

        assert_eq!(tree.schema_version, 1);
        assert_eq!(tree.work_id, work_id.to_string());
        assert_eq!(
            tree.periodical.issn_electronic.as_deref(),
            Some("2041-1723")
        );
        assert_eq!(tree.volumes.len(), 1);
        assert_eq!(tree.volumes[0].issues.len(), 1);
        assert_eq!(tree.volumes[0].issues[0].label.as_deref(), Some("3-4"));
        let articles = &tree.volumes[0].issues[0].articles;
        assert_eq!(articles.len(), 1);
        assert_eq!(articles[0].title, "一篇期刊文章");
        assert_eq!(articles[0].source_key, "europepmc");
        // Provider 的正文观察必须原样出现在命令响应里：少了它，页面只能笼统地
        // 说「不可阅读」，无法把「来源只有元数据」与「没有观察到」分开。
        assert_eq!(
            articles[0].availability,
            haven_application::wire::PeriodicalArticleAvailabilityDto::MetadataOnly
        );
        assert_eq!(
            articles[0]
                .page_range
                .as_ref()
                .map(|range| range.start.as_str()),
            Some("e12345")
        );

        // Wire 上是闭合枚举：三个 snake_case 字面量，未定义的取值无法往返。
        let json = serde_json::to_value(&tree).unwrap();
        assert_eq!(
            json["volumes"][0]["issues"][0]["articles"][0]["availability"],
            "metadata_only"
        );
    }

    /// 期刊归属按 Work 严格查找：另一个 Work 读不到这棵期刊树。
    #[tokio::test]
    async fn another_work_does_not_read_someone_elses_periodical() {
        let state = app_state();
        seed_periodical_tree(&state.repos).await;

        let error = run_periodical_tree_get(
            &state,
            PeriodicalTreeGetRequest {
                work_id: WorkId::new().to_string(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "PERIODICAL_NOT_FOUND");
    }
}
