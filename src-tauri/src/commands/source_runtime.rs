//! 来源运行时命令（V2-B 实战批次；契约 §36.2/§36.3/§36.4 演进）。
//!
//! - `source_registry_set_endpoint`：端点只写后端持久化，响应不含端点本身。
//! - `source_work_import`：候选入库（幂等），返回真实 Work/MediaItem 身份。
//! - `stream_open` / `stream_close`：远端流受控代理会话（grant 模型）。

use tauri::State;

use haven_application::wire::{
    ErrorDto, SessionOpenRequest, SessionOpenResultDto, SourceEndpointSetRequest,
    SourceEndpointSetResult, SourceWorkImportRequest, SourceWorkImportResult, StreamKindDto,
};
use haven_domain::ids::WorkId;

use crate::ipc::{invalid_argument, run_blocking, to_error_dto, LIBRARY_CHANGED_TRANSPORT_EVENT};
use crate::state::AppState;

fn parse_optional_target_work_id(value: Option<&str>) -> Result<Option<WorkId>, ErrorDto> {
    let Some(value) = value else {
        return Ok(None);
    };
    let uuid = uuid::Uuid::parse_str(value).map_err(|_| crate::ipc::invalid_id())?;
    if uuid.to_string() != value {
        return Err(crate::ipc::invalid_id());
    }
    Ok(Some(WorkId::from_uuid(uuid)))
}

/// `source_registry_set_endpoint` 命令核心。
async fn run_source_endpoint_set(
    registry: &haven_application::services::SourceRegistryService,
    request: SourceEndpointSetRequest,
) -> Result<SourceEndpointSetResult, ErrorDto> {
    if request.source_id.trim().is_empty() || request.source_id.len() > 64 {
        return Err(invalid_argument("sourceId 非法"));
    }
    let configured = registry
        .set_endpoint(&request.source_id, &request.endpoint)
        .await
        .map_err(|e| to_error_dto(&e))?;
    Ok(SourceEndpointSetResult {
        source_id: request.source_id,
        endpoint_configured: configured,
    })
}

/// Tauri Command 薄包装。
#[tauri::command]
pub async fn source_registry_set_endpoint(
    state: State<'_, AppState>,
    request: SourceEndpointSetRequest,
) -> Result<SourceEndpointSetResult, ErrorDto> {
    let registry = state.source_registry.clone();
    run_blocking(move || async move { run_source_endpoint_set(&registry, request).await }).await
}

/// `source_work_import` 命令核心：定位候选 → 入库 → 发布 library.changed。
pub async fn run_source_work_import(
    state: &AppState,
    request: SourceWorkImportRequest,
) -> Result<(SourceWorkImportResult, bool), ErrorDto> {
    let target_work_id = parse_optional_target_work_id(request.target_work_id.as_deref())?;
    let Some(external_id) = state
        .search_source
        .candidate_external_id(&request.operation_id, request.index)
    else {
        return Err(ErrorDto {
            code: "RESOURCE_NOT_FOUND".into(),
            user_message: "搜索候选不存在或已过期".into(),
            retryable: false,
        });
    };
    let imported = state
        .source_import
        .import_candidate_into_work(&external_id, target_work_id)
        .await
        .map_err(|e| to_error_dto(&e))?;
    Ok((
        SourceWorkImportResult {
            schema_version: 1,
            work_id: imported.work_id.to_string(),
            media_item_id: imported.media_item_id.to_string(),
        },
        true,
    ))
}

/// Tauri Command 薄包装（SQLite 写入走 blocking worker；成功发布一次 library.changed）。
#[tauri::command]
pub async fn source_work_import<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    request: SourceWorkImportRequest,
) -> Result<SourceWorkImportResult, ErrorDto> {
    use tauri::Emitter;
    let state_clone = (*state.inner()).clone();
    let (result, changed) =
        run_blocking(move || async move { run_source_work_import(&state_clone, request).await })
            .await?;
    if changed {
        // 库失效通知（复用既有事件名；envelope 最小化）。
        let _ = app.emit(
            LIBRARY_CHANGED_TRANSPORT_EVENT,
            serde_json::json!({
                "schemaVersion": 1,
                "at": chrono::Utc::now().to_rfc3339(),
                "operationId": format!("import-{}", uuid::Uuid::new_v4()),
                "sequence": 1,
                "revision": serde_json::Value::Null,
            }),
        );
    }
    Ok(result)
}

/// `stream_open` 命令核心：解析远端流资源 → 注册 grant → 返回代理 URI。
pub fn run_stream_open(
    state: &AppState,
    owner_label: &str,
    request: SessionOpenRequest,
) -> Result<SessionOpenResultDto, haven_application::wire::ErrorDto> {
    // prepare 是异步 DB 操作；命令层用 block_on 收敛（协议处理器同模式）。
    let facts = tauri::async_runtime::block_on(state.stream.prepare(request))
        .map_err(|e| to_error_dto(&e))?;
    let grant = state
        .stream_registry
        .register(
            crate::stream_registry::StreamGrantFacts {
                work_id: facts.work_id.clone(),
                edition_id: facts.edition_id.clone(),
                media_item_id: facts.media_item_id.clone(),
                mime_type: facts.mime_type.clone(),
                is_hls: facts.is_hls,
                progress: facts.progress.clone(),
                upstream_url: facts.upstream_url.clone(),
            },
            &facts.upstream_url,
            owner_label,
        )
        .map_err(|e| to_error_dto(&e))?;
    // 历史足迹：远端流打开即记（幂等，同 mediaItem 只一条 last_active_at 刷新）。
    if let Ok(mid) = facts
        .media_item_id
        .parse::<haven_domain::ids::MediaItemId>()
    {
        let _ = tauri::async_runtime::block_on(state.history.record(mid));
    }
    Ok(SessionOpenResultDto {
        schema_version: 1,
        session_id: grant.to_string(),
        content_uri: Some(format!("haven-resource://stream/{grant}")),
        work_id: facts.work_id,
        edition_id: facts.edition_id,
        media_item_id: facts.media_item_id,
        engine: haven_application::wire::SessionEngineDto::Playback,
        // `prepare` already resolved the authoritative progress row.  Keep it
        // on the opening result so remote streams follow the same resume path
        // as local sessions; the grant registry also retains the same snapshot
        // for protocol-side revalidation.
        progress: facts.progress,
        stream_kind: Some(if facts.is_hls {
            StreamKindDto::Hls
        } else {
            StreamKindDto::Direct
        }),
        subtitle_tracks: None,
    })
}

/// `stream_open` 命令薄包装。
#[tauri::command]
pub async fn stream_open<R: tauri::Runtime>(
    webview: tauri::WebviewWindow<R>,
    state: State<'_, AppState>,
    request: SessionOpenRequest,
) -> Result<SessionOpenResultDto, ErrorDto> {
    let state = (*state.inner()).clone();
    let label = webview.label().to_owned();
    tauri::async_runtime::spawn_blocking(move || run_stream_open(&state, &label, request))
        .await
        .map_err(|_| ErrorDto {
            code: "INTERNAL_ERROR".into(),
            user_message: "后台任务执行失败".into(),
            retryable: false,
        })?
}

/// `stream_close` 幂等撤销。
#[tauri::command]
pub async fn stream_close<R: tauri::Runtime>(
    webview: tauri::WebviewWindow<R>,
    state: State<'_, AppState>,
    request: crate::ipc::StreamCloseRequest,
) -> Result<bool, ErrorDto> {
    let label = webview.label().to_owned();
    Ok(state.stream_registry.revoke(&request.session_id, &label))
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_application::services::ports::SourceImportPorts;
    use haven_application::services::search_source::{
        SearchEventSink, SearchSourceParticipant, SearchSourceService,
    };
    use haven_application::services::source_import::{
        RemoteContentRef, SourceCatalogEntry, SourceCatalogProvider, SourceImportService,
    };
    use haven_application::services::source_registry::SourceRegistryService;
    use haven_application::wire::{
        ContentCategory, MediaTypeDto, QueryCategory, SearchSourceEvent, SearchSourceStartRequest,
        WorkCardDto,
    };
    use haven_common::UtcMillis;
    use haven_domain::comic_catalog::{
        ComicChapterAvailability, ComicChapterCatalog, ComicChapterCatalogEntry,
    };
    use haven_domain::comic_identity::{ChapterSourceIdentity, ComicChapterMetadata};
    use haven_domain::contracts::{EditionRepository, WorkRepository};
    use haven_domain::entities::{ArtworkSet, Edition, Work};
    use haven_domain::enums::{MediaType, WorkStatus, WorkType};
    use haven_domain::ids::{EditionId, WorkId};
    use std::sync::Arc;
    use std::{future::Future, pin::Pin};

    struct CandidateParticipant {
        handle: String,
    }

    impl SearchSourceParticipant for CandidateParticipant {
        fn source_id(&self) -> &str {
            "mangadex"
        }

        fn search<'a, 'b, 'c, 'async_trait>(
            &'a self,
            _query: &'b str,
            _limit: u32,
            _is_cancelled: &'c (dyn Fn() -> bool + Send + Sync),
        ) -> Pin<
            Box<
                dyn Future<Output = Result<Vec<WorkCardDto>, haven_common::AppError>>
                    + Send
                    + 'async_trait,
            >,
        >
        where
            'a: 'async_trait,
            'b: 'async_trait,
            'c: 'async_trait,
            Self: 'async_trait,
        {
            let work_id = self.handle.clone();
            Box::pin(async move {
                Ok(vec![WorkCardDto {
                    work_id,
                    title: "候选漫画".into(),
                    original_title: None,
                    description: None,
                    categories: vec![ContentCategory::Comic],
                    available_media_types: vec![MediaTypeDto::Comic],
                    poster_uri: None,
                    backdrop_uri: None,
                    release_year: None,
                    rating_value: None,
                    rating_scale: None,
                    favorite: false,
                    progress: None,
                    primary_action: None,
                    external_ids: Vec::new(),
                }])
            })
        }
    }

    struct NoopSearchSink;

    impl SearchEventSink for NoopSearchSink {
        fn emit_search_event(&self, _event: SearchSourceEvent) {}
    }

    struct CandidateCatalogProvider {
        entry: SourceCatalogEntry,
    }

    impl SourceCatalogProvider for CandidateCatalogProvider {
        fn detail<'a, 'b, 'c, 'd, 'async_trait>(
            &'a self,
            _source_id: &'b str,
            _endpoint: &'c str,
            _external_id: &'d str,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<SourceCatalogEntry, haven_common::AppError>>
                    + Send
                    + 'async_trait,
            >,
        >
        where
            'a: 'async_trait,
            'b: 'async_trait,
            'c: 'async_trait,
            'd: 'async_trait,
            Self: 'async_trait,
        {
            let entry = self.entry.clone();
            Box::pin(async move { Ok(entry) })
        }
    }

    async fn seed_target_comic_work(state: &AppState) -> WorkId {
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let now = UtcMillis(1);
        state
            .repos
            .work
            .save(&Work {
                id: work_id,
                canonical_title: "已有漫画 Work".into(),
                original_title: None,
                sort_title: None,
                description: None,
                work_type: WorkType::Standalone,
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
        state
            .repos
            .edition
            .save(&Edition {
                id: edition_id,
                work_id,
                title: "已有漫画版本".into(),
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
        work_id
    }

    #[test]
    fn source_work_import_target_requires_a_canonical_local_uuid() {
        assert_eq!(parse_optional_target_work_id(None).unwrap(), None);
        let canonical = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        let parsed = parse_optional_target_work_id(Some(canonical)).unwrap();
        assert_eq!(parsed.map(|id| id.to_string()), Some(canonical.to_owned()));

        for value in [
            "not-a-uuid",
            "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA",
            "https://example.invalid/work",
            "",
        ] {
            let error = parse_optional_target_work_id(Some(value)).unwrap_err();
            assert_eq!(error.code, "INVALID_ID", "accepted unsafe target: {value}");
        }
    }

    #[tokio::test]
    async fn source_work_import_target_rejects_unknown_candidate_without_side_effect() {
        let state = AppState::new(Arc::new(
            haven_infrastructure::Db::open_in_memory().unwrap(),
        ));
        let error = run_source_work_import(
            &state,
            SourceWorkImportRequest {
                operation_id: "operation-not-found".into(),
                index: 0,
                target_work_id: Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into()),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "RESOURCE_NOT_FOUND");
    }

    #[tokio::test]
    async fn source_work_import_target_uses_cached_candidate_and_binds_to_local_work() {
        let mut state = AppState::new(Arc::new(
            haven_infrastructure::Db::open_in_memory().unwrap(),
        ));
        let target_work_id = seed_target_comic_work(&state).await;
        let remote_work_id = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
        let remote_chapter_id = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
        let catalog = ComicChapterCatalog::new(
            "mangadex",
            remote_work_id,
            vec![ComicChapterCatalogEntry {
                identity: ChapterSourceIdentity::new("mangadex", remote_work_id, remote_chapter_id)
                    .unwrap(),
                metadata: ComicChapterMetadata {
                    chapter_number: Some(1.0),
                    title: Some("第一话".into()),
                    page_count: Some(2),
                    ..Default::default()
                },
                availability: ComicChapterAvailability::Available,
                published_at: None,
                updated_at: None,
            }],
            UtcMillis(2),
        )
        .unwrap();
        let candidate_handle = format!("content-candidate-mangadex-{remote_work_id}");
        let entry = SourceCatalogEntry {
            external_id: remote_work_id.into(),
            title: "候选漫画详情".into(),
            year: Some(2026),
            type_name: None,
            pic: None,
            episodes: Vec::new(),
            content: None,
            director: None,
            actor: None,
            local_file: None,
            media_type: Some(MediaType::Comic),
            remote: Some(RemoteContentRef {
                source_key: "mangadex".into(),
                remote_id: format!("{remote_work_id}:{remote_chapter_id}"),
                media_type: MediaType::Comic,
                mime_type: Some("application/vnd.comicbook+zip".into()),
            }),
            comic_catalog: Some(catalog),
        };
        let import_ports: Arc<dyn SourceImportPorts> = state.repos.clone();
        let registry = SourceRegistryService::new(state.repos.clone());
        state.source_import = SourceImportService::new(
            import_ports,
            Arc::new(haven_infrastructure::db::uow::SqliteUnitOfWork::new(
                state.db.clone(),
            )),
            registry.clone(),
            Arc::new(CandidateCatalogProvider { entry }),
        );
        state.search_source = SearchSourceService::new(
            registry,
            vec![Arc::new(CandidateParticipant {
                handle: candidate_handle,
            })],
            Arc::new(NoopSearchSink),
        );

        let started = state
            .search_source
            .start(SearchSourceStartRequest {
                query: "候选漫画".into(),
                category: Some(QueryCategory::Comic),
                limit_per_source: Some(1),
            })
            .await
            .unwrap();
        let mut cached = false;
        for _ in 0..100 {
            if state
                .search_source
                .candidate_external_id(&started.operation_id, 0)
                .is_some()
            {
                cached = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(cached, "搜索候选必须先进入服务端缓存");

        let (result, changed) = run_source_work_import(
            &state,
            SourceWorkImportRequest {
                operation_id: started.operation_id,
                index: 0,
                target_work_id: Some(target_work_id.to_string()),
            },
        )
        .await
        .unwrap();
        assert!(changed);
        assert_eq!(result.work_id, target_work_id.to_string());
        assert_eq!(
            state
                .repos
                .id_for_source_ref("mangadex", remote_work_id)
                .await
                .unwrap(),
            Some(target_work_id)
        );
    }
}
