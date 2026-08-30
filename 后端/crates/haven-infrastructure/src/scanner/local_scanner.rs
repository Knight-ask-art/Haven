//! 本地扫描器编排：Enumerate → Detect → Fingerprint → Match → Index。
//!
//! 规范：LIBRARY_AND_STORAGE §28–§42、§207–§214。
//! - 幂等：同路径且 size+modified+首块指纹 未变 → skip（增量语义基础）。
//! - 新文件/变更文件 → 建立 Work → Edition → MediaItem → Resource 四层（单事务原子提交）。
//! - 单文件失败隔离：不影响其他已安全索引文件（§29 原则）。
//! - Missing 两阶段标记（§212）为后续任务（BE-SCAN-001）。
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
        // 阶段事件（契约 §14.4）：本扫描器是单遍管线（枚举与索引交错），
        // Detecting/Fingerprinting 是单文件内的子阶段——若按文件发阶段事件，
        // on_phase 的强制发射会绕过限频淹没 Channel。因此只发两个全局阶段
        // 切换：Enumerating（开始）→ Indexing（首个入库结果），单文件子阶段
        // 由 item_indexed 进度流表达。
        let mut indexing_emitted = false;
        if let Some(o) = observer {
            o.on_phase(ScanPhase::Enumerating);
        }
        for entry in WalkDir::new(&root).follow_links(false) {
            // 协作式取消（端口契约）：文件边界检查；取消时返回 Ok，已索引部分保留。
            if observer.is_some_and(|o| o.is_cancelled()) {
                break;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    report.errors += 1;
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
                        detected
                    }
                    Ok(None) => continue,
                    Err(e) if is_file_level_error(&e) => {
                        report.errors += 1;
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
            match self.index_path(token, storage_id, path, &detected).await {
                Ok(IndexOutcome::New) => report.new += 1,
                Ok(IndexOutcome::Updated) => report.updated += 1,
                Ok(IndexOutcome::Skipped) => report.skipped += 1,
                // R-MAIN-09B 返工（阻塞 2）错误分类：
                // - 仅**明确的文件级 IO/解析/内容类错误**（SCAN_STAT_FAILED / 指纹哈希类）
                //   计入 report.errors，并作为 Warning 事件上报（§14.4：单文件失败不终止）；
                // - `SCAN_TARGET_STALE` 与一切数据库错误（DATABASE_ERROR）、事务错误、
                //   未知错误 **立即向上传播**，绝不吞进 report.errors 返回 Ok。
                Err(e) if is_file_level_error(&e) => {
                    report.errors += 1;
                    if let Some(o) = observer {
                        o.on_warning(
                            format!("文件索引失败（{}）: {e}", e.code().as_str()),
                            to_progress(&report, Some(path)),
                        );
                    }
                }
                Err(e) => return Err(e),
            }
            if !indexing_emitted && (report.new > 0 || report.updated > 0 || report.errors > 0) {
                indexing_emitted = true;
                if let Some(o) = observer {
                    o.on_phase(ScanPhase::Indexing);
                }
            }
            emit_progress(observer, &report, path);
        }
        Ok(report)
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

    /// 单个文件或图片目录的完整管线（R-MAIN-09B：写事务带 token guard）。
    /// 原子性：existing 单行 / 四层 new 在**单一事务**内提交；事务内先重读校验 token 再 save，
    /// disconnect/rebind/remove 无法在校验与写之间提交。stale → SCAN_TARGET_STALE（立即终止）。
    /// `token=None` 仅供 crate 内测试 helper。
    async fn index_path(
        &self,
        token: Option<&ScanTargetToken>,
        storage_id: StorageLocationId,
        path: &Path,
        detected: &crate::scanner::detect::DetectResult,
    ) -> Result<IndexOutcome, AppError> {
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
            // 变化检测：size / modified_ms / 首块指纹 任一变化 → 需要刷新。
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
                return Ok(IndexOutcome::Skipped);
            }

            // 更新：始终刷新文件指纹/哈希（不重复建实体）。
            let full_hash = if !is_image_sequence && size <= FULL_HASH_THRESHOLD {
                Some(full_hash_off_thread(path).await?)
            } else {
                None
            };
            let mut updated = resource;
            updated.size = Some(size);
            updated.modified_ms = Some(modified_ms);
            updated.fingerprint_first = Some(fingerprint.first_chunk_sha256);
            updated.fingerprint_last = Some(fingerprint.last_chunk_sha256);
            updated.mime_type = Some(detected.mime_type.to_owned());
            updated.resource_type = detected.resource_type;
            updated.hash = full_hash.map(|digest| haven_domain::entities::ContentHash {
                algorithm: haven_domain::enums::HashAlgorithm::Sha256,
                digest,
            });
            if needs_overlay_restore || needs_source_normalize {
                updated.availability = Availability::Available;
                updated.availability_source = AvailabilitySource::User;
            }
            // 否则（user/unknown 显式非可用）保持 availability/availability_source 不变。
            updated.updated_at = haven_common::UtcMillis::now();
            #[cfg(test)]
            self.run_before_write_hook();
            self.db.with_tx(|tx| {
                // R-MAIN-09B：写事务内先重读校验 token，再 save（guard 强制）。
                if let Some(t) = token {
                    verify_token_on_conn(tx, t)?;
                }
                crate::db::repos::resource::save_on_conn(tx, &updated)
            })?;
            return Ok(IndexOutcome::Updated);
        }

        // 新文件：Fingerprint（大文件只做快速指纹，不做全量哈希 §42）。
        let full_hash = if !is_image_sequence && size <= FULL_HASH_THRESHOLD {
            Some(full_hash_off_thread(path).await?)
        } else {
            None
        };

        // Candidate → Index Write（单事务，四层原子提交）。
        let title = detected
            .title_hint
            .clone()
            .unwrap_or_else(|| file_name(path));
        let now = haven_common::UtcMillis::now();

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
            locator: ResourceLocator::LocalPath { path: path_str },
            mime_type: Some(detected.mime_type.to_owned()),
            size: Some(size),
            hash: full_hash.map(|digest| haven_domain::entities::ContentHash {
                algorithm: haven_domain::enums::HashAlgorithm::Sha256,
                digest,
            }),
            availability: Availability::Available,
            // 扫描器显式设置 → user 来源：位置级操作（disconnect/path-missing/恢复）不覆盖。
            availability_source: haven_domain::enums::AvailabilitySource::User,
            modified_ms: Some(modified_ms),
            fingerprint_first: Some(fingerprint.first_chunk_sha256),
            fingerprint_last: Some(fingerprint.last_chunk_sha256),
            created_at: now,
            updated_at: now,
        };

        // Candidate → Index Write（单事务，四层原子提交；失败整体回滚）。
        #[cfg(test)]
        self.run_before_write_hook();
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
        match row {
            Some(resource) => match &resource.locator {
                ResourceLocator::LocalPath { path: p } if p == path => Ok(Some(resource)),
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
    path.to_string_lossy().replace('\\', "/")
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
}
