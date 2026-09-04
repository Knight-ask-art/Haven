//! Comic page manifest query. Page bytes remain on the controlled protocol.

use tauri::{Runtime, State, Webview};

use haven_application::wire::{
    ComicChapterCatalogDto, ComicChapterCatalogGetRequest, ComicChapterSourceCandidatesDto,
    ComicChapterSourceCandidatesGetRequestDto, ComicPageManifestDto, ComicPageManifestGetRequest,
    ComicPageProgressRemapRequestDto, ComicProgressMigrationRequestDto,
    ComicProgressMigrationResultDto, ComicProgressMigrationRevertRequestDto,
    ComicProgressMigrationRevertResultDto, ComicRegisteredChapterCatalogDto, ErrorDto,
};
use haven_domain::ids::MediaItemId;

use crate::ipc::{invalid_id, run_blocking, to_error_dto};
use crate::state::AppState;

/// 漫画章节目录命令核心：只读取来源观察，不创建/更新 Work、Edition、MediaItem。
pub async fn run_comic_chapter_catalog_get(
    state: &AppState,
    request: ComicChapterCatalogGetRequest,
) -> Result<ComicChapterCatalogDto, ErrorDto> {
    state
        .comic_catalog
        .get(request)
        .await
        .map_err(|error| to_error_dto(&error))
}

/// 获取来源作品的有界章节目录。Provider URL、页面授权和内部匹配键不出 IPC。
#[tauri::command]
pub async fn comic_chapter_catalog_get(
    state: State<'_, AppState>,
    request: ComicChapterCatalogGetRequest,
) -> Result<ComicChapterCatalogDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_comic_chapter_catalog_get(&state, request).await }).await
}

/// 读取 SQLite 中已登记的章节来源身份和刷新状态；不访问 Provider，不触发刷新。
pub async fn run_comic_chapter_catalog_registered_get(
    state: &AppState,
    request: ComicChapterCatalogGetRequest,
) -> Result<ComicRegisteredChapterCatalogDto, ErrorDto> {
    state
        .comic_catalog
        .get_registered(request)
        .await
        .map_err(|error| to_error_dto(&error))
}

#[tauri::command]
pub async fn comic_chapter_catalog_registered_get(
    state: State<'_, AppState>,
    request: ComicChapterCatalogGetRequest,
) -> Result<ComicRegisteredChapterCatalogDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(
        move || async move { run_comic_chapter_catalog_registered_get(&state, request).await },
    )
    .await
}

/// 显式刷新并登记来源作品的漫画章节目录；刷新仍受 generation CAS 保护。
pub async fn run_comic_chapter_catalog_refresh(
    state: &AppState,
    request: ComicChapterCatalogGetRequest,
) -> Result<ComicChapterCatalogDto, ErrorDto> {
    state
        .comic_catalog
        .refresh(request)
        .await
        .map_err(|error| to_error_dto(&error))
}

#[tauri::command]
pub async fn comic_chapter_catalog_refresh(
    state: State<'_, AppState>,
    request: ComicChapterCatalogGetRequest,
) -> Result<ComicChapterCatalogDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_comic_chapter_catalog_refresh(&state, request).await })
        .await
}

/// 查询当前章节所属 Work 下的其他已登记来源，并返回后端生成的匹配证据。
pub async fn run_comic_chapter_source_candidates_get(
    state: &AppState,
    request: ComicChapterSourceCandidatesGetRequestDto,
) -> Result<ComicChapterSourceCandidatesDto, ErrorDto> {
    state
        .comic_progress_migration
        .source_candidates_wire(request)
        .await
        .map_err(|error| to_error_dto(&error))
}

#[tauri::command]
pub async fn comic_chapter_source_candidates_get(
    state: State<'_, AppState>,
    request: ComicChapterSourceCandidatesGetRequestDto,
) -> Result<ComicChapterSourceCandidatesDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(
        move || async move { run_comic_chapter_source_candidates_get(&state, request).await },
    )
    .await
}

/// 显式执行一次跨来源章节进度迁移。低置信度匹配只有在请求明确允许最佳
/// 努力时才会应用；应用结果必须携带低置信度和可撤销快照。
pub async fn run_comic_progress_migrate(
    state: &AppState,
    request: ComicProgressMigrationRequestDto,
) -> Result<ComicProgressMigrationResultDto, ErrorDto> {
    state
        .comic_progress_migration
        .migrate_wire(request)
        .await
        .map_err(|error| to_error_dto(&error))
}

#[tauri::command]
pub async fn comic_progress_migrate(
    state: State<'_, AppState>,
    request: ComicProgressMigrationRequestDto,
) -> Result<ComicProgressMigrationResultDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_comic_progress_migrate(&state, request).await }).await
}

/// 重新检查 owner-bound Session 的页面事实，触发可撤销的当前页重定位。
/// 页面身份由后端 Provider 生成，前端不能提交 oldPages/newPages。
pub async fn run_comic_progress_remap(
    state: &AppState,
    owner_webview_label: &str,
    request: ComicPageProgressRemapRequestDto,
) -> Result<ComicProgressMigrationResultDto, ErrorDto> {
    let uuid = uuid::Uuid::parse_str(&request.session_id).map_err(|_| invalid_id())?;
    let session_id = uuid.to_string();
    if session_id != request.session_id {
        return Err(invalid_id());
    }
    let prepared = state
        .session_registry
        .lookup_for_owner(&session_id, owner_webview_label)
        .map_err(|error| to_error_dto(&error))?;
    let media_item_id: MediaItemId = prepared.media_item_id.parse().map_err(|_| invalid_id())?;
    let pages = state
        .session
        .inspect_comic_pages(&prepared)
        .await
        .map_err(|error| to_error_dto(&error))?;
    let sync = state
        .comic_page_identity
        .synchronize_prepared_pages(media_item_id, &pages, request.expected_revision)
        .await
        .map_err(|error| to_error_dto(&error))?;
    state
        .session_registry
        .replace_comic_pages(&session_id, owner_webview_label, pages)
        .map_err(|error| to_error_dto(&error))?;
    haven_application::services::comic_progress_migration::migration_result_to_dto(sync.migration)
        .map_err(|error| to_error_dto(&error))
}

#[tauri::command]
pub async fn comic_progress_remap<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, AppState>,
    request: ComicPageProgressRemapRequestDto,
) -> Result<ComicProgressMigrationResultDto, ErrorDto> {
    let owner_webview_label = webview.label().to_owned();
    let state = (*state.inner()).clone();
    run_blocking(move || async move {
        run_comic_progress_remap(&state, &owner_webview_label, request).await
    })
    .await
}

/// 以应用 revision 做 CAS 撤销一次迁移；返回 false 表示目标已被后续写入。
pub async fn run_comic_progress_revert(
    state: &AppState,
    request: ComicProgressMigrationRevertRequestDto,
) -> Result<ComicProgressMigrationRevertResultDto, ErrorDto> {
    state
        .comic_progress_migration
        .revert_wire(request)
        .await
        .map_err(|error| to_error_dto(&error))
}

#[tauri::command]
pub async fn comic_progress_revert(
    state: State<'_, AppState>,
    request: ComicProgressMigrationRevertRequestDto,
) -> Result<ComicProgressMigrationRevertResultDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_comic_progress_revert(&state, request).await }).await
}

pub async fn run_comic_page_manifest_get(
    state: &AppState,
    owner_webview_label: &str,
    request: ComicPageManifestGetRequest,
) -> Result<ComicPageManifestDto, ErrorDto> {
    let uuid = uuid::Uuid::parse_str(&request.session_id).map_err(|_| invalid_id())?;
    let session_id = uuid.to_string();
    if session_id != request.session_id {
        return Err(invalid_id());
    }
    state
        .session_registry
        .comic_manifest(&session_id, owner_webview_label)
        .map_err(|error| to_error_dto(&error))
}

#[tauri::command]
pub async fn comic_page_manifest_get<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, AppState>,
    request: ComicPageManifestGetRequest,
) -> Result<ComicPageManifestDto, ErrorDto> {
    let owner_webview_label = webview.label().to_owned();
    let state = (*state.inner()).clone();
    run_blocking(move || async move {
        run_comic_page_manifest_get(&state, &owner_webview_label, request).await
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_application::wire::ComicChapterSourceIdentityDto;
    use haven_domain::comic_catalog::ComicChapterSourceStatus;
    use haven_domain::comic_identity::{
        ChapterSourceIdentity, ChapterSourceRef, ComicChapterMetadata, EditionProfile, PageIdentity,
    };
    use haven_domain::contracts::{
        ChapterSourceRepository, ComicPageIdentityRepository, EditionRepository,
        MediaItemRepository, ProgressRepository, WorkRepository,
    };
    use haven_domain::entities::{ArtworkSet, Edition, MediaIndex, MediaItem, Progress, Work};
    use haven_domain::enums::{CompletionState, MediaItemStatus, MediaType, WorkStatus, WorkType};
    use haven_domain::ids::{EditionId, MediaItemId, ProgressId, WorkId};
    use haven_domain::locator::{ComicLocator, Locator};
    use haven_infrastructure::Db;
    use std::sync::Arc;

    async fn seed_progress_migration_fixture(
        state: &AppState,
    ) -> (ChapterSourceIdentity, ChapterSourceIdentity, MediaItemId) {
        let work_id = WorkId::new();
        let source_edition_id = EditionId::new();
        let target_edition_id = EditionId::new();
        let source_media_item_id = MediaItemId::new();
        let target_media_item_id = MediaItemId::new();
        let now = haven_common::UtcMillis(1);

        state
            .repos
            .work
            .save(&Work {
                id: work_id,
                canonical_title: "IPC 漫画迁移测试".into(),
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
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        for (id, title) in [
            (source_edition_id, "IPC 源版本"),
            (target_edition_id, "IPC 目标版本"),
        ] {
            state
                .repos
                .edition
                .save(&Edition {
                    id,
                    work_id,
                    title: title.into(),
                    subtitle: None,
                    edition_type: MediaType::Comic,
                    release_date: None,
                    language: Some("zh-cn".into()),
                    region: None,
                    publisher_or_studio: None,
                    description: None,
                    artwork: ArtworkSet::default(),
                    created_at: now,
                    updated_at: now,
                })
                .await
                .unwrap();
        }
        for (id, edition_id) in [
            (source_media_item_id, source_edition_id),
            (target_media_item_id, target_edition_id),
        ] {
            state
                .repos
                .media_item
                .save(&MediaItem {
                    id,
                    edition_id,
                    parent_id: None,
                    media_type: MediaType::Comic,
                    title: "第 12 话".into(),
                    index: MediaIndex::Chapter {
                        volume: None,
                        chapter: 12.0,
                    },
                    duration_ms: None,
                    page_count: Some(3),
                    chapter_count: None,
                    published_at: None,
                    status: MediaItemStatus::Available,
                    created_at: now,
                    updated_at: now,
                })
                .await
                .unwrap();
        }

        let source = ChapterSourceIdentity::new("source-a", "work-a", "chapter-a").unwrap();
        let target = ChapterSourceIdentity::new("source-b", "work-b", "chapter-b").unwrap();
        for (identity, media_item_id) in [
            (source.clone(), source_media_item_id),
            (target.clone(), target_media_item_id),
        ] {
            state
                .repos
                .chapter_source
                .save(&ChapterSourceRef {
                    media_item_id,
                    identity,
                    metadata: ComicChapterMetadata {
                        edition_profile: EditionProfile::from_language(Some("zh-cn")),
                        chapter_number: Some(12.0),
                        volume_number: None,
                        title: Some("第 12 话".into()),
                        page_count: Some(3),
                        authoritative_content_key: None,
                    },
                    source_order: 0,
                    availability: ComicChapterSourceStatus::Available,
                    published_at: None,
                    source_updated_at: None,
                    last_seen_generation: None,
                    updated_at: haven_common::UtcMillis(2),
                })
                .await
                .unwrap();
        }
        state
            .repos
            .page_identity
            .replace(
                source_media_item_id,
                &[
                    PageIdentity::stable("page-a"),
                    PageIdentity::stable("removed"),
                    PageIdentity::stable("page-c"),
                ],
                haven_common::UtcMillis(2),
            )
            .await
            .unwrap();
        state
            .repos
            .page_identity
            .replace(
                target_media_item_id,
                &[
                    PageIdentity::stable("page-a"),
                    PageIdentity::stable("page-c"),
                    PageIdentity::stable("page-d"),
                ],
                haven_common::UtcMillis(2),
            )
            .await
            .unwrap();
        state
            .repos
            .progress
            .save_if_revision(
                &Progress {
                    id: ProgressId::new(),
                    work_id,
                    edition_id: source_edition_id,
                    media_item_id: source_media_item_id,
                    locator: Locator::Comic(ComicLocator {
                        chapter_item_id: source_media_item_id,
                        page_index: 1,
                        page_progression: Some(0.5),
                    }),
                    completion: CompletionState::InProgress,
                    percentage: Some(0.5),
                    last_active_at: haven_common::UtcMillis(10),
                    updated_at: haven_common::UtcMillis(10),
                    revision: None,
                    keyframe_uri: None,
                },
                None,
            )
            .await
            .unwrap();

        (source, target, target_media_item_id)
    }

    #[tokio::test]
    async fn unsupported_catalog_source_is_rejected_before_network_access() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let error = run_comic_chapter_catalog_get(
            &state,
            ComicChapterCatalogGetRequest {
                source_id: "unknown-source".into(),
                remote_work_id: "not-a-uuid".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "SOURCE_CATALOG_UNSUPPORTED");
        assert!(!error.retryable);
    }

    #[tokio::test]
    async fn malformed_mangadex_work_id_is_rejected_at_application_boundary() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let error = run_comic_chapter_catalog_get(
            &state,
            ComicChapterCatalogGetRequest {
                source_id: "mangadex".into(),
                remote_work_id: "not-a-uuid".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "INVALID_ARGUMENT");
        assert!(!error.retryable);
    }

    #[tokio::test]
    async fn refresh_rejects_unknown_source_before_catalog_access() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let error = run_comic_chapter_catalog_refresh(
            &state,
            ComicChapterCatalogGetRequest {
                source_id: "unknown-source".into(),
                remote_work_id: "aaaaaaaa-aaaa-4aaa-8000-aaaaaaaaaaaa".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "SOURCE_CATALOG_UNSUPPORTED");
        assert!(!error.retryable);
    }

    #[tokio::test]
    async fn registered_catalog_query_is_empty_without_persisted_chapters() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let result = run_comic_chapter_catalog_registered_get(
            &state,
            ComicChapterCatalogGetRequest {
                source_id: "mangadex".into(),
                remote_work_id: "aaaaaaaa-aaaa-4aaa-8000-aaaaaaaaaaaa".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(result.schema_version, 1);
        assert_eq!(result.source_id, "mangadex");
        assert!(result.refresh_state.is_none());
        assert!(result.chapters.is_empty());
    }

    #[tokio::test]
    async fn registered_catalog_rejects_url_shaped_identity_before_repository_lookup() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let error = run_comic_chapter_catalog_registered_get(
            &state,
            ComicChapterCatalogGetRequest {
                source_id: "mangadex".into(),
                remote_work_id: "https://example.invalid/manga".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "INVALID_ARGUMENT");
        assert!(!error.retryable);
    }

    #[tokio::test]
    async fn invalid_session_id_is_rejected_before_registry_lookup() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let error = run_comic_page_manifest_get(
            &state,
            "main",
            ComicPageManifestGetRequest {
                session_id: "NOT-A-CANONICAL-ID".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "INVALID_ID");
        assert!(!error.retryable);
    }

    #[tokio::test]
    async fn progress_migration_command_applies_and_reverts_with_cas() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let (source, target, target_media_item_id) = seed_progress_migration_fixture(&state).await;
        let result = run_comic_progress_migrate(
            &state,
            ComicProgressMigrationRequestDto {
                source: ComicChapterSourceIdentityDto {
                    source_id: source.source_key,
                    remote_work_id: source.remote_work_id,
                    remote_chapter_id: source.remote_chapter_id,
                },
                target: ComicChapterSourceIdentityDto {
                    source_id: target.source_key,
                    remote_work_id: target.remote_work_id,
                    remote_chapter_id: target.remote_chapter_id,
                },
                allow_best_effort: false,
                allow_target_overwrite: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            result.status,
            haven_application::wire::ComicProgressMigrationStatusDto::Applied
        );
        assert_eq!(result.page_migration.target_page_index, Some(1));
        assert!(result.snapshot_id.is_some());
        assert!(result.applied_revision.is_some());

        let migrated = state
            .repos
            .progress
            .get_for_media_item(target_media_item_id)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            migrated.locator,
            Locator::Comic(ComicLocator { page_index: 1, .. })
        ));

        let reverted = run_comic_progress_revert(
            &state,
            ComicProgressMigrationRevertRequestDto {
                migration_id: result.snapshot_id.unwrap(),
                expected_applied_revision: result.applied_revision.unwrap(),
            },
        )
        .await
        .unwrap();
        assert!(reverted.reverted);
        assert!(state
            .repos
            .progress
            .get_for_media_item(target_media_item_id)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn source_candidates_command_returns_backend_match_projection() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let (source, _target, _target_media_item_id) =
            seed_progress_migration_fixture(&state).await;
        let result = run_comic_chapter_source_candidates_get(
            &state,
            ComicChapterSourceCandidatesGetRequestDto {
                source: ComicChapterSourceIdentityDto {
                    source_id: source.source_key,
                    remote_work_id: source.remote_work_id,
                    remote_chapter_id: source.remote_chapter_id,
                },
            },
        )
        .await
        .unwrap();
        assert_eq!(result.schema_version, 1);
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(
            result.candidates[0].match_result.kind,
            haven_application::wire::ComicChapterMatchKindDto::SameLogicalChapterVariant
        );
    }

    #[tokio::test]
    async fn progress_migration_command_rejects_url_and_noncanonical_inputs() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let error = run_comic_progress_migrate(
            &state,
            ComicProgressMigrationRequestDto {
                source: ComicChapterSourceIdentityDto {
                    source_id: "source-a".into(),
                    remote_work_id: "work-a".into(),
                    remote_chapter_id: "https://example.invalid/chapter".into(),
                },
                target: ComicChapterSourceIdentityDto {
                    source_id: "source-b".into(),
                    remote_work_id: "work-b".into(),
                    remote_chapter_id: "chapter-b".into(),
                },
                allow_best_effort: false,
                allow_target_overwrite: false,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "INVALID_ARGUMENT");

        let error = run_comic_progress_remap(
            &state,
            "main",
            ComicPageProgressRemapRequestDto {
                session_id: "NOT-A-CANONICAL-ID".into(),
                expected_revision: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "INVALID_ID");
    }
}
