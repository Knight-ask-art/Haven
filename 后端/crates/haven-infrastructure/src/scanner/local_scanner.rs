//! 本地扫描器编排：Enumerate → Detect → Fingerprint → Match → Index。
//!
//! 规范：LIBRARY_AND_STORAGE §28–§42、§207–§214。
//! - 幂等：同路径且 size+modified+首块指纹 未变 → skip（增量语义基础）。
//! - 新文件/变更文件 → 建立 Work → Edition → MediaItem → Resource 四层（单事务原子提交）。
//! - 单文件失败隔离：不影响其他已安全索引文件（§29 原则）。
//! - D1 全量扫描协调（两阶段）：
//!   1. 扫描开始时一次性快照本存储位置上已有 local_path 资源（不持锁跨 IO）；
//!   2. 枚举并准备指纹，记录被观察到的路径身份；完整清单确定后逐项事务写入；
//!   3. 仅在**完整成功**（无取消、无枚举/文件级/索引错误）且 token/根目录复核通过时，
//!      把快照中"根内路径正向 NotFound 且未被观察"的 Available 资源标记为 Missing/Storage；
//!      User/Unknown 显式非可用状态、OfflineAvailable（下载归属）一律不动。
//! - D1 保守改名复用：**整次扫描结束后**（已知全部新条目）才尝试匹配：
//!   同存储、位于根内、类型+MIME+size 一致、首末指纹相等、且新全量哈希与旧哈希相等
//!   （旧行无哈希或大文件无全量哈希 → 不复用）；旧路径正向 NotFound（仅 NotFound 视为缺失，
//!   其它 IO 错误抑制匹配）、目标未被占用、唯一旧候选且唯一新候选。
//!   当前图片目录没有完整内容哈希，与大文件一样不自动复用改名身份。
//!
//! R-MAIN-09B：生产唯一入口 `scan_target(&ScanTarget)`，关闭 get_scan_target 返回后到
//! 消费/写入之间的 DB 状态 TOCTOU：
//! - 消费前用自身 Db 重读 storage_locations 校验不可伪造 token（复合比较
//!   provider/root_ref/status/updated_at）；
//! - 枚举前重新 canonicalize/is_dir/read_dir 探测 target.root_path（保守相等比较）；
//! - 每个 Resource 写事务内在同一事务先重读校验 token 再 save（existing 或四层 new）；
//!   扫描结束前再复核一次。stale → retryable `SCAN_TARGET_STALE`，立即终止不吞 errors。
//! - 旧裸入口 `scan_storage_location` / `index_file` 收窄为 crate 内测试 helper。
//!
//! 注意：这只是消费时重探测；同路径目录删除重建等 OS 级竞态不在本模块声称解决（残余债务）。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use haven_application::services::scan::{
    LibraryScanner, ScanObserver, ScanProgress, ScanReport as AppScanReport,
};
use haven_application::services::storage_location::{ScanTarget, ScanTargetToken};
use haven_application::wire::ScanPhase;
use haven_common::AppError;
use haven_domain::entities::{Edition, MediaItem, Resource, ResourceLocator, Work};
use haven_domain::enums::{
    Availability, AvailabilitySource, MediaItemStatus, MediaType, WorkStatus, WorkType,
};
use haven_domain::ids::{EditionId, MediaItemId, ResourceId, StorageLocationId, WorkId};

use crate::db::Db;
use crate::db::repos::map_db_error;
use crate::epub::validate_epub_file;
use crate::scanner::detect::{
    detect_by_extension, detect_image_sequence, is_supported_image_file_name,
};
use crate::scanner::fingerprint::{
    FULL_HASH_THRESHOLD, FileFingerprint, fast_fingerprint, full_hash_sha256,
};

/// 单次扫描的统计结果。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ScanReport {
    pub files_seen: u64,
    pub recognized: u64,
    pub new: u64,
    pub updated: u64,
    pub skipped: u64,
    pub errors: u64,
}

/// 本地媒体库扫描器。
pub struct LocalLibraryScanner {
    db: Arc<Db>,
    /// R-MAIN-09B：cfg(test) 仅有的 before-write hook——在文件指纹/hash 准备完成后、
    /// 写事务 guard 之前确定性触发（禁止 sleep），用于证明 stale 时 0 写入。
    #[cfg(test)]
    before_write_hook: std::sync::Mutex<Option<Box<dyn Fn() + Send>>>,
    /// R-MAIN-09B（最终阻塞）：cfg(test) 仅有的 after-metadata-before-fingerprint hook——
    /// metadata 读取成功后、fast_fingerprint 读取前确定性触发，用于证明指纹 IO 失败是
    /// 文件级错误（计 errors、0 写）而非终止整次扫描。仅测试可见，不改变生产语义。
    #[cfg(test)]
    after_metadata_hook: std::sync::Mutex<Option<Box<dyn Fn() + Send>>>,
    /// R-MAIN-09B（最终阻塞）：cfg(test) 仅有的 after-fingerprint-open hook——注入到
    /// `fast_fingerprint_with_after_open_hook`，在 File::open 成功后、首次 hash_range 前
    /// 确定性执行（测试把文件截断为 0 → 真实 FINGERPRINT_IO_FAILED）。生产构建不含此字段。
    #[cfg(test)]
    after_fingerprint_open_hook: std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

/// D1：单个条目的索引准备结果（只读阶段产物，写入推迟到整次枚举完成后）。
struct PreparedEntry {
    detected: crate::scanner::detect::DetectResult,
    fingerprint: FileFingerprint,
    modified_ms: u64,
    full_hash: Option<String>,
}

impl PreparedEntry {
    fn size(&self) -> u64 {
        self.fingerprint.size
    }
}

impl LocalLibraryScanner {
    pub fn new(db: Arc<Db>) -> Self {
        Self {
            db,
            #[cfg(test)]
            before_write_hook: std::sync::Mutex::new(None),
            #[cfg(test)]
            after_metadata_hook: std::sync::Mutex::new(None),
            #[cfg(test)]
            after_fingerprint_open_hook: std::sync::Mutex::new(None),
        }
    }

    /// R-MAIN-09B：写事务 guard 前的测试 hook（仅 cfg(test)）。
    #[cfg(test)]
    pub(crate) fn set_before_write_hook(&self, f: Box<dyn Fn() + Send>) {
        *self.before_write_hook.lock().unwrap() = Some(f);
    }

    /// R-MAIN-09B（最终阻塞）：after-metadata-before-fingerprint 测试 hook（仅 cfg(test)）。
    #[cfg(test)]
    pub(crate) fn set_after_metadata_hook(&self, f: Box<dyn Fn() + Send>) {
        *self.after_metadata_hook.lock().unwrap() = Some(f);
    }

    /// R-MAIN-09B（最终阻塞）：after-fingerprint-open 测试 hook（仅 cfg(test)），
    /// 注入 `fast_fingerprint_with_after_open_hook`，在 File::open 成功后、首次 hash_range 前执行。
    /// 一次性（FnOnce），测试内部自管失败（unwrap/expect）。
    #[cfg(test)]
    pub(crate) fn set_after_fingerprint_open_hook(&self, f: Box<dyn FnOnce() + Send>) {
        *self.after_fingerprint_open_hook.lock().unwrap() = Some(f);
    }

    #[cfg(test)]
    fn run_before_write_hook(&self) {
        // 保留 hook（不 take）：每次写事务 guard 前都执行；测试内部用计数决定生效时机。
        if let Some(hook) = self.before_write_hook.lock().unwrap().as_ref() {
            hook();
        }
    }

    #[cfg(test)]
    fn run_after_metadata_hook(&self) {
        // 保留 hook（不 take）：每次 metadata 读取成功后、指纹读取前执行。
        if let Some(hook) = self.after_metadata_hook.lock().unwrap().as_ref() {
            hook();
        }
    }

    /// **生产唯一入口**（R-MAIN-09B）：消费 `get_scan_target` 返回的 target。
    /// 开始前校验 token + 字段- token 绑定 + 重新探测 root；枚举；扫描结束前再复核 token。
    pub async fn scan_target(&self, target: &ScanTarget) -> Result<ScanReport, AppError> {
        // R-MAIN-09B 返工：**字段- token 绑定校验**（第二道防线）——即使未来私有边界被
        // 绕过导致失配 target 进入，也必须在消费 root/id 前拒绝。权威值只从 getter/token 取。
        if target.storage_location_id() != target.token().storage_location_id()
            || !path_eq(target.root_path(), Path::new(target.token().root_ref()))
        {
            return Err(stale_err("扫描目标字段与快照 token 不一致，拒绝消费"));
        }
        // 开始复核：自身 Db 重读 storage_locations 严格校验 token（SCAN_TARGET_STALE）。
        self.verify_token(target.token())?;
        // 消费前重新 FS 探测（canonicalize/is_dir/read_dir），与 target 组件语义相等。
        let root = self.reverify_root(target.root_path())?;
        let report = self
            .scan_inner(
                Some(target.token()),
                target.storage_location_id(),
                root,
                None,
            )
            .await?;
        // 扫描结束前再复核一次：无写之后的已知状态变化不得伪报成功。
        self.verify_token(target.token())?;
        Ok(report)
    }

    /// 观察版生产入口（BE-SCAN-001 第二步）：与 `scan_target` 完全相同的
    /// token/FS 守卫链，额外按 `ScanObserver` 端口回调阶段/进度/警告，
    /// 并在每个文件边界检查协作式取消。守卫代码刻意保持双份显式展开
    /// （不抽公共函数），避免给既有 17 个 scan_target 测试引入路径变更。
    pub async fn scan_target_observed(
        &self,
        target: &ScanTarget,
        observer: &dyn ScanObserver,
    ) -> Result<AppScanReport, AppError> {
        if target.storage_location_id() != target.token().storage_location_id()
            || !path_eq(target.root_path(), Path::new(target.token().root_ref()))
        {
            return Err(stale_err("扫描目标字段与快照 token 不一致，拒绝消费"));
        }
        self.verify_token(target.token())?;
        let root = self.reverify_root(target.root_path())?;
        let report = self
            .scan_inner(
                Some(target.token()),
                target.storage_location_id(),
                root,
                Some(observer),
            )
            .await?;
        self.verify_token(target.token())?;
        Ok(to_app_report(report))
    }

    /// crate 内测试 helper（收窄；R-MAIN-09B：生产路径必须经 `scan_target` 的 token guard）。
    /// 无 token 校验，仅供 local_scanner 模块测试建立实体。
    #[cfg(test)]
    pub(crate) async fn scan_storage_location(
        &self,
        storage_id: StorageLocationId,
        root: &Path,
    ) -> Result<ScanReport, AppError> {
        if !root.is_dir() {
            return Err(AppError::new(
                "SCAN_ROOT_INVALID",
                haven_common::ErrorKind::Validation,
                format!("扫描根目录不存在或不是目录: {root:?}"),
                false,
            ));
        }
        self.scan_inner(None, storage_id, root.to_path_buf(), None)
            .await
    }

    /// 共享枚举循环。`token=None` 仅供测试 helper（无 guard）；
    /// `observer=Some` 时按端口契约回调（BE-SCAN-001 第二步）。
    async fn scan_inner(
        &self,
        token: Option<&ScanTargetToken>,
        storage_id: StorageLocationId,
        root: PathBuf,
        observer: Option<&dyn ScanObserver>,
    ) -> Result<ScanReport, AppError> {
        let mut report = ScanReport::default();
        // D1：扫描开始快照（一次读，不跨 IO 持锁）——用于结束后 Missing 协调与改名匹配。
        let snapshot = self.snapshot_local_resources(storage_id)?;
        // 阶段事件（契约 §14.4）：枚举与指纹交错，写入在完整清单之后进行，
        // Detecting/Fingerprinting 是单文件内的子阶段——若按文件发阶段事件，
        // on_phase 的强制发射会绕过限频淹没 Channel。因此只发两个全局阶段
        // 切换：Enumerating（开始）→ Indexing（首个入库结果），单文件子阶段
        // 由 item_indexed 进度流表达。
        let mut indexing_emitted = false;
        if let Some(o) = observer {
            o.on_phase(ScanPhase::Enumerating);
        }
        // D1：本存储位置中根目录内的旧候选（改名复用）与被观察到的条目数。
        let root_candidates: Vec<Resource> = snapshot
            .into_iter()
            .filter(|r| match &r.locator {
                ResourceLocator::LocalPath { path } => in_scanned_scope(&root, Path::new(path)),
                _ => false,
            })
            .collect();
        // D1：枚举是否成功去过所有条目（不存在 = 有取消 / 有枚举或文件级错误）。
        let mut scan_complete = true;
        // D1：所有实际访问到的条目身份（含剪枝目录、临时文件、未知扩展名、符号链接、
        // 图片页文件）——这些路径绝不能被标记 Missing。
        let mut observed: std::collections::HashSet<String> = std::collections::HashSet::new();
        // D1：推迟写入的新条目（Existing Match 未命中且暂不能断定是否改名/复制）。
        let mut deferred: Vec<(PathBuf, PreparedEntry)> = Vec::new();
        for entry in WalkDir::new(&root)
            .follow_links(false)
            // 忽略的目录必须在**枚举阶段**剪枝：只 `continue` 会跳过目录条目本身，
            // 但 WalkDir 仍会进入其子目录，隐藏/临时目录里的文件照样进入媒体库。
            // 根目录自身（depth 0）永不剪枝——即使存储根目录名恰好是临时样式；
            // 出错条目交给下面的错误分支计数与上报。
            .into_iter()
            .filter_entry(|entry| entry.depth() == 0 || !is_pruned_directory(entry))
        {
            // 协作式取消（端口契约）：文件边界检查；取消时返回 Ok，已索引部分保留。
            if observer.is_some_and(|o| o.is_cancelled()) {
                scan_complete = false;
                break;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    report.errors += 1;
                    scan_complete = false;
                    if let Some(o) = observer {
                        // C-06：Warning 事件不得携带本地绝对路径（walkdir Display 含全路径），
                        // 仅上报目录名 + IO 错误类别。
                        o.on_warning(
                            format_enumeration_warning(e.path(), e.io_error().map(|io| io.kind())),
                            to_progress(&report, None),
                        );
                    }
                    continue;
                }
            };
            // D1：条目已被访问（含被剪枝的目录本身）→ 其路径不是缺失证据。
            observed.insert(observation_key(entry.path()));
            // 符号链接显式上报（follow_links(false) 下不遍历）：链接组织的目录
            // 此前会被静默扫成"0 条目 Completed"，无从排查（审查 P2-1）。
            // C-06：仅上报条目名，不携带本地路径。
            if entry.file_type().is_symlink() {
                report.skipped += 1;
                if let Some(o) = observer {
                    o.on_warning(
                        format!(
                            "跳过符号链接（不跟随）: {}",
                            entry.file_name().to_string_lossy()
                        ),
                        to_progress(&report, None),
                    );
                }
                continue;
            }
            let path = entry.path();
            let detected = if entry.file_type().is_file() {
                report.files_seen += 1;
                // 跳过临时文件（§36 File Stability：size stable + not temporary）。
                if is_temporary(path) {
                    report.skipped += 1;
                    emit_progress(observer, &report, path);
                    continue;
                }
                match detect_by_extension(&file_name(path)) {
                    Some(d) => d,
                    None => {
                        report.skipped += 1;
                        emit_progress(observer, &report, path);
                        continue;
                    }
                }
            } else if entry.file_type().is_dir() {
                // 存储根目录只负责容纳资源，不能因为根目录下直接放了图片就把
                // 整个媒体库登记成一个漫画资源。隐藏/临时目录也不作为来源。
                if entry.depth() == 0 || is_temporary(path) {
                    continue;
                }
                match detect_image_sequence_directory(path) {
                    Ok(Some(detected)) => {
                        report.files_seen += 1;
                        // D1：图片页文件本身绝不作为独立资源或缺失候选。
                        observe_image_sequence_pages(path, &mut observed);
                        detected
                    }
                    Ok(None) => continue,
                    Err(e) if is_file_level_error(&e) => {
                        report.errors += 1;
                        scan_complete = false;
                        if let Some(o) = observer {
                            o.on_warning(
                                format!("图片目录索引失败（{}）", e.code().as_str()),
                                to_progress(&report, Some(path)),
                            );
                        }
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            } else {
                continue;
            };

            report.recognized += 1;
            // D1：metadata / 指纹 / 全量哈希（只读）+ 现有精确路径匹配；写入推迟。
            match self.prepare_entry(token, storage_id, path, &detected).await {
                Ok(Some(prepared)) => deferred.push((path.to_path_buf(), prepared)),
                Ok(None) => report.skipped += 1,
                // R-MAIN-09B 返工（阻塞 2）错误分类：
                // - 仅**明确的文件级 IO/解析/内容类错误**（SCAN_STAT_FAILED / 指纹哈希类）
                //   计入 report.errors，并作为 Warning 事件上报（§14.4：单文件失败不终止）；
                // - `SCAN_TARGET_STALE` 与一切数据库错误（DATABASE_ERROR）、事务错误、
                //   未知错误 **立即向上传播**，绝不吞进 report.errors 返回 Ok。
                Err(e) if is_file_level_error(&e) => {
                    report.errors += 1;
                    // D1：任何文件级错误都抑制整次 Missing 协调（文件仍算被观察到）。
                    scan_complete = false;
                    if let Some(o) = observer {
                        // C-06：Warning 只带稳定错误码与条目名；`{e}` 可能携带底层
                        // IO/解析错误的原文（含路径），且会被前端保留最近一条，
                        // 因此不上报原始错误详情。
                        o.on_warning(
                            format!("文件索引失败（{}）: {}", e.code().as_str(), file_name(path)),
                            to_progress(&report, Some(path)),
                        );
                    }
                }
                Err(e) => return Err(e),
            }
            if !indexing_emitted && report.errors > 0 {
                indexing_emitted = true;
                if let Some(o) = observer {
                    o.on_phase(ScanPhase::Indexing);
                }
            }
            emit_progress(observer, &report, path);
        }
        if observer.is_some_and(|o| o.is_cancelled()) {
            scan_complete = false;
        }

        // D1 第二阶段：写入推迟条目。只有完整成功时，才用"整次扫描已知的全部新条目 +
        // 全部旧候选"判定唯一改名候选；不完整时一律按新条目写入（不改任何旧身份）。
        let reuse_plan = if scan_complete {
            plan_rename_reuse(&root, &root_candidates, &deferred, &observed)
        } else {
            Vec::new()
        };
        for (index, (path, prepared)) in deferred.iter().enumerate() {
            if observer.is_some_and(|o| o.is_cancelled()) {
                scan_complete = false;
                break;
            }
            let reuse = reuse_plan
                .get(index)
                .copied()
                .flatten()
                .and_then(|old| root_candidates.iter().find(|r| r.id == old));
            match self
                .write_entry(token, storage_id, path, prepared, reuse)
                .await
            {
                Ok(IndexOutcome::New) => report.new += 1,
                Ok(IndexOutcome::Updated) => report.updated += 1,
                Ok(IndexOutcome::Skipped) => report.skipped += 1,
                Err(e) if is_file_level_error(&e) => {
                    report.errors += 1;
                    scan_complete = false;
                    if let Some(o) = observer {
                        o.on_warning(
                            format!("文件索引失败（{}）: {}", e.code().as_str(), file_name(path)),
                            to_progress(&report, Some(path)),
                        );
                    }
                }
                Err(e) => return Err(e),
            }
            if !indexing_emitted && (report.new > 0 || report.updated > 0) {
                indexing_emitted = true;
                if let Some(o) = observer {
                    o.on_phase(ScanPhase::Indexing);
                }
            }
            emit_progress(observer, &report, path);
        }

        // D1 第三阶段：Missing 协调。仅在完整成功、未取消、根目录与 token 复核通过时执行；
        // 任何不确定（IO 错误非 NotFound、并发占用）都跳过，绝不猜测。
        if scan_complete && !observer.is_some_and(|o| o.is_cancelled()) {
            if let Some(t) = token {
                self.verify_token(t)?;
            }
            self.reverify_root(&root)?;
            self.reconcile_missing(
                token,
                storage_id,
                &root,
                &root_candidates,
                &observed,
                observer,
            )
            .await?;
        }
        Ok(report)
    }

    /// D1：扫描开始快照——本存储位置全部 local_path 资源（单次读，不持锁跨 IO）。
    fn snapshot_local_resources(
        &self,
        storage_id: StorageLocationId,
    ) -> Result<Vec<Resource>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, media_item_id, resource_type, source_id, storage_location_id,
                        locator_json, mime_type, size, hash_algorithm, hash_digest,
                        availability, availability_source, modified_ms, fingerprint_first, fingerprint_last,
                        created_at, updated_at
                 FROM resources
                 WHERE storage_location_id = ?1 AND locator_kind = 'local_path'",
            )
            .map_err(map_db_error("查询存储位置资源快照失败"))?;
        let rows = stmt
            .query_map(
                rusqlite::params![storage_id.to_string()],
                crate::db::repos::resource::row_to_resource,
            )
            .map_err(map_db_error("查询存储位置资源快照失败"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(map_db_error("读取存储位置资源快照失败"))
    }

    /// D1：只读的索引准备阶段（metadata / 类型校验 / EPUB 校验 / 指纹 / 全量哈希 /
    /// 精确路径匹配）。返回 `None` 表示精确路径已存在且未变化 → Skipped（无写入）。
    async fn prepare_entry(
        &self,
        token: Option<&ScanTargetToken>,
        storage_id: StorageLocationId,
        path: &Path,
        detected: &crate::scanner::detect::DetectResult,
    ) -> Result<Option<PreparedEntry>, AppError> {
        let _ = token;
        let meta = std::fs::metadata(path).map_err(|e| {
            AppError::new(
                "SCAN_STAT_FAILED",
                haven_common::ErrorKind::Io,
                "读取文件属性失败",
                false,
            )
            .with_source(e)
        })?;
        let is_image_sequence =
            detected.resource_type == haven_domain::enums::ResourceType::ImageSequence;
        if (is_image_sequence && !meta.is_dir()) || (!is_image_sequence && !meta.is_file()) {
            return Err(AppError::new(
                "SCAN_STAT_FAILED",
                haven_common::ErrorKind::Io,
                "来源类型与文件系统条目不一致",
                false,
            ));
        }
        let file_size = meta.len();
        if !is_image_sequence && file_size > detected.max_size_bytes {
            return Err(AppError::new(
                "FORMAT_FILE_TOO_LARGE",
                haven_common::ErrorKind::Validation,
                "文件超过当前格式的大小限制",
                false,
            ));
        }

        if !is_image_sequence && detected.mime_type == "application/epub+zip" {
            let epub_path = path.to_path_buf();
            tokio::task::spawn_blocking(move || validate_epub_file(&epub_path))
                .await
                .map_err(|_| {
                    AppError::new(
                        "EPUB_GUARD_FAILED",
                        haven_common::ErrorKind::Internal,
                        "验证 EPUB 文件失败",
                        false,
                    )
                })??;
        }
        let modified_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let path_str = normalize_path(path);

        // R-MAIN-09B（最后阻塞）：metadata 读取成功后、fingerprint 读取前触发测试 hook
        //（cfg(test) only；生产构建无此调用）。
        #[cfg(test)]
        if !is_image_sequence {
            self.run_after_metadata_hook();
        }

        // 图片目录使用有界的目录清单指纹；普通文件仍使用 FastFingerprint。
        // 两条路径都只产生固定大小的哈希状态，不把图片内容加载进内存。
        let fingerprint = if is_image_sequence {
            directory_fingerprint(path, modified_ms)
        } else {
            // FastFingerprint（§40：size + 首末块哈希）——大文件也始终计算（成本低）。
            // cfg(test) 下允许注入 after-fingerprint-open hook（截断 → FINGERPRINT_IO_FAILED）；
            // 无 hook 时走与生产完全相同的 `fast_fingerprint` 算法。
            #[cfg(test)]
            {
                let hook = self.after_fingerprint_open_hook.lock().unwrap().take();
                match hook {
                    Some(h) => crate::scanner::fingerprint::fast_fingerprint_with_after_open_hook(
                        path,
                        file_size,
                        modified_ms,
                        h,
                    ),
                    None => fast_fingerprint(path, file_size, modified_ms),
                }
            }
            #[cfg(not(test))]
            {
                fast_fingerprint(path, file_size, modified_ms)
            }
        }?;
        let size = fingerprint.size;
        if size > detected.max_size_bytes {
            return Err(AppError::new(
                "FORMAT_FILE_TOO_LARGE",
                haven_common::ErrorKind::Validation,
                "文件超过当前格式的大小限制",
                false,
            ));
        }

        // Existing Match（§28 第 5 步）：路径 + StorageLocationId 身份（审查修复）。
        let existing = self
            .find_resource_by_local_path(&path_str, storage_id)
            .await?;
        if let Some(resource) = existing {
            let unchanged = resource.resource_type == detected.resource_type
                && resource.size == Some(size)
                && resource.modified_ms == Some(modified_ms)
                && resource.fingerprint_first.as_deref()
                    == Some(fingerprint.first_chunk_sha256.as_str());
            // R-MAIN-08A 恢复/规范化判定（Scanner 逐项验证是唯一恢复路径）：
            // - **storage overlay**（availability_source == Storage）且非 Available → 恢复 Available/User
            //   （即使 fingerprint 未变，文件已存在必须撤销 overlay）。
            // - **Available 的来源规范**：source ∈ {Storage, Unknown} 且 availability == Available
            //   → 文件验证后规范为 Available/User（覆盖未知来源）。
            // - **显式非可用状态**：source == User 且非 Available（SourceUnavailable /
            //   TemporarilyUnavailable / Unknown / 自身 Missing）→ **保持 availability/source**，
            //   fingerprint 变化只更新必要字段，绝不改显式状态。
            // - **Unknown 且非 Available** → 保守保持（未知来源不当作 overlay）。
            // 只有 fingerprint 未变 且 无需 overlay 恢复/来源规范时才 Skipped。
            let needs_overlay_restore = resource.availability_source == AvailabilitySource::Storage
                && resource.availability != Availability::Available;
            let needs_source_normalize = resource.availability == Availability::Available
                && matches!(
                    resource.availability_source,
                    AvailabilitySource::Storage | AvailabilitySource::Unknown
                );
            let needs_mime_normalize = resource.mime_type.as_deref() != Some(detected.mime_type);
            let needs_type_normalize = resource.resource_type != detected.resource_type;
            if unchanged
                && !needs_overlay_restore
                && !needs_source_normalize
                && !needs_mime_normalize
                && !needs_type_normalize
            {
                return Ok(None);
            }
        }

        // 全量哈希（大文件不做 §42）：只为小文件计算，同时作为改名复用的强证据。
        let full_hash = if !is_image_sequence && size <= FULL_HASH_THRESHOLD {
            Some(full_hash_off_thread(path).await?)
        } else {
            None
        };
        Ok(Some(PreparedEntry {
            detected: detected.clone(),
            fingerprint,
            modified_ms,
            full_hash,
        }))
    }

    /// D1：写入单个条目（Existing 更新 / 改名复用 / 四层新链），每笔写入事务内**先**校验 token。
    /// `reuse=Some(old)` 表示已证明为改名：事务内重读旧行并全部复核，不复用则退回新链
    /// （交由扫描结束后的 Missing 协调处理旧行，绝不猜测）。
    async fn write_entry(
        &self,
        token: Option<&ScanTargetToken>,
        storage_id: StorageLocationId,
        path: &Path,
        prepared: &PreparedEntry,
        reuse: Option<&Resource>,
    ) -> Result<IndexOutcome, AppError> {
        let detected = &prepared.detected;
        let fingerprint = &prepared.fingerprint;
        let size = fingerprint.size;
        let modified_ms = prepared.modified_ms;
        let path_str = normalize_path(path);
        #[cfg(test)]
        self.run_before_write_hook();
        // 准备与提交之间可能经过较长枚举；变化后的文件不能沿用旧指纹或承接旧身份。
        let meta = std::fs::metadata(path).map_err(|e| {
            AppError::new(
                "SCAN_STAT_FAILED",
                haven_common::ErrorKind::Io,
                "提交索引前读取文件属性失败",
                true,
            )
            .with_source(e)
        })?;
        let changed = if detected.resource_type == haven_domain::enums::ResourceType::ImageSequence
        {
            let current = directory_fingerprint(path, prepared.modified_ms)?;
            !meta.is_dir()
                || current.size != size
                || current.first_chunk_sha256 != fingerprint.first_chunk_sha256
                || current.last_chunk_sha256 != fingerprint.last_chunk_sha256
        } else {
            let current_modified = meta
                .modified()
                .ok()
                .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|value| value.as_millis() as u64)
                .unwrap_or(0);
            !meta.is_file() || meta.len() != size || current_modified != modified_ms
        };
        if changed {
            return Err(AppError::new(
                "SCAN_STAT_FAILED",
                haven_common::ErrorKind::Io,
                "文件在扫描期间发生变化，请重新扫描",
                true,
            ));
        }
        if reuse.is_some() {
            if let Some(expected) = &prepared.full_hash {
                if full_hash_off_thread(path).await? != *expected {
                    return Err(AppError::new(
                        "SCAN_STAT_FAILED",
                        haven_common::ErrorKind::Io,
                        "改名候选内容已变化，请重新扫描",
                        true,
                    ));
                }
            }
        }
        let existing = self
            .find_resource_by_local_path(&path_str, storage_id)
            .await?;
        if let Some(resource) = existing {
            let needs_overlay_restore = resource.availability_source == AvailabilitySource::Storage
                && resource.availability != Availability::Available;
            let needs_source_normalize = resource.availability == Availability::Available
                && matches!(
                    resource.availability_source,
                    AvailabilitySource::Storage | AvailabilitySource::Unknown
                );
            let mut updated = resource;
            updated.size = Some(size);
            updated.modified_ms = Some(modified_ms);
            updated.fingerprint_first = Some(fingerprint.first_chunk_sha256.clone());
            updated.fingerprint_last = Some(fingerprint.last_chunk_sha256.clone());
            updated.mime_type = Some(detected.mime_type.to_owned());
            updated.resource_type = detected.resource_type;
            updated.hash =
                prepared
                    .full_hash
                    .as_ref()
                    .map(|digest| haven_domain::entities::ContentHash {
                        algorithm: haven_domain::enums::HashAlgorithm::Sha256,
                        digest: digest.clone(),
                    });
            if needs_overlay_restore || needs_source_normalize {
                updated.availability = Availability::Available;
                updated.availability_source = AvailabilitySource::User;
            }
            // 否则（user/unknown 显式非可用）保持 availability/availability_source 不变。
            updated.updated_at = haven_common::UtcMillis::now();
            self.db.with_tx(|tx| {
                // R-MAIN-09B：写事务内先重读校验 token，再 save（guard 强制）。
                if let Some(t) = token {
                    verify_token_on_conn(tx, t)?;
                }
                crate::db::repos::resource::save_on_conn(tx, &updated)
            })?;
            return Ok(IndexOutcome::Updated);
        }

        if let Some(old) = reuse {
            // 改名复用：身份（Resource → MediaItem → Edition → Work）保持不变，
            // 进度/书签/收藏/历史随 MediaItemId 保留。
            let now = haven_common::UtcMillis::now();
            let new_locator = ResourceLocator::LocalPath {
                path: path_str.clone(),
            };
            let hash =
                prepared
                    .full_hash
                    .as_ref()
                    .map(|digest| haven_domain::entities::ContentHash {
                        algorithm: haven_domain::enums::HashAlgorithm::Sha256,
                        digest: digest.clone(),
                    });
            let old_id = old.id;
            let reused = self.db.with_tx(|tx| {
                // R-MAIN-09B：写事务内先重读校验 token。
                if let Some(t) = token {
                    verify_token_on_conn(tx, t)?;
                }
                // 事务内重读旧行并复核身份与"新路径未被占用"；任一变化 → 不复用。
                let fresh = read_resource_on_conn(tx, old_id)
                    .map_err(map_db_error("读取改名候选资源失败"))?;
                let occupied: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM resources
                         WHERE storage_location_id = ?1 AND locator_kind = 'local_path'
                           AND json_extract(locator_json, '$.local_path.path') = ?2",
                        rusqlite::params![storage_id.to_string(), &path_str],
                        |row| row.get(0),
                    )
                    .map_err(map_db_error("校验改名目标路径失败"))?;
                let Some(mut fresh) = fresh.filter(|_| occupied == 0) else {
                    return Ok(false);
                };
                let still_same_identity = fresh.id == old.id
                    && fresh.storage_location_id == Some(storage_id)
                    && fresh.resource_type == old.resource_type
                    && fresh.media_item_id == old.media_item_id
                    && fresh.mime_type == old.mime_type
                    && fresh.size == old.size
                    && fresh.fingerprint_first == old.fingerprint_first
                    && fresh.fingerprint_last == old.fingerprint_last
                    && fresh.hash == old.hash;
                let old_locator_unchanged = match (&fresh.locator, &old.locator) {
                    (
                        ResourceLocator::LocalPath { path: a },
                        ResourceLocator::LocalPath { path: b },
                    ) => same_path_identity(a, b),
                    _ => false,
                };
                let old_still_absent = match &fresh.locator {
                    ResourceLocator::LocalPath { path } => is_positively_absent(Path::new(path)),
                    _ => false,
                };
                if !still_same_identity
                    || !old_locator_unchanged
                    || !old_still_absent
                    || fresh.availability == Availability::OfflineAvailable
                {
                    return Ok(false);
                }
                // 与 Existing 更新完全相同的可用性规则：只恢复 storage overlay /
                // 规范化 Available 来源；User/Unknown 显式非可用状态原样保留。
                let needs_overlay_restore = fresh.availability_source
                    == AvailabilitySource::Storage
                    && fresh.availability != Availability::Available;
                let needs_source_normalize = fresh.availability == Availability::Available
                    && matches!(
                        fresh.availability_source,
                        AvailabilitySource::Storage | AvailabilitySource::Unknown
                    );
                if needs_overlay_restore || needs_source_normalize {
                    fresh.availability = Availability::Available;
                    fresh.availability_source = AvailabilitySource::User;
                }
                fresh.locator = new_locator;
                fresh.resource_type = detected.resource_type;
                fresh.mime_type = Some(detected.mime_type.to_owned());
                fresh.size = Some(size);
                fresh.modified_ms = Some(modified_ms);
                fresh.fingerprint_first = Some(fingerprint.first_chunk_sha256.clone());
                fresh.fingerprint_last = Some(fingerprint.last_chunk_sha256.clone());
                fresh.hash = hash;
                fresh.updated_at = now;
                crate::db::repos::resource::save_on_conn(tx, &fresh)?;
                Ok(true)
            })?;
            if reused {
                return Ok(IndexOutcome::Updated);
            }
        }

        // 新文件：Fingerprint（大文件只做快速指纹，不做全量哈希 §42）。
        // Candidate → Index Write（单事务，四层原子提交）。
        let now = haven_common::UtcMillis::now();
        let title = detected
            .title_hint
            .clone()
            .unwrap_or_else(|| file_name(path));

        let work = Work {
            id: WorkId::new(),
            canonical_title: title.clone(),
            original_title: None,
            sort_title: Some(title.clone()),
            description: None,
            work_type: WorkType::Standalone,
            release_year: None,
            language: None,
            director: None,
            actor: None,
            status: WorkStatus::Unknown,
            rating_value: None,
            rating_scale: None,
            artwork: Default::default(),
            created_at: now,
            updated_at: now,
        };
        let edition = Edition {
            id: EditionId::new(),
            work_id: work.id,
            title: title.clone(),
            subtitle: None,
            edition_type: detected.media_type,
            release_date: None,
            language: None,
            region: None,
            publisher_or_studio: None,
            description: None,
            artwork: Default::default(),
            created_at: now,
            updated_at: now,
        };
        let media_item = MediaItem {
            id: MediaItemId::new(),
            edition_id: edition.id,
            parent_id: None,
            media_type: detected.media_type,
            title: title.clone(),
            index: media_index_for(detected.media_type),
            duration_ms: None,
            page_count: None,
            chapter_count: None,
            published_at: None,
            status: MediaItemStatus::Available,
            created_at: now,
            updated_at: now,
        };
        let resource = Resource {
            id: ResourceId::new(),
            media_item_id: media_item.id,
            resource_type: detected.resource_type,
            source_id: None,
            storage_location_id: Some(storage_id),
            locator: ResourceLocator::LocalPath {
                path: path_str.clone(),
            },
            mime_type: Some(detected.mime_type.to_owned()),
            size: Some(size),
            hash: prepared
                .full_hash
                .as_ref()
                .map(|digest| haven_domain::entities::ContentHash {
                    algorithm: haven_domain::enums::HashAlgorithm::Sha256,
                    digest: digest.clone(),
                }),
            availability: Availability::Available,
            // 扫描器显式设置 → user 来源：位置级操作（disconnect/path-missing/恢复）不覆盖。
            availability_source: haven_domain::enums::AvailabilitySource::User,
            modified_ms: Some(modified_ms),
            fingerprint_first: Some(fingerprint.first_chunk_sha256.clone()),
            fingerprint_last: Some(fingerprint.last_chunk_sha256.clone()),
            created_at: now,
            updated_at: now,
        };

        // Candidate → Index Write（单事务，四层原子提交；失败整体回滚）。
        self.db.with_tx(|tx| {
            // R-MAIN-09B：写事务内先重读校验 token，再四层 save（guard 强制）。
            if let Some(t) = token {
                verify_token_on_conn(tx, t)?;
            }
            crate::db::repos::work::save_on_conn(tx, &work)?;
            crate::db::repos::edition::save_on_conn(tx, &edition)?;
            crate::db::repos::media_item::save_on_conn(tx, &media_item)?;
            crate::db::repos::resource::save_on_conn(tx, &resource)?;
            Ok(())
        })?;
        Ok(IndexOutcome::New)
    }

    /// 消费前重新 FS 探测（R-MAIN-09B）：canonicalize + is_dir + read_dir，
    /// 且结果与 target 路径保守相等（`path_eq`）；失败 → SCAN_TARGET_STALE。
    fn reverify_root(&self, target_root: &Path) -> Result<PathBuf, AppError> {
        let canonical = std::fs::canonicalize(target_root)
            .map_err(|_| stale_err("扫描根目录已不可达（删除/改名），请重新获取扫描目标"))?;
        if !canonical.is_dir() {
            return Err(stale_err("扫描根目录不再是目录，请重新获取扫描目标"));
        }
        if std::fs::read_dir(&canonical).is_err() {
            return Err(stale_err("扫描根目录不可读，请重新获取扫描目标"));
        }
        let resolved = strip_verbatim(&canonical);
        if !path_eq(&resolved, target_root) {
            return Err(stale_err(
                "扫描根目录与目标路径不一致（目录被替换），请重新获取扫描目标",
            ));
        }
        Ok(resolved)
    }

    /// 用自身 Db 重读 storage_locations 校验 token（复合比较；不存在 → stale）。
    fn verify_token(&self, token: &ScanTargetToken) -> Result<(), AppError> {
        let conn = self.db.lock();
        verify_token_on_conn(&conn, token)
    }

    /// D1 第三阶段：Missing 协调（单事务）。
    ///
    /// 只处理根内、未被观察、且**正向证实不存在**（`try_exists` 仅 NotFound 算缺失；
    /// 权限等其它 IO 错误整次放弃）的候选。仅 `Available`（任意来源）→ `Missing/Storage`；
    /// `Storage` 来源的非可用行保持原样；`User`/`Unknown` 显式非可用与 `OfflineAvailable`
    /// 一律不动。事务内先校验 token 与取消，再对每行重读并复核身份/路径/可用性谓词，
    /// 变更即跳过（绝不覆盖并发状态）。
    async fn reconcile_missing(
        &self,
        token: Option<&ScanTargetToken>,
        storage_id: StorageLocationId,
        root: &Path,
        candidates: &[Resource],
        observed: &std::collections::HashSet<String>,
        observer: Option<&dyn ScanObserver>,
    ) -> Result<(), AppError> {
        let mut absent: Vec<Resource> = Vec::new();
        for candidate in candidates {
            let ResourceLocator::LocalPath { path } = &candidate.locator else {
                continue;
            };
            if !matches!(candidate.availability, Availability::Available) {
                continue;
            }
            // 未被观察才可能是缺失；规范化身份比较（Windows 大小写 + 分隔符）。
            let identity = observation_key(Path::new(path));
            if observed.contains(&identity) {
                continue;
            }
            let Some(resolved) = resolve_under_root(root, Path::new(path)) else {
                continue;
            };
            match std::fs::symlink_metadata(&resolved) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    absent.push(candidate.clone())
                }
                // 已存在（注释 9 的剪枝/符号链接子树等残余）：不是缺失证据。
                Ok(_) => {}
                // 权限/其它错误：无法证明不存在 → 整次协调放弃。
                Err(_) => return Ok(()),
            }
        }
        if absent.is_empty() {
            return Ok(());
        }
        #[cfg(test)]
        self.run_before_write_hook();
        self.db.with_tx(|tx| {
            if let Some(t) = token {
                verify_token_on_conn(tx, t)?;
            }
            if observer.is_some_and(|o| o.is_cancelled()) {
                return Ok(());
            }
            let mut updates = Vec::new();
            for candidate in &absent {
                if observer.is_some_and(|o| o.is_cancelled()) {
                    return Ok(());
                }
                let Some(fresh) = read_resource_on_conn(tx, candidate.id)
                    .map_err(map_db_error("读取缺失候选资源失败"))?
                else {
                    continue;
                };
                let ResourceLocator::LocalPath { path } = &candidate.locator else {
                    continue;
                };
                let ResourceLocator::LocalPath { path: fresh_path } = &fresh.locator else {
                    continue;
                };
                if norm_differs(fresh_path, path) || fresh.storage_location_id != Some(storage_id) {
                    continue;
                }
                // 只有 Available（任意来源）才变为 Missing/Storage；并发变化 → 跳过。
                if fresh.availability != Availability::Available
                    || fresh.availability_source != candidate.availability_source
                    || fresh.hash != candidate.hash
                    || fresh.size != candidate.size
                    || fresh.modified_ms != candidate.modified_ms
                    || fresh.fingerprint_first != candidate.fingerprint_first
                    || fresh.fingerprint_last != candidate.fingerprint_last
                {
                    continue;
                }
                match std::fs::symlink_metadata(Path::new(fresh_path)) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => updates.push(fresh),
                    Ok(_) => {}
                    Err(_) => return Ok(()),
                }
            }
            if observer.is_some_and(|o| o.is_cancelled()) {
                return Ok(());
            }
            for mut fresh in updates {
                fresh.availability = Availability::Missing;
                fresh.availability_source = AvailabilitySource::Storage;
                fresh.updated_at = haven_common::UtcMillis::now();
                crate::db::repos::resource::save_on_conn(tx, &fresh)?;
            }
            Ok(())
        })
    }

    /// 按路径 + StorageLocationId 查找已有资源。
    /// 错误显式传播（审查修复：原实现 .ok()?/.flatten() 吞掉查询/反序列化错误，
    /// 会把"查询失败"误判为"不存在"导致重复建 Work）。
    ///
    /// 点查实现（P0-1）：`json_extract(locator_json, '$.local_path.path')` 表达式
    /// 条件命中迁移 010 的 `idx_resources_local_path` 索引；此前为逐文件全表枚举
    /// 候选集 + 内存 JSON 反序列化比对（O(N²)，且放大全局连接锁持有时间）。
    /// SQL BINARY 比较与 Rust 字节相等语义一致；保留末端等值复核作防御。
    async fn find_resource_by_local_path(
        &self,
        path: &str,
        storage_id: StorageLocationId,
    ) -> Result<Option<Resource>, AppError> {
        let conn = self.db.lock();
        use rusqlite::OptionalExtension;
        let row: Option<Resource> = conn
            .query_row(
                "SELECT id, media_item_id, resource_type, source_id, storage_location_id,
                        locator_json, mime_type, size, hash_algorithm, hash_digest,
                        availability, availability_source, modified_ms, fingerprint_first, fingerprint_last,
                        created_at, updated_at
                 FROM resources
                 WHERE storage_location_id = ?1
                   AND locator_kind = 'local_path'
                   AND json_extract(locator_json, '$.local_path.path') = ?2
                 LIMIT 1",
                rusqlite::params![storage_id.to_string(), path],
                crate::db::repos::resource::row_to_resource,
            )
            .optional()
            .map_err(map_db_error("查询已有资源失败"))?;
        // Older download workers persisted native Windows separators. Keep the
        // indexed lookup first, then recognize those rows without reimporting
        // the same file or destructively rewriting existing library facts.
        #[cfg(windows)]
        let row = match row {
            Some(resource) => Some(resource),
            None => conn
                .query_row(
                    "SELECT id, media_item_id, resource_type, source_id, storage_location_id,
                            locator_json, mime_type, size, hash_algorithm, hash_digest,
                            availability, availability_source, modified_ms, fingerprint_first, fingerprint_last,
                            created_at, updated_at
                     FROM resources
                     WHERE storage_location_id = ?1 AND locator_kind = 'local_path'
                       AND replace(json_extract(locator_json, '$.local_path.path'), char(92), '/') = ?2
                     LIMIT 1",
                    rusqlite::params![storage_id.to_string(), path],
                    crate::db::repos::resource::row_to_resource,
                )
                .optional()
                .map_err(map_db_error("查询旧版下载资源失败"))?,
        };
        match row {
            Some(resource) => match &resource.locator {
                ResourceLocator::LocalPath { path: p }
                    if crate::local_path_identity(Path::new(p)) == path =>
                {
                    Ok(Some(resource))
                }
                // 防御：索引/JSON 语义与内存比对不一致时不误判（返回 None 走新建路径）。
                _ => Ok(None),
            },
            None => Ok(None),
        }
    }
}

/// 单文件索引结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexOutcome {
    New,
    Updated,
    Skipped,
}

/// R-MAIN-09B：最小 token 校验（读真实 storage_locations 行，复合比较
/// provider/root_ref/status/updated_at）。行不存在（remove 后）或不匹配 → SCAN_TARGET_STALE。
/// `conn` 可为普通连接或事务连接（写事务内 guard 复用）。
pub(crate) fn verify_token_on_conn(
    conn: &rusqlite::Connection,
    token: &ScanTargetToken,
) -> Result<(), AppError> {
    use rusqlite::OptionalExtension;
    let row: Option<(String, String, String, i64)> = conn
        .query_row(
            "SELECT provider_type, root_ref, status, updated_at FROM storage_locations WHERE id = ?1",
            rusqlite::params![token.storage_location_id().to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(map_db_error("校验扫描 token 失败"))?;
    let Some((provider, root_ref, status, updated_at)) = row else {
        return Err(stale_err("存储位置已被移除"));
    };
    // 序列化失败显式报错（原 unwrap_or_default 会把空串当期望值比对 → 假 STALE）。
    let expected_provider = enum_db_str(&token.provider())?;
    let expected_status = enum_db_str(&token.status())?;
    if provider != expected_provider
        || root_ref != token.root_ref()
        || status != expected_status
        || updated_at != token.updated_at().0
    {
        return Err(stale_err(
            "存储位置在获取扫描目标后发生变化（断开/重绑/移除），请重新获取",
        ));
    }
    Ok(())
}

/// `LibraryScanner` 端口实现（BE-SCAN-001 第二步）：application 定义端口，
/// infra 提供实现；`ScanService` 经此驱动本地扫描（ADR-003 §6 依赖方向）。
#[async_trait]
impl LibraryScanner for LocalLibraryScanner {
    async fn scan(
        &self,
        target: &ScanTarget,
        observer: &dyn ScanObserver,
    ) -> Result<AppScanReport, AppError> {
        self.scan_target_observed(target, observer).await
    }
}

/// infra `ScanReport` → application `ScanReport`（字段同名同型直拷）。
fn to_app_report(r: ScanReport) -> AppScanReport {
    AppScanReport {
        files_seen: r.files_seen,
        recognized: r.recognized,
        new: r.new,
        updated: r.updated,
        skipped: r.skipped,
        errors: r.errors,
    }
}

/// 生成端口进度快照。`current_item` 只取文件名——**完整本地路径不进事件流**
/// （与 C-06「不向高权限 WebView 暴露本地路径」安全边界一致）。
fn to_progress(report: &ScanReport, path: Option<&Path>) -> ScanProgress {
    ScanProgress {
        files_seen: report.files_seen,
        recognized: report.recognized,
        new: report.new,
        updated: report.updated,
        skipped: report.skipped,
        errors: report.errors,
        current_item: path.map(file_name),
    }
}

fn emit_progress(observer: Option<&dyn ScanObserver>, report: &ScanReport, path: &Path) {
    if let Some(o) = observer {
        o.on_progress(to_progress(report, Some(path)));
    }
}

/// 全量哈希移入 blocking 池（审查 P1-1）：单文件上限 FULL_HASH_THRESHOLD
/// （512MiB 量级）的顺序读盘直接调用会阻塞 tokio worker 数秒，扫描并发下
/// 可能饿死同 runtime 的其他 async 任务。
async fn full_hash_off_thread(path: &Path) -> Result<String, AppError> {
    let owned = path.to_path_buf();
    tokio::task::spawn_blocking(move || full_hash_sha256(&owned))
        .await
        .map_err(|e| {
            AppError::new(
                "INTERNAL_ERROR",
                haven_common::ErrorKind::Internal,
                "文件哈希任务失败",
                false,
            )
            .with_source(e)
        })
        .and_then(|inner| inner)
}

/// 仅把包含至少一个直接图片子项的可见目录登记为 `ImageSequence`。
///
/// 目录内容仍由 `LocalComicPageProvider` 在会话打开时做完整的魔数、路径和
/// 大小复核；扫描阶段不读取图片正文，也不把目录下的任意文件当作漫画页面。
fn detect_image_sequence_directory(
    path: &Path,
) -> Result<Option<crate::scanner::detect::DetectResult>, AppError> {
    let Ok(entries) = fs::read_dir(path) else {
        // WalkDir 会在后续枚举阶段报告不可读目录。这里不重复制造一个资源，
        // 以便单个目录权限问题继续隔离，不影响同一存储位置的其他条目。
        return Ok(None);
    };
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_temporary(&entry.path()) {
            continue;
        }
        if is_supported_image_file_name(&name) {
            let title = path
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default();
            return Ok(Some(detect_image_sequence(&title)));
        }
    }
    Ok(None)
}

/// 为图片目录构造稳定且有界的清单指纹。
///
/// 每个页面只采样首/末块（复用文件 FastFingerprint），因此不会把整本漫画
/// 载入内存；清单同时包含规范化名称、大小和修改时间，页面内容发生变化时
/// 即使总大小不变也能触发下一次扫描更新。
fn directory_fingerprint(path: &Path, modified_ms: u64) -> Result<FileFingerprint, AppError> {
    let entries = fs::read_dir(path).map_err(|error| {
        AppError::new(
            "SCAN_STAT_FAILED",
            haven_common::ErrorKind::Io,
            "读取图片目录失败",
            false,
        )
        .with_source(error)
    })?;
    let mut files = Vec::<(String, PathBuf)>::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            AppError::new(
                "SCAN_STAT_FAILED",
                haven_common::ErrorKind::Io,
                "读取图片目录条目失败",
                false,
            )
            .with_source(error)
        })?;
        let file_type = entry.file_type().map_err(|error| {
            AppError::new(
                "SCAN_STAT_FAILED",
                haven_common::ErrorKind::Io,
                "读取图片文件属性失败",
                false,
            )
            .with_source(error)
        })?;
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_temporary(&entry.path()) && is_supported_image_file_name(&name) {
            files.push((name, entry.path()));
        }
    }
    files.sort_by(|(left, _), (right, _)| {
        crate::comic::natural_cmp(left, right).then_with(|| left.cmp(right))
    });
    if files.is_empty() {
        return Err(AppError::new(
            "FORMAT_UNSUPPORTED",
            haven_common::ErrorKind::Unsupported,
            "图片目录中没有可阅读页面",
            false,
        ));
    }

    let mut total_size = 0u64;
    let mut hasher = Sha256::new();
    for (name, file_path) in files {
        let metadata = fs::metadata(&file_path).map_err(|error| {
            AppError::new(
                "SCAN_STAT_FAILED",
                haven_common::ErrorKind::Io,
                "读取图片文件属性失败",
                false,
            )
            .with_source(error)
        })?;
        let size = metadata.len();
        total_size = total_size.checked_add(size).ok_or_else(|| {
            AppError::new(
                "FORMAT_FILE_TOO_LARGE",
                haven_common::ErrorKind::Validation,
                "图片目录总大小溢出",
                false,
            )
        })?;
        if total_size > crate::scanner::detect::MAX_IMAGE_SEQUENCE_BYTES {
            return Err(AppError::new(
                "FORMAT_FILE_TOO_LARGE",
                haven_common::ErrorKind::Validation,
                "图片目录超过当前漫画大小限制",
                false,
            ));
        }
        let page_modified_ms = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|value| value.as_millis() as u64)
            .unwrap_or(0);
        let page_fingerprint = fast_fingerprint(&file_path, size, page_modified_ms)?;
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update(size.to_le_bytes());
        hasher.update(page_modified_ms.to_le_bytes());
        hasher.update(page_fingerprint.first_chunk_sha256.as_bytes());
        hasher.update([0]);
        hasher.update(page_fingerprint.last_chunk_sha256.as_bytes());
        hasher.update([0]);
    }
    let digest = hex_digest(&hasher.finalize());
    Ok(FileFingerprint {
        size: total_size,
        modified_ms,
        first_chunk_sha256: digest.clone(),
        last_chunk_sha256: digest,
    })
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn enum_db_str(e: &impl serde::Serialize) -> Result<String, AppError> {
    crate::db::repos::enum_to_db_str(e)
}

/// 去除 Windows `\\?\` 长路径前缀（非 Windows 原样返回）。
fn strip_verbatim(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

/// 路径相等性（R-MAIN-09B 返工）：Windows 上做 ASCII case-insensitive 字符串比较，
/// Unix 上字节比较。**不是** `Path::components()` 语义——注释如实反映；保守方向：
/// 仅当完全相等才判定一致，差异一律按 stale 处理（宁可 false-stale，不误消费）。
/// 不承诺识别同路径目录删除/重建等 OS identity 竞态。
fn path_eq(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy()
            .eq_ignore_ascii_case(b.to_string_lossy().as_ref())
    } else {
        a == b
    }
}

fn stale_err(msg: impl Into<String>) -> AppError {
    AppError::new(
        "SCAN_TARGET_STALE",
        haven_common::ErrorKind::Conflict,
        msg,
        true,
    )
}

/// R-MAIN-09B 返工（阻塞 2）：判定错误是否为**预期的文件级 IO/解析/内容类错误**，
/// 可以计入 `report.errors` 继续扫描。数据库错误（DATABASE_ERROR）、事务错误、
/// `SCAN_TARGET_STALE`、未知错误一律**向上传播**（不吞）。
/// 白名单：单文件 metadata / 格式验证 / 指纹 / 哈希读取类 IO 错误——
/// `SCAN_STAT_FAILED`（metadata）、`FORMAT_UNSUPPORTED` / `SECURITY_POLICY_DENIED`
///（EPUB 内容边界）、`FINGERPRINT_READ_FAILED`（内容变化检测）、
/// `FINGERPRINT_IO_FAILED`（指纹 open/seek/read 失败）。精确 code 白名单，不放宽。
fn is_file_level_error(e: &AppError) -> bool {
    matches!(
        e.code().as_str(),
        "SCAN_STAT_FAILED"
            | "FORMAT_UNSUPPORTED"
            | "SECURITY_POLICY_DENIED"
            | "FINGERPRINT_READ_FAILED"
            | "FINGERPRINT_IO_FAILED"
            | "FORMAT_FILE_TOO_LARGE"
    )
}

fn is_temporary(path: &Path) -> bool {
    let name = file_name(path);
    name.starts_with('.')
        || name.ends_with(".part")
        || name.ends_with(".partial")
        || name.ends_with(".tmp")
        || name.ends_with("~")
}

/// 枚举剪枝判定：只对目录生效（`filter_entry` 同样会对文件调用）。
/// 与 `is_temporary` 使用同一套命名规则：隐藏目录（`.` 前缀）与 `*.part` /
/// `*.partial` / `*.tmp` / `*~` 临时目录整棵子树都不进入媒体库。
/// 符号链接目录由 `follow_links(false)` 保证不会被遍历。
fn is_pruned_directory(entry: &walkdir::DirEntry) -> bool {
    entry.file_type().is_dir() && is_temporary(entry.path())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// C-06：枚举失败只暴露 basename 与稳定 IO 类别；walkdir 的 Display/path
/// 可能包含完整本地根目录，不能直接进入 production IPC Warning。
fn format_enumeration_warning(path: Option<&Path>, kind: Option<std::io::ErrorKind>) -> String {
    let name = path
        .map(file_name)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "未知条目".into());
    let kind = kind
        .map(|kind| format!("{kind:?}"))
        .unwrap_or_else(|| "io".into());
    format!("枚举失败（{kind}）: {name}")
}

/// 统一路径分隔符为反斜杠无关的规范形式（Windows 存储用 `\`，比较时统一为 `/` 便于跨盘比较）。
fn normalize_path(path: &Path) -> String {
    crate::local_path_identity(path)
}

/// D1：两个路径身份是否相同（Windows 大小写不敏感 + 分隔符归一；Unix 字节相等）。
fn same_path_identity(a: impl AsRef<Path>, b: impl AsRef<Path>) -> bool {
    let (a, b) = (normalize_path(a.as_ref()), normalize_path(b.as_ref()));
    if cfg!(windows) {
        a.eq_ignore_ascii_case(&b)
    } else {
        a == b
    }
}

fn observation_key(path: &Path) -> String {
    let key = normalize_path(path);
    if cfg!(windows) {
        key.to_ascii_lowercase()
    } else {
        key
    }
}

/// D1：存储路径是否与计划身份不同（不区分大小写平台上的规范化比较）。
fn norm_differs(stored: &str, planned: &str) -> bool {
    !same_path_identity(stored, planned)
}

/// D1：组件级根内解析（不靠字符串前缀）。要求 `path` 深于 `root`、根组件逐一相等
/// （Windows 大小写不敏感 + 分隔符归一）、剩余组件全为 `Normal`；返回重组后的绝对路径。
fn resolve_under_root(root: &Path, path: &Path) -> Option<PathBuf> {
    use std::path::Component;
    let root: Vec<Component<'_>> = root.components().collect();
    let parts: Vec<Component<'_>> = path.components().collect();
    if parts.len() <= root.len() {
        return None;
    }
    for (a, b) in root.iter().zip(parts.iter()) {
        if !same_path_identity(Path::new(a.as_os_str()), Path::new(b.as_os_str())) {
            return None;
        }
    }
    let rest = &parts[root.len()..];
    if rest.iter().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(path.to_path_buf())
}

fn in_scanned_scope(root: &Path, path: &Path) -> bool {
    if resolve_under_root(root, path).is_none() {
        return false;
    }
    let mut cursor = root.to_path_buf();
    for part in path.components().skip(root.components().count()) {
        if is_temporary(Path::new(part.as_os_str())) {
            return false;
        }
        cursor.push(part.as_os_str());
        if std::fs::symlink_metadata(&cursor).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return false;
        }
    }
    true
}

/// D1：`symlink_metadata` 探测——仅 `NotFound` 视为"已不存在"。
fn is_positively_absent(path: &Path) -> bool {
    matches!(
        std::fs::symlink_metadata(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound
    )
}

/// D1：记录图片目录内直接页文件的路径身份（页文件绝不作为独立资源或缺失候选）。
fn observe_image_sequence_pages(dir: &Path, observed: &mut std::collections::HashSet<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        observed.insert(observation_key(&entry.path()));
    }
}

/// D1：在连接上按 id 读取 Resource（普通连接或事务连接）。
fn read_resource_on_conn(
    conn: &rusqlite::Connection,
    id: ResourceId,
) -> rusqlite::Result<Option<Resource>> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT id, media_item_id, resource_type, source_id, storage_location_id,
                locator_json, mime_type, size, hash_algorithm, hash_digest,
                availability, availability_source, modified_ms, fingerprint_first, fingerprint_last,
                created_at, updated_at
         FROM resources WHERE id = ?1",
        rusqlite::params![id.to_string()],
        crate::db::repos::resource::row_to_resource,
    )
    .optional()
}

/// D1：单个旧候选与新条目是否具备**强改名证据**。
///
/// 全部满足：类型与 MIME 一致、size 精确相等、首末指纹相等（图片序列为目录清单摘要，
/// 不含路径/目录名，因此路径无关）；小文件若旧行已有全量哈希，则新全量哈希必须相等；
/// 大文件（无全量哈希）**不复用**（首末块碰撞风险，留作显式残余）；
/// 旧路径正向 NotFound、根内、未被观察、非 `OfflineAvailable`（下载归属）。
fn rename_evidence_matches(
    root: &Path,
    old: &Resource,
    new_path: &Path,
    prepared: &PreparedEntry,
    observed: &std::collections::HashSet<String>,
) -> bool {
    if old.availability == Availability::OfflineAvailable {
        return false;
    }
    let ResourceLocator::LocalPath { path: old_path } = &old.locator else {
        return false;
    };
    let Some(old_resolved) = resolve_under_root(root, Path::new(old_path)) else {
        return false;
    };
    if old.resource_type != prepared.detected.resource_type
        || old.mime_type.as_deref() != Some(prepared.detected.mime_type)
        || old.size != Some(prepared.size())
        || old.fingerprint_first.as_deref()
            != Some(prepared.fingerprint.first_chunk_sha256.as_str())
        || old.fingerprint_last.as_deref() != Some(prepared.fingerprint.last_chunk_sha256.as_str())
    {
        return false;
    }
    let hash_matches = match &prepared.full_hash {
        Some(new_hash) => match &old.hash {
            Some(stored) => {
                stored.algorithm == haven_domain::enums::HashAlgorithm::Sha256
                    && stored.digest == *new_hash
            }
            // 旧行无哈希（如旧版下载行）：指纹+size+类型仅为弱证据，不作为唯一依据时
            // 仍由"唯一旧/唯一新"兜底；保守起见这里直接拒绝，不猜测。
            None => false,
        },
        // 大文件没有全量哈希：宁可不复用，也不冒首末块碰撞风险。
        None => false,
    };
    if !hash_matches {
        return false;
    }
    let identity = observation_key(&old_resolved);
    if observed.contains(&identity) || same_path_identity(&old_resolved, new_path) {
        return false;
    }
    is_positively_absent(&old_resolved)
}

/// D1：整次扫描结束后计算改名复用计划（`plan[new_index] = Some(old_resource_id)`）。
///
/// 只有在"唯一旧候选 ↔ 唯一新条目"时复用；任一旧候选对应 ≥2 个新条目、或任一新条目
/// 对应 ≥2 个旧候选 → 全部不复用（不任意配对）。调用方保证只有完整成功的扫描才调用，
/// 因此 `deferred` 就是本次扫描的全部新条目清单。
fn plan_rename_reuse(
    root: &Path,
    root_candidates: &[Resource],
    deferred: &[(PathBuf, PreparedEntry)],
    observed: &std::collections::HashSet<String>,
) -> Vec<Option<ResourceId>> {
    let mut plan = vec![None; deferred.len()];
    let mut by_hash: std::collections::HashMap<&str, Vec<&Resource>> =
        std::collections::HashMap::new();
    for old in root_candidates {
        if old.availability == Availability::OfflineAvailable {
            continue;
        }
        if let Some(hash) = &old.hash {
            if hash.algorithm == haven_domain::enums::HashAlgorithm::Sha256 {
                by_hash.entry(hash.digest.as_str()).or_default().push(old);
            }
        }
    }
    let mut uses = std::collections::HashMap::new();
    for (index, (path, prepared)) in deferred.iter().enumerate() {
        let Some(hash) = &prepared.full_hash else {
            continue;
        };
        let Some(candidates) = by_hash.get(hash.as_str()) else {
            continue;
        };
        // 仍在原路径上的同内容副本也构成歧义，不能把缺失的另一个身份任意接过来。
        let hits: Vec<&Resource> = candidates
            .iter()
            .copied()
            .filter(|old| {
                old.resource_type == prepared.detected.resource_type
                    && old.mime_type.as_deref() == Some(prepared.detected.mime_type)
                    && old.size == Some(prepared.size())
                    && old.fingerprint_first.as_deref()
                        == Some(prepared.fingerprint.first_chunk_sha256.as_str())
                    && old.fingerprint_last.as_deref()
                        == Some(prepared.fingerprint.last_chunk_sha256.as_str())
            })
            .collect();
        if hits.len() == 1 && rename_evidence_matches(root, hits[0], path, prepared, observed) {
            plan[index] = Some(hits[0].id);
            *uses.entry(hits[0].id).or_insert(0usize) += 1;
        }
    }
    for id in &mut plan {
        if id.is_some_and(|id| uses.get(&id) != Some(&1)) {
            *id = None;
        }
    }
    plan
}

fn media_index_for(media_type: MediaType) -> haven_domain::entities::MediaIndex {
    use haven_domain::entities::MediaIndex;
    match media_type {
        MediaType::Movie => MediaIndex::Movie,
        MediaType::Episode => MediaIndex::Episode {
            season: None,
            episode: 1,
        },
        _ => MediaIndex::Custom {
            label: "file".into(),
            ordinal: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    use crate::db::uow::SqliteStorageUoW;
    use haven_application::services::storage_location::{
        StorageLocationService, StorageLocationUoW,
    };

    fn sample_storage(dir: &TempDir) -> (StorageLocationId, PathBuf) {
        let id = StorageLocationId::new();
        (id, dir.path().to_path_buf())
    }

    fn write(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    fn write_valid_epub(path: &Path) {
        let file = std::fs::File::create(path).unwrap();
        let mut writer = ZipWriter::new(file);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        writer.start_file("mimetype", stored).unwrap();
        writer.write_all(b"application/epub+zip").unwrap();
        writer.start_file("META-INF/container.xml", stored).unwrap();
        writer
            .write_all(
                br#"<container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#,
            )
            .unwrap();
        writer.start_file("OEBPS/content.opf", stored).unwrap();
        writer
            .write_all(
                br#"<package><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="chapter"/></spine></package>"#,
            )
            .unwrap();
        writer.start_file("OEBPS/chapter.xhtml", stored).unwrap();
        writer
            .write_all(b"<html><body>content</body></html>")
            .unwrap();
        writer.finish().unwrap();
    }

    #[test]
    fn enumeration_warning_redacts_absolute_root_path() {
        let path = if cfg!(windows) {
            PathBuf::from(r"C:\Users\owner\Secrets\library\unreadable")
        } else {
            PathBuf::from("/Users/owner/Secrets/library/unreadable")
        };
        let warning =
            format_enumeration_warning(Some(&path), Some(std::io::ErrorKind::PermissionDenied));
        assert!(warning.contains("unreadable"));
        assert!(warning.contains("PermissionDenied"));
        assert!(!warning.contains("Secrets"));
        assert!(!warning.contains("owner"));
        assert!(!warning.contains(&path.to_string_lossy().to_string()));
    }

    #[tokio::test]
    #[cfg(windows)]
    async fn rescan_recognizes_legacy_download_separators() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        write(&root, "download.txt", b"downloaded text");
        let db = Arc::new(Db::open_in_memory().unwrap());
        let query_plan: String = db
            .lock()
            .query_row(
                "EXPLAIN QUERY PLAN SELECT id FROM resources WHERE storage_location_id = ?1
               AND locator_kind = 'local_path'
               AND replace(json_extract(locator_json, '$.local_path.path'), char(92), '/') = ?2",
                rusqlite::params![
                    storage_id.to_string(),
                    normalize_path(&root.join("download.txt"))
                ],
                |r| r.get(3),
            )
            .unwrap();
        assert!(
            query_plan.contains("idx_resources_windows_local_path"),
            "{query_plan}"
        );
        let scanner = LocalLibraryScanner::new(db.clone());
        assert_eq!(
            scanner
                .scan_storage_location(storage_id, &root)
                .await
                .unwrap()
                .new,
            1
        );
        db.lock()
            .execute(
                "UPDATE resources SET locator_json = json_set(locator_json, '$.local_path.path',
                 replace(json_extract(locator_json, '$.local_path.path'), '/', char(92)))",
                [],
            )
            .unwrap();
        let report = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(report.new, 0);
        let count: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM works", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn full_scan_indexes_supported_files_and_skips_unknown() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        write(&root, "Movie.A.mkv", b"fake-video-bytes");
        write(&root, "Novel.B.txt", b"some text content");
        write(&root, "junk.xyz", b"not supported");

        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        let report = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();

        assert_eq!(report.files_seen, 3);
        assert_eq!(report.recognized, 2);
        assert_eq!(report.new, 2);
        assert_eq!(report.skipped, 1, "未知扩展名跳过");

        // DB 验证：两个 Work，各自 Edition/MediaItem/Resource 一条。
        let count = |table: &str| -> i64 {
            db.lock()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(count("works"), 2);
        assert_eq!(count("editions"), 2);
        assert_eq!(count("media_items"), 2);
        assert_eq!(count("resources"), 2);
        let mimes: Vec<String> = db
            .lock()
            .prepare("SELECT mime_type FROM resources ORDER BY mime_type")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            mimes,
            vec![
                "text/plain; charset=utf-8".to_string(),
                "video/x-matroska".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn image_directory_is_indexed_as_image_sequence_and_rescan_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        let chapter = root.join("某漫画 第1话");
        std::fs::create_dir(&chapter).unwrap();
        write(&chapter, "page10.jpg", b"page ten");
        write(&chapter, "page2.png", b"page two");
        write(&chapter, "notes.dat", b"metadata");

        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        let first = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(first.recognized, 1, "图片目录应登记为一个漫画资源");
        assert_eq!(first.new, 1);

        let (resource_type, locator): (String, String) = db
            .lock()
            .query_row(
                "SELECT resource_type, locator_json FROM resources LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(resource_type, "image_sequence");
        assert!(locator.contains("某漫画 第1话"));

        let second = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(second.new, 0, "重扫图片目录不得重复建立 Work");
        assert_eq!(
            db.lock()
                .query_row("SELECT COUNT(*) FROM works", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn comic_archive_rescan_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        write(&root, "某漫画 第1卷.cbz", b"cbz placeholder");

        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        let first = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(first.new, 1);

        let second = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(second.new, 0, "CBZ 重扫不得重复建立 Work");
        assert_eq!(second.skipped, 1);
        assert_eq!(
            db.lock()
                .query_row("SELECT COUNT(*) FROM works", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            db.lock()
                .query_row("SELECT resource_type FROM resources", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            "comic_archive"
        );
    }

    #[tokio::test]
    async fn scan_validates_epub_before_indexing_and_keeps_other_files_isolated() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        write(&root, "broken.epub", b"not an epub archive");
        write_valid_epub(&root.join("valid.epub"));
        write(&root, "notes.txt", b"keep scanning");

        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        let report = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();

        assert_eq!(report.files_seen, 3);
        assert_eq!(report.recognized, 3);
        assert_eq!(report.new, 2, "有效 EPUB 与其他文件可正常索引");
        assert_eq!(report.errors, 1, "损坏 EPUB 作为文件级错误隔离");

        let resource_count: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM resources", [], |row| row.get(0))
            .unwrap();
        assert_eq!(resource_count, 2, "损坏 EPUB 不得写入资源");
    }

    #[tokio::test]
    async fn rescan_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        write(&root, "Movie.A.mkv", b"video");

        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        let first = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(first.new, 1);

        let second = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(second.new, 0, "幂等：不重复建实体");
        assert_eq!(second.skipped, 1, "未变化文件跳过");

        let count: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM works", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn modified_file_is_updated_not_duplicated() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        let path = write(&root, "Novel.B.txt", b"v1");

        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, b"v2-longer-content").unwrap();

        let second = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(second.new, 0);
        assert!(
            second.updated >= 1,
            "文件变化应标记 updated（got skipped={} errors={}）",
            second.skipped,
            second.errors
        );

        let works: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM works", [], |r| r.get(0))
            .unwrap();
        assert_eq!(works, 1, "同一路径仍是一个 Work");
    }

    #[tokio::test]
    async fn same_size_content_change_is_detected() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        let path = write(&root, "Novel.B.txt", b"AAAA-BBBB");

        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        let old_fingerprint: String = db
            .lock()
            .query_row("SELECT fingerprint_first FROM resources", [], |r| r.get(0))
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, b"AAAA-CCCC").unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            9,
            "前提：内容变化但文件大小不变"
        );

        let second = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(second.new, 0);
        assert!(
            second.updated >= 1,
            "同大小内容变化应标记 updated（got skipped={} errors={}）",
            second.skipped,
            second.errors
        );

        let works: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM works", [], |r| r.get(0))
            .unwrap();
        assert_eq!(works, 1, "同一路径仍是一个 Work");

        let fingerprint: Option<String> = db
            .lock()
            .query_row("SELECT fingerprint_first FROM resources", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            fingerprint.as_deref().map(str::len),
            Some(64),
            "指纹为 sha256 hex"
        );
        assert_ne!(
            fingerprint.as_deref(),
            Some(old_fingerprint.as_str()),
            "指纹应随内容更新（fast_fingerprint 首块）"
        );
    }

    #[tokio::test]
    async fn incremental_adds_only_new_file() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        write(&root, "Movie.A.mkv", b"video");

        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();

        write(&root, "Novel.C.txt", b"new book");
        let second = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(second.new, 1, "只新增一个新文件");
        assert_eq!(second.skipped, 1, "旧文件未变化跳过");

        let works: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM works", [], |r| r.get(0))
            .unwrap();
        assert_eq!(works, 2);
    }

    #[tokio::test]
    async fn temporary_files_are_skipped() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        write(&root, "Movie.A.mkv.part", b"partial");
        write(&root, ".hidden.mkv", b"hidden");

        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        let report = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(report.new, 0);
        assert_eq!(report.skipped, 2, "临时文件全部跳过");
    }

    #[tokio::test]
    async fn invalid_root_errors() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db);
        let err = scanner
            .scan_storage_location(StorageLocationId::new(), Path::new("Z:/nope"))
            .await
            .unwrap_err();
        assert_eq!(err.code().as_str(), "SCAN_ROOT_INVALID");
    }

    /// R-MAIN-08A：**storage overlay（fingerprint 未变）** 也必须被 Scanner 恢复 Available/user。
    #[tokio::test]
    async fn storage_overlay_is_restored_even_when_fingerprint_unchanged() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        write(&root, "Movie.A.mkv", b"fake-video-bytes");

        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();

        // 手动加 storage overlay（Missing/storage），fingerprint 不变。
        db.lock()
            .execute(
                "UPDATE resources SET availability = 'missing', availability_source = 'storage'
                 WHERE storage_location_id = ?1",
                rusqlite::params![storage_id.to_string()],
            )
            .unwrap();

        let report = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(report.new, 0, "不得新增实体");
        assert_eq!(
            report.updated, 1,
            "storage overlay 必须被恢复（fingerprint 未变也恢复）"
        );
        assert_eq!(report.skipped, 0);

        let (avail, src): (String, String) = db
            .lock()
            .query_row(
                "SELECT availability, availability_source FROM resources WHERE storage_location_id = ?1",
                rusqlite::params![storage_id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(avail, "available", "overlay 必须撤销");
        assert_eq!(src, "user", "恢复后来源规范为 user");
    }

    /// R-MAIN-08A：**Existing Available 但 source=Unknown** → 文件验证后规范为 Available/user。
    #[tokio::test]
    async fn available_unknown_source_is_normalized_to_user() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        write(&root, "Movie.A.mkv", b"fake-video-bytes");

        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        db.lock()
            .execute(
                "UPDATE resources SET availability_source = 'unknown'
                 WHERE storage_location_id = ?1",
                rusqlite::params![storage_id.to_string()],
            )
            .unwrap();

        let report = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(
            report.updated, 1,
            "Available/unknown 必须规范为 Available/user"
        );
        let src: String = db
            .lock()
            .query_row(
                "SELECT availability_source FROM resources WHERE storage_location_id = ?1",
                rusqlite::params![storage_id.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(src, "user");
    }

    /// R-MAIN-08A：**四种 user 显式非可用状态** + 真实存在的文件 → 重扫后必须保持
    /// availability/source（不得无条件改 Available/user）；fingerprint 未变时 Skipped。
    #[tokio::test]
    async fn user_unavailable_states_survive_rescan() {
        for state in [
            "source_unavailable",
            "temporarily_unavailable",
            "unknown",
            "missing",
        ] {
            let dir = TempDir::new().unwrap();
            let (storage_id, root) = sample_storage(&dir);
            write(&root, "Movie.A.mkv", b"fake-video-bytes");

            let db = Arc::new(Db::open_in_memory().unwrap());
            let scanner = LocalLibraryScanner::new(db.clone());
            scanner
                .scan_storage_location(storage_id, &root)
                .await
                .unwrap();
            db.lock()
                .execute(
                    "UPDATE resources SET availability = ?1, availability_source = 'user'
                     WHERE storage_location_id = ?2",
                    rusqlite::params![state, storage_id.to_string()],
                )
                .unwrap();

            let report = scanner
                .scan_storage_location(storage_id, &root)
                .await
                .unwrap();
            assert_eq!(report.new, 0, "{state}: 不得新增实体");
            assert_eq!(
                report.updated, 0,
                "{state}: fingerprint 未变 + user 显式状态 → Skipped"
            );

            let (avail, src): (String, String) = db
                .lock()
                .query_row(
                    "SELECT availability, availability_source FROM resources WHERE storage_location_id = ?1",
                    rusqlite::params![storage_id.to_string()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(avail, state, "{state}: user 显式状态必须保持");
            assert_eq!(src, "user");
        }
    }

    /// R-MAIN-08A：**user 非可用 + fingerprint 变化** → 只刷新必要字段，不改显式状态。
    #[tokio::test]
    async fn user_unavailable_state_survives_content_change() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        let path = write(&root, "Movie.A.mkv", b"v1");

        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        db.lock()
            .execute(
                "UPDATE resources SET availability = 'source_unavailable', availability_source = 'user'
                 WHERE storage_location_id = ?1",
                rusqlite::params![storage_id.to_string()],
            )
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, b"v2-longer-content").unwrap();

        let report = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(report.new, 0);
        assert!(report.updated >= 1, "内容变化应标记 updated");

        let (avail, src): (String, String) = db
            .lock()
            .query_row(
                "SELECT availability, availability_source FROM resources WHERE storage_location_id = ?1",
                rusqlite::params![storage_id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(avail, "source_unavailable", "内容变化不得改写显式状态");
        assert_eq!(src, "user");
    }

    /// R-MAIN-08A：**Unknown 且非 Available** → 保守保持（未知来源不当作 overlay）。
    #[tokio::test]
    async fn unknown_nonavailable_source_is_conservatively_kept() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        write(&root, "Movie.A.mkv", b"fake-video-bytes");

        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        db.lock()
            .execute(
                "UPDATE resources SET availability = 'source_unavailable', availability_source = 'unknown'
                 WHERE storage_location_id = ?1",
                rusqlite::params![storage_id.to_string()],
            )
            .unwrap();

        let report = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(
            report.updated, 0,
            "unknown+非 Available 不得当作 overlay 恢复"
        );

        let (avail, src): (String, String) = db
            .lock()
            .query_row(
                "SELECT availability, availability_source FROM resources WHERE storage_location_id = ?1",
                rusqlite::params![storage_id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(avail, "source_unavailable");
        assert_eq!(src, "unknown");
    }

    // ---- R-MAIN-09B：写事务 guard（cfg(test) before-write hook，禁止 sleep）----

    /// 构造真实位置 + 生产入口 target（application service + infra UoW 组装）。
    async fn make_target(
        db: &Arc<Db>,
        root: &Path,
    ) -> (
        haven_application::services::storage_location::ScanTarget,
        haven_domain::ids::StorageLocationId,
        StorageLocationService,
    ) {
        let svc = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
        let id = svc.add_local("库".into(), root).await.unwrap();
        let target = svc.get_scan_target(id).await.unwrap();
        (target, id, svc)
    }

    /// R-MAIN-09B：stale 在**文件 hash 准备完成后、写事务 guard 前**（before-write hook）
    /// 发生 → 0 条新 Work/Resource 写入（确定性）。
    #[tokio::test]
    async fn stale_before_write_guard_writes_zero_entities() {
        use crate::db::uow::SqliteStorageUoW;
        use haven_domain::enums::StorageStatus;

        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        write(&root, "Movie.A.mkv", b"fake-video-bytes");

        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        let (target, id, _svc) = make_target(&db, &root).await;

        // before-write hook：fingerprint/hash 准备完成后、写事务 guard 前执行 disconnect。
        let hook_uow = Arc::new(SqliteStorageUoW::new(db.clone()));
        scanner.set_before_write_hook(Box::new(move || {
            hook_uow
                .run(&|tx| {
                    let mut loc = tx.load_location(id).unwrap().unwrap();
                    loc.status = StorageStatus::Disconnected;
                    loc.updated_at = haven_common::UtcMillis::now();
                    tx.save_location(&loc)
                })
                .unwrap();
        }));

        let err = scanner.scan_target(&target).await.unwrap_err();
        assert_eq!(err.code().as_str(), "SCAN_TARGET_STALE");
        assert!(err.retryable());
        let works: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM works", [], |r| r.get(0))
            .unwrap();
        assert_eq!(works, 0, "guard 前 stale → 0 条 Work/Resource 写入");
    }

    /// R-MAIN-09B：mid-scan guard 错误不得被计入 report.errors 后返回 Ok——必须 Err。
    #[tokio::test]
    async fn mid_scan_guard_error_propagates_not_swallowed() {
        use crate::db::uow::SqliteStorageUoW;
        use haven_domain::enums::StorageStatus;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        write(&root, "Movie.A.mkv", b"fake-video-bytes");
        write(&root, "Novel.B.txt", b"some text");

        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        let (target, id, _svc) = make_target(&db, &root).await;

        let hook_uow = Arc::new(SqliteStorageUoW::new(db.clone()));
        let hook_id = id;
        let calls = Arc::new(AtomicUsize::new(0));
        let calls2 = calls.clone();
        scanner.set_before_write_hook(Box::new(move || {
            if calls2.fetch_add(1, Ordering::SeqCst) == 1 {
                // 第二个文件 guard 前 disconnect。
                hook_uow
                    .run(&|tx| {
                        let mut loc = tx.load_location(hook_id).unwrap().unwrap();
                        loc.status = StorageStatus::Disconnected;
                        loc.updated_at = haven_common::UtcMillis::now();
                        tx.save_location(&loc)
                    })
                    .unwrap();
            }
        }));

        let result = scanner.scan_target(&target).await;
        let err = result.expect_err("mid-scan guard 错误必须传播，不得吞进 errors 返回 Ok");
        assert_eq!(err.code().as_str(), "SCAN_TARGET_STALE");
        assert!(err.retryable());
        // R-MAIN-09B 返工（4）：hook 必须在两个文件写事务 guard 前各触发一次。
        assert!(
            calls.load(Ordering::SeqCst) >= 2,
            "before-write hook 必须至少触发 2 次（两文件各一次）"
        );
        // 首文件部分提交保留 1 条；第二文件未写入（断开后 guard 拦截）。
        let works: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM works", [], |r| r.get(0))
            .unwrap();
        assert_eq!(works, 1, "首文件部分提交保留 1 条 Work，第二文件不得写入");
    }

    /// R-MAIN-09B 返工（阻塞 2）：真实 DB/事务错误（写事务 guard 内 storage_locations 表被
    /// DROP）必须向上传播为 Err(DATABASE_ERROR)，**绝不**吞进 report.errors 返回 Ok。
    #[tokio::test]
    async fn db_error_in_write_guard_propagates_not_swallowed() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        write(&root, "Movie.A.mkv", b"fake-video-bytes");

        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        let (target, _id, _svc) = make_target(&db, &root).await;

        // 写事务 guard 前 DROP storage_locations → verify_token_on_conn 真实 DATABASE_ERROR。
        let db2 = db.clone();
        let guard_calls = Arc::new(AtomicUsize::new(0));
        let guard_calls2 = guard_calls.clone();
        scanner.set_before_write_hook(Box::new(move || {
            guard_calls2.fetch_add(1, Ordering::SeqCst);
            if guard_calls2.load(Ordering::SeqCst) == 1 {
                db2.lock()
                    .execute_batch("DROP TABLE storage_locations")
                    .unwrap();
            }
        }));

        let result = scanner.scan_target(&target).await;
        let err = result
            .expect_err("写事务内的 DATABASE_ERROR 必须向上传播，不得吞进 report.errors 返回 Ok");
        assert_eq!(
            err.code().as_str(),
            "DATABASE_ERROR",
            "DROP 表后的 token 校验错误必须是 DATABASE_ERROR"
        );
        assert!(err.retryable());
        assert!(
            guard_calls.load(Ordering::SeqCst) >= 1,
            "before-write hook 必须触发（证明错误发生在写事务 guard 内）"
        );
        // 未写入任何实体。
        let works: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM works", [], |r| r.get(0))
            .unwrap();
        assert_eq!(works, 0, "DB 错误时 0 条写入");
    }

    /// R-MAIN-09B（最终阻塞）：`FINGERPRINT_READ_FAILED`（指纹 **open** 失败）是预期
    /// 文件级 IO 错误——metadata 成功后、指纹读取前文件被删除 → fast_fingerprint 的
    /// File::open 失败返回 FINGERPRINT_READ_FAILED → scan_target **Ok(report)**
    /// （errors 计数、0 实体写入），而非 Err 终止整次扫描。
    #[tokio::test]
    async fn fingerprint_open_failure_is_file_level_not_fatal() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let path = write(&root, "Movie.A.mkv", b"fake-video-bytes");

        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        let (target, _id, _svc) = make_target(&db, &root).await;

        // after-metadata hook：metadata 成功（存在）后、fingerprint 读取前删除文件，
        // 使 fast_fingerprint 的 File::open 返回 FINGERPRINT_READ_FAILED。确定性、无 sleep。
        let calls = Arc::new(AtomicUsize::new(0));
        let calls2 = calls.clone();
        let path2 = path.clone();
        scanner.set_after_metadata_hook(Box::new(move || {
            calls2.fetch_add(1, Ordering::SeqCst);
            if calls2.load(Ordering::SeqCst) == 1 {
                std::fs::remove_file(&path2).unwrap();
            }
        }));

        let report = scanner
            .scan_target(&target)
            .await
            .expect("FINGERPRINT_READ_FAILED 是文件级错误，扫描必须 Ok（不得终止）");
        assert_eq!(report.files_seen, 1, "文件已被枚举");
        assert_eq!(report.recognized, 1, "扩展名已识别");
        assert_eq!(report.errors, 1, "指纹 open 失败必须计入 errors");
        assert_eq!(report.new, 0, "不得建立新实体");
        assert_eq!(report.updated, 0);
        assert!(
            calls.load(Ordering::SeqCst) >= 1,
            "after-metadata hook 必须非零触发"
        );
        // 0 Work / 0 Resource 写入。
        let works: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM works", [], |r| r.get(0))
            .unwrap();
        assert_eq!(works, 0, "IO 失败不得写入 Work");
        let resources: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM resources", [], |r| r.get(0))
            .unwrap();
        assert_eq!(resources, 0, "IO 失败不得写入 Resource");
        // 结束 token 复核通过（文件删除不改变 storage_locations 行）。
        let locations: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM storage_locations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(locations, 1, "存储位置行保留");
    }

    /// R-MAIN-09B（最终阻塞，两层互锁第 2 层）：真实 scan_target 回归——after-fingerprint-open
    /// hook 把刚成功打开的 Movie.A.mkv 截断为 0 字节（metadata 捕获的 size 仍为非零原值），
    /// 随后 read 遇 unexpected EOF → 真实 `FINGERPRINT_IO_FAILED`（非手造 AppError）→
    /// scan_target 必须 Ok(report)（errors=1、0 实体写入）。若白名单删除
    /// FINGERPRINT_IO_FAILED，本测试将失败为 Err，从而不是假绿。
    #[tokio::test]
    async fn fingerprint_io_failure_after_open_is_file_level_not_fatal() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        // 大于 CHUNK_SIZE，保证 hash_range 需实际读取（截断后 read 遇 EOF，remaining>0）。
        let big = vec![7u8; 100 * 1024];
        let path = root.join("Movie.A.mkv");
        std::fs::write(&path, &big).unwrap();

        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        let (target, _id, _svc) = make_target(&db, &root).await;

        // after-fingerprint-open hook：File::open 成功后、首次 hash_range 前截断为 0 字节。
        let calls = Arc::new(AtomicUsize::new(0));
        let calls2 = calls.clone();
        let path2 = path.clone();
        scanner.set_after_fingerprint_open_hook(Box::new(move || {
            calls2.fetch_add(1, Ordering::SeqCst);
            let f = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&path2)
                .unwrap();
            f.set_len(0).unwrap();
        }));

        let report = scanner
            .scan_target(&target)
            .await
            .expect("FINGERPRINT_IO_FAILED 是文件级错误，扫描必须 Ok（不得终止）");
        assert_eq!(report.files_seen, 1, "文件已被枚举");
        assert_eq!(report.recognized, 1, "扩展名已识别");
        assert_eq!(report.errors, 1, "指纹 IO 失败必须计入 errors");
        assert_eq!(report.new, 0, "不得建立新实体");
        assert_eq!(report.updated, 0);
        assert!(
            calls.load(Ordering::SeqCst) >= 1,
            "after-fingerprint-open hook 必须非零触发"
        );
        // 0 Work / 0 Resource 写入。
        let works: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM works", [], |r| r.get(0))
            .unwrap();
        assert_eq!(works, 0, "IO 失败不得写入 Work");
        let resources: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM resources", [], |r| r.get(0))
            .unwrap();
        assert_eq!(resources, 0, "IO 失败不得写入 Resource");
        // 结束 token 复核通过（截断不改变 storage_locations 行）。
        let locations: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM storage_locations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(locations, 1, "存储位置行保留");
        let _ = big;
    }

    // ---- D1：Missing 协调 + 保守改名复用 ----

    fn availability_of(db: &Arc<Db>, path: &str) -> (String, String) {
        use rusqlite::OptionalExtension;
        db.lock()
            .query_row(
                "SELECT availability, availability_source FROM resources
                 WHERE json_extract(locator_json, '$.local_path.path') = ?1",
                rusqlite::params![path],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .unwrap()
            .expect("资源行必须存在")
    }

    fn set_state(db: &Arc<Db>, path: &str, availability: &str, source: &str) {
        db.lock()
            .execute(
                "UPDATE resources SET availability = ?1, availability_source = ?2
                 WHERE json_extract(locator_json, '$.local_path.path') = ?3",
                rusqlite::params![availability, source, path],
            )
            .unwrap();
    }

    fn table_count(db: &Arc<Db>, table: &str) -> i64 {
        db.lock()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[tokio::test]
    async fn deleted_file_is_marked_missing_storage_after_successful_rescan() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        let gone = write(&root, "gone.mkv", b"delete me");
        write(&root, "keep.txt", b"keep me");
        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        std::fs::remove_file(&gone).unwrap();

        let report = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(report.errors, 0);
        assert_eq!(report.new, 0);
        assert_eq!(
            availability_of(&db, &normalize_path(&gone)),
            ("missing".into(), "storage".into())
        );
        assert_eq!(
            availability_of(&db, &normalize_path(&root.join("keep.txt"))),
            ("available".into(), "user".into())
        );

        // 幂等：再次全量扫描不产生任何变化。
        let again = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(again.updated, 0);
        assert_eq!(again.new, 0);
    }

    #[tokio::test]
    async fn restored_file_becomes_available_again() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        let path = write(&root, "Movie.A.mkv", b"same bytes");
        let content = std::fs::read(&path).unwrap();
        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(availability_of(&db, &normalize_path(&path)).0, "missing");

        std::fs::write(&path, &content).unwrap();
        let report = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(report.new, 0);
        assert_eq!(
            availability_of(&db, &normalize_path(&path)),
            ("available".into(), "user".into())
        );
    }

    #[tokio::test]
    async fn explicit_user_unknown_and_offline_states_are_never_reconciled() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        let files = [
            write(&root, "a.txt", b"a"),
            write(&root, "b.txt", b"b"),
            write(&root, "c.mkv", b"c"),
        ];
        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        set_state(&db, &normalize_path(&files[0]), "unknown", "user");
        set_state(
            &db,
            &normalize_path(&files[1]),
            "source_unavailable",
            "unknown",
        );
        set_state(&db, &normalize_path(&files[2]), "offline_available", "user");
        for f in &files {
            std::fs::remove_file(f).unwrap();
        }
        let report = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(report.errors, 0);
        assert_eq!(
            availability_of(&db, &normalize_path(&files[0])),
            ("unknown".into(), "user".into())
        );
        assert_eq!(
            availability_of(&db, &normalize_path(&files[1])),
            ("source_unavailable".into(), "unknown".into())
        );
        assert_eq!(
            availability_of(&db, &normalize_path(&files[2])),
            ("offline_available".into(), "user".into())
        );
    }

    #[tokio::test]
    async fn file_level_error_suppresses_reconciliation() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        let present = write(&root, "Movie.A.mkv", b"present");
        let gone = write(&root, "gone.txt", b"gone");
        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        std::fs::remove_file(&gone).unwrap();
        // 文件级错误：metadata 后删除现有文件 → FINGERPRINT_READ_FAILED。
        let calls = Arc::new(AtomicUsize::new(0));
        let calls2 = calls.clone();
        let present2 = present.clone();
        scanner.set_after_metadata_hook(Box::new(move || {
            if calls2.fetch_add(1, Ordering::SeqCst) == 0 {
                std::fs::remove_file(&present2).unwrap();
            }
        }));
        let report = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(report.errors, 1);
        assert_eq!(
            availability_of(&db, &normalize_path(&gone)),
            ("available".into(), "user".into()),
            "文件级错误时不得标记任何 Missing"
        );
    }

    #[tokio::test]
    async fn cancelled_scan_marks_nothing_missing() {
        struct CancelAfterFirst {
            calls: std::sync::atomic::AtomicUsize,
        }
        impl ScanObserver for CancelAfterFirst {
            fn on_phase(&self, _phase: ScanPhase) {}
            fn on_progress(&self, _progress: ScanProgress) {
                self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            fn on_warning(&self, _message: String, _progress: ScanProgress) {}
            fn is_cancelled(&self) -> bool {
                self.calls.load(std::sync::atomic::Ordering::SeqCst) >= 1
            }
        }
        use std::sync::atomic::Ordering;
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let gone = write(&root, "gone.txt", b"gone");
        write(&root, "b.txt", b"b");
        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        let (target, _id, _svc) = make_target(&db, &root).await;
        scanner.scan_target(&target).await.unwrap();
        std::fs::remove_file(&gone).unwrap();

        let observer = CancelAfterFirst {
            calls: std::sync::atomic::AtomicUsize::new(0),
        };
        let report = scanner
            .scan_target_observed(&target, &observer)
            .await
            .unwrap();
        assert!(observer.calls.load(Ordering::SeqCst) >= 1);
        assert_eq!(report.errors, 0);
        assert_eq!(
            availability_of(&db, &normalize_path(&gone)),
            ("available".into(), "user".into()),
            "取消的扫描不得标记 Missing"
        );
    }

    #[tokio::test]
    async fn stale_during_reconcile_writes_zero_missing() {
        use haven_domain::enums::StorageStatus;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let gone = write(&root, "gone.txt", b"gone");
        write(&root, "b.txt", b"b");
        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        let (target, id, _svc) = make_target(&db, &root).await;
        scanner.scan_target(&target).await.unwrap();
        std::fs::remove_file(&gone).unwrap();

        let hook_uow = Arc::new(SqliteStorageUoW::new(db.clone()));
        let calls = Arc::new(AtomicUsize::new(0));
        let calls2 = calls.clone();
        scanner.set_before_write_hook(Box::new(move || {
            if calls2.fetch_add(1, Ordering::SeqCst) == 0 {
                hook_uow
                    .run(&|tx| {
                        let mut loc = tx.load_location(id).unwrap().unwrap();
                        loc.status = StorageStatus::Disconnected;
                        loc.updated_at = haven_common::UtcMillis::now();
                        tx.save_location(&loc)
                    })
                    .unwrap();
            }
        }));
        let err = scanner.scan_target(&target).await.unwrap_err();
        assert_eq!(err.code().as_str(), "SCAN_TARGET_STALE");
        assert!(calls.load(Ordering::SeqCst) >= 1);
        assert_eq!(
            availability_of(&db, &normalize_path(&gone)),
            ("available".into(), "user".into()),
            "stale 时协调事务回滚，0 条 Missing"
        );
    }

    #[tokio::test]
    async fn legacy_separator_rows_are_reconciled_and_not_used_for_reuse() {
        let source = r"C:\legacy\dir\a.txt";
        assert!(resolve_under_root(Path::new(r"C:\legacy"), Path::new(source)).is_some());
        assert!(resolve_under_root(Path::new(r"C:\legacy"), Path::new(r"C:\legacy")).is_none());
        assert!(
            resolve_under_root(Path::new(r"C:\legacy"), Path::new(r"C:\other\a.txt")).is_none()
        );
        assert!(
            resolve_under_root(Path::new(r"C:\legacy"), Path::new(r"C:\legacy\..\x")).is_none()
        );
        if !cfg!(windows) {
            // Unix 上反斜杠是合法文件名字符，不得被当作分隔符归一。
            assert!(resolve_under_root(Path::new("/l"), Path::new(r"/l\sub")).is_none());
            return;
        }
        assert!(same_path_identity(source, "C:/LEGACY/dir/a.txt"));
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        let path = write(&root, "download.txt", b"downloaded text");
        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        db.lock()
            .execute(
                "UPDATE resources SET locator_json = json_set(locator_json, '$.local_path.path',
                 replace(json_extract(locator_json, '$.local_path.path'), '/', char(92)))",
                [],
            )
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        let state: String = db
            .lock()
            .query_row("SELECT availability FROM resources", [], |r| r.get(0))
            .unwrap();
        assert_eq!(state, "missing", "旧分隔符行同样必须被协调为 Missing");
    }

    #[tokio::test]
    async fn renamed_file_reuses_identity_and_user_state() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        let old = write(&root, "a.mkv", b"rename-me-content");
        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        let (resource_id, media_item_id): (String, String) = db
            .lock()
            .query_row("SELECT id, media_item_id FROM resources", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        db.lock()
            .execute(
                "INSERT INTO progress (id, work_id, edition_id, media_item_id, locator_json,
                     completion, last_active_at, updated_at)
                 SELECT 'p1', w.id, e.id, m.id, '{}', 'in_progress', 1, 1
                 FROM media_items m JOIN editions e ON e.id = m.edition_id
                 JOIN works w ON w.id = e.work_id WHERE m.id = ?1",
                rusqlite::params![media_item_id],
            )
            .unwrap();

        let new = root.join("b.mkv");
        std::fs::rename(&old, &new).unwrap();
        let report = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(report.new, 0, "改名不得新建 Work 链");
        assert_eq!(report.updated, 1);
        assert_eq!(table_count(&db, "works"), 1);
        assert_eq!(table_count(&db, "resources"), 1);
        let (id, locator): (String, String) = db
            .lock()
            .query_row("SELECT id, locator_json FROM resources", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(id, resource_id, "Resource 身份必须保留");
        let locator_path = format!("\"path\":\"{}\"", normalize_path(&new));
        assert!(locator.contains(&locator_path), "{locator}");
        let progress_media_item: String = db
            .lock()
            .query_row("SELECT media_item_id FROM progress", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            progress_media_item, media_item_id,
            "进度仍绑定同一 MediaItem"
        );
        assert_eq!(
            availability_of(&db, &normalize_path(&new)),
            ("available".into(), "user".into())
        );
    }

    #[tokio::test]
    async fn rename_with_changed_content_creates_new_identity_and_marks_old_missing() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        let old = write(&root, "a.mkv", b"original-content");
        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        let new = root.join("b.mkv");
        std::fs::rename(&old, &new).unwrap();
        std::fs::write(&new, b"original-content-plus-more").unwrap();
        let report = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(report.new, 1);
        assert_eq!(table_count(&db, "works"), 2);
        assert_eq!(
            availability_of(&db, &normalize_path(&old)),
            ("missing".into(), "storage".into())
        );
    }

    #[tokio::test]
    async fn copy_with_original_present_keeps_original_and_creates_new_identity() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        let original = write(&root, "a.txt", b"copy-me");
        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        std::fs::copy(&original, root.join("b.txt")).unwrap();
        let report = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(report.new, 1, "原文件仍在 → 不得复用为改名");
        assert_eq!(table_count(&db, "works"), 2);
        assert_eq!(
            availability_of(&db, &normalize_path(&original)),
            ("available".into(), "user".into())
        );
        assert_eq!(
            availability_of(&db, &normalize_path(&root.join("b.txt"))),
            ("available".into(), "user".into())
        );
    }

    #[tokio::test]
    async fn ambiguous_old_and_new_matches_never_pair_arbitrarily() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        let a = write(&root, "a.txt", b"identical");
        let b = write(&root, "b.txt", b"identical");
        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        std::fs::remove_file(&a).unwrap();
        std::fs::remove_file(&b).unwrap();
        std::fs::write(root.join("c.txt"), b"identical").unwrap();
        let report = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(report.new, 1, "两个旧候选 → 不任意配对");
        assert_eq!(table_count(&db, "resources"), 3);
        assert_eq!(
            availability_of(&db, &normalize_path(&a)),
            ("missing".into(), "storage".into())
        );
        assert_eq!(
            availability_of(&db, &normalize_path(&b)),
            ("missing".into(), "storage".into())
        );
    }

    #[test]
    fn resolved_root_rejects_escape_and_other_roots() {
        let root = if cfg!(windows) {
            Path::new(r"C:\library")
        } else {
            Path::new("/library")
        };
        let inside = if cfg!(windows) {
            Path::new(r"C:\library\sub\a.txt")
        } else {
            Path::new("/library/sub/a.txt")
        };
        let escape = if cfg!(windows) {
            Path::new(r"C:\library\..\outside.txt")
        } else {
            Path::new("/library/../outside.txt")
        };
        let elsewhere = if cfg!(windows) {
            Path::new(r"C:\other\a.txt")
        } else {
            Path::new("/other/a.txt")
        };
        assert!(resolve_under_root(root, inside).is_some());
        assert!(
            resolve_under_root(root, root).is_none(),
            "根目录本身不是资源"
        );
        assert!(resolve_under_root(root, escape).is_none(), "不得越出根");
        assert!(resolve_under_root(root, elsewhere).is_none());
        if cfg!(windows) {
            assert!(
                resolve_under_root(Path::new(r"C:\Library"), Path::new(r"c:\library\x")).is_some()
            );
            assert!(same_path_identity(r"C:\library\a.mkv", "c:/library/A.mkv"));
        } else {
            // Unix：反斜杠是文件名字符，不得当分隔符。
            assert!(resolve_under_root(Path::new("/l"), Path::new(r"/l/sub")).is_some());
            assert!(resolve_under_root(Path::new(r"/l"), Path::new(r"/l\x")).is_none());
            assert!(!same_path_identity("/library/a.mkv", "/library/A.mkv"));
        }
    }

    #[test]
    fn large_files_without_full_hash_are_not_reused() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let fp = FileFingerprint {
            size: 10,
            modified_ms: 2,
            first_chunk_sha256: "aa".into(),
            last_chunk_sha256: "bb".into(),
        };
        let old = Resource {
            id: ResourceId::new(),
            media_item_id: MediaItemId::new(),
            resource_type: haven_domain::enums::ResourceType::LocalFile,
            source_id: None,
            storage_location_id: Some(StorageLocationId::new()),
            locator: ResourceLocator::LocalPath {
                path: normalize_path(&root.join("old.mkv")),
            },
            mime_type: Some("video/x-matroska".into()),
            size: Some(10),
            hash: None,
            availability: Availability::Available,
            availability_source: AvailabilitySource::User,
            modified_ms: Some(1),
            fingerprint_first: Some("aa".into()),
            fingerprint_last: Some("bb".into()),
            created_at: haven_common::UtcMillis(0),
            updated_at: haven_common::UtcMillis(0),
        };
        let with_hash = Resource {
            hash: Some(haven_domain::entities::ContentHash {
                algorithm: haven_domain::enums::HashAlgorithm::Sha256,
                digest: "cc".into(),
            }),
            ..old.clone()
        };
        let prepared = PreparedEntry {
            detected: detect_by_extension("new.mkv").unwrap(),
            fingerprint: fp,
            modified_ms: 2,
            full_hash: None,
        };
        let observed = std::collections::HashSet::new();
        assert!(!rename_evidence_matches(
            &root,
            &old,
            &root.join("new.mkv"),
            &prepared,
            &observed
        ));
        // 大文件（无全量哈希）：即便旧行已有哈希也不复用（首末块碰撞风险）。
        assert!(!rename_evidence_matches(
            &root,
            &with_hash,
            &root.join("new.mkv"),
            &prepared,
            &observed
        ));
    }

    #[tokio::test]
    async fn multiple_new_copies_do_not_arbitrarily_inherit_one_old_identity() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        let old = write(&root, "old.txt", b"same content");
        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        std::fs::remove_file(&old).unwrap();
        write(&root, "b.txt", b"same content");
        write(&root, "c.txt", b"same content");
        let report = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(report.new, 2);
        assert_eq!(table_count(&db, "resources"), 3);
        assert_eq!(availability_of(&db, &normalize_path(&old)).0, "missing");
    }

    #[tokio::test]
    async fn an_extant_copy_prevents_reusing_another_missing_identity() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        let old = write(&root, "old.txt", b"same content");
        let kept = write(&root, "kept.txt", b"same content");
        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        std::fs::remove_file(&old).unwrap();
        write(&root, "new.txt", b"same content");
        let report = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(report.new, 1);
        assert_eq!(table_count(&db, "resources"), 3);
        assert_eq!(availability_of(&db, &normalize_path(&old)).0, "missing");
        assert_eq!(availability_of(&db, &normalize_path(&kept)).0, "available");
    }

    #[tokio::test]
    async fn a_file_restored_before_the_reconcile_transaction_is_not_marked_missing() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        let path = write(&root, "book.txt", b"content");
        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        let restored = path.clone();
        scanner.set_before_write_hook(Box::new(move || {
            std::fs::write(&restored, b"content").unwrap();
        }));
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(availability_of(&db, &normalize_path(&path)).0, "available");
    }

    #[tokio::test]
    async fn a_unique_rename_is_not_blocked_by_an_unrelated_new_file() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        let old = write(&root, "old.txt", b"unique book");
        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        let old_id = scanner
            .find_resource_by_local_path(&normalize_path(&old), storage_id)
            .await
            .unwrap()
            .unwrap()
            .id;
        let moved = root.join("moved.txt");
        std::fs::rename(&old, &moved).unwrap();
        write(&root, "another.txt", b"different book");
        let report = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(report.new, 1);
        assert_eq!(report.updated, 1);
        assert_eq!(table_count(&db, "resources"), 2);
        assert_eq!(
            scanner
                .find_resource_by_local_path(&normalize_path(&moved), storage_id)
                .await
                .unwrap()
                .unwrap()
                .id,
            old_id
        );
    }

    #[tokio::test]
    async fn content_changed_after_preparation_is_not_indexed_or_used_to_mark_missing() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        let old = write(&root, "old.txt", b"old");
        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        std::fs::remove_file(&old).unwrap();
        let added = write(&root, "new.txt", b"new");
        scanner.set_before_write_hook(Box::new(move || {
            std::fs::write(&added, b"changed after preparation").unwrap();
        }));
        let report = scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(report.errors, 1);
        assert_eq!(report.new, 0);
        assert_eq!(table_count(&db, "resources"), 1);
        assert_eq!(availability_of(&db, &normalize_path(&old)).0, "available");
    }

    #[tokio::test]
    async fn legacy_rows_inside_a_pruned_subtree_are_outside_missing_reconciliation() {
        let dir = TempDir::new().unwrap();
        let (storage_id, root) = sample_storage(&dir);
        let path = write(&root, "book.txt", b"content");
        let db = Arc::new(Db::open_in_memory().unwrap());
        let scanner = LocalLibraryScanner::new(db.clone());
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        let mut resource = scanner
            .find_resource_by_local_path(&normalize_path(&path), storage_id)
            .await
            .unwrap()
            .unwrap();
        let hidden = root.join(".hidden").join("book.txt");
        std::fs::create_dir_all(hidden.parent().unwrap()).unwrap();
        std::fs::rename(&path, &hidden).unwrap();
        resource.locator = ResourceLocator::LocalPath {
            path: normalize_path(&hidden),
        };
        db.with_tx(|tx| crate::db::repos::resource::save_on_conn(tx, &resource))
            .unwrap();
        std::fs::remove_file(&hidden).unwrap();
        scanner
            .scan_storage_location(storage_id, &root)
            .await
            .unwrap();
        assert_eq!(
            availability_of(&db, &normalize_path(&hidden)).0,
            "available"
        );
    }
}
