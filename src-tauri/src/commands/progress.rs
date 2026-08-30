//! `progress_save` / `progress_recent` / `progress_reset` commands（IPC）。

use tauri::State;

use haven_application::wire::{
    ErrorDto, ProgressRecentRequest, ProgressResetRequest, ProgressSaveRequest, ProgressSaveResult,
    ProgressSummaryDto,
};
use haven_domain::ids::MediaItemId;

use crate::ipc::{invalid_id, run_blocking, to_error_dto};
use crate::state::AppState;

const DEFAULT_RECENT_LIMIT: u32 = 50;

/// 解析并强制要求 canonical UUID 文本，避免同一 ID 通过多种字符串形态进入 IPC。
fn validate_canonical_media_item_id(value: &str) -> Result<(), ErrorDto> {
    let id: MediaItemId = value.parse().map_err(|_| invalid_id())?;
    if id.to_string() != value {
        return Err(invalid_id());
    }
    Ok(())
}

pub async fn run_progress_save(
    state: &AppState,
    request: ProgressSaveRequest,
) -> Result<ProgressSaveResult, ErrorDto> {
    validate_canonical_media_item_id(&request.media_item_id)?;
    state
        .progress
        .save(request)
        .await
        .map_err(|error| to_error_dto(&error))
}

#[tauri::command]
pub async fn progress_save(
    state: State<'_, AppState>,
    request: ProgressSaveRequest,
) -> Result<ProgressSaveResult, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_progress_save(&state, request).await }).await
}

pub async fn run_progress_recent(
    state: &AppState,
    request: ProgressRecentRequest,
) -> Result<Vec<ProgressSummaryDto>, ErrorDto> {
    let limit = request.limit.unwrap_or(DEFAULT_RECENT_LIMIT);
    state
        .progress
        .recent(limit)
        .await
        .map_err(|e| to_error_dto(&e))
}

#[tauri::command]
pub async fn progress_recent(
    state: State<'_, AppState>,
    request: ProgressRecentRequest,
) -> Result<Vec<ProgressSummaryDto>, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_progress_recent(&state, request).await }).await
}

pub async fn run_progress_reset(
    state: &AppState,
    request: ProgressResetRequest,
) -> Result<(), ErrorDto> {
    let id: MediaItemId = request.media_item_id.parse().map_err(|_| invalid_id())?;
    if id.to_string() != request.media_item_id {
        return Err(invalid_id());
    }
    state.progress.reset(id).await.map_err(|e| to_error_dto(&e))
}

#[tauri::command]
pub async fn progress_reset(
    state: State<'_, AppState>,
    request: ProgressResetRequest,
) -> Result<(), ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_progress_reset(&state, request).await }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use haven_application::wire::{CompletionWire, LocatorDto, VideoLocatorDto};
    use haven_domain::contracts::{EditionRepository, MediaItemRepository, WorkRepository};
    use haven_domain::entities::{Edition, MediaIndex, MediaItem, Work};
    use haven_domain::enums::{MediaItemStatus, MediaType, WorkStatus, WorkType};
    use haven_domain::ids::{EditionId, WorkId};
    use haven_infrastructure::Db;

    async fn seed_movie(state: &AppState) -> MediaItemId {
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let now = haven_common::UtcMillis::now();
        state
            .repos
            .work
            .save(&Work {
                id: work_id,
                canonical_title: "IPC 测试作品".into(),
                original_title: None,
                sort_title: None,
                description: None,
                work_type: WorkType::Fiction,
                release_year: None,
                language: None,
                director: None,
                actor: None,
                status: WorkStatus::Completed,
                rating_value: None,
                rating_scale: None,
                artwork: Default::default(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        state
            .repos
            .edition
            .save(&Edition {
                id: edition_id,
                work_id,
                title: "IPC 测试版本".into(),
                subtitle: None,
                edition_type: MediaType::Movie,
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
        state
            .repos
            .media_item
            .save(&MediaItem {
                id: media_item_id,
                edition_id,
                parent_id: None,
                media_type: MediaType::Movie,
                title: "IPC 测试电影".into(),
                index: MediaIndex::Movie,
                duration_ms: Some(100_000),
                page_count: None,
                chapter_count: None,
                published_at: None,
                status: MediaItemStatus::Available,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        media_item_id
    }

    fn request(id: MediaItemId, position_ms: u64) -> ProgressSaveRequest {
        ProgressSaveRequest {
            media_item_id: id.to_string(),
            locator: LocatorDto::Video(VideoLocatorDto { position_ms }),
            completion: Some(CompletionWire::InProgress),
            expected_revision: None,
            keyframe: None,
        }
    }

    #[tokio::test]
    async fn progress_save_invalid_id_is_stable_and_does_not_write() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let error = run_progress_save(
            &state,
            ProgressSaveRequest {
                media_item_id: "not-a-canonical-id".into(),
                locator: LocatorDto::Video(VideoLocatorDto { position_ms: 0 }),
                completion: None,
                expected_revision: None,
                keyframe: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "INVALID_ID");
        assert!(!error.retryable);
    }

    #[tokio::test]
    async fn progress_save_round_trip_and_revision_conflict() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let media_item_id = seed_movie(&state).await;
        let first = run_progress_save(&state, request(media_item_id, 10_000))
            .await
            .unwrap();
        assert!(!first.revision.is_empty());

        let mut second = request(media_item_id, 20_000);
        second.expected_revision = Some(first.revision.clone());
        let second_result = run_progress_save(&state, second).await.unwrap();
        assert_ne!(second_result.revision, first.revision);

        let mut stale = request(media_item_id, 30_000);
        stale.expected_revision = Some(first.revision);
        let error = run_progress_save(&state, stale).await.unwrap_err();
        assert_eq!(error.code, "REVISION_CONFLICT");
    }

    #[tokio::test]
    async fn progress_save_incompatible_locator_is_stable() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let media_item_id = seed_movie(&state).await;
        let mut request = request(media_item_id, 10_000);
        request.locator = LocatorDto::Book(haven_application::wire::BookLocatorDto {
            publication_resource: "chapter.xhtml".into(),
            progression: None,
            text_anchor: None,
            format_locator: None,
        });
        let error = run_progress_save(&state, request).await.unwrap_err();
        assert_eq!(error.code, "LOCATOR_KIND_INCOMPATIBLE");
    }

    #[tokio::test]
    async fn progress_save_rejects_mismatched_comic_chapter_without_write() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let media_item_id = seed_movie(&state).await;
        let request = ProgressSaveRequest {
            media_item_id: media_item_id.to_string(),
            locator: LocatorDto::Comic(haven_application::wire::ComicLocatorDto {
                chapter_item_id: haven_domain::ids::MediaItemId::new().to_string(),
                page_index: 1,
                page_progression: None,
            }),
            completion: Some(CompletionWire::InProgress),
            expected_revision: None,
            keyframe: None,
        };
        let error = run_progress_save(&state, request).await.unwrap_err();
        assert_eq!(error.code, "INVALID_ARGUMENT");
        assert!(state.progress.get(media_item_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn progress_recent_default_limit_empty_db() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let result = run_progress_recent(&state, ProgressRecentRequest { limit: None })
            .await
            .unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn progress_reset_invalid_id_is_stable() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let error = run_progress_reset(
            &state,
            ProgressResetRequest {
                media_item_id: "not-a-uuid".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "INVALID_ID");
        assert!(!error.retryable);
    }

    #[tokio::test]
    async fn progress_reset_unknown_item_is_not_found() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let error = run_progress_reset(
            &state,
            ProgressResetRequest {
                media_item_id: haven_domain::ids::MediaItemId::new().to_string(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "PROGRESS_NOT_FOUND");
    }
}
