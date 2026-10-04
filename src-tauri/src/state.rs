//! AppState：Composition Root 单一持有 DB、Repository 与 Application Services。
//!
//! 原则（IPC-TAURI-001A/B）：
//! - 所有 Command 只通过 `State<'_, AppState>` 访问共享 Services；
//!   禁止 Command 临时创建第二套 DB/Repository。
//! - DB 在 setup 阶段打开一次（应用数据目录），后续全部复用。

use std::sync::Arc;

use tauri::Emitter;

use haven_application::services::agent::{AgentProposalService, RepositoryAgentSubjectScope};
use haven_application::services::agent_context::AgentContextQueryService;
use haven_application::services::agent_settings_ipc::AgentSettingsIpcService;
use haven_application::services::agent_skill::AgentSkillService;
use haven_application::services::agent_trace::{
    AgentTraceQueryService, InMemoryAgentTraceCollector,
};
use haven_application::services::ai_provider::AiProviderProfileService;
use haven_application::services::appearance::AppearanceService;
use haven_application::services::cache::CacheService;
use haven_application::services::cast::{CastGrantRegistry, CastService};
use haven_application::services::comic::ComicPageService;
use haven_application::services::comic_catalog::ComicCatalogService;
use haven_application::services::comic_page_identity::ComicPageIdentityService;
use haven_application::services::comic_progress_migration::ComicProgressMigrationService;
use haven_application::services::credential_access::CredentialAccessService;
use haven_application::services::download::DownloadService;
use haven_application::services::download_batch::DownloadBatchService;
use haven_application::services::enrichment::EnrichmentService;
use haven_application::services::enrichment::MetadataChangedSink;
use haven_application::services::error_report::ErrorReportService;
use haven_application::services::favorite::FavoriteService;
use haven_application::services::history::HistoryService;
use haven_application::services::home::HomeService;
use haven_application::services::interface_fonts::InterfaceFontService;
use haven_application::services::library::LibraryService;
use haven_application::services::marker::MarkerService;
use haven_application::services::mcp_client_config::McpClientConfigService;
use haven_application::services::periodical_query::PeriodicalQueryService;
use haven_application::services::ports::{
    ComicCatalogRefreshReceiptPort, ComicCatalogWorkPorts, SourceImportPorts, SourceRegistryPorts,
};
use haven_application::services::progress::comic_progress_subject::ComicProgressSubjectService;
use haven_application::services::progress::ProgressService;
use haven_application::services::reader_search::ReaderSearchService;
use haven_application::services::reader_toc::ReaderTocService;
use haven_application::services::reading_overview::ReadingOverviewService;
use haven_application::services::resource::ResourceService;
use haven_application::services::resource_preferences::ResourcePreferenceService;
use haven_application::services::scan::ScanService;
use haven_application::services::search_history::SearchHistoryService;
use haven_application::services::search_source::SearchSourceParticipant;
use haven_application::services::search_source::SearchSourceService;
use haven_application::services::session::SessionService;
use haven_application::services::setting_proposals::SettingProposalService;
use haven_application::services::settings::SettingsService;
use haven_application::services::source_import::SourceImportService;
use haven_application::services::source_registry::SourceRegistryService;
use haven_application::services::storage_location::StorageLocationService;
use haven_application::services::stream::StreamService;
use haven_application::services::trending::{
    ArtworkCachePort, TrendingCachePort, TrendingProvider, TrendingService,
};
use haven_application::services::tvbox_config_import::TvboxConfigSaveService;
use haven_application::services::tvbox_config_preview::TvboxConfigPreviewService;
use haven_application::services::work::WorkService;
use haven_application::services::VideoScreenshotService;
use haven_infrastructure::app_info::LocalAppInfoProvider;
use haven_infrastructure::appearance_assets::LocalAppearanceAssetStorage;
use haven_infrastructure::artwork_cache::ArtworkCache;
use haven_infrastructure::cast::{AxumCastMediaServer, SoapCastControl, SsdpMdnsDiscovery};
use haven_infrastructure::cms10::{Cms10CatalogProvider, Cms10Client, Cms10SearchParticipant};
use haven_infrastructure::comic::LocalComicPageProvider;
use haven_infrastructure::db::repos::{
    SqliteInterfaceFontUoW, SqliteRepositories, SqliteSettingProposalUow, SqliteSettingsUoW,
};
use haven_infrastructure::db::uow::{SqliteStorageUoW, SqliteUnitOfWork};
use haven_infrastructure::download::{LocalDownloadRunner, LocalOfflineResourceFiles};
use haven_infrastructure::epub::LocalEpubTocProvider;
use haven_infrastructure::error_report::LocalErrorReportProvider;
use haven_infrastructure::metadata_sources::{M3uSearchParticipant, MetadataClient};
use haven_infrastructure::reader_search::LocalReaderSearchProvider;
use haven_infrastructure::scanner::LocalLibraryScanner;
use haven_infrastructure::system_fonts::{LocalFontFileInspector, LocalSystemFontCatalog};
use haven_infrastructure::video_screenshot::LocalVideoScreenshotProvider;
use haven_infrastructure::Db;

use crate::agent_broker::AgentBrokerManager;
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
    pub cloud_storage: Arc<haven_application::services::cloud_storage::CloudStorageService>,
    pub cloud_browse: Arc<haven_application::services::cloud_storage::browse::CloudBrowseService>,
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
    /// 漫画章节目录只读服务；刷新持久化仍由独立用例负责。
    pub comic_catalog: ComicCatalogService,
    /// 漫画章节换源与页面变化的进度迁移服务；所有写入走 CAS + 可撤销快照。
    pub comic_progress_migration: ComicProgressMigrationService,
    /// 从后端 Prepared Page facts 同步稳定身份并触发页面进度重定位。
    pub comic_page_identity: ComicPageIdentityService,
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
    /// 报刊层级只读查询（期刊 → 卷 → 期 → 文章）。
    ///
    /// 只持有 PeriodicalRepository：查询路径不碰网络 Provider，也不写存储；
    /// 与 source_import 的报刊导入路径共用同一份持久化契约（不建立第二套数据源）。
    pub periodical: PeriodicalQueryService,
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
    /// AI Provider Profile 的非敏感配置与只读模型目录（A2 基础切片）。
    ///
    /// 不持有任何 secret：凭据在 CredentialStore，本服务只在模型发现调用栈内
    /// 临时取出。Provider 无法通过它写入本机状态。
    pub ai_provider: AiProviderProfileService,
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
    pub(crate) update_preparation: Arc<crate::commands::app_update::UpdatePreparation>,
    /// 阅读目录（契约 §19.1 `reader_toc_get`；EPUB 专用）。
    pub reader_toc: ReaderTocService,
    /// 阅读全文检索（契约 §19.1 `reader_search`）。
    pub reader_search: ReaderSearchService,
    /// 阅读检索事件出口（每次 `reader_search_start` 绑定 Channel）。
    pub reader_search_sink: Arc<TauriReaderSearchEventSink>,
    /// 当前窗口的有界视频截图上传与保存服务。
    pub video_screenshot: VideoScreenshotService,
    /// 设置变更提案的创建/读取/拒绝/显式确认应用（与设置 Agent 共用同一 UoW）。
    pub setting_proposals: SettingProposalService,
    /// 全局设置 Agent 的 Typed Application 入口（读取/提案/批准/回执）。
    pub agent_settings: AgentSettingsIpcService,
    /// 界面自定义字体：本机字体枚举 + 导入字体资产（枚举/导入/删除/字节）。
    ///
    /// 只持有 opaque id 与族名的读写能力；字节仅经 `haven-resource://font/<id>`
    /// 受控协议出站，WebView 永远拿不到路径。
    pub interface_fonts: InterfaceFontService,
    /// 外观资产与首页布局（契约 §12；045/046）。
    ///
    /// 只持有组合根共享的同一个 `SqliteRepositories`（appearance 登记行与布局）与
    /// 一个受控资产存储；资产字节根目录跟随数据库所在的数据目录，不建立第二套 DB。
    pub appearance: AppearanceService,
    /// 阅读总览（契约 §12）：已闭合会话事实的登记与按窗口聚合。
    ///
    /// 端口直接由组合根共享的同一个 `SqliteRepositories` 提供（会话事实 + MediaItem
    /// 的真实 `media_type`），不新建第二套 DB，也不引入平行读取路径。写入只发生在
    /// 真实 Reader/Player 会话闭合处；时刻取观测值，不接受调用方传入的时长。
    pub reading_overview: ReadingOverviewService,
    /// Agent 轨迹的有界进程内 collector；未来 UI 通过 `agent_trace_get` 读取。
    pub agent_trace: Arc<InMemoryAgentTraceCollector>,
    pub agent_trace_query: AgentTraceQueryService,
    /// 外部 Agent 共享的只读上下文与资源偏好提案 Application 入口。
    ///
    /// Broker 只持有该服务的 Clone，不自行创建 Repository、DB 或 Provider；
    /// 因此内置 Agent、MCP 与未来其它外部 Agent 入口看到的是同一份 authoritative
    /// context 与 Proposal 事实。
    pub agent_context: AgentContextQueryService,
    /// 外部 Agent 接入 Broker（A5 接线切片；契约 §4.5）。
    ///
    /// **默认关闭**：构造它不创建端点，端点只在用户显式开启后才存在。
    /// manager 内部转调的 `AgentSettingsBrokerApi` 复用上面同一个
    /// `agent_settings`，因此这里不出现第二套 DB/Repository/UoW。
    pub agent_broker: Arc<AgentBrokerManager>,
    /// 内置 Skill 运行时：分发内容（编译期嵌入）+ 权威启用状态（SQLite）。
    ///
    /// 启用状态是**用户逐项决定**的持久化事实，没有内存影子副本；因此重启后
    /// `agent_skill_list` 与请求路径看到的是同一件事。
    pub agent_skills: AgentSkillService,
    /// 外部 MCP 客户端（Codex / Claude Code）的 Haven 条目自动配置。
    ///
    /// 只服务两个**固定**文件里的一个键：`~/.codex/config.toml` 的
    /// `[mcp_servers.haven]` 与 `~/.claude.json` 的 `mcpServers.haven`。路径由
    /// Application 依据采集到的环境事实算出，端口不接受调用方提供的路径或内容。
    pub mcp_client_config: McpClientConfigService,
    /// TVBox / FongMi 配置预览（Film/TV Provider 基础切片）。
    ///
    /// 只持有一个无状态的端口实现：构造它不发起任何请求、不落库、不建缓存。
    /// 每次调用都走仓库既有的受控 HTTP（URL 策略 + 逐跳 DNS 固定 + 大小上限），
    /// 返回的也只是形态摘要。
    pub tvbox_config_preview: TvboxConfigPreviewService,
    /// TVBox / FongMi 配置的保存/导入（Film/TV Provider 基础切片第二步）。
    ///
    /// 复用组合根同一个 `SourceRegistryService`（身份与启用状态的唯一所有者）与
    /// 组合根共享的 `SqliteRepositories`（原文缓存，迁移 051）。来源**默认停用**。
    pub tvbox_config_save: TvboxConfigSaveService,
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
        // 界面字体：枚举/导入/删除共用同一份 DB 与 UoW；字体目录枚举在本机只读进行，
        // 结果在实现内记忆化（进程内一次扫描）。
        let interface_fonts = InterfaceFontService::new(
            Arc::new(SqliteInterfaceFontUoW::new(db.clone())),
            Arc::new(LocalSystemFontCatalog::new()),
            Arc::new(LocalFontFileInspector),
            repos.clone(),
        );
        // Agent 设置切片：提案服务与批准入口共用 AppState 的同一份 Settings /
        // SettingProposal UoW 与 Repository，不新建第二套 DB 句柄。
        let setting_proposals = SettingProposalService::new(
            repos.clone(),
            Arc::new(SqliteSettingProposalUow::new(db.clone())),
        );
        let agent_subject_scope = Arc::new(RepositoryAgentSubjectScope::new(
            repos.clone(),
            repos.clone(),
            repos.clone(),
        ));
        let agent_proposals = AgentProposalService::new(
            setting_proposals.clone(),
            agent_subject_scope,
            repos.clone(),
        )
        .with_settings_service(settings.clone());
        let agent_context_proposals = agent_proposals.clone();
        let agent_trace = Arc::new(InMemoryAgentTraceCollector::new());
        let agent_trace_query = AgentTraceQueryService::new(agent_trace.clone());
        let agent_settings = AgentSettingsIpcService::new(
            setting_proposals.clone(),
            agent_proposals,
            settings.clone(),
        )
        .with_trace(agent_trace.clone());
        // A5 接线：Broker **默认关闭**。构造只建一个空 manager——不解析端点、
        // 不碰文件系统、不新建 DB 句柄；端点只在用户显式 enable 时创建。
        let agent_broker = Arc::new(AgentBrokerManager::default());
        // 原生 Skill 运行时：技能正文在编译期嵌入二进制，启用状态在 SQLite。
        // 构造失败（内嵌技能文档不合法）是**启动期**错误而不是运行期降级——
        // 一个少了一项技能的目录会让用户在界面上找不到他以为存在的东西。
        let agent_skills = AgentSkillService::new(
            Arc::new(haven_infrastructure::agent_skill::BuiltinAgentSkillRegistry::new()?),
            repos.clone(),
        );
        // 外部 MCP 客户端一键配置：构造只持有一个无状态的端口实现——不解析端点、
        // 不读用户目录、不碰文件系统。真正的路径判定与写入都发生在命令被调用的那一刻，
        // 且只针对 `~/.codex/config.toml` / `~/.claude.json` 里 Haven 那一条。
        let mcp_client_config = McpClientConfigService::new(Arc::new(
            haven_infrastructure::mcp_client_config::LocalMcpClientConfig::new(),
        ));
        let resource_preferences = ResourcePreferenceService::new(
            repos.clone(),
            repos.clone(),
            repos.clone(),
            settings.clone(),
        );
        let download_sink = Arc::new(TauriDownloadEventSink::new());
        let download_batch = Arc::new(DownloadBatchService::new(repos.clone()));
        let unit_of_work = Arc::new(SqliteUnitOfWork::new(db.clone()));
        let favorite = FavoriteService::new(repos.clone(), unit_of_work.clone());
        // 漫画 Progress 的连续性解析必须是组合根共享的同一个服务实例。
        // 各 Application service 只持有它的 Clone，不得在命令内临时创建
        // 第二套 Subject repository/UoW。
        let comic_progress_subjects =
            ComicProgressSubjectService::new(repos.clone(), unit_of_work.clone());
        let progress =
            ProgressService::with_comic_progress_subjects(repos.clone(), unit_of_work.clone());
        let library = LibraryService::new(repos.clone())
            .with_comic_progress_subjects(comic_progress_subjects.clone());
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
        // Remote Session 与漫画页 Provider 共享同一个受控正文来源实例。
        // 这样生产组合根不会落回仅支持本地文件的默认 Service，也不会
        // 创建第二套 HTTP client/来源事实源。
        let online_client = Arc::new(
            haven_infrastructure::online_sources::OnlineContentClient::new().map_err(|e| {
                haven_common::AppError::new(
                    "INTERNAL_ERROR",
                    haven_common::ErrorKind::Internal,
                    e.user_message(),
                    false,
                )
            })?,
        );
        // 专用报刊 Provider：与正文 Provider 共享同一条固定主机客户端，
        // 不建立第二套 HTTP 栈；期刊 Repository 与 AppState 的 SqliteRepositories 同源。
        let periodical_provider = Arc::new(
            haven_infrastructure::periodical::EuropePmcPeriodicalProvider::new(
                online_client.clone(),
            ),
        );
        let periodical_repository: Arc<dyn haven_domain::contracts::PeriodicalRepository> =
            repos.clone();
        // 只读查询服务与导入路径共用同一个 Repository 实例（同一 DB 句柄）。
        let periodical = PeriodicalQueryService::new(periodical_repository.clone());
        let history = HistoryService::new(repos.clone(), Arc::new(settings.clone()));
        let marker = MarkerService::new(repos.clone());
        let home = HomeService::new(repos.clone())
            .with_comic_progress_subjects(comic_progress_subjects.clone());
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
        // Task 1：用户登记的 RSS/Atom 订阅源。同一个 FeedProvider 实例同时被注入
        // 目录详情/在线会话/离线获取、导入路由和搜索参与者，避免出现两条互不
        // 一致的订阅源读取路径；未注入的候选保持 fail closed。
        let feed_source: Arc<dyn haven_infrastructure::article_feeds::FeedSource> = Arc::new(
            haven_infrastructure::article_feeds::FeedClient::new().map_err(|e| {
                haven_common::AppError::new(
                    "INTERNAL_ERROR",
                    haven_common::ErrorKind::Internal,
                    e.user_message(),
                    false,
                )
            })?,
        );
        let feed_provider = Arc::new(haven_infrastructure::article_feeds::FeedProvider::new(
            source_registry.clone(),
            feed_source.clone(),
        ));
        // Remote Session 与漫画页 Provider 共享同一个受控正文来源实例；订阅源
        // Provider 必须在这里注入，否则 feed 来源的在线会话与离线获取会明确
        // 失败（fail closed），而不是回退到其它固定主机。
        let online_catalog = Arc::new(
            haven_infrastructure::online_sources::OnlineCatalogProvider::new(online_client.clone())
                .with_feed_provider(feed_provider.clone()),
        );
        // 漫画页远端 Provider 在系统凭据库构造之后再接线（自托管漫画库需要按
        // sourceId 解析 API key），因此这里不提前设置 `with_remote_provider`。
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
        //
        // 组合根只构造一个 CredentialStore：自定义源凭据、Provider Profile 凭据与
        // AI Provider 凭据都是同一个系统凭据库的视图，多建实例只会多出互不相干的句柄。
        let credential_store = haven_infrastructure::credential::credential_store()?;
        let cloud_ports = haven_infrastructure::cloud_drive::GoogleDrivePorts::new(
            option_env!("HAVEN_GOOGLE_OAUTH_CLIENT_ID").map(str::to_owned),
            option_env!("HAVEN_GOOGLE_OAUTH_CLIENT_SECRET")
                .map(haven_domain::credential::SecretString::new),
            Arc::new(crate::cloud_oauth_browser::SystemGoogleOAuthBrowser),
        );
        let cloud_storage = Arc::new(
            haven_application::services::cloud_storage::CloudStorageService::new(
                Arc::new(
                    haven_infrastructure::db::repos::SqliteCloudStorageRepository::new(db.clone()),
                ),
                cloud_ports.drive,
                cloud_ports.auth,
                credential_store.clone(),
            ),
        );
        let cloud_browse = Arc::new(
            haven_application::services::cloud_storage::browse::CloudBrowseService::new(
                cloud_storage.clone(),
            ),
        );
        let opds_client = Arc::new(
            haven_infrastructure::opds::OpdsClient::new()?.with_credential_resolver(Arc::new({
                let store = credential_store.clone();
                move |source_id: &str| {
                    let store = store.clone();
                    let source_id = source_id.to_owned();
                    Box::pin(async move {
                        let target = haven_application::services::source_registry::SourceRegistryService::custom_credential_target(&source_id).ok()?;
                        let secret = store.get(&target).await.ok()??;
                        Some(secret.expose().to_owned())
                    })
                }
            })),
        );
        // Task 2：用户登记的自托管漫画库（Komga/Kavita）。同一个 Provider 实例
        // 同时承接目录详情/章节目录、在线逐页读取、章节归档下载与搜索参与者，
        // 避免出现两条互不一致的漫画库读取路径；API key 只经系统凭据库解析，
        // 并且只作为请求头注入（绝不进入 URL/query/wire/日志）。
        let comic_library_gateway: Arc<
            dyn haven_infrastructure::comic_library_sources::ComicLibraryGateway,
        > = Arc::new(
            haven_infrastructure::comic_library_sources::HttpComicLibraryGateway::new().map_err(
                |e| {
                    haven_common::AppError::new(
                        "INTERNAL_ERROR",
                        haven_common::ErrorKind::Internal,
                        e.user_message(),
                        false,
                    )
                },
            )?,
        );
        let comic_library_api_key: Arc<
            haven_infrastructure::comic_library_sources::ApiKeyResolver,
        > = Arc::new({
            let store = credential_store.clone();
            move |source_id: &str| {
                let store = store.clone();
                let source_id = source_id.to_owned();
                Box::pin(async move {
                    let kind =
                        haven_application::services::source_registry::SourceRegistryService::comic_library_kind(
                            &source_id,
                        )?;
                    let target =
                        haven_application::services::source_registry::SourceRegistryService::credential_target_for_kind(
                            &source_id,
                            kind,
                        )
                        .ok()?;
                    let secret = store.get(&target).await.ok()??;
                    Some(secret.expose().to_owned())
                })
            }
        });
        let comic_library = Arc::new(
            haven_infrastructure::comic_library_sources::ComicLibraryProvider::new(
                source_registry.clone(),
                comic_library_gateway.clone(),
                comic_library_api_key.clone(),
            ),
        );
        let comic_library_catalog: Arc<dyn haven_application::services::SourceCatalogProvider> =
            comic_library.clone();
        let comic_library_acquisition: Arc<
            dyn haven_application::services::ports::RemoteAcquisitionPort,
        > = comic_library.clone();
        let comic_library_pages: Arc<
            dyn haven_application::services::comic::RemoteComicPageProvider,
        > = comic_library.clone();
        // 本地漫画、MangaDex 与自托管漫画库共用一个漫画页路由端口。
        let comic_pages = comic_pages.with_remote_provider(Arc::new(
            haven_infrastructure::opds::RoutingRemoteComicPageProvider::new(
                online_catalog.clone(),
                comic_library_pages,
            ),
        ));
        // V2-H1：OPDS 书源——3 个内置参与者 + 已启用自定义源动态参与者。
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
        // 用户登记的 RSS/Atom 订阅源：`custom_feed_` 前缀比 `custom_` 更长，
        // 最长前缀路由会把订阅源交给本参与者，而不会落到上面的自定义 OPDS。
        participants.push(Arc::new(
            haven_infrastructure::article_feeds::FeedSearchParticipant::new(
                source_registry.clone(),
                feed_source.clone(),
            ),
        ));
        // 自托管漫画库：每个家族前缀一个参与者，最长前缀路由保证 `custom_feed_`
        // 与 `custom_komga_`/`custom_kavita_` 互不接管。
        for prefix in [
            haven_application::services::source_registry::CUSTOM_KOMGA_SOURCE_PREFIX,
            haven_application::services::source_registry::CUSTOM_KAVITA_SOURCE_PREFIX,
        ] {
            participants.push(Arc::new(
                haven_infrastructure::comic_library_sources::ComicLibrarySearchParticipant::new(
                    source_registry.clone(),
                    comic_library_gateway.clone(),
                    comic_library_api_key.clone(),
                    prefix,
                ),
            ));
        }
        let search_source =
            SearchSourceService::new(source_registry.clone(), participants, search_sink.clone());
        let opds_catalog = Arc::new(haven_infrastructure::opds::OpdsCatalogProvider::new(
            opds_client.clone(),
        ));
        let remote_session = Arc::new(haven_infrastructure::opds::RoutingRemoteSessionPort::new(
            opds_catalog.clone(),
            online_catalog.clone(),
        ));
        let comic_progress_migration = ComicProgressMigrationService::new(repos.clone())
            .with_subject_service(comic_progress_subjects.clone());
        let comic_page_identity =
            ComicPageIdentityService::new(repos.clone(), comic_progress_migration.clone())
                .with_comic_progress_subjects(comic_progress_subjects.clone());
        let session =
            SessionService::new_with_remote(repos.clone(), comic_pages.clone(), remote_session)
                .with_cloud_storage(cloud_storage.clone())
                .with_comic_page_identity_sync(comic_page_identity.clone())
                .with_comic_progress_subjects(comic_progress_subjects.clone());
        let work =
            WorkService::new(repos.clone()).with_comic_progress_subjects(comic_progress_subjects);
        let online_catalog_source: Arc<dyn haven_application::services::SourceCatalogProvider> =
            online_catalog.clone();
        let remote_acquisition = Arc::new(
            haven_infrastructure::opds::RoutingRemoteAcquisitionPort::new(
                opds_catalog.clone(),
                online_catalog.clone(),
            )
            .with_comic_library(comic_library_acquisition),
        );
        let download = DownloadService::new(
            repos.clone(),
            Arc::new(LocalDownloadRunner::new_with_remote(
                repos.clone(),
                Arc::new(settings.clone()),
                download_sink.clone(),
                download_batch.clone(),
                remote_acquisition,
            )),
            Arc::new(LocalOfflineResourceFiles),
            download_sink.clone(),
            Arc::new(settings.clone()),
            download_batch.clone(),
        );
        let catalog_router = haven_infrastructure::opds::RoutingSourceCatalogProvider::new(
            Arc::new(Cms10CatalogProvider::new(cms10_client.clone())),
            opds_catalog.clone(),
            online_catalog_source,
        );
        let catalog_router = Arc::new(catalog_router);
        let import_ports: Arc<dyn SourceImportPorts> = repos.clone();
        let source_import = SourceImportService::new(
            import_ports,
            unit_of_work.clone(),
            source_registry.clone(),
            catalog_router.clone(),
        )
        .with_periodical_source(periodical_provider.clone(), periodical_repository.clone())
        .with_feed_source(feed_provider.clone())
        .with_comic_library_source(comic_library_catalog.clone());
        let registered_chapters: Arc<dyn haven_domain::contracts::ChapterSourceRepository> =
            repos.clone();
        let comic_catalog_ports: Arc<dyn ComicCatalogWorkPorts> = repos.clone();
        let comic_catalog_receipts: Arc<dyn ComicCatalogRefreshReceiptPort> = repos.clone();
        let comic_catalog = ComicCatalogService::new(source_import.clone(), registered_chapters)
            .with_work_catalog_ports(comic_catalog_ports)
            .with_refresh_receipt_port(comic_catalog_receipts);
        // V2-F（契约 §36.8）：enrichment 流水线 + 扫描 Completed 钩子。
        let enrich_ports: Arc<dyn haven_application::services::ports::EnrichmentPorts> =
            repos.clone();
        let import_ports2: Arc<dyn SourceImportPorts> = repos.clone();
        let metadata_sink = Arc::new(TauriMetadataChangedSink::new());
        let enrichment = EnrichmentService::new(
            enrich_ports,
            SourceImportService::new(
                import_ports2,
                unit_of_work.clone(),
                source_registry.clone(),
                catalog_router.clone(),
            )
            .with_periodical_source(periodical_provider, periodical_repository)
            .with_feed_source(feed_provider)
            .with_comic_library_source(comic_library_catalog),
        );
        {
            let enrichment = enrichment.clone();
            let sink = metadata_sink.clone();
            scan.set_on_completed(move || {
                let enrichment = enrichment.clone();
                let sink = sink.clone();
                Box::pin(async move {
                    let result = tauri::async_runtime::spawn_blocking(move || {
                        tauri::async_runtime::block_on(enrichment.run_pending())
                    })
                    .await;
                    match result {
                        Ok(Ok(outcomes)) => {
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
                        Ok(Err(error)) => {
                            // 流水线失败不影响扫描终态；状态留在 pending/上次值，
                            // 但必须保留错误码，避免后台失败完全不可见。
                            eprintln!(
                                "[enrichment] run_pending failed: {}",
                                error.code().as_str()
                            );
                        }
                        Err(error) => {
                            // JoinError 包括 worker panic/取消；记录后仍不阻断扫描。
                            eprintln!("[enrichment] run_pending worker failed: {error}");
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
        let credential_access = CredentialAccessService::new(credential_store.clone());
        // A2 AI Provider 基础切片：profile 持久化复用同一份 SqliteRepositories（同一 DB
        // 句柄，不建第二套连接），凭据复用上面同一个 CredentialStore，出站模型发现走
        // Infrastructure 适配器。Application service 是命令层唯一可达的入口。
        let ai_provider_profiles: Arc<dyn haven_domain::contracts::AiProviderProfileRepository> =
            repos.clone();
        let ai_provider = AiProviderProfileService::new(
            ai_provider_profiles,
            credential_store,
            Arc::new(haven_infrastructure::ai_provider::OpenAiCompatibleModelCatalog::new()),
        )
        .with_settings_recommendation(Arc::new(
            haven_infrastructure::ai_provider::OpenAiCompatibleSettingsRecommender::new(),
        ))
        .with_agent_settings(agent_settings.clone())
        // 原生 Skill 运行时接进**真实**的模型请求路径：每次设置建议都会先解析
        // 当前生效的内置技能，并把它们的说明性正文拼进 Provider 请求的 system 消息。
        // 这一步不改变权限——Proposal 仍停在 pending，批准仍只在 Haven UI 里完成。
        .with_agent_skills(agent_skills.clone())
        .with_trace(agent_trace.clone());
        let agent_context = AgentContextQueryService::new(
            repos.clone(),
            repos.clone(),
            repos.clone(),
            repos.clone(),
            repos.clone(),
            repos.clone(),
            repos.clone(),
            repos.clone(),
            settings.clone(),
            agent_context_proposals,
            ai_provider.clone(),
        )
        .with_trace(agent_trace.clone());
        // Trending：Query 只读 SQLite 快照；Refresh 才访问豆瓣并写技术缓存。
        // 生产组合根不使用静态榜单兜底，来源不可用时由 Refresh 返回可重试错误。
        let artwork_cache = Arc::new(ArtworkCache::new(
            db.clone(),
            ArtworkCache::default_root(db.as_ref()),
        )?);
        // 外观（契约 §12）：登记行与首页布局走组合根共享的 SqliteRepositories，
        // 资产字节放进跟随数据库数据目录的受控存储——两半共用同一个 DB 事实源。
        let appearance = AppearanceService::new(
            repos.clone(),
            Arc::new(LocalAppearanceAssetStorage::new(
                LocalAppearanceAssetStorage::default_root(db.as_ref()),
            )),
        );
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
        // 阅读总览（契约 §12）：端口由组合根共享的同一个 SqliteRepositories 直接实现
        // （会话事实表 + MediaItem 真实 media_type），因此登记与读取同源，不需要在
        // src-tauri 里再包一层转发 shim。
        let reading_overview = ReadingOverviewService::new(repos.clone());
        // TVBox / FongMi 配置预览：端口由 Infrastructure 的受控 HTTP 适配器实现
        // （infra→app 方向，ADR-003 §6）。构造不触网。
        let tvbox_config_preview = TvboxConfigPreviewService::new(Arc::new(
            haven_infrastructure::tvbox_config_fetch::HttpTvboxConfigPreview,
        ));
        // TVBox / FongMi 配置保存：同一个受控 HTTP 适配器家族（这里回原文），
        // 身份仍由上面那一个 source_registry 决定，原文落在组合根共享的 SQLite 仓储上。
        let source_config_cache: Arc<dyn haven_application::services::SourceConfigCache> =
            repos.clone();
        let tvbox_config_save = TvboxConfigSaveService::new(
            Arc::new(haven_infrastructure::tvbox_config_fetch::HttpTvboxConfigImport),
            source_registry.clone(),
            source_config_cache,
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
            cloud_storage: cloud_storage.clone(),
            cloud_browse,
            settings,
            resource_preferences,
            search_history,
            cache,
            scan,
            scan_sink,
            work,
            resource: ResourceService::new(repos).with_cloud_storage(cloud_storage.clone()),
            comic_pages,
            comic_catalog,
            comic_progress_migration,
            comic_page_identity,
            session,
            history,
            marker,
            home,
            source_registry,
            search_source,
            search_sink,
            source_import,
            periodical,
            enrichment,
            metadata_sink,
            stream,
            stream_registry,
            credential_access,
            ai_provider,
            trending,
            artwork_cache,
            app_info,
            error_report,
            cast,
            cast_media,
            cast_grants,
            session_registry: Arc::new(SessionRegistry::new()),
            update_preparation: Arc::new(crate::commands::app_update::UpdatePreparation::default()),
            reader_toc,
            reader_search,
            reader_search_sink,
            video_screenshot,
            setting_proposals,
            agent_settings,
            interface_fonts,
            appearance,
            reading_overview,
            agent_trace,
            agent_trace_query,
            agent_context,
            agent_broker,
            agent_skills,
            mcp_client_config,
            tvbox_config_preview,
            tvbox_config_save,
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
