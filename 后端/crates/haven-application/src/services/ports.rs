//! Application 端口：组合 Repository 契约，供具体实现（SqliteRepositories）注入。
//!
//! 依赖方向：Application → Domain 契约；具体实现由组装层（src-tauri / 测试）提供。

use std::path::Path;

use async_trait::async_trait;
use haven_common::AppError;
use haven_domain::comic_catalog::{ComicCatalogRefreshReceipt, ComicChapterCatalogState};
use haven_domain::comic_identity::{
    ChapterSourceRef, ComicProgressMigrationSnapshot, EditionProfile, PageIdentity,
};
use haven_domain::comic_progress_subject::{ComicProgressSubject, ComicProgressSubjectMember};
use haven_domain::contracts::{
    ChapterSourceRepository, ComicCatalogRefreshOutcomeRepository, ComicPageIdentityRepository,
    ComicProgressMigrationRepository, ComicProgressSubjectRepository, EditionProfileRepository,
    EditionRepository, EnrichmentRepository, FavoriteRepository, MarkerRepository,
    MediaItemRepository, ProgressRepository, ResourceRepository, SettingsRepository,
    StorageLocationRepository, WorkRepository,
};
use haven_domain::entities::{Edition, FavoriteTarget, MediaItem, Progress, Resource, Work};
use haven_domain::ids::{ComicCatalogRefreshId, ComicProgressMigrationId, MediaItemId, WorkId};

/// LibraryService 所需端口。
/// `Send + Sync`：默认实现方法在 `Arc<dyn LibraryPorts>` 路径下要求
/// `&dyn LibraryPorts: Send`（async_trait 对默认实现的 bound），故显式声明。
pub trait LibraryPorts:
    WorkRepository
    + EditionRepository
    + MediaItemRepository
    + ProgressRepository
    + FavoriteRepository
    + Send
    + Sync
{
}
impl<T> LibraryPorts for T where
    T: WorkRepository
        + EditionRepository
        + MediaItemRepository
        + ProgressRepository
        + FavoriteRepository
        + Send
        + Sync
{
}

pub trait WorkGetPorts:
    LibraryPorts
    + ResourceRepository
    + StorageLocationRepository
    + MarkerRepository
    + haven_domain::contracts::WorkRelationRepository
    + Send
    + Sync
{
}
impl<T> WorkGetPorts for T where
    T: LibraryPorts
        + ResourceRepository
        + StorageLocationRepository
        + MarkerRepository
        + haven_domain::contracts::WorkRelationRepository
        + Send
        + Sync
{
}

/// Resource 列表所需端口。MediaItem 归属和 StorageLocation 显示名都由后端校验/解析。
pub trait ResourceListPorts:
    MediaItemRepository + ResourceRepository + StorageLocationRepository + Send + Sync
{
}
impl<T> ResourceListPorts for T where
    T: MediaItemRepository + ResourceRepository + StorageLocationRepository + Send + Sync
{
}

/// `session_open` 所需的全部只读端口。
pub trait SessionOpenPorts:
    WorkRepository
    + EditionRepository
    + MediaItemRepository
    + ResourceRepository
    + StorageLocationRepository
    + ProgressRepository
    + Send
    + Sync
{
}
impl<T> SessionOpenPorts for T where
    T: WorkRepository
        + EditionRepository
        + MediaItemRepository
        + ResourceRepository
        + StorageLocationRepository
        + ProgressRepository
        + Send
        + Sync
{
}

/// FavoriteService 所需端口。
/// `Send + Sync`：Tauri State 要求 Service 可跨线程共享（与 LibraryPorts 同规则）。
pub trait FavoritePorts: WorkRepository + FavoriteRepository + Send + Sync {}
impl<T> FavoritePorts for T where T: WorkRepository + FavoriteRepository + Send + Sync {}

/// SourceRegistryService 所需端口（来源启用状态持久化于 settings KV，契约 §36.2）。
/// `as_settings` 访问方法由 blanket impl 提供（MSRV 1.85 无 trait upcasting，
/// 与 CredentialDeletePorts 同规则）。
pub trait SourceRegistryPorts: SettingsRepository + Send + Sync {
    fn as_settings(&self) -> &dyn SettingsRepository;
}
impl<T> SourceRegistryPorts for T
where
    T: SettingsRepository + Send + Sync,
{
    fn as_settings(&self) -> &dyn SettingsRepository {
        self
    }
}

/// SourceImportService 所需端口（作品/版本/条目/资源四仓组合，V2-B 入库）。
pub trait SourceImportPorts:
    WorkRepository
    + EditionRepository
    + EditionProfileRepository
    + MediaItemRepository
    + ResourceRepository
    + ChapterSourceRepository
    + ComicCatalogRefreshOutcomeRepository
    + haven_domain::contracts::ImageProxyRepository
    + Send
    + Sync
{
}

/// 漫画章节匹配与进度迁移所需端口。
///
/// 比较在 Application 侧完成，原子进度写入由
/// `ComicProgressMigrationRepository` 在 Infrastructure 事务中完成。
pub trait ComicProgressMigrationPorts:
    ChapterSourceRepository
    + ComicPageIdentityRepository
    + ComicProgressMigrationRepository
    + MediaItemRepository
    + EditionRepository
    + ProgressRepository
    + Send
    + Sync
{
}
impl<T> ComicProgressMigrationPorts for T where
    T: ChapterSourceRepository
        + ComicPageIdentityRepository
        + ComicProgressMigrationRepository
        + MediaItemRepository
        + EditionRepository
        + ProgressRepository
        + Send
        + Sync
{
}
impl<T> SourceImportPorts for T where
    T: WorkRepository
        + EditionRepository
        + EditionProfileRepository
        + MediaItemRepository
        + ResourceRepository
        + ChapterSourceRepository
        + ComicCatalogRefreshOutcomeRepository
        + haven_domain::contracts::ImageProxyRepository
        + Send
        + Sync
{
}

/// Work 级漫画章节只读聚合所需端口。
///
/// 聚合只读取 Work（含 `list_source_refs`）、Edition 画像、MediaItem、
/// ChapterSourceRef、Resource、ComicProgressSubject、Progress 和刷新 Receipt；
/// 任何写入、事务或网络读取都不在该用例内。MSRV 1.85 无 trait upcasting，因此
/// 逐个提供 `as_*` 访问方法（与 `SourceRegistryPorts::as_settings` 同规则）。
pub trait ComicCatalogWorkPorts:
    WorkRepository
    + EditionRepository
    + EditionProfileRepository
    + MediaItemRepository
    + ChapterSourceRepository
    + ResourceRepository
    + ComicProgressSubjectRepository
    + ProgressRepository
    + Send
    + Sync
{
    fn as_work(&self) -> &(dyn WorkRepository + Send + Sync);
    fn as_edition(&self) -> &dyn EditionRepository;
    fn as_edition_profile(&self) -> &dyn EditionProfileRepository;
    fn as_media_item(&self) -> &dyn MediaItemRepository;
    fn as_chapter_source(&self) -> &dyn ChapterSourceRepository;
    fn as_resource(&self) -> &(dyn ResourceRepository + Send + Sync);
    fn as_comic_progress_subject(&self) -> &dyn ComicProgressSubjectRepository;
    fn as_progress(&self) -> &(dyn ProgressRepository + Send + Sync);
}

impl<T> ComicCatalogWorkPorts for T
where
    T: WorkRepository
        + EditionRepository
        + EditionProfileRepository
        + MediaItemRepository
        + ChapterSourceRepository
        + ResourceRepository
        + ComicProgressSubjectRepository
        + ProgressRepository
        + Send
        + Sync,
{
    fn as_work(&self) -> &(dyn WorkRepository + Send + Sync) {
        self
    }
    fn as_edition(&self) -> &dyn EditionRepository {
        self
    }
    fn as_edition_profile(&self) -> &dyn EditionProfileRepository {
        self
    }
    fn as_media_item(&self) -> &dyn MediaItemRepository {
        self
    }
    fn as_chapter_source(&self) -> &dyn ChapterSourceRepository {
        self
    }
    fn as_resource(&self) -> &(dyn ResourceRepository + Send + Sync) {
        self
    }
    fn as_comic_progress_subject(&self) -> &dyn ComicProgressSubjectRepository {
        self
    }
    fn as_progress(&self) -> &(dyn ProgressRepository + Send + Sync) {
        self
    }
}

/// 可选的漫画目录刷新 Receipt 读取端口。
///
/// 没有实现 `ComicCatalogRefreshOutcomeRepository` 的组装层不注入该端口，
/// 聚合根保持空 receipts 并回落到 `NeverSynced`（或按 refresh_state 推导）。
pub trait ComicCatalogRefreshReceiptPort:
    ComicCatalogRefreshOutcomeRepository + Send + Sync
{
}

impl<T> ComicCatalogRefreshReceiptPort for T where
    T: ComicCatalogRefreshOutcomeRepository + Send + Sync
{
}

/// 收藏当前状态（事务内读取；revision 为状态版本，R-FAV-001）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FavoriteState {
    pub active: bool,
    pub revision: Option<String>,
}

/// 事务内可用的收藏操作（Unit of Work 作用域；BE-APP-001 事务编排）。
pub trait FavoriteTxPorts {
    fn work_exists(&self, work_id: WorkId) -> Result<bool, AppError>;
    /// 读取收藏当前状态（active + 状态版本 revision；从未变更过返回 None）。
    fn favorite_state(&self, target: &FavoriteTarget) -> Result<Option<FavoriteState>, AppError>;
    /// 应用收藏变更并写入新 revision（状态版本语义，R-FAV-001）。
    fn apply_favorite(
        &self,
        target: &FavoriteTarget,
        on: bool,
        revision: &str,
    ) -> Result<(), AppError>;
}

/// Unit of Work 端口：把"检查 + 写入"等跨 Repository 操作包进单一事务。
/// 实现方（SqliteUnitOfWork）负责 begin/commit/rollback；闭包内不得执行异步 IO。
/// 注意：方法不能泛型（dyn 兼容性），因此返回类型固定为 `Result<(), AppError>`。
pub trait UnitOfWork: Send + Sync {
    fn run_favorite(
        &self,
        f: &dyn Fn(&dyn FavoriteTxPorts) -> Result<(), AppError>,
    ) -> Result<(), AppError>;

    /// 将来源导入的 Work、去重引用、Edition、MediaItem 与 Resource 一起提交。
    /// 闭包内不得执行异步 IO；任一步失败都必须回滚整次导入。
    fn run_source_import(
        &self,
        provider: &str,
        external_id: &str,
        work: &Work,
        edition: &Edition,
        items: &[MediaItem],
        resources: &[Resource],
    ) -> Result<(), AppError>;

    /// 原子提交一次漫画章节目录刷新。网络读取必须发生在该方法外；此处只接收
    /// 已经完成 provider 校验和 Application 匹配的同步写入计划。
    /// 默认实现给内存测试 UoW 返回不支持，真实 SQLite 实现执行 generation CAS。
    fn run_comic_chapter_refresh(&self, _plan: &ComicChapterRefreshPlan) -> Result<(), AppError> {
        Err(AppError::new(
            "COMIC_REFRESH_UOW_UNAVAILABLE",
            haven_common::ErrorKind::Internal,
            "当前 UnitOfWork 不支持漫画章节刷新事务",
            false,
        ))
    }

    /// 在同一个 SQLite Immediate 事务中提交漫画进度主体、成员、页面身份、
    /// 刷新结果、Progress CAS 和可选迁移快照。闭包/实现不得执行异步 IO。
    fn run_comic_progress_subject_write(
        &self,
        _plan: &ComicProgressSubjectWritePlan,
    ) -> Result<ComicProgressSubjectWriteResult, AppError> {
        Err(AppError::new(
            "COMIC_PROGRESS_SUBJECT_UOW_UNAVAILABLE",
            haven_common::ErrorKind::Internal,
            "当前 UnitOfWork 不支持漫画进度主体事务",
            false,
        ))
    }

    /// 与既有 Subject 聚合快照进行 Immediate-transaction CAS 的受检写入。
    fn run_checked_comic_progress_subject_write(
        &self,
        _plan: &ComicProgressSubjectWritePlan,
        _precondition: &ComicProgressSubjectWritePrecondition,
    ) -> Result<ComicProgressSubjectWriteResult, AppError> {
        Err(AppError::new(
            "COMIC_PROGRESS_SUBJECT_UOW_UNAVAILABLE",
            haven_common::ErrorKind::Internal,
            "当前 UnitOfWork 不支持受检漫画进度主体事务",
            false,
        ))
    }

    /// 原子合并两个或多个已有 Subject；默认实现供不支持事务的测试替身
    /// 明确返回 unsupported，避免静默退化为多次独立写入。
    fn run_comic_progress_subject_merge(
        &self,
        _plan: &ComicProgressSubjectMergePlan,
    ) -> Result<ComicProgressSubjectWriteResult, AppError> {
        Err(AppError::new(
            "COMIC_PROGRESS_SUBJECT_UOW_UNAVAILABLE",
            haven_common::ErrorKind::Internal,
            "当前 UnitOfWork 不支持漫画进度主体合并事务",
            false,
        ))
    }
}

#[derive(Debug, Clone)]
pub struct ComicProgressWriteCandidate {
    pub progress: Progress,
    pub expected_revision: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ComicPageIdentityWriteCandidate {
    pub media_item_id: MediaItemId,
    pub pages: Vec<PageIdentity>,
    pub expected_revision: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ComicProgressSubjectWritePlan {
    pub subject: ComicProgressSubject,
    pub members: Vec<ComicProgressSubjectMember>,
    pub page_identity_write: Option<ComicPageIdentityWriteCandidate>,
    pub progress_writes: Vec<ComicProgressWriteCandidate>,
    pub migration_snapshot: Option<ComicProgressMigrationSnapshot>,
    pub refresh_receipt: Option<ComicCatalogRefreshReceipt>,
}

/// 受检 Subject 写入的事务内前置条件。不能在 Application 的异步读取点判断后
/// 再相信旧快照；SQLite UoW 必须在同一 Immediate 事务内重新读取并比较。
#[derive(Debug, Clone)]
pub enum ComicProgressSubjectWritePrecondition {
    AbsentActiveMember {
        media_item_id: MediaItemId,
    },
    /// 首次建立一个跨来源 Subject 时，所有待绑定 MediaItem 都必须仍然没有
    /// active member。这样来源/目标两条成员的首次合并也有明确的事务内 CAS
    /// 前置条件，而不是把第二条成员的唯一索引错误当作并发协议。
    AbsentActiveMembers {
        media_item_ids: Vec<MediaItemId>,
    },
    ExactSnapshot {
        subject: ComicProgressSubject,
        members: Vec<ComicProgressSubjectMember>,
        require_authoritative_progress_none: bool,
        /// When present, the checked transaction must also prove that this
        /// MediaItem still has no Progress row before applying the plan.
        require_progress_absent_for_media_item: Option<MediaItemId>,
        /// When present, the checked transaction must also prove that the
        /// specified Progress owner still has this revision. This is used by
        /// a read-only cross-MediaItem projection: the wire revision belongs
        /// to the source row, while the write plan inserts a new target row.
        require_progress_revision_for_media_item: Option<(MediaItemId, String)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComicProgressSubjectWriteResult {
    pub applied_progress_revisions: Vec<String>,
    pub migration_id: Option<ComicProgressMigrationId>,
    pub refresh_id: Option<ComicCatalogRefreshId>,
}

/// 参与 Subject 合并的旧聚合快照。
///
/// `expected_subject`/`expected_members` 只用于 Immediate 事务内 CAS；真正写入
/// 的新状态位于 [`ComicProgressSubjectMergePlan`] 的 survivor/redirected_subjects。
#[derive(Debug, Clone)]
pub struct ComicProgressSubjectMergeExpected {
    pub expected_subject: ComicProgressSubject,
    pub expected_members: Vec<ComicProgressSubjectMember>,
}

/// 两个或多个已有 Subject 的原子合并计划。
///
/// loser 不会被删除：Application 先把它们变为 Redirected，并将 active member
/// 退休；survivor 再吸收这些成员。可选的 Progress/snapshot 写入与这组 Subject
/// 状态共享同一个 Immediate 事务。
#[derive(Debug, Clone)]
pub struct ComicProgressSubjectMergePlan {
    pub survivor: ComicProgressSubject,
    pub survivor_members: Vec<ComicProgressSubjectMember>,
    pub redirected_subjects: Vec<ComicProgressSubject>,
    pub expected_subjects: Vec<ComicProgressSubjectMergeExpected>,
    pub progress_writes: Vec<ComicProgressWriteCandidate>,
    pub migration_snapshot: Option<ComicProgressMigrationSnapshot>,
}

/// 一个需要一起落库的漫画 Edition 及其身份画像。
#[derive(Debug, Clone)]
pub struct ComicEditionWrite {
    pub edition: Edition,
    pub profile: EditionProfile,
}

/// 漫画目录刷新/首次章节化导入的完整事务计划。
///
/// `expected_generation=0` 表示首次建立目录；已有状态则必须精确匹配，提交后
/// `state.generation` 应为 expected + 1。计划中的 chapter_refs 可以包含旧章节的
/// `Missing` 保留记录，但不会删除任何 MediaItem、Progress、Marker 或 History。
#[derive(Debug, Clone)]
pub struct ComicChapterRefreshPlan {
    pub source_key: String,
    pub remote_work_id: String,
    pub expected_generation: u64,
    pub state: ComicChapterCatalogState,
    pub work: Work,
    pub editions: Vec<ComicEditionWrite>,
    pub items: Vec<MediaItem>,
    pub resources: Vec<Resource>,
    pub chapter_refs: Vec<ChapterSourceRef>,
    /// Successful refreshes append this receipt in the same SQLite
    /// transaction as the catalog rows. Failed observations are persisted by
    /// the application after the catalog transaction has rolled back.
    pub refresh_receipt: Option<ComicCatalogRefreshReceipt>,
}

/// Enrichment 流水线所需端口（契约 §36.8）。
pub trait EnrichmentPorts: WorkRepository + EnrichmentRepository + Send + Sync {}
impl<T> EnrichmentPorts for T where T: WorkRepository + EnrichmentRepository + Send + Sync {}

/// Application 侧的远端正文获取端口。实现层负责固定主机、响应校验和
/// 临时文件写入；Application/Download Worker 不接触 URL、Cookie 或 Provider
/// 的内部请求细节。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteAcquiredFile {
    pub size_bytes: u64,
    pub mime: String,
}

/// A single inclusive byte range requested by a remote reader.  The end is
/// optional because HTTP permits an open-ended `bytes=start-` request; the
/// infrastructure provider remains responsible for enforcing its own maximum
/// response size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteByteRange {
    pub start: u64,
    pub end: Option<u64>,
}

/// The bounded response returned by a remote reading session.  The body is
/// kept inside the application/interface boundary and is never serialized as
/// JSON IPC; callers use the controlled `haven-resource` protocol instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSessionBody {
    pub mime_type: String,
    pub bytes: Vec<u8>,
    pub total_size: u64,
    pub content_range: Option<RemoteContentRange>,
    pub accept_ranges: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteContentRange {
    pub start: u64,
    pub end: u64,
    pub total: u64,
}

#[async_trait]
pub trait RemoteAcquisitionPort: Send + Sync {
    async fn acquire(
        &self,
        source_key: &str,
        remote_id: &str,
        destination: &Path,
    ) -> Result<RemoteAcquiredFile, AppError>;
}

/// Controlled remote read port used by Article/PDF sessions.  Implementations
/// own the provider URL, redirect and response validation; the application
/// only passes the opaque source identity and an optional bounded range.
#[async_trait]
pub trait RemoteSessionPort: Send + Sync {
    async fn read(
        &self,
        source_key: &str,
        remote_id: &str,
        range: Option<RemoteByteRange>,
    ) -> Result<RemoteSessionBody, AppError>;
}
