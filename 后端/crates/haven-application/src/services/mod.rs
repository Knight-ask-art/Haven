//! Application Service 边界（BE-APP-001）。
//!
//! 规范：TECHNICAL_ARCHITECTURE §17、ADR-002。
//! - 表达 Use Case：流程编排、事务边界、Repository 调用。
//! - 只依赖 domain 契约（trait object），不依赖 Sqlite/Tauri 实现。
//! - 实现方（src-tauri / 测试）通过组合端口注入具体 Repository。

pub mod app_info;
pub mod cache;
pub mod cast;
pub mod comic;
pub mod credential;
pub mod credential_access;
pub mod download;
pub mod download_batch;
pub mod enrichment;
pub mod error_report;
pub mod favorite;
pub mod history;
pub mod home;
pub mod library;
pub mod marker;
pub mod ports;
pub mod progress;
pub mod reader_search;
pub mod reader_toc;
pub mod resource;
pub mod resource_preferences;
pub mod scan;
pub mod search_history;
pub mod search_source;
pub mod session;
pub mod settings;
pub mod settings_file;
pub mod source_import;
pub mod source_registry;
pub mod storage_location;
pub mod stream;
pub mod trending;
pub mod video_screenshot;
pub mod work;

pub use app_info::{AppInfoPorts, AppInfoService, DirectoryKind};
pub use cache::{ArtworkCacheClearPort, CacheService};
pub use cast::{CastControlPort, CastDiscoveryPort, CastGrantRegistry, CastMediaPort, CastService};
pub use comic::{
    ComicImageMime, ComicPageBody, ComicPageProvider, ComicPageService, PreparedComicPage,
    PreparedComicPageAvailability, PreparedComicPageSource, RemoteComicPageProvider,
};
pub use credential::{CredentialDeleteOutcome, CredentialDeletePorts, CredentialDeletionService};
pub use credential_access::{CredentialAccessService, DEFAULT_PROFILE_ID};
pub use download::{
    DownloadEventSink, DownloadPorts, DownloadRunner, DownloadService, OfflineResourceFiles,
};
pub use enrichment::{EnrichedWorkOutcome, EnrichmentService, MetadataChangedSink};
pub use error_report::{
    ErrorReportFacts, ErrorReportIssueDraft, ErrorReportPorts, ErrorReportService,
};
pub use favorite::FavoriteService;
pub use history::{HistoryPorts, HistoryService};
pub use home::HomeService;
pub use library::LibraryService;
pub use marker::{MarkerPorts, MarkerService};
pub use ports::SessionOpenPorts;
pub use ports::{FavoritePorts, LibraryPorts};
pub use progress::{ProgressPorts, ProgressService};
pub use reader_search::{RawBookContent, RawChapter, ReaderSearchProvider, ReaderSearchService};
pub use resource::ResourceService;
pub use resource_preferences::{
    PreferenceSnapshot, PreferenceTarget, PreferenceUpdateResult, ResourcePreferenceService,
};
pub use scan::{
    CancelOutcome, LibraryScanner, ScanEventSink, ScanObserver, ScanProgress, ScanReport,
    ScanService,
};
pub use search_history::{
    SEARCH_HISTORY_LIMIT, SEARCH_HISTORY_MAX_TERM_CHARS, SearchHistoryService,
};
pub use search_source::{
    DEFAULT_LIMIT_PER_SOURCE, MAX_LIMIT_PER_SOURCE, MAX_QUERY_LEN, SearchCancelOutcome,
    SearchEventSink, SearchSourceParticipant, SearchSourceService,
};
pub use session::{PreparedSession, PreparedSessionSource, SessionService};
pub use settings::{
    SettingsService, SettingsSnapshot, SettingsTxPorts, SettingsUoW, SettingsUpdateResult,
};
pub use source_import::{
    CMS10_CANDIDATE_PREFIX, ImportedWork, OPDS_CANDIDATE_PREFIX, SourceCatalogEntry,
    SourceCatalogProvider, SourceImportService,
};
pub use source_registry::SourceRegistryService;
pub use storage_location::{
    DefaultRootProbe, ProbeOutcome, RootProbe, ScanTarget, ScanTargetToken, StorageLocationService,
    StorageLocationUoW, StorageTxPorts,
};
pub use stream::{StreamOpenFacts, StreamService};
pub use trending::{
    ArtworkCachePort, CANONICAL_BOARD_IDS, RemoteArtworkCandidate, TrendingBoardCacheEntry,
    TrendingBoardCandidate, TrendingCachePort, TrendingItemCandidate, TrendingProvider,
    TrendingService,
};
pub use video_screenshot::{VideoScreenshotService, VideoScreenshotStoragePort};
pub use work::WorkService;
