//! 共享 Fixture 装载验证（C-07：Rust 端）。
//!
//! 每个 fixture JSON 必须能被对应 Wire DTO 反序列化——与 TS/Mock Client
//! 共用同一消费基线。路径基准：仓库根 `contracts/ipc/v1/fixtures/`。

use haven_application::wire::{
    AvailabilityDto, ComicPageAvailabilityDto, ComicPageManifestDto, CredentialDeleteRequest,
    CredentialProviderDto, CredentialSetRequest, CredentialStatusDto, CredentialStatusRequest,
    EnrichmentStateDto, EnrichmentStatusRequest, EnrichmentStatusWire, ErrorDto,
    ExternalIdProviderDto, FavoriteChangedDto, FavoriteSetRequest, FavoriteSetResult, LabelHint,
    LibraryChangedDto, LibraryScanEvent, LibraryShelvesDto, LocatorKindDto, MediaStateDto,
    MetadataChangedDto, PageDto, ReaderTocResultDto, ResourceListDto, ResourceTypeDto,
    ScanCancelResultDto, ScanPhase, ScanStartResult, SearchSourceCancelResultDto,
    SearchSourceEvent, SearchSourceEventKind, SearchSourceStartRequest, SearchStartResultDto,
    SourceCategoryDto, SourceKindDto, SourceModeDto, SourceRegistryDto, SourceRegistrySetRequest,
    SourceRegistrySetResult, StorageLocationDto, StorageProviderTypeDto, StorageStatusDto,
    StreamKindDto, WorkCardDto,
};

fn load(name: &str) -> String {
    let base = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../contracts/ipc/v1/fixtures/"
    );
    std::fs::read_to_string(format!("{base}{name}"))
        .unwrap_or_else(|e| panic!("fixture {name} 缺失: {e}"))
}

#[test]
fn library_list_normal_loads() {
    let json = load("library/list.normal.json");
    let page: PageDto<WorkCardDto> = serde_json::from_str(&json).unwrap();
    assert_eq!(page.schema_version, 1);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].work_id.len(), 36);
    assert_eq!(page.items[0].title, "三体");
    assert!(page.items[0].favorite);
    let action = page.items[0].primary_action.as_ref().unwrap();
    assert_eq!(action.label_hint, LabelHint::Continue);
    assert!(page.items[0].progress.is_some());
}

#[test]
fn library_list_empty_loads() {
    let json = load("library/list.empty.json");
    let page: PageDto<WorkCardDto> = serde_json::from_str(&json).unwrap();
    assert_eq!(page.items.len(), 0);
    assert_eq!(page.total, Some(0));
    assert!(page.next_cursor.is_none());
}

#[test]
fn library_list_error_is_error_dto() {
    let json = load("library/list.error.json");
    let err: ErrorDto = serde_json::from_str(&json).unwrap();
    assert_eq!(err.code, "CURSOR_EXPIRED");
    assert!(!err.retryable);
}

#[test]
fn shelves_normal_loads() {
    let json = load("library/shelves.normal.json");
    let shelves: LibraryShelvesDto = serde_json::from_str(&json).unwrap();
    assert_eq!(shelves.schema_version, 1);
    assert_eq!(shelves.shelves.len(), 1);
    assert_eq!(shelves.shelves[0].shelf_id, "shelf-continue");
    assert!(shelves.shelves[0].preview.is_empty(), "空列表固定 []");
}

#[test]
fn favorite_set_request_loads() {
    let json = load("favorite/set.normal.json");
    let req: FavoriteSetRequest = serde_json::from_str(&json).unwrap();
    assert!(req.favorite);
    assert_eq!(req.work_id.len(), 36);
}

#[test]
fn favorite_error_loads() {
    let json = load("favorite/set.error-work-not-found.json");
    let err: ErrorDto = serde_json::from_str(&json).unwrap();
    assert_eq!(err.code, "WORK_NOT_FOUND");
}

#[test]
fn scan_terminal_events_load() {
    for name in [
        "terminal.completed.json",
        "terminal.cancelled.json",
        "terminal.failed.json",
    ] {
        let json = load(&format!("scan/{name}"));
        let ev: LibraryScanEvent = serde_json::from_str(&json).unwrap();
        let terminal = matches!(
            ev.kind,
            haven_application::wire::ScanPhase::Completed
                | haven_application::wire::ScanPhase::Cancelled
                | haven_application::wire::ScanPhase::Failed
        );
        assert!(terminal, "{name} 必须是终态事件");
        assert!(ev.sequence > 0);
        assert!(ev.at.contains('T'), "{name} at 必须是 RFC3339");
    }
}

#[test]
fn error_catalog_is_machine_readable() {
    let json = load("errors/catalog.json");
    let catalog: serde_json::Value = serde_json::from_str(&json).unwrap();
    let codes = catalog["codes"].as_array().unwrap();
    assert!(codes.len() >= 24, "catalog 必须覆盖 v1 Error Catalog");
    let names: Vec<&str> = codes.iter().map(|c| c["code"].as_str().unwrap()).collect();
    for required in [
        "INVALID_CURSOR",
        "CURSOR_EXPIRED",
        "REVISION_CONFLICT",
        "LOCATOR_KIND_INCOMPATIBLE",
        "CREDENTIAL_ACCESS_FAILED",
        "DATABASE_ERROR",
    ] {
        assert!(names.contains(&required), "catalog 缺少 {required}");
    }
}

#[test]
fn missing_artwork_list_loads_with_nulls() {
    let json = load("library/list.missing-artwork.json");
    let page: PageDto<WorkCardDto> = serde_json::from_str(&json).unwrap();
    let card = &page.items[0];
    assert!(card.poster_uri.is_none(), "缺图必须显式 null");
    assert!(card.backdrop_uri.is_none());
    assert!(card.primary_action.is_none());
    assert!(card.progress.is_none());
}

#[test]
fn favorite_success_and_repeat_load() {
    let success: FavoriteSetResult =
        serde_json::from_str(&load("favorite/set.success.json")).unwrap();
    assert_eq!(success.work_id.len(), 36);
    assert!(success.favorite);
    assert!(success.revision.is_some(), "状态变更后必须带非空 revision");

    let repeated: FavoriteSetRequest =
        serde_json::from_str(&load("favorite/set.repeated-idempotent.json")).unwrap();
    assert!(repeated.favorite);
    assert_eq!(repeated.work_id, success.work_id, "重复提交同一目标");
}

#[test]
fn favorite_first_false_has_null_revision() {
    // R-FAV-002：首次 favorite=false（从未变更）→ revision=null。
    let first_false: FavoriteSetResult =
        serde_json::from_str(&load("favorite/set.first-false.json")).unwrap();
    assert!(!first_false.favorite);
    assert!(first_false.revision.is_none(), "无版本历史 → revision=null");
    assert_eq!(first_false.work_id.len(), 36);
}

#[test]
fn scan_warning_and_already_running_load() {
    let warning: LibraryScanEvent = serde_json::from_str(&load("scan/warning.json")).unwrap();
    assert_eq!(
        warning.kind,
        haven_application::wire::ScanPhase::Warning,
        "warning 不是终态，可继续"
    );

    let already: ScanStartResult =
        serde_json::from_str(&load("scan/already-running.json")).unwrap();
    assert_eq!(already.schema_version, 1);
    assert!(already.already_running, "R-C02：幂等返回既有任务");
    assert!(!already.task_id.is_empty());
    assert!(!already.operation_id.is_empty());
}

#[test]
fn storage_list_and_scan_cancel_fixtures_load() {
    let locations: Vec<StorageLocationDto> =
        serde_json::from_str(&load("storage/list.normal.json")).unwrap();
    assert_eq!(locations.len(), 1);
    assert_eq!(
        locations[0].location_id,
        "11111111-1111-4111-8111-111111111111"
    );
    assert_eq!(locations[0].display_name, "本地媒体库");
    assert_eq!(locations[0].provider_type, StorageProviderTypeDto::Local);
    assert_eq!(locations[0].status, StorageStatusDto::Connected);
    let serialized = serde_json::to_value(&locations[0]).expect("StorageLocationDto 可序列化");
    let fields = serialized
        .as_object()
        .expect("StorageLocationDto 必须序列化为对象");
    assert_eq!(fields.len(), 4, "存储位置公开 DTO 必须是四字段白名单");
    for field in ["locationId", "displayName", "providerType", "status"] {
        assert!(fields.contains_key(field), "缺少公开字段 {field}");
    }
    for sensitive in [
        "rootPath",
        "rootRef",
        "root_ref",
        "credentialRef",
        "credential_ref",
    ] {
        assert!(
            !fields.contains_key(sensitive),
            "存储位置 DTO 不得包含敏感字段 {sensitive}"
        );
    }

    let empty: Vec<StorageLocationDto> =
        serde_json::from_str(&load("storage/list.empty.json")).unwrap();
    assert!(empty.is_empty());

    let accepted: ScanCancelResultDto =
        serde_json::from_str(&load("scan/cancel.accepted.json")).unwrap();
    assert!(!accepted.already_terminal);
    assert_eq!(accepted.phase, ScanPhase::Cancelled);

    let terminal: ScanCancelResultDto =
        serde_json::from_str(&load("scan/cancel.terminal.json")).unwrap();
    assert!(terminal.already_terminal);
    assert_eq!(terminal.phase, ScanPhase::Completed);
}

#[test]
fn change_events_load() {
    let library: LibraryChangedDto =
        serde_json::from_str(&load("events/library.changed.json")).unwrap();
    assert_eq!(library.schema_version, 1);
    assert!(
        library.revision.is_some(),
        "revision 用于缓存失效（null=全量刷新）"
    );
    assert!(library.sequence > 0);

    let favorite: FavoriteChangedDto =
        serde_json::from_str(&load("events/favorite.changed.json")).unwrap();
    assert_eq!(favorite.schema_version, 1);
    assert!(favorite.favorite);
    assert_eq!(favorite.work_id.len(), 36);
    // R-FAV-001：事件 revision 与 FavoriteSetResult 同源且恒非空。
    assert!(
        !favorite.revision.is_empty(),
        "favorite.changed 必须携带非空 revision"
    );
    let success: FavoriteSetResult =
        serde_json::from_str(&load("favorite/set.success.json")).unwrap();
    assert_eq!(
        Some(favorite.revision.as_str()),
        success.revision.as_deref(),
        "事件与 Mutation Result 使用同一状态版本"
    );
}

#[test]
fn resource_mixed_availability_loads() {
    let result: ResourceListDto =
        serde_json::from_str(&load("resource/list.mixed-availability.json")).unwrap();
    assert_eq!(result.schema_version, 1);
    assert_eq!(result.items.len(), 2);
    assert_eq!(result.items[0].resource_type, ResourceTypeDto::LocalFile);
    assert_eq!(result.items[0].availability, AvailabilityDto::Available);
    assert!(result.items[0].is_local);
    assert_eq!(
        result.items[1].availability,
        AvailabilityDto::SourceUnavailable
    );
    assert!(!result.items[1].requires_reauthorization);
}

#[test]
fn comic_page_manifest_loads_without_resource_locator_or_path() {
    let result: ComicPageManifestDto =
        serde_json::from_str(&load("comic/page-manifest.normal.json")).unwrap();
    assert_eq!(result.schema_version, 1);
    assert_eq!(result.page_count, 2);
    assert_eq!(result.pages.len(), 2);
    assert_eq!(result.pages[0].page_index, 0);
    assert_eq!(
        result.pages[0].availability,
        ComicPageAvailabilityDto::Ready
    );
    assert!(
        result.pages[0]
            .content_uri
            .as_deref()
            .unwrap()
            .starts_with("haven-resource://comic-page/")
    );

    let value = serde_json::to_value(result).unwrap();
    let serialized = value.to_string();
    for forbidden in [
        "rootPath",
        "rootRef",
        "locator",
        "credentialRef",
        "signedUrl",
        "mimeType",
        "byteLength",
    ] {
        assert!(
            !serialized.contains(forbidden),
            "漫画清单不得包含 {forbidden}"
        );
    }

    let empty: ComicPageManifestDto =
        serde_json::from_str(&load("comic/page-manifest.empty.json")).unwrap();
    assert_eq!(empty.page_count, 0);
    assert!(empty.pages.is_empty());

    let partial: ComicPageManifestDto =
        serde_json::from_str(&load("comic/page-manifest.partial-unavailable.json")).unwrap();
    assert_eq!(partial.page_count, 3);
    assert_eq!(partial.pages[1].page_index, 1);
    assert_eq!(
        partial.pages[1].availability,
        ComicPageAvailabilityDto::Unavailable
    );
    assert!(partial.pages[1].content_uri.is_none());
    assert_eq!(partial.pages[2].page_index, 2);
}

#[test]
fn reader_toc_fixture_loads_with_stable_item_shapes() {
    let result: ReaderTocResultDto = serde_json::from_str(&load("reader/toc.normal.json")).unwrap();
    assert_eq!(result.schema_version, 1);
    assert_eq!(result.session_id, "11111111-1111-4111-8111-111111111111");
    assert_eq!(result.items.len(), 5);
    assert_eq!(result.items[0].title, "序言");
    assert_eq!(result.items[0].depth, 0);
    assert_eq!(result.items[2].depth, 1);
    assert!(
        result
            .items
            .iter()
            .all(|item| { item.id.len() == 16 && item.id.chars().all(|c| c.is_ascii_hexdigit()) })
    );
    assert!(
        result
            .items
            .iter()
            .all(|item| (0.0..=1.0).contains(&item.progression))
    );

    let empty: ReaderTocResultDto = serde_json::from_str(&load("reader/toc.empty.json")).unwrap();
    assert_eq!(empty.schema_version, 1);
    assert!(empty.items.is_empty(), "空目录固定 []");
}

// ---------- v0.2 契约冻结（契约 §36；CONTRACT-V02-*） ----------

#[test]
fn source_registry_fixtures_load() {
    let registry: SourceRegistryDto =
        serde_json::from_str(&load("source/registry.normal.json")).unwrap();
    assert_eq!(
        registry.schema_version, 2,
        "来源注册表 schemaVersion 固定为 2"
    );
    assert!(!registry.sources.is_empty());
    for category in [
        SourceCategoryDto::Video,
        SourceCategoryDto::Book,
        SourceCategoryDto::Comic,
        SourceCategoryDto::Periodical,
    ] {
        assert!(
            registry
                .sources
                .iter()
                .filter(|source| source.categories.contains(&category))
                .count()
                >= 3,
            "每个内容分类至少需要三个可搜索来源: {category:?}"
        );
    }
    assert!(
        registry
            .sources
            .iter()
            .all(|source| source.source_id != "tmdb"),
        "需要 API Key 且未接入 Provider 的 TMDB 不得出现在内置目录"
    );
    let cms10 = registry
        .sources
        .iter()
        .find(|s| s.source_id == "cms10")
        .expect("fixture 必须包含 cms10");
    assert!(cms10.kinds.contains(&SourceKindDto::Stream));
    assert_eq!(cms10.mode, SourceModeDto::Collection);
    assert!(cms10.categories.contains(&SourceCategoryDto::Video));
    assert!(!cms10.notes.is_empty());
    assert!(cms10.endpoint_configured, "端点已配置仅是布尔投影");
    let archive = registry
        .sources
        .iter()
        .find(|s| s.source_id == "archive")
        .expect("fixture 必须包含 Internet Archive metadata Provider");
    assert!(archive.kinds.contains(&SourceKindDto::Metadata));
    assert_eq!(archive.mode, SourceModeDto::Single);
    assert!(!archive.endpoint_configured, "固定 API 不依赖用户端点");

    let request: SourceRegistrySetRequest =
        serde_json::from_str(&load("source/set.request.json")).unwrap();
    assert!(request.enabled);

    let result: SourceRegistrySetResult =
        serde_json::from_str(&load("source/set.success.json")).unwrap();
    assert_eq!(result.source_id, request.source_id);
    assert_eq!(result.enabled, request.enabled, "幂等设置返回同值");

    let err: ErrorDto =
        serde_json::from_str(&load("source/set.error-unknown-source.json")).unwrap();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    assert!(!err.retryable);
}

#[test]
fn search_source_channel_fixtures_load() {
    let request: SearchSourceStartRequest =
        serde_json::from_str(&load("search/start.request.json")).unwrap();
    assert_eq!(request.query, "庆余年");
    assert!(request.category.is_none());
    assert!(request.limit_per_source.is_none());

    let fresh: SearchStartResultDto =
        serde_json::from_str(&load("search/start.success.json")).unwrap();
    assert!(!fresh.already_running);
    let already: SearchStartResultDto =
        serde_json::from_str(&load("search/start.already-running.json")).unwrap();
    assert!(already.already_running, "R-C02 同款幂等语义");
    assert_eq!(already.operation_id, fresh.operation_id);

    let started: SearchSourceEvent =
        serde_json::from_str(&load("search/source.started.json")).unwrap();
    assert_eq!(started.kind, SearchSourceEventKind::Started);
    assert_eq!(started.sequence, 1);
    assert!(started.data.source_id.is_none());

    let result: SearchSourceEvent =
        serde_json::from_str(&load("search/source.source-result.json")).unwrap();
    assert_eq!(result.kind, SearchSourceEventKind::SourceResult);
    let source_id = result.data.source_id.as_deref().expect("必须携带 sourceId");
    assert_eq!(source_id, "bangumi");
    assert_eq!(result.data.works.len(), 1);
    let card = &result.data.works[0];
    assert!(
        card.external_ids
            .iter()
            .any(|id| id.provider == ExternalIdProviderDto::Bangumi),
        "Source 候选必须携带 external_ids 去重键"
    );

    let warning: SearchSourceEvent =
        serde_json::from_str(&load("search/source.warning.json")).unwrap();
    assert_eq!(warning.kind, SearchSourceEventKind::Warning);
    assert_eq!(
        warning.data.code.as_deref(),
        Some("SOURCE_UNAVAILABLE"),
        "局部失败使用稳定错误码"
    );
    assert!(warning.data.works.is_empty());

    for (name, kind) in [
        (
            "search/source.completed.json",
            SearchSourceEventKind::Completed,
        ),
        (
            "search/source.cancelled.json",
            SearchSourceEventKind::Cancelled,
        ),
        ("search/source.failed.json", SearchSourceEventKind::Failed),
    ] {
        let event: SearchSourceEvent = serde_json::from_str(&load(name)).unwrap();
        assert_eq!(event.kind, kind, "{name} 终态种类");
        assert!(event.sequence > 0);
        assert!(event.at.contains('T'), "{name} at 必须是 RFC3339");
    }

    let accepted: SearchSourceCancelResultDto =
        serde_json::from_str(&load("search/cancel.accepted.json")).unwrap();
    assert!(!accepted.already_terminal);
    let terminal: SearchSourceCancelResultDto =
        serde_json::from_str(&load("search/cancel.terminal.json")).unwrap();
    assert!(terminal.already_terminal);
}

#[test]
fn credential_profile_fixtures_load_without_secret_leakage() {
    let configured: CredentialStatusDto =
        serde_json::from_str(&load("credential/status.configured.json")).unwrap();
    assert!(configured.configured);
    let not_configured: CredentialStatusDto =
        serde_json::from_str(&load("credential/status.not-configured.json")).unwrap();
    assert!(!not_configured.configured);
    assert!(not_configured.updated_at.is_none());

    // 状态响应形状不得出现 secret/credentialRef/target 字段。
    let serialized = serde_json::to_string(&configured).unwrap();
    for forbidden in ["secret", "credentialRef", "target", "password"] {
        assert!(!serialized.contains(forbidden), "状态投影禁止 {forbidden}");
    }

    let status_request: CredentialStatusRequest = serde_json::from_str(
        &serde_json::json!({"provider": "webdav", "profileId": null}).to_string(),
    )
    .unwrap();
    assert_eq!(status_request.provider, CredentialProviderDto::Webdav);
    assert!(status_request.profile_id.is_none(), "null = 默认 profile");

    let set_request: CredentialSetRequest =
        serde_json::from_str(&load("credential/set.request.json")).unwrap();
    assert_eq!(set_request.secret, "fixture-not-a-real-secret");

    let delete_request: CredentialDeleteRequest =
        serde_json::from_str(&load("credential/delete.request.json")).unwrap();
    assert_eq!(delete_request.provider, CredentialProviderDto::Webdav);
}

#[test]
fn media_state_fixture_loads_with_reserved_rating_null() {
    let state: MediaStateDto =
        serde_json::from_str(&load("media-state/state.normal.json")).unwrap();
    assert_eq!(
        state.schema_version, 2,
        "UserMediaState schemaVersion 固定为 2"
    );
    assert!(state.favorite);
    let progress = state.progress.as_ref().expect("normal 必须有进度");
    assert_eq!(progress.locator_kind, LocatorKindDto::Book);
    let history = state
        .history_summary
        .as_ref()
        .expect("normal 必须有历史摘要");
    assert_eq!(history.open_count, 7);
    assert_eq!(state.marker_count, 3);
}

#[test]
fn enrichment_and_metadata_changed_fixtures_load() {
    let request: EnrichmentStatusRequest =
        serde_json::from_str(&load("enrichment/status.request.json")).unwrap();
    assert!(request.work_id.is_none(), "null = 全部记录");

    let pending: EnrichmentStateDto =
        serde_json::from_str(&load("enrichment/state.pending.json")).unwrap();
    assert_eq!(pending.status, EnrichmentStatusWire::Pending);
    assert!(pending.source_id.is_none());
    let enriched: EnrichmentStateDto =
        serde_json::from_str(&load("enrichment/state.enriched.json")).unwrap();
    assert_eq!(enriched.status, EnrichmentStatusWire::Enriched);
    assert_eq!(enriched.source_id.as_deref(), Some("gutenberg"));

    let event: MetadataChangedDto =
        serde_json::from_str(&load("events/metadata.changed.json")).unwrap();
    assert_eq!(event.schema_version, 1);
    assert_eq!(event.status, EnrichmentStatusWire::Enriched);
    assert_eq!(event.work_id, enriched.work_id, "事件与记录同源 workId");
}

#[test]
fn source_runtime_v2b_fixtures_load() {
    // V2-B 实战批次：端点配置与候选导入（端点本身不得出现在结果投影中）。
    let request: haven_application::wire::SourceEndpointSetRequest =
        serde_json::from_str(&load("source/endpoint-set.request.json")).unwrap();
    assert_eq!(request.source_id, "cms10");
    assert!(request.endpoint.starts_with("https://"));

    let result: haven_application::wire::SourceEndpointSetResult =
        serde_json::from_str(&load("source/endpoint-set.result.json")).unwrap();
    assert!(result.endpoint_configured);
    let serialized = serde_json::to_string(&result).unwrap();
    assert!(
        !serialized.contains("api.php"),
        "端点设置结果投影不得回传端点地址"
    );

    let import: haven_application::wire::SourceWorkImportRequest =
        serde_json::from_str(&load("source/import.request.json")).unwrap();
    assert_eq!(import.index, 0);

    let imported: haven_application::wire::SourceWorkImportResult =
        serde_json::from_str(&load("source/import.result.json")).unwrap();
    assert_eq!(imported.schema_version, 1);
    assert_eq!(imported.work_id.len(), 36);
    assert_eq!(imported.media_item_id.len(), 36);
}

#[test]
fn remote_stream_resource_list_loads() {
    let list: ResourceListDto =
        serde_json::from_str(&load("resource/list.remote-stream.json")).unwrap();
    assert_eq!(list.items.len(), 2);
    assert_eq!(list.items[0].resource_type, ResourceTypeDto::RemoteStream);
    assert_eq!(list.items[0].stream_kind, Some(StreamKindDto::Hls));
    assert_eq!(list.items[1].stream_kind, Some(StreamKindDto::Direct));
    let serialized = serde_json::to_string(&list).unwrap();
    for forbidden in ["http://", "https://", "m3u8?", ".m3u8", "signedUrl"] {
        assert!(
            !serialized.contains(forbidden),
            "在线流投影不得携带原始 URL 片段 {forbidden}"
        );
    }
}
