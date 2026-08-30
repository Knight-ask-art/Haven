//! Resource preference Commands（V02-RESOURCE-PREF-001）。
//! Command 仅负责 request 校验、ID 转换、Application 调用和 ErrorDto 映射。

use tauri::{Runtime, State};

use haven_application::services::resource_preferences::{
    PreferenceTarget, ResourcePreferenceService,
};
use haven_application::wire::{
    PreferenceGetRequest, PreferenceGetResult, PreferenceTargetDto, PreferenceUpdateRequest,
    PreferenceUpdateResult,
};
use haven_domain::ids::{EditionId, MediaItemId};

use crate::ipc::{invalid_id, run_blocking, to_error_dto};
use crate::state::AppState;

fn parse_ids(
    media_item_id: &str,
    edition_id: &str,
) -> Result<(MediaItemId, EditionId), haven_application::wire::ErrorDto> {
    let media_item_id = media_item_id.parse().map_err(|_| invalid_id())?;
    let edition_id = edition_id.parse().map_err(|_| invalid_id())?;
    Ok((media_item_id, edition_id))
}

fn map_target(target: PreferenceTargetDto) -> PreferenceTarget {
    match target {
        PreferenceTargetDto::Edition => PreferenceTarget::Edition,
        PreferenceTargetDto::MediaItem => PreferenceTarget::MediaItem,
    }
}

fn map_snapshot(snapshot: haven_application::services::PreferenceSnapshot) -> PreferenceGetResult {
    PreferenceGetResult {
        schema_version: 1,
        media_item_id: snapshot.media_item_id.to_string(),
        edition_id: snapshot.edition_id.to_string(),
        reading_patch: snapshot.reading_patch.map(Into::into),
        comic_patch: snapshot.comic_patch.map(Into::into),
        edition_reading_patch: snapshot.edition_reading_patch.map(Into::into),
        edition_comic_patch: snapshot.edition_comic_patch.map(Into::into),
        media_item_reading_patch: snapshot.media_item_reading_patch.map(Into::into),
        media_item_comic_patch: snapshot.media_item_comic_patch.map(Into::into),
        effective_reading: snapshot.effective_reading.into(),
        effective_comic: snapshot.effective_comic.into(),
        media_item_revision: snapshot.media_item_revision,
        edition_revision: snapshot.edition_revision,
    }
}

pub async fn run_preference_get(
    state: &AppState,
    request: PreferenceGetRequest,
) -> Result<PreferenceGetResult, haven_application::wire::ErrorDto> {
    let (media_item_id, edition_id) = parse_ids(&request.media_item_id, &request.edition_id)?;
    state
        .resource_preferences
        .get(media_item_id, edition_id)
        .await
        .map(map_snapshot)
        .map_err(|error| to_error_dto(&error))
}

pub async fn run_preference_update(
    state: &AppState,
    request: PreferenceUpdateRequest,
) -> Result<PreferenceUpdateResult, haven_application::wire::ErrorDto> {
    let (media_item_id, edition_id) = parse_ids(&request.media_item_id, &request.edition_id)?;
    let target = map_target(request.target);
    let result = state
        .resource_preferences
        .update(
            media_item_id,
            edition_id,
            target,
            request.data(),
            request.expected_revision.as_deref(),
        )
        .await
        .map_err(|error| to_error_dto(&error))?;
    Ok(PreferenceUpdateResult {
        result: map_snapshot(result.snapshot),
        target: request.target,
        revision: result.revision,
        changed: result.changed,
    })
}

#[tauri::command]
pub async fn preference_get(
    state: State<'_, AppState>,
    request: PreferenceGetRequest,
) -> Result<PreferenceGetResult, haven_application::wire::ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_preference_get(&state, request).await }).await
}

#[tauri::command]
pub async fn preference_update<R: Runtime>(
    _app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    request: PreferenceUpdateRequest,
) -> Result<PreferenceUpdateResult, haven_application::wire::ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_preference_update(&state, request).await }).await
}

// Keep the service type referenced in this module's public boundary so accidental
// direct repository access in future commands remains visible to reviewers.
#[allow(dead_code)]
fn _service_type(_: &ResourcePreferenceService) {}
