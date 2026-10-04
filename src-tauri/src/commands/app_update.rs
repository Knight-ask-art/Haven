//! Fixed, window-owned update preparation. No path, URL or process arguments.
//! The official updater remains the only installer/signature authority.

use haven_application::wire::ErrorDto;
use haven_common::{AppError, ErrorKind};
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::State;

use crate::ipc::{run_blocking, to_error_dto};
use crate::session_registry::WindowCloseLease;
use crate::state::AppState;

/// Window-owned guards survive IPC preparation and block new sessions until
/// the updater exits, or the failed handoff is explicitly cancelled.
#[derive(Default)]
pub(crate) struct UpdatePreparation {
    leases: Mutex<HashMap<String, WindowCloseLease>>,
}

impl UpdatePreparation {
    fn retain(&self, lease: WindowCloseLease) -> Result<(), ErrorDto> {
        let mut leases = self.leases.lock().map_err(|_| preparation_unavailable())?;
        let label = lease.owner_window_label().to_owned();
        if leases.contains_key(&label) {
            return Err(preparation_unavailable());
        }
        leases.insert(label, lease);
        Ok(())
    }

    pub(crate) fn release(&self, label: &str) -> Result<(), ErrorDto> {
        let lease = self
            .leases
            .lock()
            .map_err(|_| preparation_unavailable())?
            .remove(label);
        drop(lease);
        Ok(())
    }
}

fn preparation_unavailable() -> ErrorDto {
    to_error_dto(&AppError::new(
        "UPDATER_PREPARATION_FAILED",
        ErrorKind::Internal,
        "更新安装准备未完成，请重新打开应用后重试",
        false,
    ))
}

pub async fn run_app_update_prepare(state: &AppState, window_label: &str) -> Result<(), ErrorDto> {
    let Some(mut lease) = state.session_registry.begin_window_close(window_label) else {
        return Err(to_error_dto(&AppError::new(
            "UPDATER_BUSY",
            ErrorKind::Conflict,
            "应用正在关闭，请稍后重试更新",
            true,
        )));
    };
    state.video_screenshot.cancel_owner(window_label);
    let closed = lease.take_closed_sessions();
    crate::revoke_then_record(state, lease.owner_window_label(), &closed).await;
    state.update_preparation.retain(lease)
}

#[tauri::command]
pub fn app_update_cancel<R: tauri::Runtime>(
    state: State<'_, AppState>,
    window: tauri::Window<R>,
) -> Result<(), ErrorDto> {
    state.update_preparation.release(window.label())
}

#[tauri::command]
pub async fn app_update_prepare<R: tauri::Runtime>(
    state: State<'_, AppState>,
    window: tauri::Window<R>,
) -> Result<(), ErrorDto> {
    let state = (*state.inner()).clone();
    let window_label = window.label().to_owned();
    run_blocking(move || async move { run_app_update_prepare(&state, &window_label).await }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_infrastructure::Db;
    use std::sync::Arc;

    #[tokio::test]
    async fn preparation_holds_the_barrier_until_the_failed_handoff_is_cancelled() {
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::try_new(Arc::new(
            Db::open(&directory.path().join("haven.db")).unwrap(),
        ))
        .unwrap();
        run_app_update_prepare(&state, "main").await.unwrap();
        assert!(state.session_registry.begin_window_close("main").is_none());
        let error = run_app_update_prepare(&state, "main").await.unwrap_err();
        assert_eq!(error.code, "UPDATER_BUSY");
        state.update_preparation.release("other-window").unwrap();
        assert!(state.session_registry.begin_window_close("main").is_none());
        state.update_preparation.release("main").unwrap();
        run_app_update_prepare(&state, "main").await.unwrap();
        state.update_preparation.release("main").unwrap();
        assert!(state.session_registry.begin_window_close("main").is_some());
    }

    #[tokio::test]
    async fn preparation_cannot_cut_a_concurrent_window_close_short() {
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::try_new(Arc::new(
            Db::open(&directory.path().join("haven.db")).unwrap(),
        ))
        .unwrap();
        let lease = state.session_registry.begin_window_close("main").unwrap();
        state.update_preparation.release("main").unwrap();
        assert!(state.session_registry.begin_window_close("main").is_none());
        let error = run_app_update_prepare(&state, "main").await.unwrap_err();
        assert_eq!(error.code, "UPDATER_BUSY");
        assert!(error.retryable);
        drop(lease);
        run_app_update_prepare(&state, "main").await.unwrap();
    }

    #[tokio::test]
    async fn sessions_cannot_open_between_preparation_and_native_installation() {
        use haven_application::services::{PreparedSession, PreparedSessionSource};
        use haven_application::wire::SessionEngineDto;
        use haven_domain::enums::{MediaType, ResourceType};
        use haven_domain::ids::ResourceId;
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::try_new(Arc::new(
            Db::open(&directory.path().join("haven.db")).unwrap(),
        ))
        .unwrap();
        let prepared = || PreparedSession {
            work_id: "w".into(),
            edition_id: "e".into(),
            media_item_id: "m".into(),
            engine: SessionEngineDto::Playback,
            resource_id: ResourceId::new(),
            storage_location_id: None,
            canonical_root: None,
            canonical_file: None,
            subtitle_tracks: vec![],
            source: PreparedSessionSource::Local,
            mime_type: None,
            media_type: MediaType::Movie,
            resource_type: ResourceType::LocalFile,
            comic_pages: None,
            progress: None,
        };
        run_app_update_prepare(&state, "main").await.unwrap();
        assert!(state
            .session_registry
            .register(prepared(), "main".into(), "main".into())
            .is_err());
        state.update_preparation.release("main").unwrap();
        assert!(state
            .session_registry
            .register(prepared(), "main".into(), "main".into())
            .is_ok());
    }
}
