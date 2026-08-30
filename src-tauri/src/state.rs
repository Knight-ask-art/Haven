//! AppState：Composition Root 单一持有 DB、Repository 与 Application Services。
//!
//! 原则（IPC-TAURI-001A/B）：
//! - 所有 Command 只通过 `State<'_, AppState>` 访问共享 Services；
//!   禁止 Command 临时创建第二套 DB/Repository。
//! - DB 在 setup 阶段打开一次（应用数据目录），后续全部复用。

use std::path::PathBuf;
use std::sync::Arc;

/// 开箱即用书库根：用户「下载」目录下的 `栖阅/`（不把整个 Downloads 登记为扫描根）。
fn default_books_library_root(db: &haven_infrastructure::Db) -> PathBuf {
    let downloads = std::env::var_os("USERPROFILE")
        .map(|home| PathBuf::from(home).join("Downloads"))
        .filter(|p| p.is_dir())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Downloads")))
        .filter(|p| p.is_dir());
    let base = downloads.unwrap_or_else(|| {
        db.path()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(std::env::temp_dir)
    });
    base.join("栖阅")
}

use tauri::Emitter;

use haven_application::services::cache::CacheService;
use haven_application::services::cast::{CastGrantRegistry, CastService};
use haven_application::services::comic::ComicPageService;
use haven_application::services::credential_access::CredentialAccessService;
use haven_application::services::download::DownloadService;
use haven_application::services::download_batch::DownloadBatchService;
use haven_application::services::enrichment::EnrichmentService;
use haven_application::services::enrichment::MetadataChangedSink;
use haven_application::services::error_report::ErrorReportService;
use haven_application::services::favorite::FavoriteService;
use haven_application::services::history::HistoryService;
use haven_application::services::home::HomeService;
use haven_application::services::library::LibraryService;
use haven_application::services::marker::MarkerService;
use haven_application::services::ports::SourceImportPorts;
use haven_application::services::ports::SourceRegistryPorts;
use haven_application::services::progress::ProgressService;
use haven_application::services::reader_search::ReaderSearchService;
use haven_application::services::reader_toc::ReaderTocService;
use haven_application::services::resource::ResourceService;
use haven_application::services::resource_preferences::ResourcePreferenceService;
use haven_application::services::scan::ScanService;
use haven_application::services::search_history::SearchHistoryService;
use haven_application::services::search_source::SearchSourceParticipant;
use haven_application::services::search_source::SearchSourceService;
use haven_application::services::session::SessionService;
use haven_application::services::settings::SettingsService;
use haven_application::services::source_import::SourceImportService;
use haven_application::services::source_registry::SourceRegistryService;
use haven_application::services::storage_location::StorageLocationService;
use haven_application::services::stream::StreamService;
use haven_application::services::trending::{
    ArtworkCachePort, TrendingCachePort, TrendingProvider, TrendingService,
};
use haven_application::services::work::WorkService;
use haven_application::services::VideoScreenshotService;
use haven_infrastructure::app_info::LocalAppInfoProvider;
use haven_infrastructure::artwork_cache::ArtworkCache;
use haven_infrastructure::cast::{AxumCastMediaServer, SoapCastControl, SsdpMdnsDiscovery};
use haven_infrastructure::cms10::{Cms10CatalogProvider, Cms10Client, Cms10SearchParticipant};
use haven_infrastructure::comic::LocalComicPageProvider;
use haven_infrastructure::db::repos::{SqliteRepositories, SqliteSettingsUoW};
use haven_infrastructure::db::uow::{SqliteStorageUoW, SqliteUnitOfWork};
use haven_infrastructure::download::{LocalDownloadRunner, LocalOfflineResourceFiles};
use haven_infrastructure::epub::LocalEpubTocProvider;
use haven_infrastructure::error_report::LocalErrorReportProvider;
use haven_infrastructure::metadata_sources::{M3uSearchParticipant, MetadataClient};
use haven_infrastructure::reader_search::LocalReaderSearchProvider;
use haven_infrastructure::scanner::LocalLibraryScanner;
use haven_infrastructure::video_screenshot::LocalVideoScreenshotProvider;
use haven_infrastructure::Db;

use crate::download_sink::TauriDownloadEventSink;
use crate::reader_search_sink::TauriReaderSearchEventSink;
use crate::scan_sink::TauriScanEventSink;
use crate::search_sink::TauriSearchEventSink;
use crate::session_registry::SessionRegistry;
use crate::stream_registry::StreamRegistry;

/// 应用全局状态（Library/Favorite/Storage/Settings/Scan）。
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Db>,
    pub repos: Arc<SqliteRepositories>,
    pub library: LibraryService,
    pub favorite: FavoriteService,
    pub download: DownloadService,
    pub download_sink: Arc<TauriDownloadEventSink>,
    pub progress: ProgressService,
    pub storage_location: StorageLocationService,
    pub settings: SettingsService,
    pub resource_preferences: ResourcePreferenceService,
    pub search_history: SearchHistoryService,
    pub cache: CacheService,
    /// 扫描服务（BE-SCAN-001）：后台任务 + Channel 进度 + 协作取消。
    pub scan: ScanService,
    /// 扫描事件出口（每次 library_scan_start 绑定 Channel + app emitter）。
    pub scan_sink: Arc<TauriScanEventSink>,
    pub work: WorkService,
    pub resource: ResourceService,
    pub comic_pages: ComicPageService,
    pub session: SessionService,
    pub history: HistoryService,
    pub marker: MarkerService,
    pub home: HomeService,
    /// 来源注册表（契约 §36.2；V2-A 冻结批次）。
    pub source_registry: SourceRegistryService,
    /// 渐进式来源搜索（契约 §36.3；V2-B 起接入 CMS10 参与者）。
    pub search_source: SearchSourceService,
    /// 搜索事件出口（每次 search_source_start 绑定 Channel）。
    pub search_sink: Arc<TauriSearchEventSink>,
    /// 来源候选入库（V2-B 实战批次）。
    pub source_import: SourceImportService,
    /// 元数据自动流水线（契约 §36.8；V2-F 批次）。
    pub enrichment: EnrichmentService,
    /// `metadata.changed` 事件出口（流水线状态变更广播）。
    pub metadata_sink: Arc<TauriMetadataChangedSink>,
    /// 远端流播放会话（契约 §36.4 受控代理）。
    pub stream: StreamService,
    /// 流授权注册表（grant → 上游主机白名单）。
    pub stream_registry: Arc<StreamRegistry>,
    /// Provider Profile 凭据（契约 §36.5；WebDAV 前置）。
    pub credential_access: CredentialAccessService,
    pub trending: TrendingService,
    pub artwork_cache: Arc<ArtworkCache>,
    /// About / Diagnostics：只返回构建信息和固定目录的脱敏投影。
    pub app_info: haven_application::services::AppInfoService,
    /// 用户主动确认后生成的脱敏诊断报告。
    pub error_report: ErrorReportService,
    pub cast: CastService,
    pub cast_media: Arc<AxumCastMediaServer>,
    pub cast_grants: Arc<CastGrantRegistry>,
    pub(crate) session_registry: Arc<SessionRegistry>,
    /// 阅读目录（契约 §19.1 `reader_toc_get`；EPUB 专用）。
    pub reader_toc: ReaderTocService,
    /// 阅读全文检索（契约 §19.1 `reader_search`）。
    pub reader_search: ReaderSearchService,
    /// 阅读检索事件出口（每次 `reader_search_start` 绑定 Channel）。
    pub reader_search_sink: Arc<TauriReaderSearchEventSink>,
    /// 当前窗口的有界视频截图上传与保存服务。
    pub video_screenshot: VideoScreenshotService,
}

impl AppState {
    /// 组装组合根。`db` 由 setup 打开并传入（测试可用内存库）。
    pub fn new(db: Arc<Db>) -> Self {
        Self::try_new(db).expect("初始化 AppState 失败")
    }

    /// 生产组装路径：恢复未正常结束的下载任务，错误必须阻止应用带病启动。
    pub fn try_new(db: Arc<Db>) -> Result<Self, haven_common::AppError> {
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        repos.download.recover_interrupted()?;
        let settings = SettingsService::new(Arc::new(SqliteSettingsUoW::new(db.clone())));
        let resource_preferences = ResourcePreferenceService::new(
            repos.clone(),
            repos.clone(),
            repos.clone(),
            settings.clone(),
        );
        let download_sink = Arc::new(TauriDownloadEventSink::new());
        let download_batch = Arc::new(DownloadBatchService::new(repos.clone()));
        let download = DownloadService::new(
            repos.clone(),
            Arc::new(LocalDownloadRunner::new(
                repos.clone(),
                Arc::new(settings.clone()),
                download_sink.clone(),
                download_batch.clone(),
            )),
            Arc::new(LocalOfflineResourceFiles),
            download_sink.clone(),
            Arc::new(settings.clone()),
            download_batch.clone(),
        );
        let favorite =
            FavoriteService::new(repos.clone(), Arc::new(SqliteUnitOfWork::new(db.clone())));
        let progress = ProgressService::new(repos.clone());
        let library = LibraryService::new(repos.clone());
        let storage_location =
            StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
        let search_history = SearchHistoryService::new(repos.clone());
        // 扫描：LocalLibraryScanner 实现 LibraryScanner 端口（infra→app 方向，
        // ADR-003 §6）；事件经 TauriScanEventSink 广播（Channel 按 id 去重 + cap 收敛）。
        let scanner = Arc::new(LocalLibraryScanner::new(db.clone()));
        let scan_sink = Arc::new(TauriScanEventSink::new());
        let mut scan = ScanService::new(scanner, storage_location.clone(), scan_sink.clone());
        let comic_pages = ComicPageService::new(Arc::new(LocalComicPageProvider::new()));
        let reader_toc = ReaderTocService::new(Arc::new(LocalEpubTocProvider::new()));
        let reader_search = ReaderSearchService::new(Arc::new(LocalReaderSearchProvider::new()));
        let reader_search_sink = Arc::new(TauriReaderSearchEventSink::new());
        let video_screenshot =
            VideoScreenshotService::new(Arc::new(LocalVideoScreenshotProvider::new()));
        let session = SessionService::new(repos.clone(), comic_pages.clone());
        let history = HistoryService::new(repos.clone(), Arc::new(settings.clone()));
        let marker = MarkerService::new(repos.clone());
        let home = HomeService::new(repos.clone());
        // v0.2 来源批次（契约 §36.2/§36.3/§36.5）：
        // - 来源注册表：静态内置目录 + settings KV 持久化启用状态与端点。
        // - 渐进式搜索：固定公开 metadata、CMS10、M3U 与已验证 OPDS 参与者均已接入；
        //   每个内置 sourceId 必须在对应适配器中有真实搜索路径。
        // - 凭据：平台 CredentialStore（Windows keyring / 非 Windows unsupported）。
        // 启动时硬校验静态目录与 Provider 注册集合完全一致；未来新增内置
        // sourceId 若未同时接入真实搜索参与者，直接 fail closed，不能静默显示。
        haven_infrastructure::metadata_sources::validate_builtin_search_coverage()?;
        let source_registry_settings: Arc<dyn SourceRegistryPorts> = repos.clone();
        let source_registry = SourceRegistryService::new(source_registry_settings);
        let search_sink = Arc::new(TauriSearchEventSink::new());
        let cms10_client = Arc::new(Cms10Client::new().map_err(|e| {
            haven_common::AppError::new(
                "INTERNAL_ERROR",
                haven_common::ErrorKind::Internal,
                e.user_message(),
                false,
            )
        })?);
        let metadata_client = Arc::new(MetadataClient::new().map_err(|e| {
            haven_common::AppError::new(
                "INTERNAL_ERROR",
                haven_common::ErrorKind::Internal,
                e.user_message(),
                false,
            )
        })?);
        // V2-H 收尾批次：自定义源凭据解析器——搜索/导入请求前从系统 keyring 取
        // Basic Auth secret（内存即取即用，禁止落盘/日志）。
        let credential_store = haven_infrastructure::credential::credential_store()?;
        let opds_client = Arc::new(
            haven_infrastructure::opds::OpdsClient::new()?.with_credential_resolver(Arc::new(
                move |source_id: &str| {
                    let store = credential_store.clone();
                    let source_id = source_id.to_owned();
                    Box::pin(async move {
                        let target = haven_application::services::source_registry::SourceRegistryService::custom_credential_target(&source_id).ok()?;
                        let secret = store.get(&target).await.ok()??;
                        Some(secret.expose().to_owned())
                    })
                },
            )),
        );
        // V2-H1：OPDS 书源——3 个内置参与者 + 已启用自定义源动态参与者。
        let storage_repo: Arc<
            dyn haven_domain::contracts::StorageLocationRepository + Send + Sync,
        > = Arc::new(
            haven_infrastructure::db::repos::SqliteStorageLocationRepository::new(db.clone()),
        );
        let mut participants: Vec<Arc<dyn SearchSourceParticipant>> = vec![Arc::new(
            Cms10SearchParticipant::new(source_registry.clone(), cms10_client.clone()),
        )];
        participants.extend(
            haven_infrastructure::metadata_sources::metadata_participants(metadata_client.clone()),
        );
        participants.push(Arc::new(M3uSearchParticipant::new(
            source_registry.clone(),
            metadata_client,
        )));
        for sid in haven_infrastructure::opds::OPDS_SOURCE_IDS {
            participants.push(Arc::new(
                haven_infrastructure::opds::OpdsSearchParticipant::new(
                    sid,
                    source_registry.clone(),
                    opds_client.clone(),
                ),
            ));
        }
        // 自定义源：单个前缀路由参与者承接全部 `custom_` 源
        // （端点/启用状态每次搜索时经注册表读取，无需启动期阻塞 IO）。
        participants.push(Arc::new(
            haven_infrastructure::opds::OpdsSearchParticipant::new(
                haven_infrastructure::opds::CUSTOM_OPDS_ID_PREFIX.to_owned(),
                source_registry.clone(),
                opds_client.clone(),
            ),
        ));
        let search_source =
            SearchSourceService::new(source_registry.clone(), participants, search_sink.clone());
        let catalog_router = haven_infrastructure::opds::RoutingSourceCatalogProvider::new(
            Arc::new(Cms10CatalogProvider::new(cms10_client.clone())),
            Arc::new(haven_infrastructure::opds::OpdsCatalogProvider::new(
                opds_client.clone(),
                storage_repo,
                default_books_library_root(&db),
            )),
        );
        let catalog_router = Arc::new(catalog_router);
        let import_ports: Arc<dyn SourceImportPorts> = repos.clone();
        let source_import = SourceImportService::new(
            import_ports,
            source_registry.clone(),
            catalog_router.clone(),
        );
        // V2-F（契约 §36.8）：enrichment 流水线 + 扫描 Completed 钩子。
        let enrich_ports: Arc<dyn haven_application::services::ports::EnrichmentPorts> =
            repos.clone();
        let import_ports2: Arc<dyn SourceImportPorts> = repos.clone();
        let metadata_sink = Arc::new(TauriMetadataChangedSink::new());
        let enrichment = EnrichmentService::new(
            enrich_ports,
            SourceImportService::new(
                import_ports2,
                source_registry.clone(),
                catalog_router.clone(),
            ),
        );
        {
            let enrichment = enrichment.clone();
            let sink = metadata_sink.clone();
            scan.set_on_completed(move || {
                let enrichment = enrichment.clone();
                let sink = sink.clone();
                Box::pin(async move {
                    match enrichment.run_pending().await {
                        Ok(outcomes) => {
                            for outcome in outcomes {
                                sink.emit_metadata_changed(haven_application::wire::MetadataChangedDto {
                                    schema_version: 1,
                                    at: chrono::Utc::now().to_rfc3339(),
                                    operation_id: uuid::Uuid::new_v4().to_string(),
                                    sequence: 1,
                                    work_id: outcome.work_id.to_string(),
                                    status: outcome.status,
                                    source_id: if outcome.status == haven_application::wire::EnrichmentStatusWire::Enriched { Some("cms10".into()) } else { None },
                                    error: None,
                                });
                            }
                        }
                        Err(_) => {
                            // 流水线失败不影响扫描终态；状态留在 pending/上次值。
                        }
                    }
                })
            });
        }
        let stream_registry = Arc::new(StreamRegistry::new());
        // StreamService 与本地 Session 复用同一组只读端口。
        let stream_ports: Arc<dyn haven_application::services::ports::SessionOpenPorts> =
            repos.clone();
        let stream = StreamService::new(stream_ports.clone());
        let credential_access =
            CredentialAccessService::new(haven_infrastructure::credential::credential_store()?);
        // Trending：Query 只读 SQLite 快照；Refresh 才访问豆瓣并写技术缓存。
        // 生产组合根不使用静态榜单兜底，来源不可用时由 Refresh 返回可重试错误。
        let artwork_cache = Arc::new(ArtworkCache::new(
            db.clone(),
            ArtworkCache::default_root(db.as_ref()),
        )?);
        let data_dir = db
            .path()
            .and_then(|path| path.parent().map(|parent| parent.to_path_buf()))
            .unwrap_or_else(|| std::env::temp_dir().join("haven-data"));
        let cache_dir = ArtworkCache::default_root(db.as_ref());
        let logs_dir = data_dir.join("Logs");
        let app_info_provider = Arc::new(LocalAppInfoProvider::new(
            db.clone(),
            data_dir.clone(),
            logs_dir,
            cache_dir,
        ));
        let app_info = haven_application::services::AppInfoService::new(app_info_provider.clone());
        let error_report = ErrorReportService::new(Arc::new(LocalErrorReportProvider::new(
            app_info_provider,
            data_dir,
        )));
        let artwork_cache_port: Arc<dyn ArtworkCachePort> = artwork_cache.clone();
        let artwork_cache_clear_port: Arc<
            dyn haven_application::services::cache::ArtworkCacheClearPort,
        > = artwork_cache.clone();
        let cache = CacheService::new(artwork_cache_clear_port);
        let trending_provider: Arc<dyn TrendingProvider> =
            Arc::new(haven_infrastructure::trending::DoubanTrendingProvider::new()?);
        let trending_cache: Arc<dyn TrendingCachePort> = repos.clone();
        let trending = TrendingService::new(trending_provider, trending_cache, artwork_cache_port);
        // Cast 双栈（发现 + 控制 + 媒体服务 + grant）
        let cast_media = Arc::new(AxumCastMediaServer::new());
        let cast_grants = Arc::new(CastGrantRegistry::new(cast_media.base_url().to_owned()));
        let cast_discovery: Arc<dyn haven_application::services::CastDiscoveryPort> =
            Arc::new(SsdpMdnsDiscovery::new());
        let cast_control: Arc<dyn haven_application::services::CastControlPort> =
            Arc::new(SoapCastControl::new());
        let cast_media_port: Arc<dyn haven_application::services::CastMediaPort> =
            cast_media.clone();
        let cast = CastService::new(
            cast_discovery,
            cast_control,
            cast_media_port,
            stream.clone(),
            cast_grants.clone(),
        );
        Ok(Self {
            db,
            repos: repos.clone(),
            library,
            favorite,
            download,
            download_sink,
            progress,
            storage_location,
            settings,
            resource_preferences,
            search_history,
            cache,
            scan,
            scan_sink,
            work: WorkService::new(repos.clone()),
            resource: ResourceService::new(repos),
            comic_pages,
            session,
            history,
            marker,
            home,
            source_registry,
            search_source,
            search_sink,
            source_import,
            enrichment,
            metadata_sink,
            stream,
            stream_registry,
            credential_access,
            trending,
            artwork_cache,
            app_info,
            error_report,
            cast,
            cast_media,
            cast_grants,
            session_registry: Arc::new(SessionRegistry::new()),
            reader_toc,
            reader_search,
            reader_search_sink,
            video_screenshot,
        })
    }
}

/// `metadata.changed` 广播出口（契约 §36.8）。
/// AppHandle 由 setup 阶段注入；未注入时事件静默丢弃（无窗口场景，如测试）。
pub struct TauriMetadataChangedSink {
    app: std::sync::Mutex<Option<tauri::AppHandle<tauri::Wry>>>,
}

impl Default for TauriMetadataChangedSink {
    fn default() -> Self {
        Self {
            app: std::sync::Mutex::new(None),
        }
    }
}

impl TauriMetadataChangedSink {
    pub fn new() -> Self {
        Self {
            app: std::sync::Mutex::new(None),
        }
    }

    pub fn bind(&self, app: tauri::AppHandle<tauri::Wry>) {
        *self.app.lock().unwrap_or_else(|e| e.into_inner()) = Some(app);
    }
}

impl MetadataChangedSink for TauriMetadataChangedSink {
    fn emit_metadata_changed(&self, event: haven_application::wire::MetadataChangedDto) {
        if let Some(app) = self.app.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = app.emit(crate::ipc::METADATA_CHANGED_TRANSPORT_EVENT, event);
        }
    }
}
