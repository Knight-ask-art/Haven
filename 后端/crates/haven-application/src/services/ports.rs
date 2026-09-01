//! Application 端口：组合 Repository 契约，供具体实现（SqliteRepositories）注入。
//!
//! 依赖方向：Application → Domain 契约；具体实现由组装层（src-tauri / 测试）提供。

use std::path::Path;

use async_trait::async_trait;
use haven_common::AppError;
use haven_domain::contracts::{
    EditionRepository, EnrichmentRepository, FavoriteRepository, MarkerRepository,
    MediaItemRepository, ProgressRepository, ResourceRepository, SettingsRepository,
    StorageLocationRepository, WorkRepository,
};
use haven_domain::entities::{Edition, FavoriteTarget, MediaItem, Resource, Work};
use haven_domain::ids::WorkId;

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
    + MarkerRepository
    + haven_domain::contracts::WorkRelationRepository
    + Send
    + Sync
{
}
impl<T> WorkGetPorts for T where
    T: LibraryPorts
        + ResourceRepository
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
    + MediaItemRepository
    + ResourceRepository
    + haven_domain::contracts::ImageProxyRepository
    + Send
    + Sync
{
}
impl<T> SourceImportPorts for T where
    T: WorkRepository
        + EditionRepository
        + MediaItemRepository
        + ResourceRepository
        + haven_domain::contracts::ImageProxyRepository
        + Send
        + Sync
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
