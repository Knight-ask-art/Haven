//! Application 端口：组合 Repository 契约，供具体实现（SqliteRepositories）注入。
//!
//! 依赖方向：Application → Domain 契约；具体实现由组装层（src-tauri / 测试）提供。

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use haven_common::{AppError, ErrorKind, UtcMillis};
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
use haven_domain::entities::{
    ArtworkRef, Edition, FavoriteTarget, MediaItem, Progress, Resource, Work,
};
use haven_domain::enums::MediaType;
use haven_domain::ids::{ComicCatalogRefreshId, ComicProgressMigrationId, MediaItemId, WorkId};
use haven_domain::periodical::{Periodical, PeriodicalArticle, PeriodicalIssue, PeriodicalVolume};

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

    /// 在同一个 SQLite 事务中提交 期刊 → 卷 → 期 → 文章 与
    /// Work → Edition → MediaItem → Resource 的整条导入链。
    ///
    /// 不允许用多次独立 save 假装原子：任一层的失败都必须回滚整次导入，
    /// 不留下来源引用或孤儿层级。默认实现给不支持该事务的测试替身返回
    /// 明确的 Unsupported。
    fn run_periodical_import(&self, _plan: &PeriodicalImportPlan) -> Result<(), AppError> {
        Err(AppError::new(
            "PERIODICAL_IMPORT_UOW_UNAVAILABLE",
            haven_common::ErrorKind::Internal,
            "当前 UnitOfWork 不支持报刊导入事务",
            false,
        ))
    }

    /// 把一次「自动补全」的缺失元数据补丁应用到**既有** Work。
    ///
    /// 补丁只提供候选值，因此实现必须在同一事务内重新读取目标 Work 后**只补空缺**：
    /// 不改写已有值（含用户编辑），不改变 identity、Work 类型或任何本地关系，也不
    /// 新建 Work/Edition/MediaItem/Resource。默认实现给不支持该事务的测试替身返回
    /// 明确的 Unsupported，避免退化成事务外的读-改-写或整包覆盖写。
    fn run_work_enrichment(
        &self,
        _plan: &WorkEnrichmentPlan,
    ) -> Result<WorkEnrichmentOutcome, AppError> {
        Err(AppError::new(
            "WORK_ENRICHMENT_UOW_UNAVAILABLE",
            haven_common::ErrorKind::Internal,
            "当前 UnitOfWork 不支持作品元数据补全事务",
            false,
        ))
    }
}

/// 一次「自动补全」对既有 Work 的**元数据补丁**。
///
/// 只携带来源条目的展示型元数据与受控海报引用：不含播放地址、集数、远端请求细节
/// 或本地路径。语义是「只补空缺」——实现方绝不能覆盖 Work 上已有的值，也不能据此
/// 创建任何新的 Work/Edition/MediaItem/Resource。
#[derive(Debug, Clone, PartialEq)]
pub struct WorkEnrichmentPlan {
    pub work_id: WorkId,
    /// 事务内 CAS 前置条件：目标 Work 的标题必须仍与该值完全相等。标题在事务外被
    /// 改写（用户改名）后，本次匹配结果不再可信，整个补丁必须放弃。
    pub expected_title: String,
    /// 来源条目的媒介类型（Application 已按 `type_name`/集数推导）。实现方据此拒绝
    /// 把视频元数据套到非视频作品（例如同名书籍）上。
    pub remote_media_type: MediaType,
    /// 以下字段是候选值：`None` 表示来源没有该字段，`Some` 也只用于补空缺。
    pub description: Option<String>,
    pub release_year: Option<i32>,
    pub director: Option<String>,
    pub actor: Option<String>,
    /// 受控海报引用（`haven://artwork/<id>`）；外部 URL 已在事务外经图片代理注册。
    pub poster: Option<ArtworkRef>,
    /// 可选的来源身份绑定；只在安全时（目标 Work 尚无任何来源引用且该身份未被其它
    /// Work 占用）由实现方绑定，见 [`WorkEnrichmentOutcome`]。
    pub source_ref: Option<WorkEnrichmentSourceRef>,
    pub updated_at: UtcMillis,
}

/// 自动补全可以绑定的来源身份（`work_source_refs` 的键）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkEnrichmentSourceRef {
    pub provider: String,
    pub external_id: String,
}

/// 一次元数据补丁的事务内结果。
///
/// 前两项都表示「本次补全成立」（来源条目与目标 Work 匹配），区别只在有没有真正写入；
/// 后三项表示补丁被放弃，调用方不得把状态记成已补全。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkEnrichmentOutcome {
    /// 至少写入了一列（元数据或来源身份）。
    Applied,
    /// 目标 Work 已具备全部候选字段且没有可绑定的来源身份：没有任何写入。
    NothingToFill,
    /// 目标 Work 已不存在（删除竞态）。
    WorkMissing,
    /// 目标 Work 的标题已变化，本次匹配结果不再可信。
    TitleChanged,
    /// 目标 Work 的版本类型与来源条目的媒介类型不同类（例如同名书籍与视频）。
    MediaTypeMismatch,
}

/// 一次报刊文章导入的完整原子写入计划。
///
/// 计划已包含 Work → Edition → MediaItem → Resource 与 期刊 → 卷 → 期 → 文章
/// 的全部行；Application 在事务外完成 Provider 校验与身份匹配，SQLite UoW 在
/// 单一事务内写入并重新校验归属与来源一致性。计划不携带 URL、Cookie、请求头
/// 或本地路径。
#[derive(Debug, Clone)]
pub struct PeriodicalImportPlan {
    pub source_key: String,
    /// 期刊的全部来源绑定身份（由 print/electronic ISSN 派生）：同一个 Work
    /// 可以保留多个同来源身份，重复导入同一期刊必须命中同一 Work。
    pub journal_source_refs: Vec<String>,
    pub work: Work,
    pub edition: Edition,
    pub periodical: Periodical,
    pub volume: PeriodicalVolume,
    pub issue: PeriodicalIssue,
    pub article: PeriodicalArticle,
    pub item: MediaItem,
    pub resource: Resource,
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
        subject: Box<ComicProgressSubject>,
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

/// 一次远端获取的**真实网络字节**进度快照。
///
/// `bytes` 是**当前产物尝试**已经读到的字节（限速与速率仍按累计网络字节计算）：
/// 已知总大小时它夹在总大小内，且绝不把失败/被丢弃尝试读到的字节算进完成度；
/// 总大小未知时它就是本次获取累计读取的网络字节，此时不宣称任何百分比。
/// 它**不是**最终产物大小：转换类来源（订阅源、Europe PMC、Wikisource）的网络字节
/// 与落盘产物不同，最终产物大小只由 [`RemoteAcquiredFile::size_bytes`] 给出。
/// `total_bytes` 未知时为 `None`（诚实表达未知），绝不拿单页/单次响应大小冒充整体。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RemoteAcquisitionProgress {
    pub bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bps: Option<u64>,
    pub eta_seconds: Option<u64>,
}

/// 远端获取进度观察者。
///
/// 实现方会在网络读取路径上被同步调用，因此**只能保存最新值**：不得执行阻塞 IO、
/// 数据库写入或 IPC 发布；持久化与事件发布由调用方按界（例如 100ms）完成。
pub trait RemoteAcquisitionObserver: Send + Sync {
    fn on_progress(&self, progress: RemoteAcquisitionProgress);
}

/// 一次受控远端获取的作用域上下文：有限速率（字节/秒）+ 进度上报。
///
/// 同一个 `acquire_controlled` 调用内的多页下载、重试与主机回退**必须共享**同一个
/// 实例：限速按累计网络字节计算，不随单页或单次尝试重置。获取产物之前为定位产物而
/// 发出的元数据/目录请求（条目页、页清单）同样必须携带这个实例：它们只计网络字节，
/// 不计入产物进度。上下文只在该次获取内存在，不使用全局状态或任务局部上下文。
///
/// 限速语义：`record` 在**网络读取之外**按累计字节补齐应等待的时长，因此调用方
/// 必须把每块网络等待（连接/响应头/单块读取）各自包在独立超时里，不能用一个总
/// 超时同时约束限速睡眠——否则大文件在有限速率下必然超时失败。
pub struct RemoteTransferContext<'a> {
    limit_bps: Option<u64>,
    observer: &'a dyn RemoteAcquisitionObserver,
    started: Instant,
    state: Mutex<RemoteTransferState>,
}

#[derive(Debug, Clone, Copy, Default)]
struct RemoteTransferState {
    /// 累计网络字节：限速与速率的唯一依据，跨重试/主机回退**从不重置**。
    network_bytes: u64,
    /// 当前**产物**尝试已经读到的字节；每次尝试（重试/主机回退）开始时归零。
    artifact_bytes: u64,
    total_bytes: Option<u64>,
}

impl<'a> RemoteTransferContext<'a> {
    /// `limit_bps`：字节/秒；`None` 表示不限速（仍然上报进度）。
    pub fn new(limit_bps: Option<u64>, observer: &'a dyn RemoteAcquisitionObserver) -> Self {
        Self {
            limit_bps: limit_bps.filter(|limit| *limit > 0),
            observer,
            started: Instant::now(),
            state: Mutex::new(RemoteTransferState::default()),
        }
    }

    /// 本次获取生效的有限速率（字节/秒）；`None` 表示不限速。
    pub fn limit_bps(&self) -> Option<u64> {
        self.limit_bps
    }

    /// 声明**当前产物尝试**的已知总大小。只有「响应体就是最终产物」的获取可以声明；
    /// 分页下载与转换类来源必须保持 `None`（未知总大小是诚实值）。
    /// 每次新的产物尝试（[`RemoteTransferContext::begin_artifact_attempt`]）都会把它
    /// 清回未知，避免上一次尝试的响应大小冒充这一次的整体大小。
    pub fn set_total_bytes(&self, total_bytes: Option<u64>) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .total_bytes = total_bytes;
    }

    /// 开始一次**产物**获取尝试：产物进度归零，已知总大小回到未知。
    ///
    /// 只由「响应体就是最终产物」的获取在每次尝试（传输层重试、主机回退、逐页读取）
    /// 开始时调用。累计网络字节与限速依据**不重置**：被丢弃尝试读到的字节仍然计入
    /// 网络上限，只是不再冒充当前产物的完成度。
    pub fn begin_artifact_attempt(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.artifact_bytes = 0;
        state.total_bytes = None;
    }

    /// 记录一次**非产物**网络读取的字节数（目录页、条目元数据、分页响应、转换类正文）：
    /// 只计入限速与速率，不计入产物进度——元数据字节绝不能冒充产物完成度。
    /// 先记账，再按有限速率补齐应等待的时长，最后把最新进度交给观察者；
    /// 等待发生在网络读取之外。
    pub async fn record(&self, bytes: u64) {
        self.record_read(bytes, false).await;
    }

    /// 记录一次**产物**响应体的网络读取：同时计入限速、速率与当前产物尝试的进度。
    pub async fn record_artifact(&self, bytes: u64) {
        self.record_read(bytes, true).await;
    }

    async fn record_read(&self, bytes: u64, artifact: bool) {
        let network_bytes = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.network_bytes = state.network_bytes.saturating_add(bytes);
            if artifact {
                state.artifact_bytes = state.artifact_bytes.saturating_add(bytes);
            }
            state.network_bytes
        };
        if let Some(limit) = self.limit_bps {
            let delay = transfer_delay(self.started.elapsed(), network_bytes, limit);
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }
        self.observer.on_progress(self.progress());
    }

    /// 当前进度快照。
    ///
    /// 已知总大小时上报的是**当前产物尝试**读到的字节（夹在总大小内，不超过 100%）：
    /// 失败尝试读到的字节已经计入网络与限速，但绝不能与当前尝试的产物字节相加，
    /// 否则回退/重试后的累计网络字节一碰到总大小就会谎报 100%。总大小未知时保持
    /// 未知，只报累计网络字节，不伪造百分比。
    pub fn progress(&self) -> RemoteAcquisitionProgress {
        let state = *self.state.lock().unwrap_or_else(|error| error.into_inner());
        let elapsed = self.started.elapsed().as_secs_f64();
        let speed_bps =
            (elapsed > 0.0).then(|| (state.network_bytes as f64 / elapsed).round() as u64);
        let bytes = match state.total_bytes {
            Some(total) => state.artifact_bytes.min(total),
            None => state.network_bytes,
        };
        let eta_seconds = state.total_bytes.and_then(|total| {
            speed_bps
                .filter(|speed| *speed > 0)
                .map(|speed| total.saturating_sub(bytes).div_ceil(speed))
        });
        RemoteAcquisitionProgress {
            bytes,
            total_bytes: state.total_bytes,
            speed_bps,
            eta_seconds,
        }
    }
}

/// 按累计字节与有限速率推算还需等待的时长（限速睡眠不进入任何网络超时窗口）。
fn transfer_delay(elapsed: Duration, bytes: u64, limit_bps: u64) -> Duration {
    if limit_bps == 0 || bytes == 0 {
        return Duration::ZERO;
    }
    Duration::from_secs_f64(bytes as f64 / limit_bps as f64).saturating_sub(elapsed)
}

#[async_trait]
pub trait RemoteAcquisitionPort: Send + Sync {
    /// 既有获取路径：无观察者、无显式限速。保留以兼容既有调用方与夹具；
    /// 生产 Provider 必须同时实现 [`RemoteAcquisitionPort::acquire_controlled`]。
    async fn acquire(
        &self,
        source_key: &str,
        remote_id: &str,
        destination: &Path,
    ) -> Result<RemoteAcquiredFile, AppError>;

    /// 受控获取：把有限速率约束贯穿到**真实网络字节读取路径**（不是获取完成后的
    /// 补睡眠），并按界上报进度。取消语义与 `acquire` 一致：future 被丢弃即中止。
    ///
    /// 默认实现只在**没有**有限速率约束时退化为 `acquire`；一旦调用方施加了有限
    /// 速率，默认实现必须显式失败，绝不静默绕过限速。
    async fn acquire_controlled(
        &self,
        source_key: &str,
        remote_id: &str,
        destination: &Path,
        transfer: &RemoteTransferContext<'_>,
    ) -> Result<RemoteAcquiredFile, AppError> {
        if transfer.limit_bps().is_some() {
            return Err(AppError::new(
                "INTERNAL_ERROR",
                ErrorKind::Internal,
                "该来源尚未实现受控获取路径，无法应用下载限速",
                false,
            ));
        }
        self.acquire(source_key, remote_id, destination).await
    }
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
