//! 云盘账户 / 目录 / 对象绑定的服务端事实与仓储契约（Google Drive 只读切片）。
//!
//! 边界：
//! - 真实 Provider ID（账户、目录、文件）大小写敏感，只允许存在于 Rust 侧与对应表；
//!   它们不进入 Wire、日志或前端状态。`StorageLocation.root_ref` 只写**内部 UUID**，
//!   因此既有 008 的 `lower(root_ref)` 唯一索引保持原样、与云盘 ID 无关。
//! - 明文令牌、access_token、scope、授权码与回调 query 一律不落库：账户行只保存
//!   不透明 `CredentialRef`（`haven:google_drive:<random-profile-id>`），秘密只在
//!   CredentialStore；凭据清理 outbox 同样只保存 ref。
//! - 资源复用既有 `ResourceLocator::StorageObject`，其 `object_id` 是**内部对象
//!   UUID**，绝不是 Provider file ID；不新增 CloudObject 枚举。
//! - 每个方法 = 一个短事务。OAuth / Drive HTTP / CredentialStore / FS 等 IO 由调用方
//!   在事务外先完成；任何实现都不得把网络 IO 放进事务。
//! - CAS 前置条件不满足是**显式结果**（[`CloudCasOutcome`]），不是 bool。
//!
//! 容量门禁（实现必须在写入事务内计数并拒绝，不允许无界表）：
//! [`MAX_CLOUD_ACCOUNTS`] / [`MAX_CLOUD_FOLDERS_PER_ACCOUNT`] /
//! [`MAX_CLOUD_OBJECTS_PER_FOLDER`]。

use async_trait::async_trait;

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::enums::StorageStatus;
use haven_domain::ids::{CredentialRef, MediaItemId, ResourceId, StorageLocationId};

/// 只读 PDF 对象的字节上限（128 MiB）：单文件只读消费，不做任意大小下载。
pub const CLOUD_OBJECT_MAX_BYTES: u64 = 128 * 1024 * 1024;

/// 同时存在的云盘账户上限。
pub const MAX_CLOUD_ACCOUNTS: u32 = 8;
/// 单个账户可登记的目录上限。
pub const MAX_CLOUD_FOLDERS_PER_ACCOUNT: u32 = 32;
/// 单个目录可索引的对象上限。
pub const MAX_CLOUD_OBJECTS_PER_FOLDER: u32 = 20_000;
/// 单次读取凭据清理 outbox 的上限。
pub const CLOUD_CREDENTIAL_CLEANUP_BATCH: u32 = 64;
pub const MAX_CLOUD_STAGED_CREDENTIALS: u32 = 256;
pub const CLOUD_CREDENTIAL_STAGE_TTL_MS: i64 = 600_000;

/// 云盘账户（服务端事实）。一个账户只有**一份**凭据，可被多个目录共享。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAccount {
    /// 内部 UUID 字符串；与任何 Provider 标识无关。
    pub id: String,
    /// Provider 键（本切片为 `google_drive`；凭据 ref 的作用域同源）。
    pub provider: String,
    /// Provider 侧账户 ID（Google `user.permissionId`）；大小写敏感、服务端专用。
    pub provider_account_id: String,
    pub display_name: String,
    /// 授权代际：恒 > 0；connect / disconnect 递增，CAS 与快照都以它为准。
    pub generation: i64,
    pub connected: bool,
    /// 不透明凭据引用；断开后保留到 keystore 删除完成（ADR-001 顺序）。
    pub credential_ref: Option<CredentialRef>,
    pub updated_at: UtcMillis,
}

/// 连接 / 重新授权请求（只含非敏感事实；凭据另以 ref 传入）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudConnectRequest {
    pub provider: String,
    pub provider_account_id: String,
    pub display_name: String,
}

/// 目录绑定：某账户下的一个 Provider 文件夹 ↔ 一个内部 StorageLocation。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudFolderBinding {
    pub location_id: StorageLocationId,
    pub account_id: String,
    /// Provider folder ID；大小写敏感、服务端专用。
    pub provider_folder_id: String,
    /// 写入 `StorageLocation.root_ref` 的内部 UUID。绝不用 Provider ID：真实 ID 大小写
    /// 敏感，会被 `lower(root_ref)` 唯一索引错误折叠成同一个位置。
    pub root_ref: String,
    pub created_at: UtcMillis,
}

/// 目录登记请求：只携带 Provider 侧事实；内部 location / root_ref UUID 由服务端生成。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudFolderRequest {
    pub account_id: String,
    pub provider_folder_id: String,
}

/// 对象绑定：一个云盘文件 ↔ 一条真实 Resource（索引行，不复制内容）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudObjectBinding {
    /// 内部对象 UUID；即 `ResourceLocator::StorageObject.object_id`。
    pub id: String,
    pub location_id: StorageLocationId,
    /// Provider file ID；大小写敏感、服务端专用。
    pub provider_file_id: String,
    pub display_name: String,
    pub size_bytes: u64,
    pub resource_id: ResourceId,
    pub media_item_id: MediaItemId,
}

/// 读取 / 导入前的绑定快照（租约）：账户代际 + 位置状态 + 对象行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudObjectSnapshot {
    pub object: CloudObjectBinding,
    pub folder: CloudFolderBinding,
    pub account_id: String,
    pub account_generation: i64,
    pub location_status: StorageStatus,
}

/// PDF 导入候选（来自 Provider stat；不含路径、URL 或令牌）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudPdfCandidate {
    pub provider_file_id: String,
    pub display_name: String,
    pub size_bytes: u64,
}

/// CAS 结果：条件不满足是显式结果，调用方必须重新读取后再决定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudCasOutcome<T> {
    /// 条件成立并已提交；返回提交后的权威状态。
    Applied(T),
    /// 代际 / 凭据 ref / connected 已变化（并发冲突）。
    Stale,
    /// 目标账户或绑定不存在。
    Missing,
}

/// [`CloudStorageRepository::finish_credential_cleanup`] 的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudCredentialCleanupOutcome {
    /// ready outbox 行已删除，且账户的断开引用已清除。
    Cleared,
    /// ready outbox 行已删除，但账户引用已换成新的：**保留**当前引用不动。
    RefRetained,
    /// outbox 中本就没有 ready 引用（重复清理幂等；不得消费 staged）。
    AlreadyClean,
}

/// [`CloudStorageRepository::remove_folder`] 的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudFolderRemoveOutcome {
    /// 该位置的应用内索引（Resource + 仅由该位置派生的孤儿内容链与用户状态）、下载元数据、
    /// 对象 / 目录绑定与位置行已按与本地位置相同的 remove 语义清理；远端文件、共享账户凭据、
    /// 其他目录与仍被其他位置引用的共享内容不受影响。
    Removed,
    /// 位置不存在或不是云盘目录（幂等成功，不做任何事）。
    Absent,
}

/// [`CloudStorageRepository::import_pdf`] 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudPdfImportOutcome {
    /// 新建了内容链与对象绑定。
    Created(CloudObjectBinding),
    /// 同一 (location, provider_file_id) 已有绑定：刷新 Provider 文件名 / 大小，不重复建内容。
    /// 内容链身份、用户标题与 availability 保留；元数据原样命中不写入。
    Existing(CloudObjectBinding),
}

/// 快照 / 代际已失效（断开、换代或绑定被移除）时的稳定错误。
pub fn cloud_binding_stale() -> AppError {
    AppError::new(
        "CLOUD_BINDING_STALE",
        ErrorKind::Conflict,
        "云盘绑定已失效，请重新读取后再试",
        true,
    )
}

/// 云盘账户 / 目录 / 对象绑定的唯一持久化契约。
///
/// 实现方（Infrastructure SQLite）必须：
/// - 每个方法一个短事务，`expected_*` 前置条件在**同一事务内**重读校验（读→校验→写
///   原子）；不满足即返回 [`CloudCasOutcome::Stale`] / [`CloudCasOutcome::Missing`]，
///   绝不「最后写入获胜」；
/// - 真实 Provider ID 一律 `COLLATE BINARY` 精确比较；
/// - 不把凭据明文、Provider ID 或本地路径写进错误消息、日志或任何 Wire 结构。
#[async_trait]
pub trait CloudStorageRepository: Send + Sync {
    // ---------- 账户 ----------
    /// 全部账户（含断开墓碑），按 `updated_at` 倒序。
    async fn list_accounts(&self) -> Result<Vec<CloudAccount>, AppError>;
    async fn get_account(&self, account_id: &str) -> Result<Option<CloudAccount>, AppError>;
    /// 按 Provider 身份精确查找（大小写敏感，不得用 lower/默认排序规则）。
    async fn get_account_by_provider(
        &self,
        provider: &str,
        provider_account_id: &str,
    ) -> Result<Option<CloudAccount>, AppError>;

    // ---------- 凭据清理 outbox ----------
    /// 在 `CredentialStore::set` **之前**登记待清理引用：即使写 keystore 与 DB 提交之间
    /// 崩溃，也存在可重放的清理记录。outbox 只保存不透明 ref，不保存秘密。
    async fn stage_credential(&self, credential: &CredentialRef) -> Result<(), AppError>;
    /// 失败的 set / CAS 写入转为 ready。不得退休任何 connected 账户当前使用的引用。
    /// 若暂存记录已被回收，则重建 ready 记录，保证迟到的 set 也有补偿清理所有者。
    async fn retire_staged_credential(&self, credential: &CredentialRef) -> Result<(), AppError>;
    /// 恢复超过固定 staging TTL 的非活动暂存引用；一批最多 cleanup batch。
    /// 清理领取后 connect / replace 的 staged 前置条件必须失败，调用方补偿迟到写入。
    async fn recover_staged_credentials(&self, now: UtcMillis) -> Result<u32, AppError>;
    /// 仅列出 ready 引用（登记顺序，最多 `limit` 条），staged 从不进入普通删除列表。`limit` 超过
    /// [`CLOUD_CREDENTIAL_CLEANUP_BATCH`] 必须被拒绝，不接受无界读取。
    async fn pending_credentials(&self, limit: u32) -> Result<Vec<CredentialRef>, AppError>;
    /// keystore 删除成功后的收尾：只移除 ready 行；**仅当**账户仍 `connected = false`
    /// 且 `credential_ref` 仍等于该引用时才清空它（ADR-001：绝不提前清掉活动引用）。
    async fn finish_credential_cleanup(
        &self,
        credential: &CredentialRef,
    ) -> Result<CloudCredentialCleanupOutcome, AppError>;

    // ---------- 账户生命周期（CAS） ----------
    /// 连接 / 重新授权。`expected_prior_generation = None` 表示「尚无该账户」，
    /// `Some(gen)` 表示期望当前代际。成功时写入**新代际**（> 旧代际）、
    /// `connected = true`、`credential_ref = new_credential`；旧引用（若有）移入 outbox，
    /// 同一事务要求 new_credential 仍是 staged 且未超 TTL，并消耗其记录（先 stage + store.set）。
    /// 同一账户的全部目录位置一并回到 `Connected`。账户 / 目录数超出
    /// [`MAX_CLOUD_ACCOUNTS`] / [`MAX_CLOUD_FOLDERS_PER_ACCOUNT`] 时拒绝。
    async fn connect(
        &self,
        request: &CloudConnectRequest,
        expected_prior_generation: Option<i64>,
        new_credential: &CredentialRef,
    ) -> Result<CloudCasOutcome<CloudAccount>, AppError>;
    /// 仅刷新凭据（不换代）：要求 `connected = true`、代际匹配、当前 ref 等于
    /// `expected_credential`。同一事务要求新引用尚为 staged 且未超 TTL；成功后旧 ref ready、新 staged 消耗。
    async fn replace_credential(
        &self,
        account_id: &str,
        expected_generation: i64,
        expected_credential: &CredentialRef,
        new_credential: &CredentialRef,
    ) -> Result<CloudCasOutcome<CloudAccount>, AppError>;
    /// 断开：墓碑 `connected = false` + 代际递增；**保留** `credential_ref` 并把它登记进
    /// outbox（keystore 删除由调用方在事务外完成，随后 `finish_credential_cleanup`）。
    /// 该账户全部位置转 `Disconnected`，其下**非 user 来源**的资源标记
    /// `StorageUnavailable`。不删除任何远端文件，也不删除账户 / 目录行。
    async fn disconnect(
        &self,
        account_id: &str,
        expected_generation: i64,
    ) -> Result<CloudCasOutcome<CloudAccount>, AppError>;

    // ---------- 目录 ----------
    async fn get_folder(
        &self,
        location_id: StorageLocationId,
    ) -> Result<Option<CloudFolderBinding>, AppError>;
    /// 登记目录（幂等）：要求账户存在、`connected`、代际匹配。同一
    /// (account, provider_folder_id) 已存在时**原样返回既有绑定**（不新建位置、不覆盖）。
    /// 新建时生成内部 location UUID 与 root_ref UUID，写入
    /// `provider_type = google_drive`、`status = connected`、`credential_ref = NULL` 的
    /// StorageLocation（凭据只在账户行），位置 display_name 取 `display_name`。
    async fn register_folder(
        &self,
        request: &CloudFolderRequest,
        expected_generation: i64,
        display_name: &str,
    ) -> Result<CloudCasOutcome<CloudFolderBinding>, AppError>;
    /// 移除目录：同一事务内执行与本地位置 remove **完全相同**的应用内清理（删除该位置
    /// Resource 与仅由该位置派生的孤儿内容链及其用户状态，复用同一份 purge 实现），并连带
    /// 清理该位置的下载元数据与云盘绑定行。远端（Provider）文件绝不删除；**不触碰共享账户
    /// 凭据**，也不影响同账户其他目录或仍被其他位置引用的共享内容。
    async fn remove_folder(
        &self,
        location_id: StorageLocationId,
    ) -> Result<CloudFolderRemoveOutcome, AppError>;

    // ---------- 对象 ----------
    async fn get_object(&self, object_id: &str) -> Result<Option<CloudObjectBinding>, AppError>;
    /// 读取 / 导入前取得快照（含账户代际与位置状态），供网络 IO 前后校验。
    async fn object_snapshot(
        &self,
        object_id: &str,
    ) -> Result<Option<CloudObjectSnapshot>, AppError>;
    /// IO 后复核：账户代际、位置状态与对象行都必须与快照一致；不一致返回
    /// [`cloud_binding_stale`]，调用方必须丢弃本次 IO 结果，不得凭旧凭据重试。
    async fn assert_current(
        &self,
        expected_account_generation: i64,
        snapshot: &CloudObjectSnapshot,
    ) -> Result<(), AppError>;
    /// 把已通过 stat 的 PDF 导入统一内容模型：同一事务写入真实
    /// Work / Edition / MediaItem / Resource + 对象绑定。类型走既有扫描器语义
    /// （`MediaType::Document`、`ResourceType::PublicationFile`、`application/pdf`；
    /// `category` 由 Repository 从 media_type 推导为 `periodical`），Resource 用
    /// `ResourceLocator::StorageObject` 且 `object_id` 是内部对象 UUID。
    /// 同一 (location, provider_file_id) 已存在 → `Existing`，只刷新 Provider 文件名 / 大小
    /// 与 Resource 大小，不重复建内容或覆盖用户标题；原样命中不写入。
    /// `size_bytes` 必须落在 `1..=`[`CLOUD_OBJECT_MAX_BYTES`]。
    async fn import_pdf(
        &self,
        binding: &CloudFolderBinding,
        expected_generation: i64,
        file: &CloudPdfCandidate,
    ) -> Result<CloudCasOutcome<CloudPdfImportOutcome>, AppError>;
}
