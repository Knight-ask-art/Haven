//! 漫画页面身份同步用例。
//!
//! Session 在后端取得新的页面清单后，先把 provider 生成的身份与上次
//! 观察结果比较；只有序列确实变化时才替换身份并执行一次进度重定位。
//! 迁移失败时恢复旧身份，避免页面证据和进度状态出现半成功结果。

use std::sync::Arc;

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::comic_identity::{
    PageIdentity, PageMappingConfidence, PageMappingStrategy, PageMigration,
    has_opaque_control_character,
};
use haven_domain::contracts::ComicPageIdentityRepository;
use haven_domain::ids::MediaItemId;

use super::comic::PreparedComicPage;
use super::comic_progress_migration::{
    ComicPageProgressRemapRequest, ComicProgressMigrationResult, ComicProgressMigrationService,
};
use super::ports::ComicProgressMigrationPorts;

/// 一次页面身份同步的结果。调用方通常只需要继续使用新的运行时页面
/// 清单；迁移结果保留下来供测试、日志或未来的 UI 提示使用。
#[derive(Debug, Clone, PartialEq)]
pub struct ComicPageIdentitySyncResult {
    pub changed: bool,
    pub migration: ComicProgressMigrationResult,
}

#[derive(Clone)]
pub struct ComicPageIdentityService {
    ports: Arc<dyn ComicProgressMigrationPorts>,
    progress_migration: ComicProgressMigrationService,
}

impl ComicPageIdentityService {
    pub fn new(
        ports: Arc<dyn ComicProgressMigrationPorts>,
        progress_migration: ComicProgressMigrationService,
    ) -> Self {
        Self {
            ports,
            progress_migration,
        }
    }

    /// 将后端刚刚生成的页面身份写入持久化表，并在页面序列发生变化时
    /// 触发可撤销的最佳努力进度迁移。前端永远不会参与身份生成。
    pub async fn synchronize_prepared_pages(
        &self,
        media_item_id: MediaItemId,
        pages: &[PreparedComicPage],
        expected_revision: Option<String>,
    ) -> Result<ComicPageIdentitySyncResult, AppError> {
        let expected_revision = expected_revision
            .map(validate_expected_revision)
            .transpose()?;
        let new_pages = pages
            .iter()
            .map(|page| page.identity.clone())
            .collect::<Vec<_>>();
        self.synchronize_page_identities(media_item_id, new_pages, expected_revision)
            .await
    }

    async fn synchronize_page_identities(
        &self,
        media_item_id: MediaItemId,
        new_pages: Vec<PageIdentity>,
        expected_revision: Option<String>,
    ) -> Result<ComicPageIdentitySyncResult, AppError> {
        let old_snapshot =
            ComicPageIdentityRepository::get_snapshot(&*self.ports, media_item_id).await?;
        if old_snapshot.pages == new_pages {
            return Ok(ComicPageIdentitySyncResult {
                changed: false,
                migration: unchanged_result(),
            });
        }

        let new_page_revision = ComicPageIdentityRepository::replace_if_revision(
            &*self.ports,
            media_item_id,
            &new_pages,
            UtcMillis::now(),
            old_snapshot.revision.as_deref(),
        )
        .await?
        .ok_or_else(page_identity_revision_conflict)?;

        let migration = self
            .progress_migration
            .remap_page_progress(ComicPageProgressRemapRequest {
                media_item_id,
                old_pages: old_snapshot.pages.clone(),
                new_pages,
                expected_revision,
            })
            .await;
        match migration {
            Ok(migration) => Ok(ComicPageIdentitySyncResult {
                changed: true,
                migration,
            }),
            Err(error) => {
                // 页面身份仓库本身的 replace 是事务性的，但它与 Progress
                // 迁移跨两个 application 操作；失败时明确做补偿恢复。
                let rollback = ComicPageIdentityRepository::replace_if_revision(
                    &*self.ports,
                    media_item_id,
                    &old_snapshot.pages,
                    UtcMillis::now(),
                    Some(&new_page_revision),
                )
                .await;
                match rollback {
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        return Err(page_identity_rollback_conflict());
                    }
                    Err(_) => {
                        return Err(AppError::new(
                            "COMIC_PAGE_IDENTITY_ROLLBACK_FAILED",
                            haven_common::ErrorKind::Database,
                            "漫画页面身份迁移失败且无法恢复，请重新打开资源",
                            false,
                        ));
                    }
                }
                Err(error)
            }
        }
    }
}

fn unchanged_result() -> ComicProgressMigrationResult {
    ComicProgressMigrationResult {
        status: super::comic_progress_migration::ComicProgressMigrationStatus::Unchanged,
        match_result: None,
        page_migration: PageMigration {
            target_page_index: None,
            confidence: PageMappingConfidence::Low,
            strategy: PageMappingStrategy::NoTarget,
            reversible: true,
        },
        snapshot_id: None,
        applied_revision: None,
    }
}

fn validate_expected_revision(value: String) -> Result<String, AppError> {
    let trimmed = value.trim();
    if has_opaque_control_character(&value)
        || trimmed.is_empty()
        || trimmed.len() > 256
        || trimmed.contains("://")
        || trimmed.to_ascii_lowercase().starts_with("data:")
    {
        return Err(AppError::new(
            "INVALID_ARGUMENT",
            ErrorKind::Validation,
            "漫画进度迁移字段 expectedRevision 非法",
            false,
        ));
    }
    Ok(trimmed.to_owned())
}

fn page_identity_revision_conflict() -> AppError {
    AppError::new(
        "COMIC_PAGE_IDENTITY_REVISION_CONFLICT",
        ErrorKind::Conflict,
        "漫画页面身份已被其他会话更新，请刷新后重试",
        false,
    )
}

fn page_identity_rollback_conflict() -> AppError {
    AppError::new(
        "COMIC_PAGE_IDENTITY_ROLLBACK_CONFLICT",
        ErrorKind::Conflict,
        "漫画页面身份在迁移失败期间又被更新，已保留较新的页面观察，请重新打开资源",
        false,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use haven_common::UtcMillis;
    use haven_domain::comic_identity::{PageIdentity, PageMappingConfidence, PageMappingStrategy};
    use haven_domain::contracts::{
        ComicPageIdentityRepository, EditionRepository, MediaItemRepository, ProgressRepository,
        WorkRepository,
    };
    use haven_domain::entities::{ArtworkSet, Edition, MediaIndex, MediaItem, Progress, Work};
    use haven_domain::enums::{CompletionState, MediaItemStatus, MediaType, WorkStatus, WorkType};
    use haven_domain::ids::{EditionId, MediaItemId, ProgressId, WorkId};
    use haven_domain::locator::{ComicLocator, Locator};
    use haven_infrastructure::db::Db;
    use haven_infrastructure::db::repos::SqliteRepositories;

    use super::*;
    use crate::services::comic::{PreparedComicPageAvailability, PreparedComicPageSource};
    use crate::services::comic_progress_migration::ComicProgressMigrationService;
    use crate::services::comic_progress_migration::ComicProgressMigrationStatus;

    async fn seed_fixture(with_progress: bool) -> (Arc<SqliteRepositories>, MediaItemId) {
        let repositories = Arc::new(SqliteRepositories::new(Arc::new(
            Db::open_in_memory().unwrap(),
        )));
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        repositories
            .work
            .save(&Work {
                id: work_id,
                canonical_title: "页面身份同步测试".into(),
                original_title: None,
                sort_title: None,
                description: None,
                work_type: WorkType::Fiction,
                release_year: None,
                language: Some("zh-cn".into()),
                director: None,
                actor: None,
                status: WorkStatus::Completed,
                rating_value: None,
                rating_scale: None,
                artwork: ArtworkSet::default(),
                created_at: UtcMillis(1),
                updated_at: UtcMillis(1),
            })
            .await
            .unwrap();
        repositories
            .edition
            .save(&Edition {
                id: edition_id,
                work_id,
                title: "中文漫画版".into(),
                subtitle: None,
                edition_type: MediaType::Comic,
                release_date: None,
                language: Some("zh-cn".into()),
                region: None,
                publisher_or_studio: None,
                description: None,
                artwork: ArtworkSet::default(),
                created_at: UtcMillis(1),
                updated_at: UtcMillis(1),
            })
            .await
            .unwrap();
        repositories
            .media_item
            .save(&MediaItem {
                id: media_item_id,
                edition_id,
                parent_id: None,
                media_type: MediaType::Comic,
                title: "第 1 话".into(),
                index: MediaIndex::Chapter {
                    volume: None,
                    chapter: 1.0,
                },
                duration_ms: None,
                page_count: Some(4),
                chapter_count: None,
                published_at: None,
                status: MediaItemStatus::Available,
                created_at: UtcMillis(1),
                updated_at: UtcMillis(1),
            })
            .await
            .unwrap();

        if with_progress {
            repositories
                .progress
                .save_if_revision(
                    &Progress {
                        id: ProgressId::new(),
                        work_id,
                        edition_id,
                        media_item_id,
                        locator: Locator::Comic(ComicLocator {
                            chapter_item_id: media_item_id,
                            page_index: 1,
                            page_progression: Some(0.5),
                        }),
                        completion: CompletionState::InProgress,
                        percentage: Some(0.5),
                        last_active_at: UtcMillis(1),
                        updated_at: UtcMillis(1),
                        revision: None,
                        keyframe_uri: None,
                    },
                    None,
                )
                .await
                .unwrap()
                .unwrap();
        }
        (repositories, media_item_id)
    }

    fn prepared_pages(keys: &[&str]) -> Vec<PreparedComicPage> {
        keys.iter()
            .map(|key| PreparedComicPage {
                availability: PreparedComicPageAvailability::Ready,
                identity: PageIdentity::stable(*key),
                source: PreparedComicPageSource::RemotePage {
                    page_name: (*key).to_owned(),
                },
            })
            .collect()
    }

    fn service(repositories: Arc<SqliteRepositories>) -> ComicPageIdentityService {
        ComicPageIdentityService::new(
            repositories.clone(),
            ComicProgressMigrationService::new(repositories),
        )
    }

    #[tokio::test]
    async fn first_observation_is_saved_and_repeated_observation_is_idempotent() {
        let (repositories, media_item_id) = seed_fixture(false).await;
        let service = service(repositories.clone());
        let pages = prepared_pages(&["a", "b"]);

        let first = service
            .synchronize_prepared_pages(media_item_id, &pages, None)
            .await
            .unwrap();
        assert!(first.changed);
        assert_eq!(
            first.migration.status,
            ComicProgressMigrationStatus::NoSourceProgress
        );
        let first_snapshot = repositories
            .page_identity
            .get_snapshot(media_item_id)
            .await
            .unwrap();
        assert_eq!(
            first_snapshot.pages,
            [PageIdentity::stable("a"), PageIdentity::stable("b")]
        );
        assert!(first_snapshot.revision.is_some());

        let second = service
            .synchronize_prepared_pages(media_item_id, &pages, None)
            .await
            .unwrap();
        assert!(!second.changed);
        assert_eq!(
            second.migration.status,
            ComicProgressMigrationStatus::Unchanged
        );
        assert_eq!(
            repositories
                .page_identity
                .get_snapshot(media_item_id)
                .await
                .unwrap()
                .revision,
            first_snapshot.revision
        );
    }

    #[tokio::test]
    async fn inserted_page_keeps_progress_on_the_same_stable_page() {
        let (repositories, media_item_id) = seed_fixture(true).await;
        repositories
            .page_identity
            .replace(
                media_item_id,
                &[
                    PageIdentity::stable("a"),
                    PageIdentity::stable("b"),
                    PageIdentity::stable("c"),
                ],
                UtcMillis(2),
            )
            .await
            .unwrap();
        let service = service(repositories.clone());

        let result = service
            .synchronize_prepared_pages(
                media_item_id,
                &prepared_pages(&["intro", "a", "b", "c"]),
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            result.migration.status,
            ComicProgressMigrationStatus::Applied
        );
        assert_eq!(result.migration.page_migration.target_page_index, Some(2));
        assert_eq!(
            result.migration.page_migration.strategy,
            PageMappingStrategy::StableKey
        );
        let progress = repositories
            .progress
            .get_for_media_item(media_item_id)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            progress.locator,
            Locator::Comic(ComicLocator { page_index: 2, .. })
        ));
    }

    #[tokio::test]
    async fn deleted_current_page_uses_nearest_surviving_page() {
        let (repositories, media_item_id) = seed_fixture(true).await;
        repositories
            .page_identity
            .replace(
                media_item_id,
                &[
                    PageIdentity::stable("a"),
                    PageIdentity::stable("deleted"),
                    PageIdentity::stable("c"),
                ],
                UtcMillis(2),
            )
            .await
            .unwrap();
        let result = service(repositories.clone())
            .synchronize_prepared_pages(media_item_id, &prepared_pages(&["a", "c"]), None)
            .await
            .unwrap();
        assert_eq!(result.migration.page_migration.target_page_index, Some(1));
        assert_eq!(
            result.migration.page_migration.strategy,
            PageMappingStrategy::NearestSurvivingPage
        );
    }

    #[tokio::test]
    async fn fingerprint_and_proportional_fallbacks_are_explained() {
        let (repositories, media_item_id) = seed_fixture(true).await;
        repositories
            .page_identity
            .replace(
                media_item_id,
                &[
                    PageIdentity::fingerprint("page-a"),
                    PageIdentity::fingerprint("page-b"),
                ],
                UtcMillis(2),
            )
            .await
            .unwrap();
        let service = service(repositories.clone());
        let fingerprint_pages = [
            PreparedComicPage {
                availability: PreparedComicPageAvailability::Ready,
                identity: PageIdentity::fingerprint("page-a"),
                source: PreparedComicPageSource::RemotePage {
                    page_name: "renamed-a".into(),
                },
            },
            PreparedComicPage {
                availability: PreparedComicPageAvailability::Ready,
                identity: PageIdentity::fingerprint("page-b"),
                source: PreparedComicPageSource::RemotePage {
                    page_name: "renamed-b".into(),
                },
            },
        ];
        // Same page identities are intentionally a no-op; change the order to
        // exercise content fingerprint matching without relying on page names.
        let fingerprint_pages = vec![fingerprint_pages[1].clone(), fingerprint_pages[0].clone()];
        let result = service
            .synchronize_prepared_pages(media_item_id, &fingerprint_pages, None)
            .await
            .unwrap();
        assert_eq!(result.migration.page_migration.target_page_index, Some(0));
        assert_eq!(
            result.migration.page_migration.strategy,
            PageMappingStrategy::ContentFingerprint
        );

        repositories
            .page_identity
            .replace(
                media_item_id,
                &[PageIdentity::default(), PageIdentity::default()],
                UtcMillis(3),
            )
            .await
            .unwrap();
        let fallback = service
            .synchronize_prepared_pages(
                media_item_id,
                &prepared_pages(&["new-a", "new-b", "new-c"]),
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            fallback.migration.page_migration.strategy,
            PageMappingStrategy::ProportionalFallback
        );
        assert_eq!(
            fallback.migration.page_migration.confidence,
            PageMappingConfidence::Low
        );
        assert!(fallback.migration.page_migration.reversible);
    }

    #[tokio::test]
    async fn progress_cas_failure_compensates_page_identity_update() {
        let (repositories, media_item_id) = seed_fixture(true).await;
        repositories
            .page_identity
            .replace(
                media_item_id,
                &[PageIdentity::stable("old-a"), PageIdentity::stable("old-b")],
                UtcMillis(2),
            )
            .await
            .unwrap();
        let service = service(repositories.clone());

        let error = service
            .synchronize_prepared_pages(
                media_item_id,
                &prepared_pages(&["new-a", "new-b", "new-c"]),
                Some("stale-progress-revision".into()),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "REVISION_CONFLICT");
        let snapshot = repositories
            .page_identity
            .get_snapshot(media_item_id)
            .await
            .unwrap();
        assert_eq!(
            snapshot.pages,
            [PageIdentity::stable("old-a"), PageIdentity::stable("old-b")]
        );
    }
}
