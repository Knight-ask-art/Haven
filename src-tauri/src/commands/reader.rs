//! Reader commands. TOC facts are extracted server-side from the Session's
//! registered file; paths and archive entries never cross the IPC boundary.

use tauri::{Runtime, State, Webview};

use haven_application::wire::{
    ErrorDto, ReaderSearchCancelRequest, ReaderSearchCancelResultDto, ReaderSearchEvent,
    ReaderSearchRequest, ReaderSearchResultDto, ReaderTocGetRequest, ReaderTocResultDto,
};
use tauri::ipc::Channel;

use crate::ipc::{invalid_id, run_blocking, to_error_dto};
use crate::state::AppState;

pub async fn run_reader_toc_get(
    state: &AppState,
    owner_webview_label: &str,
    request: ReaderTocGetRequest,
) -> Result<ReaderTocResultDto, ErrorDto> {
    let uuid = uuid::Uuid::parse_str(&request.session_id).map_err(|_| invalid_id())?;
    let session_id = uuid.to_string();
    if session_id != request.session_id {
        return Err(invalid_id());
    }
    let prepared = state
        .session_registry
        .lookup_for_owner(&session_id, owner_webview_label)
        .map_err(|error| to_error_dto(&error))?;
    let items = state
        .reader_toc
        .toc(&prepared)
        .map_err(|error| to_error_dto(&error))?;
    Ok(ReaderTocResultDto {
        schema_version: 1,
        session_id,
        items,
    })
}

#[tauri::command]
pub async fn reader_toc_get<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, AppState>,
    request: ReaderTocGetRequest,
) -> Result<ReaderTocResultDto, ErrorDto> {
    let owner_webview_label = webview.label().to_owned();
    let state = (*state.inner()).clone();
    run_blocking(
        move || async move { run_reader_toc_get(&state, &owner_webview_label, request).await },
    )
    .await
}

pub async fn run_reader_search(
    state: &AppState,
    owner_webview_label: &str,
    request: ReaderSearchRequest,
) -> Result<ReaderSearchResultDto, ErrorDto> {
    let uuid = uuid::Uuid::parse_str(&request.session_id).map_err(|_| invalid_id())?;
    let session_id = uuid.to_string();
    if session_id != request.session_id {
        return Err(invalid_id());
    }
    if request.query.trim().is_empty() || request.query.chars().count() > 128 {
        return Err(ErrorDto {
            code: "INVALID_ARGUMENT".into(),
            user_message: "检索关键词无效".into(),
            retryable: false,
        });
    }
    let prepared = state
        .session_registry
        .lookup_for_owner(&session_id, owner_webview_label)
        .map_err(|error| to_error_dto(&error))?;
    let hits = state
        .reader_search
        .search(&prepared, &request.query)
        .map_err(|error| to_error_dto(&error))?;
    Ok(ReaderSearchResultDto {
        schema_version: 1,
        session_id,
        hits,
    })
}

#[tauri::command]
pub async fn reader_search<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, AppState>,
    request: ReaderSearchRequest,
) -> Result<ReaderSearchResultDto, ErrorDto> {
    let owner_webview_label = webview.label().to_owned();
    let state = (*state.inner()).clone();
    run_blocking(
        move || async move { run_reader_search(&state, &owner_webview_label, request).await },
    )
    .await
}

pub async fn run_reader_search_start(
    state: &AppState,
    owner_webview_label: &str,
    request: ReaderSearchRequest,
    on_event: Channel<ReaderSearchEvent>,
) -> Result<ReaderSearchResultDto, ErrorDto> {
    let uuid = uuid::Uuid::parse_str(&request.session_id).map_err(|_| invalid_id())?;
    let session_id = uuid.to_string();
    if session_id != request.session_id {
        return Err(invalid_id());
    }
    if request.query.trim().is_empty() || request.query.chars().count() > 128 {
        return Err(ErrorDto {
            code: "INVALID_ARGUMENT".into(),
            user_message: "检索关键词无效".into(),
            retryable: false,
        });
    }
    let prepared = state
        .session_registry
        .lookup_for_owner(&session_id, owner_webview_label)
        .map_err(|error| to_error_dto(&error))?;
    let operation_id = uuid::Uuid::new_v4().to_string();
    let route = state.reader_search_sink.bind(on_event);
    route.assign_operation(operation_id.clone());
    // Synchronously emit events via sink (fast, in-memory search)
    let sink = state.reader_search_sink.clone();
    let _ = state.reader_search.search_with_events(
        &prepared,
        &request.query,
        &operation_id,
        sink.as_ref() as &dyn haven_application::services::reader_search::ReaderSearchEventSink,
    );
    let hits = state
        .reader_search
        .search(&prepared, &request.query)
        .map_err(|error| to_error_dto(&error))?;
    state.reader_search_sink.unbind(&route);
    Ok(ReaderSearchResultDto {
        schema_version: 1,
        session_id,
        hits,
    })
}

#[tauri::command]
pub async fn reader_search_start<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, AppState>,
    request: ReaderSearchRequest,
    on_event: Channel<ReaderSearchEvent>,
) -> Result<ReaderSearchResultDto, ErrorDto> {
    let owner_webview_label = webview.label().to_owned();
    let state = (*state.inner()).clone();
    run_blocking(move || async move {
        run_reader_search_start(&state, &owner_webview_label, request, on_event).await
    })
    .await
}

pub fn run_reader_search_cancel(
    _state: &AppState,
    request: ReaderSearchCancelRequest,
) -> Result<ReaderSearchCancelResultDto, ErrorDto> {
    // Synchronous search completes immediately, so cancel is always alreadyTerminal
    Ok(ReaderSearchCancelResultDto {
        operation_id: request.operation_id,
        already_terminal: true,
    })
}

#[tauri::command]
pub async fn reader_search_cancel(
    state: State<'_, AppState>,
    request: ReaderSearchCancelRequest,
) -> Result<ReaderSearchCancelResultDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_reader_search_cancel(&state, request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_infrastructure::Db;
    use std::sync::Arc;

    #[tokio::test]
    async fn invalid_session_id_is_rejected_before_registry_lookup() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let error = run_reader_toc_get(
            &state,
            "main",
            ReaderTocGetRequest {
                session_id: "NOT-A-CANONICAL-ID".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "INVALID_ID");
        assert!(!error.retryable);
    }

    #[tokio::test]
    async fn unknown_session_is_resource_not_found() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let error = run_reader_toc_get(
            &state,
            "main",
            ReaderTocGetRequest {
                session_id: "11111111-1111-4111-8111-111111111111".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "RESOURCE_NOT_FOUND");
        assert!(!error.retryable);
    }
}
