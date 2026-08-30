//! StorageLocationService：存储位置 Use Case（BE-STORAGE-001 + R-MAIN-02/03 复审修复）。
//!
//! 规范：LIBRARY_AND_STORAGE §14–§17；本任务只实现 Local 第一阶段。
//! 原则：
//! - 前端只接触 opaque `StorageLocationId`；扫描/路径接口不接受裸路径。
//! - `add_local` 接收**可信 Native 目录选择流程**（rfd）的路径；后端仍完成绝对路径、
//!   规范化、存在性与可读性检查；**拒绝 UNC/网络共享路径**（本地第一阶段）。
//! - 同一规范化目录重复添加幂等返回既有 ID；DB 表达式唯一索引兜底并发（P1-6）。
//! - `get_scan_target` 从 DB 读取位置与路径，**事务外**探测（canonicalize/read_dir），
//!   **短事务内**重读并校验 `id + root_ref + status` 快照后才写状态（R-MAIN-03：
//!   慢盘/网络盘卡顿不再冻结全局 DB 锁）；返回**本次验证过的 canonical path**。
//! - **可用性来源（R-MAIN-02）**：位置级标记只作用于 `availability_source != user`
//!   的资源（`Storage`/`Unknown`）；用户/扫描器显式标记（SourceUnavailable /
//!   TemporarilyUnavailable / Unknown / 资源自身 Missing）**绝不**被位置级操作覆盖。
//!   资源逐项恢复由重扫（BE-SCAN-001）验证，位置级操作不做"全量改回来"。
//! - `disconnect` / `remove` / Missing 迁移 / 恢复 全部在 **单一事务（Unit of Work）** 内
//!   完成（P0-5：失败回滚，重试不会漏副作用）。
//! - `remove` 只删除应用内绑定/索引关系，**绝对禁止删除用户原始媒体目录或原文件**。
//! - WebDAV/远程 Provider 暂不实现（保留扩展边界）。

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use haven_common::AppError;
use haven_domain::entities::{Resource, ResourceLocator, StorageLocation};
use haven_domain::enums::{Availability, AvailabilitySource, StorageProviderType, StorageStatus};
use haven_domain::ids::StorageLocationId;

/// 不可伪造快照 token（R-MAIN-09B）：由 `get_scan_target` 短事务中 **authoritative current**
/// 位置生成，绑定 `storage_location_id` + `provider=Local` + `root_ref` + `status=Connected` +
/// `updated_at`（复合比较，避免仅 updated_at 毫秒碰撞）。字段私有、构造仅 application 内可见，
/// 其他 crate 只能通过只读 accessor 校验，无法用 struct literal 伪造。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanTargetToken {
    storage_location_id: StorageLocationId,
    provider: StorageProviderType,
    root_ref: String,
    status: StorageStatus,
    updated_at: haven_common::UtcMillis,
}

impl ScanTargetToken {
    pub(crate) fn new(
        storage_location_id: StorageLocationId,
        provider: StorageProviderType,
        root_ref: String,
        status: StorageStatus,
        updated_at: haven_common::UtcMillis,
    ) -> Self {
        Self {
            storage_location_id,
            provider,
            root_ref,
            status,
            updated_at,
        }
    }

    pub fn storage_location_id(&self) -> StorageLocationId {
        self.storage_location_id
    }
    pub fn provider(&self) -> StorageProviderType {
        self.provider
    }
    pub fn root_ref(&self) -> &str {
        &self.root_ref
    }
    pub fn status(&self) -> StorageStatus {
        self.status
    }
    pub fn updated_at(&self) -> haven_common::UtcMillis {
        self.updated_at
    }
}

/// 扫描目标（BE-SCAN-001 的输入；root_path 由后端从 DB 解析并**本次验证**，不信任前端）。
/// **R-MAIN-09B 返工：全部字段私有，外部无法用 struct literal 构造或篡改**——
/// Scanner 只能通过只读 getter 消费，且构造时字段与 token **强绑定**（不变量：
/// storage_location_id/root_path 与 token 必须一致，非法失配状态不可构造）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanTarget {
    storage_location_id: StorageLocationId,
    display_name: String,
    root_path: PathBuf,
    token: ScanTargetToken,
}

impl ScanTarget {
    pub(crate) fn new(
        storage_location_id: StorageLocationId,
        display_name: String,
        root_path: PathBuf,
        token: ScanTargetToken,
    ) -> Self {
        debug_assert_eq!(
            storage_location_id,
            token.storage_location_id(),
            "ScanTarget 构造不变量：id 必须与 token 一致"
        );
        debug_assert_eq!(
            root_path.to_string_lossy().as_ref(),
            token.root_ref(),
            "ScanTarget 构造不变量：root_path 必须与 token 一致"
        );
        Self {
            storage_location_id,
            display_name,
            root_path,
            token,
        }
    }

    /// 快照 token（只读；Scanner 消费前用自身 Db 校验）。
    pub fn token(&self) -> &ScanTargetToken {
        &self.token
    }

    /// 与 token 绑定的 authoritative 位置 id（只读 getter；不可篡改）。
    pub fn storage_location_id(&self) -> StorageLocationId {
        self.storage_location_id
    }

    /// 展示名（只读 getter）。
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// 与 token 绑定的 authoritative 根路径（只读 getter；不可篡改）。
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }
}

/// 路径探测结果（R-MAIN-09A）：区分「策略拒绝」与「不可达」——策略拒绝（UNC /
/// mapped network drive）**不得**把位置或资源误标 Missing。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// 策略拒绝（UNC / mapped network drive 等）：返回稳定 policy 错误，位置状态不变。
    PolicyDenied,
    /// 路径不可达（canonicalize/is_dir/read_dir 失败）。
    Unreachable,
    /// 可达：本次验证过的 canonical 路径（verbatim 已剥离）。
    Reachable(PathBuf),
}

/// 可注入路径探测 / 策略端口（R-MAIN-09A）。
///
/// 生产默认 `DefaultRootProbe`：在**任何 canonicalize/read_dir 前**做词法 UNC 预拒绝
/// 与 Windows mapped network drive（DRIVE_REMOTE）本机查询预拒绝，canonicalize 后
/// 仍保留防御性 UNC 检查。测试可注入 hook，在 probe 与短事务复核之间确定性执行
/// disconnect/rebind（制造可复现的并发快照变化，不用 sleep 碰运气）。
pub trait RootProbe: Send + Sync {
    /// 探测 root_ref 并返回分类结果。
    fn probe(&self, root_ref: &str) -> ProbeOutcome;
    /// 最近一次 probe 中实际执行的 FS canonicalize/read_dir 调用次数
    /// （生产默认实现返回真实计数；测试用它证明预拒绝阶段零 FS 调用）。
    fn last_fs_calls(&self) -> usize {
        0
    }
}

/// 生产默认路径探测：词法 UNC 预拒绝 → Windows DRIVE_REMOTE 预拒绝 →
/// canonicalize + is_dir + read_dir（计数）→ canonicalize 后防御性 UNC 检查。
pub struct DefaultRootProbe {
    fs_calls: AtomicUsize,
}

impl DefaultRootProbe {
    pub fn new() -> Self {
        Self {
            fs_calls: AtomicUsize::new(0),
        }
    }
}

impl Default for DefaultRootProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl RootProbe for DefaultRootProbe {
    fn probe(&self, root_ref: &str) -> ProbeOutcome {
        // R-MAIN-09A1：计数语义为「最近一次 probe 的 FS 调用次数」——
        // 在实际 canonicalize/read_dir **调用之前**递增（失败调用也计数，杜绝零计数假证据）。
        self.fs_calls.store(0, Ordering::SeqCst);
        // 词法预拒绝（canonicalize/read_dir 之前）：明文与 verbatim UNC。
        if is_unc_str(root_ref) {
            return ProbeOutcome::PolicyDenied;
        }
        // Windows mapped network drive 预拒绝（本机卷类型查询，DRIVE_REMOTE）。
        #[cfg(windows)]
        if mapped_drive::is_remote_drive(root_ref) {
            return ProbeOutcome::PolicyDenied;
        }

        let root = PathBuf::from(root_ref);
        self.fs_calls.fetch_add(1, Ordering::SeqCst);
        let Ok(canonical) = std::fs::canonicalize(&root) else {
            return ProbeOutcome::Unreachable;
        };
        // canonicalize 后防御性 UNC 检查（不信任 root_ref 输入的规范化形态）。
        if is_unc_path(&canonical) {
            return ProbeOutcome::PolicyDenied;
        }
        if !canonical.is_dir() {
            return ProbeOutcome::Unreachable;
        }
        // 可读性：目录可枚举（空目录也算可读）。
        self.fs_calls.fetch_add(1, Ordering::SeqCst);
        if std::fs::read_dir(&canonical).is_err() {
            return ProbeOutcome::Unreachable;
        }
        ProbeOutcome::Reachable(strip_verbatim_prefix(&canonical))
    }

    fn last_fs_calls(&self) -> usize {
        self.fs_calls.load(Ordering::SeqCst)
    }
}

/// Windows mapped network drive 识别（DRIVE_REMOTE）。
/// Unix 网络挂载（NFS 等）无法可靠区分 → 作为明确平台边界（仅 Windows 本地第一阶段）。
#[cfg(windows)]
mod mapped_drive {
    const DRIVE_REMOTE: u32 = 4;

    // GetDriveTypeW：可靠的本机卷类型查询（kernel32；无额外 crate 依赖）。
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDriveTypeW(lp_root_path_name: *const u16) -> u32;
    }

    /// root_ref（如 `C:\...` 或 verbatim `\\?\Z:\...`）所在卷是否为 mapped network drive。
    /// 返回 false 当无法取到盘符根（非盘符路径 / 查询失败 / 本地卷）。
    pub fn is_remote_drive(root_ref: &str) -> bool {
        let Some(drive_root) = drive_root(root_ref) else {
            return false;
        };
        let wide: Vec<u16> = drive_root.encode_utf16().chain(Some(0)).collect();
        // 只对 `C:\...` 形态的盘符路径查询；UNC 已在词法预拒绝阶段拦截。
        unsafe { GetDriveTypeW(wide.as_ptr()) == DRIVE_REMOTE }
    }

    /// 从 root_ref 提取盘符根（`Z:\`）。R-MAIN-09A1：**只验证前 3 字节**
    /// （A-Za-z、冒号、分隔符），**不得对整条路径 `is_ascii`**——`Z:\中文` 与
    /// verbatim `\\?\Z:\中文` 都能提取 `Z:\`。分隔符先统一为反斜杠。
    pub fn drive_root(root_ref: &str) -> Option<String> {
        let normalized = root_ref.replace('/', "\\");
        let s = normalized.strip_prefix(r"\\?\").unwrap_or(&normalized);
        let bytes = s.as_bytes();
        if bytes.len() < 3 {
            return None;
        }
        let is_letter = bytes[0].is_ascii_alphabetic();
        let is_colon = bytes[1] == b':';
        let has_sep = bytes[2] == b'\\' || bytes[2] == b'/';
        if is_letter && is_colon && has_sep {
            // 前 3 字节已验证为 ASCII 字母 + 冒号 + 分隔符，安全按字节切片。
            let mut root = String::from_utf8_lossy(&bytes[..1]).into_owned();
            root.push(':'); // 冒号在 [1]，用常量拼接避免依赖切片
            root.push('\\');
            Some(root)
        } else {
            None
        }
    }
}

/// 事务内可用的存储位置操作（P0-5：状态 + Resource 原子更新）。
pub trait StorageTxPorts {
    fn load_location(&self, id: StorageLocationId) -> Result<Option<StorageLocation>, AppError>;
    fn load_all(&self) -> Result<Vec<StorageLocation>, AppError>;
    fn save_location(&self, location: &StorageLocation) -> Result<(), AppError>;
    /// 批量覆盖某存储位置下的 Resource（R-MAIN-08 覆盖规则 a/b：仅
    /// `availability='available'` 或 `availability_source='storage'`；不得覆盖
    /// user 来源的 SourceUnavailable/TemporarilyUnavailable/Unknown/自身 Missing）。
    fn set_resources_availability(
        &self,
        storage_location_id: StorageLocationId,
        availability: Availability,
        source: AvailabilitySource,
    ) -> Result<(), AppError>;
    /// 读取某存储位置下的全部 Resource（rebind rebase 用）。
    fn load_resources(
        &self,
        storage_location_id: StorageLocationId,
    ) -> Result<Vec<Resource>, AppError>;
    /// 保存 Resource（事务内；rebind rebase 原子提交用）。
    fn save_resource(&self, resource: &Resource) -> Result<(), AppError>;
    fn delete_resources(&self, storage_location_id: StorageLocationId) -> Result<(), AppError>;
    /// 移除位置的全部应用内痕迹：删除该位置 Resource，并级联清理**仅由该位置派生**
    /// 的孤儿内容链（media_items → editions → works）及其用户状态（progress / markers /
    /// favorites / history_entries）。其他位置仍引用的内容完整保留（INTEGRATION-SLICE-001
    /// 真机验收「选错目录」缺口）。
    fn purge_location_content(
        &self,
        storage_location_id: StorageLocationId,
    ) -> Result<(), AppError>;
    fn delete_location(&self, id: StorageLocationId) -> Result<bool, AppError>;
}

/// 存储位置 Unit of Work 端口（闭包内同步操作；失败自动回滚）。
pub trait StorageLocationUoW: Send + Sync {
    /// 事务内执行（写路径；读→校验→写原子）。
    fn run(&self, f: &dyn Fn(&dyn StorageTxPorts) -> Result<(), AppError>) -> Result<(), AppError>;
    /// **事务外**短读（R-MAIN-03：FS 探测前的初始快照；不持有长事务）。
    fn read_location(&self, id: StorageLocationId) -> Result<Option<StorageLocation>, AppError>;
}

#[derive(Clone)]
pub struct StorageLocationService {
    uow: Arc<dyn StorageLocationUoW>,
    probe: Arc<dyn RootProbe>,
}

impl StorageLocationService {
    /// 生产默认：`DefaultRootProbe`（词法 UNC + Windows mapped-drive 预拒绝）。
    pub fn new(uow: Arc<dyn StorageLocationUoW>) -> Self {
        Self::with_probe(uow, Arc::new(DefaultRootProbe::new()))
    }

    /// 可注入 probe（R-MAIN-09A 确定性测试用；生产路径不变）。
    pub fn with_probe(uow: Arc<dyn StorageLocationUoW>, probe: Arc<dyn RootProbe>) -> Self {
        Self { uow, probe }
    }

    /// 全部存储位置（含状态；供设置页"已连接位置"列表）。
    pub async fn list(&self) -> Result<Vec<StorageLocation>, AppError> {
        let cell = Arc::new(Mutex::new(None::<Vec<StorageLocation>>));
        self.uow.run(&|tx| {
            let out = tx.load_all()?;
            *cell.lock().unwrap() = Some(out);
            Ok(())
        })?;
        Ok(cell.lock().unwrap().take().expect("闭包必然写入"))
    }

    /// 添加本地目录位置。路径必须来自可信目录选择流程；后端仍做完整校验。
    /// 同一规范化目录重复添加 → 幂等返回既有 ID（唯一索引兜底并发，P1-6）。
    pub async fn add_local(
        &self,
        display_name: String,
        path: &Path,
    ) -> Result<StorageLocationId, AppError> {
        let normalized = validate_local_path(path)?;
        let normalized_str = normalized.to_string_lossy().into_owned();

        // 预检查（幂等路径）：规范化路径与既有 Local 位置比较（Windows 大小写不敏感）。
        for existing in self.list().await? {
            if existing.provider_type != StorageProviderType::Local {
                continue;
            }
            if local_paths_equivalent(&existing.root_ref, &normalized_str)? {
                return Ok(existing.id);
            }
        }

        let now = haven_common::UtcMillis::now();
        let location = StorageLocation {
            id: StorageLocationId::new(),
            provider_type: StorageProviderType::Local,
            display_name: if display_name.trim().is_empty() {
                normalized
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "本地媒体库".into())
            } else {
                display_name.trim().to_owned()
            },
            root_ref: normalized_str.clone(),
            credential_ref: None,
            status: StorageStatus::Connected,
            created_at: now,
            updated_at: now,
        };
        match self.uow.run(&|tx| {
            tx.save_location(&location)?;
            Ok(())
        }) {
            Ok(()) => Ok(location.id),
            // 并发窗口兜底（P1-6 原子 insert/get-existing 语义）：只有重查命中
            // 等价路径才视为幂等并发；否则**原样传播原始错误**——原实现把磁盘
            // I/O、迁移损坏等 DATABASE_ERROR 一律降级为 INVALID_ARGUMENT
            // （不可重试），误导用户为输入问题（审查 P1-4）。
            Err(original) => {
                for existing in self.list().await? {
                    if existing.provider_type == StorageProviderType::Local
                        && local_paths_equivalent(&existing.root_ref, &normalized_str)?
                    {
                        return Ok(existing.id);
                    }
                }
                Err(original)
            }
        }
    }

    /// 重新绑定本地路径（目录被移动/改名后）。校验同 `add_local`。
    /// 目标路径属于另一位置 → 拒绝（P1-6 排己检查）。
    ///
    /// 语义（R-MAIN-08）：
    /// - **同路径**：原状态 `Connected` 才真幂等；`Missing`/`Disconnected` → 重新连接
    ///   （save_location Connected），**资源保持 storage overlay，绝不虚假恢复**，等 Scanner 逐项验证。
    /// - **新路径**：以 **Path 组件 / relative path 语义**把该位置下全部 `LocalPath` locator
    ///   从旧 root 原子 rebase 到新 root（**禁止字符串 replace**；保持 ResourceId/WorkId）；
    ///   任何无法安全 rebase（越界/不属旧 root）的 LocalPath → **整个事务失败回滚**
    ///   （root_ref / status / locators 均不半更新）。随后旧索引按覆盖规则 a/b 无效化为
    ///   `Missing`/storage，等待 Scanner 逐项验证恢复（不重复建实体）。
    pub async fn rebind_local(
        &self,
        id: StorageLocationId,
        new_path: &Path,
    ) -> Result<(), AppError> {
        let normalized = validate_local_path(new_path)?;
        let normalized_str = normalized.to_string_lossy().into_owned();

        // 排己冲突检查：目标路径不得属于其他位置。
        for existing in self.list().await? {
            if existing.id == id {
                continue;
            }
            if existing.provider_type == StorageProviderType::Local
                && local_paths_equivalent(&existing.root_ref, &normalized_str)?
            {
                return Err(validation("目标目录已属于另一个存储位置"));
            }
        }

        self.uow.run(&|tx| {
            let Some(mut location) = tx.load_location(id)? else {
                return Err(not_found());
            };
            if location.provider_type != StorageProviderType::Local {
                return Err(validation("仅支持重新绑定本地存储位置"));
            }

            // 同路径：Connected 才幂等；Missing/Disconnected → 重新连接，资源保持 overlay。
            if local_paths_equivalent(&location.root_ref, &normalized_str)? {
                if location.status == StorageStatus::Connected {
                    return Ok(());
                }
                location.status = StorageStatus::Connected;
                location.updated_at = haven_common::UtcMillis::now();
                tx.save_location(&location)?;
                return Ok(());
            }

            // 新路径：先原子 rebase locators（失败 → 事务回滚，绝不半更新）。
            let old_root = PathBuf::from(&location.root_ref);
            let new_root = &normalized;
            for mut resource in tx.load_resources(id)? {
                if let ResourceLocator::LocalPath { path } = &resource.locator {
                    let relative = rebase_relative(&old_root, Path::new(path)).ok_or_else(|| {
                        // C-06：错误文案不携带本地绝对路径（VERIFY-SEC-IPC-001 §C/缺陷2）。
                        validation(
                            "资源路径不在旧根内或无法安全 rebase（越界/空/含 ParentDir 等）；已回滚",
                        )
                    })?;
                    let rebased = normalize_path(&new_root.join(relative));
                    // 组件级 containment 防御校验（不靠字符串 starts_with；失败 → 内部错误回滚）。
                    assert_rebased_within(new_root, &rebased)?;
                    resource.locator = ResourceLocator::LocalPath { path: rebased };
                    resource.updated_at = haven_common::UtcMillis::now();
                    tx.save_resource(&resource)?;
                }
            }

            location.root_ref = normalized_str.clone();
            location.status = StorageStatus::Connected;
            location.updated_at = haven_common::UtcMillis::now();
            tx.save_location(&location)?;
            // 旧索引无效化（覆盖规则 a/b 由 SQL 承担）；逐项恢复由 Scanner 完成。
            tx.set_resources_availability(id, Availability::Missing, AvailabilitySource::Storage)
        })
    }

    /// 断开位置（幂等）：位置 `Disconnected` + 相关 Resource 标记不可用，**同一事务**（P0-5）；
    /// Work/Edition/MediaItem/Favorite/Progress/Marker/History 全部保留。
    /// 只标记 **非用户来源** 资源（R-MAIN-02：不覆盖用户显式状态）。
    pub async fn disconnect(&self, id: StorageLocationId) -> Result<(), AppError> {
        self.uow.run(&|tx| {
            let Some(mut location) = tx.load_location(id)? else {
                return Err(not_found());
            };
            if location.status == StorageStatus::Disconnected {
                return Ok(());
            }
            location.status = StorageStatus::Disconnected;
            location.updated_at = haven_common::UtcMillis::now();
            tx.save_location(&location)?;
            tx.set_resources_availability(
                id,
                Availability::StorageUnavailable,
                AvailabilitySource::Storage,
            )
        })
    }

    /// 移除位置：删除应用内绑定与**全部派生索引**（资源 + 仅由该位置派生的孤儿内容链
    /// + 关联用户状态，**同一事务**，P0-5）；不触碰用户原始文件（选错目录可整体撤销）。
    pub async fn remove(&self, id: StorageLocationId) -> Result<(), AppError> {
        self.uow.run(&|tx| {
            let Some(_) = tx.load_location(id)? else {
                return Err(not_found());
            };
            tx.purge_location_content(id)?;
            tx.delete_location(id)?;
            Ok(())
        })
    }

    /// 扫描目标：从 DB 解析真实根目录，验证 Provider=Local 且状态可扫描。
    ///
    /// R-MAIN-03/09A：**FS 探测（RootProbe）在事务外执行**，不持有全局 DB 锁；
    /// **所有结果分支（含 Connected+reachable 快路径、Missing+unreachable 快路径）都
    /// 必须进入一次短事务重读** `id + provider + root_ref + status`：
    /// - probe 期间被 rebind（root_ref 变化）或状态变化 → **retryable** 错误（不得返回
    ///   旧 target，也不得基于旧路径写状态）；
    /// - 重读发现已 Disconnected → `SECURITY_POLICY_DENIED`（明确拒绝，不返回 target）；
    /// - 策略拒绝（UNC / mapped network drive）→ 稳定 policy 错误，**位置/资源不标 Missing**。
    ///
    /// 返回**本次验证过的 canonical path**（含可读性检查）。
    ///
    /// 状态语义（R-MAIN-08）：
    /// - 路径不可达 → 位置 `Missing` + 资源按覆盖规则 a/b 标记 `Missing`（同一事务）。
    /// - 路径恢复 → **仅位置回 `Connected`；绝不批量恢复 Resource**（Scanner 逐项验证）。
    pub async fn get_scan_target(&self, id: StorageLocationId) -> Result<ScanTarget, AppError> {
        // 1. 事务外短读：位置 + 状态（探测前快照）。
        let Some(location) = self.uow.read_location(id)? else {
            return Err(not_found());
        };
        if location.provider_type != StorageProviderType::Local {
            return Err(validation("仅支持扫描本地存储位置"));
        }
        if !matches!(
            location.status,
            StorageStatus::Connected | StorageStatus::Missing
        ) {
            return Err(AppError::new(
                "SECURITY_POLICY_DENIED",
                haven_common::ErrorKind::Security,
                "存储位置未连接，拒绝扫描",
                false,
            ));
        }

        // 2. 事务外 FS 探测（可注入 RootProbe；不持 DB 锁）。
        let probe = self.probe.probe(&location.root_ref);
        #[cfg(debug_assertions)]
        if std::env::var("HAVEN_DEBUG_PROBE").is_ok() {
            eprintln!(
                "HAVEN probe: root={:?} probe={:?} status={:?}",
                location.root_ref,
                match &probe {
                    ProbeOutcome::Reachable(p) => format!("Reachable({})", p.display()),
                    ProbeOutcome::Unreachable => "Unreachable".into(),
                    ProbeOutcome::PolicyDenied => "PolicyDenied".into(),
                },
                location.status
            );
        }

        // 3. 决策目标状态（基于探测前快照）。R-MAIN-09A1：PolicyDenied **不得**在短事务前
        //    early return——也必须进入短事务复核（探测与复核之间可能发生并发变化）。
        let reachable = matches!(probe, ProbeOutcome::Reachable(_));
        let policy_denied = matches!(probe, ProbeOutcome::PolicyDenied);
        // PolicyDenied 不触发 Missing 迁移（策略拒绝 ≠ 不可达；绝不把位置/资源标 Missing）。
        let need_missing =
            !reachable && !policy_denied && location.status != StorageStatus::Missing;
        let need_recover = reachable && location.status == StorageStatus::Missing;

        // 4. **所有分支都进入短事务**重读复核（R-MAIN-09A）：probe 与复核之间
        //    发生的并发 disconnect/rebind/provider 变化必须被检测，不得返回旧 target/policy。
        let target = Arc::new(Mutex::new(None::<Result<ScanTarget, AppError>>));
        self.uow.run(&|tx| {
            let mut current = tx.load_location(id)?.ok_or_else(not_found)?;
            // 优先：probe 期间被并发断开 → 明确拒绝（不返回 target、不写状态）。
            if current.status == StorageStatus::Disconnected {
                *target.lock().unwrap() = Some(Err(AppError::new(
                    "SECURITY_POLICY_DENIED",
                    haven_common::ErrorKind::Security,
                    "存储位置在探测期间被断开，拒绝扫描",
                    false,
                )));
                return Ok(());
            }
            // provider 相对初始快照变化 → retryable（不得用旧 provider 结果）。
            if current.provider_type != location.provider_type {
                *target.lock().unwrap() =
                    Some(Err(retryable("存储位置类型在探测期间发生变化，请重试")));
                return Ok(());
            }
            // root_ref（rebind）或 status 变化 → retryable，不得基于旧路径写状态。
            if current.root_ref != location.root_ref || current.status != location.status {
                *target.lock().unwrap() =
                    Some(Err(retryable("存储位置状态在探测期间发生变化，请重试")));
                return Ok(());
            }
            // 快照一致且 probe 为策略拒绝 → 稳定 SECURITY_POLICY_DENIED（绝不写 Location/Resource）。
            if policy_denied {
                *target.lock().unwrap() = Some(Err(AppError::new(
                    "SECURITY_POLICY_DENIED",
                    haven_common::ErrorKind::Security,
                    "存储位置为网络共享（UNC / mapped drive），本地第一阶段拒绝扫描",
                    false,
                )));
                return Ok(());
            }
            if need_missing {
                current.status = StorageStatus::Missing;
                current.updated_at = haven_common::UtcMillis::now();
                tx.save_location(&current)?;
                tx.set_resources_availability(
                    id,
                    Availability::Missing,
                    AvailabilitySource::Storage,
                )?;
                // 状态迁移成功提交；RESOURCE_UNAVAILABLE 由闭包外返回（闭包 Err 会回滚）。
                *target.lock().unwrap() = Some(Err(unreachable()));
                return Ok(());
            }
            // 快路径 / 恢复：返回本次验证过的 canonical 路径。
            let root_path = match probe.clone() {
                ProbeOutcome::Reachable(p) => p,
                _ => {
                    // 已 Missing + 仍不可达：不重复写库，直接稳定错误。
                    *target.lock().unwrap() = Some(Err(unreachable()));
                    return Ok(());
                }
            };
            if need_recover {
                current.status = StorageStatus::Connected;
                current.updated_at = haven_common::UtcMillis::now();
                tx.save_location(&current)?;
            }
            // R-MAIN-09B：token 从短事务中 **authoritative current** 生成——
            // 恢复路径使用保存后的 Connected + current.updated_at；快路径使用一致快照。
            let token = ScanTargetToken::new(
                current.id,
                current.provider_type,
                current.root_ref.clone(),
                current.status,
                current.updated_at,
            );
            *target.lock().unwrap() = Some(Ok(ScanTarget::new(
                current.id,
                current.display_name,
                root_path,
                token,
            )));
            Ok(())
        })?;
        target.lock().unwrap().take().expect("闭包必然写入结果")
    }
}

/// 绝对路径 + 规范化 + 存在性 + 目录 + 可读性校验（可信 Native 流程之外的后端防线）。
/// **预拒绝（R-MAIN-09A）：在 canonicalize/read_dir 前词法拒绝 UNC（明文/verbatim）
/// 与 Windows mapped network drive**；canonicalize 后仍保留防御性 UNC 检查。
fn validate_local_path(path: &Path) -> Result<PathBuf, AppError> {
    if !path.is_absolute() {
        return Err(validation("本地媒体库路径必须是绝对路径"));
    }
    let raw = path.to_string_lossy();
    if is_unc_str(&raw) {
        return Err(validation(
            "暂不支持网络共享路径（UNC），请选择本地磁盘目录",
        ));
    }
    #[cfg(windows)]
    if mapped_drive::is_remote_drive(&raw) {
        return Err(validation(
            "暂不支持网络映射盘（mapped network drive），请选择本地磁盘目录",
        ));
    }
    let canonical = std::fs::canonicalize(path)
        .map_err(|_| validation("目录不存在或不可访问（请通过 Native 目录选择器重新选择）"))?;
    if !canonical.is_dir() {
        return Err(validation("路径不是目录（本地媒体库必须指向目录）"));
    }
    // canonicalize 后防御性 UNC 检查（不信任输入规范化形态）。
    if is_unc_path(&canonical) {
        return Err(validation(
            "暂不支持网络共享路径（UNC），请选择本地磁盘目录",
        ));
    }
    // 可读性探测：目录可枚举才允许注册。
    std::fs::read_dir(&canonical).map_err(|_| validation("目录不可读"))?;
    // 去掉 Windows 长路径前缀（\\?\），保证 root_ref 是可展示/可比较的普通绝对路径。
    Ok(strip_verbatim_prefix(&canonical))
}

/// 词法 UNC 判定（R-MAIN-09A1）：覆盖 Windows 语义下的反斜杠/正斜杠/混合分隔符、
/// verbatim 前缀与大小写变体。先统一分隔符为反斜杠，再对 verbatim/UNC 前缀做
/// ASCII case-insensitive 判断。verbatim 本地盘符（`\\?\C:\` / `//?/C:/`）不误拒。
fn is_unc_str(path: &str) -> bool {
    let normalized = path.replace('/', "\\");
    let lower = normalized.to_ascii_lowercase();
    let stripped = lower.strip_prefix(r"\\?\").unwrap_or(&lower);
    // `\\`（网络共享）与 `unc\`（verbatim UNC，已小写）均为 UNC。
    stripped.starts_with(r"\\") || stripped.starts_with(r"unc\")
}

/// canonicalize 后的防御性 UNC 判定（同 `is_unc_str`，输入为 `Path`）。
fn is_unc_path(path: &Path) -> bool {
    is_unc_str(&path.to_string_lossy())
}

/// 去除 Windows `\\?\` 长路径前缀（非 Windows 原样返回；UNC 已在 validate/probe 拒绝）。
fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

/// Windows 大小写不敏感比较；Unix 大小写敏感。用于幂等判断与 rebind 排己。
fn local_paths_equivalent(a: &str, b: &str) -> Result<bool, AppError> {
    if cfg!(windows) {
        Ok(normalize_for_compare(a) == normalize_for_compare(b))
    } else {
        Ok(a == b)
    }
}

fn normalize_for_compare(path: &str) -> String {
    path.trim_end_matches(['/', '\\']).to_lowercase()
}

/// 统一路径分隔符（与 Scanner 存储格式一致：Windows 用 `\` 输入，存 `/`）。
fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// **Path 组件 / relative path 语义**的 rebase（R-MAIN-08；禁止字符串 replace）：
/// `p` 是否位于 `old_root` 之下（Windows 组件大小写折叠比较）；是 → 返回相对组件，
/// 供调用方 `new_root.join(relative)` 重组。
///
/// **安全不变量（R-MAIN-08A/B）**：
/// - 返回的 relative **必须非空**（locator 恰等于 old_root → LocalFile 指向目录，拒绝）；
/// - 剩余组件**只能全是 `Normal`**：任何 `ParentDir`（`old_root/../outside` 越界）、
///   `RootDir`、`Prefix`、`CurDir` 等不可安全重组组件一律拒绝；
/// - **构造后按 Path 语义二次重解析**（R-MAIN-08B）：`PathBuf::from_iter` 会把
///   Windows 路径中段的 `Normal("C:")` 重解释为 drive-relative prefix（`C:outside`），
///   `new_root.join(relative)` 将替换整个路径越出 new_root；因此要求
///   `relative.is_relative()` 且重解析后的 `components()` 全部仍为 `Normal`。
///
/// 越界/不安全 → `None`。
fn rebase_relative(old_root: &Path, p: &Path) -> Option<PathBuf> {
    let old_comps: Vec<Component<'_>> = old_root.components().collect();
    let p_comps: Vec<Component<'_>> = p.components().collect();
    // 非空不变量：p 必须严格深于 old_root（剩余至少一个组件）。
    if p_comps.len() <= old_comps.len() {
        return None;
    }
    for (old, cur) in old_comps.iter().zip(p_comps.iter()) {
        if !path_component_eq(
            &old.as_os_str().to_string_lossy(),
            &cur.as_os_str().to_string_lossy(),
        ) {
            return None;
        }
    }
    let rel: Vec<Component<'_>> = p_comps[old_comps.len()..].to_vec();
    // 安全不变量：剩余组件只能全是 Normal。
    if rel.iter().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    // R-MAIN-08B：构造后再按 Path 语义重解析——中段 Normal("C:") 会被重解释为
    // drive-relative prefix；要求严格相对且重解析组件仍全部为 Normal。
    let relative = PathBuf::from_iter(rel.iter().map(|c| c.as_os_str()));
    if !relative.is_relative()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    Some(relative)
}

/// rebase 结果的 **组件级 containment 防御校验**（R-MAIN-08A；不靠字符串 starts_with）：
/// 再次用同一函数验证 `rebased` 仍严格位于 `new_root` 之下（非空、全 Normal 相对组件）。
fn assert_rebased_within(new_root: &Path, rebased: &str) -> Result<(), AppError> {
    if rebase_relative(new_root, Path::new(rebased)).is_none() {
        return Err(AppError::new(
            "INTERNAL_ERROR",
            haven_common::ErrorKind::Internal,
            "rebase 结果越出新根目录（组件级 containment 校验失败）",
            false,
        ));
    }
    Ok(())
}

fn path_component_eq(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn not_found() -> AppError {
    AppError::new(
        "RESOURCE_NOT_FOUND",
        haven_common::ErrorKind::NotFound,
        "存储位置不存在",
        false,
    )
}

fn unreachable() -> AppError {
    AppError::new(
        "RESOURCE_UNAVAILABLE",
        haven_common::ErrorKind::NotFound,
        "存储位置根目录不可达",
        false,
    )
}

/// 可重试错误（快照变化 / provider 变化等）。
fn retryable(msg: impl Into<String>) -> AppError {
    AppError::new(
        "DATABASE_ERROR",
        haven_common::ErrorKind::Database,
        msg,
        true,
    )
}

fn validation(msg: impl Into<String>) -> AppError {
    AppError::new(
        "INVALID_ARGUMENT",
        haven_common::ErrorKind::Validation,
        msg,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R-MAIN-09A1 阻塞 2：UNC 词法分类覆盖反斜杠/正斜杠/混合分隔符/verbatim/大小写变体；
    /// verbatim 本地盘符不误拒。
    #[test]
    fn unc_classification_covers_separators_verbatim_and_case() {
        for unc in [
            r"\\server\share",
            r"//server/share",
            r"//server\share",
            r"\\server/share",
            r"\\?\UNC\server\share",
            r"\\?\UNC/server/share",
            r"//?/UNC/server/share",
            r"\\?\unc\server\share",
            r"\\?\UNC\server",
        ] {
            assert!(is_unc_str(unc), "应为 UNC: {unc}");
        }
        for local in [
            r"\\?\C:\",
            r"//?/C:/",
            r"\\?\c:\users",
            "C:\\Users\\x",
            "C:/Users/x",
        ] {
            assert!(!is_unc_str(local), "本地盘不得误判为 UNC: {local}");
        }
    }

    /// R-MAIN-09A1 阻塞 4：不存在的本地绝对路径 → canonicalize 失败 → Unreachable 且
    /// fs_calls == 1（失败调用也必须计数，杜绝零计数假证据）。
    #[test]
    fn fs_failure_counts_canonicalize_call() {
        let probe = DefaultRootProbe::new();
        let missing = if cfg!(windows) {
            r"C:\haven-no-such-dir-xyz"
        } else {
            "/no/such/path/haven"
        };
        assert_eq!(probe.probe(missing), ProbeOutcome::Unreachable);
        assert_eq!(
            probe.last_fs_calls(),
            1,
            "canonicalize 失败也必须计为一次 FS 调用"
        );
    }

    /// R-MAIN-09A1 阻塞 3（Windows 专项）：盘符提取只验前 3 字节，Unicode 子路径可提取。
    #[cfg(windows)]
    #[test]
    fn drive_root_extracts_unicode_subpath() {
        assert_eq!(
            mapped_drive::drive_root(r"Z:\中文\媒体"),
            Some("Z:\\".into())
        );
        assert_eq!(
            mapped_drive::drive_root(r"\\?\Z:\中文\媒体"),
            Some("Z:\\".into())
        );
        assert_eq!(
            mapped_drive::drive_root(r"//?/Z:/中文/媒体"),
            Some("Z:\\".into())
        );
        assert_eq!(mapped_drive::drive_root(r"z:\中文"), Some("z:\\".into()));
        assert_eq!(mapped_drive::drive_root(r"C:\Users\x"), Some("C:\\".into()));
        assert_eq!(mapped_drive::drive_root("relative/path"), None);
        assert_eq!(mapped_drive::drive_root(r"1:\x"), None);
        assert_eq!(mapped_drive::drive_root(r"\\server\share"), None);
    }

    /// R-MAIN-09B 返工（阻塞 1）：**字段- token 强绑定不变量**——ScanTarget 字段私有、
    /// 构造 `pub(crate)`，外部无法构造或篡改失配 target；合法构造路径下
    /// id/root_path 必须与 token 一致（此单测直接验证绑定成立）。
    #[test]
    fn scan_target_fields_are_bound_to_token() {
        let id = StorageLocationId::new();
        let token = ScanTargetToken::new(
            id,
            haven_domain::enums::StorageProviderType::Local,
            "C:\\root".into(),
            haven_domain::enums::StorageStatus::Connected,
            haven_common::UtcMillis(1),
        );
        let target = ScanTarget::new(id, "库".into(), PathBuf::from("C:\\root"), token.clone());

        // id 绑定
        assert_eq!(
            target.storage_location_id(),
            target.token().storage_location_id()
        );
        // root_path 绑定
        assert_eq!(
            target.root_path().to_string_lossy().as_ref(),
            target.token().root_ref()
        );
        assert_eq!(
            target.token().provider(),
            haven_domain::enums::StorageProviderType::Local
        );
        assert_eq!(
            target.token().status(),
            haven_domain::enums::StorageStatus::Connected
        );
    }
}
