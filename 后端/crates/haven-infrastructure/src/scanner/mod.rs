//! 本地媒体库扫描器（第一版）。
//!
//! 规范：`plan/LIBRARY_AND_STORAGE.md` §28–§42、§207–§214。
//! 管线：Enumerate → Fast Stat → Format Detect → File/Directory Fingerprint →
//!       Existing Match（路径+size+modified 未变则跳过）→ Index Write。
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
