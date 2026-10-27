//! 本地媒体库扫描器（第一版）。
//!
//! 规范：`plan/LIBRARY_AND_STORAGE.md` §28–§42、§207–§214。
//! 管线：Enumerate → Fast Stat → Format Detect → File/Directory Fingerprint →
//!       Existing Match（路径+size+modified 未变则跳过）→ Index Write →
//!       Rename Reuse / Missing Reconcile（D1，仅完整成功扫描）。
//! - D1：扫描开始快照本存储位置已有 local_path 资源；枚举记录全部已观察路径身份；
//!   **完整成功**扫描结束时，根内、未被观察且正向 NotFound 的 `Available` 资源才标记
//!   `Missing/Storage`（User/Unknown 显式非可用、OfflineAvailable 永不触碰）。
//! - D1：改名/移动只在同一存储、同一根内、类型+MIME+size+首末指纹一致且有全量哈希相等、
//!   旧路径缺失、目标未被占用、旧/新候选均唯一时复用原 Resource/MediaItem 链（保留进度、
//!   书签、收藏、历史）；写入事务内重读复核，不猜测、不删除用户数据。
//! 原则：
//! - 扫描快、元数据后台（本版不做网络 enrichment）。
//! - 文件身份 = StorageLocationId + Path + Size + ModifiedTime + FastFingerprint；
//!   图片目录使用直接图片清单指纹并登记为 `ImageSequence`。
//! - 大文件（> FULL_HASH_THRESHOLD）首次只做 FastFingerprint，不全量 SHA-256。
//! - 逐文件提交：天然满足 §210 crash recovery（已提交批次保留）。
//! - 幂等：同路径同 stat 不重复建实体。

pub mod detect;
pub mod fingerprint;
pub mod local_scanner;

pub use local_scanner::{IndexOutcome, LocalLibraryScanner, ScanReport};
