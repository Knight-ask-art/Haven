//! 文件指纹（分层哈希策略，LIBRARY_AND_STORAGE §40–§42）。
//!
//! - FastFingerprint：size + 首块 SHA-256 + 末块 SHA-256（快速疑似匹配）。
//! - FullHash：全量 SHA-256（重复确认 / 图书身份 / 同步 / 下载校验时使用）。
//! - 大文件（≥ FULL_HASH_THRESHOLD）首次只做 FastFingerprint，不全量哈希。

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use sha2::{Digest, Sha256};

use haven_common::AppError;

/// 超过该大小（字节）的文件首次扫描不做全量 SHA-256。
pub const FULL_HASH_THRESHOLD: u64 = 512 * 1024 * 1024; // 512 MiB

/// 快速指纹采样块大小。
const CHUNK_SIZE: usize = 64 * 1024; // 64 KiB

/// 文件指纹（§40/§42）。
#[derive(Debug, Clone, PartialEq)]
pub struct FileFingerprint {
    pub size: u64,
    pub modified_ms: u64,
    pub first_chunk_sha256: String,
    pub last_chunk_sha256: String,
}

/// 对文件计算快速指纹（首块 + 末块哈希）。小文件首末块可能重叠/相同。
pub fn fast_fingerprint(
    path: &Path,
    size: u64,
    modified_ms: u64,
) -> Result<FileFingerprint, AppError> {
    fast_fingerprint_inner(path, size, modified_ms, None)
}

/// cfg(test) only：`fast_fingerprint` 的等价内部实现，但允许在 **File::open 成功之后、
/// 第一次 hash_range 之前**注入 hook（确定性构造 read 失败，如把文件截断为 0 字节）。
/// hook 为无返回值闭包（测试内部自管失败），执行时**只调用 hook()，绝不 map_err 成
/// FINGERPRINT_IO_FAILED**——该错误只能来自后续 hash_range 的真实 read EOF。
/// 生产路径不暴露该入口、无 hook 时走完全相同的算法分支。
#[cfg(test)]
pub(crate) fn fast_fingerprint_with_after_open_hook(
    path: &Path,
    size: u64,
    modified_ms: u64,
    after_open: Box<dyn FnOnce()>,
) -> Result<FileFingerprint, AppError> {
    fast_fingerprint_inner(path, size, modified_ms, Some(after_open))
}

fn fast_fingerprint_inner(
    path: &Path,
    size: u64,
    modified_ms: u64,
    after_open: Option<Box<dyn FnOnce()>>,
) -> Result<FileFingerprint, AppError> {
    let mut file = File::open(path).map_err(|e| {
        AppError::new(
            "FINGERPRINT_READ_FAILED",
            haven_common::ErrorKind::Io,
            "无法读取文件",
            false,
        )
        .with_source(e)
    })?;

    if let Some(hook) = after_open {
        // 只执行 hook（测试注入用）；hook 自身的失败由测试内部处理，不污染错误分类。
        hook();
    }

    // 首块期望读 min(CHUNK, size)：小文件读完全部字节（正常结束），
    // 大文件读 CHUNK；若实际文件比 metadata size 短（打开后/读取中被截断），
    // hash_range 在 remaining>0 时遇 EOF 会经 io_err 报 FINGERPRINT_IO_FAILED。
    let first = hash_range(&mut file, 0, (CHUNK_SIZE as u64).min(size))?;
    let last = if size > CHUNK_SIZE as u64 {
        file.seek(SeekFrom::Start(size - CHUNK_SIZE as u64))
            .map_err(io_err("定位文件失败"))?;
        hash_range(&mut file, size - CHUNK_SIZE as u64, CHUNK_SIZE as u64)?
    } else {
        first.clone()
    };

    Ok(FileFingerprint {
        size,
        modified_ms,
        first_chunk_sha256: first,
        last_chunk_sha256: last,
    })
}

/// 全量 SHA-256（分块缓冲，内存有界）。
pub fn full_hash_sha256(path: &Path) -> Result<String, AppError> {
    let mut file = File::open(path).map_err(|e| {
        AppError::new(
            "FINGERPRINT_READ_FAILED",
            haven_common::ErrorKind::Io,
            "无法读取文件",
            false,
        )
        .with_source(e)
    })?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(io_err("读取文件失败"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

/// 读取 [offset, offset+len) 区间的哈希（文件必须已定位到 offset；seek 由调用方负责）。
fn hash_range(file: &mut File, offset: u64, len: u64) -> Result<String, AppError> {
    file.seek(SeekFrom::Start(offset))
        .map_err(io_err("定位文件失败"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut remaining = len;
    while remaining > 0 {
        let want = remaining.min(buf.len() as u64) as usize;
        let n = file
            .read(&mut buf[..want])
            .map_err(io_err("读取文件失败"))?;
        if n == 0 {
            // 文件在读取期间被截断（已打开句柄后内容变短）——真实 IO 错误，
            // 经 io_err 映射为 FINGERPRINT_IO_FAILED，而不是静默当正常 EOF 吞掉。
            return Err(io_err("文件在读取期间被截断")(
                std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "file truncated while reading",
                ),
            ));
        }
        hasher.update(&buf[..n]);
        remaining -= n as u64;
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn io_err(msg: &'static str) -> impl Fn(std::io::Error) -> AppError {
    move |e| {
        AppError::new(
            "FINGERPRINT_IO_FAILED",
            haven_common::ErrorKind::Io,
            msg,
            false,
        )
        .with_source(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn full_hash_matches_known_value() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.bin");
        fs::write(&path, b"hello world").unwrap();
        // sha256sum of "hello world"
        assert_eq!(
            full_hash_sha256(&path).unwrap(),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn fast_fingerprint_is_stable_and_content_sensitive() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a.bin");
        // 前半 0x07、后半 0x09：确保首块与末块内容不同（> CHUNK_SIZE）
        let mut big = vec![7u8; 100 * 1024];
        big.extend(vec![9u8; 100 * 1024]);
        fs::write(&path, &big).unwrap();
        let meta = fs::metadata(&path).unwrap();
        let ms = meta
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let a = fast_fingerprint(&path, meta.len(), ms).unwrap();
        let b = fast_fingerprint(&path, meta.len(), ms).unwrap();
        assert_eq!(a, b, "同文件同 stat 指纹稳定");
        assert_ne!(
            a.first_chunk_sha256, a.last_chunk_sha256,
            "大文件首末块应不同"
        );

        // 内容变化 → 指纹变化
        let path2 = dir.path().join("b.bin");
        let mut big2 = big.clone();
        big2[0] = 9;
        fs::write(&path2, &big2).unwrap();
        let c = fast_fingerprint(&path2, meta.len(), ms).unwrap();
        assert_ne!(
            a.first_chunk_sha256, c.first_chunk_sha256,
            "内容变化首块哈希变化"
        );
    }

    #[test]
    fn small_file_first_and_last_chunks_identical() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("small.bin");
        fs::write(&path, b"tiny").unwrap();
        let meta = fs::metadata(&path).unwrap();
        let fp = fast_fingerprint(&path, meta.len(), 0).unwrap();
        assert_eq!(
            fp.first_chunk_sha256, fp.last_chunk_sha256,
            "小文件首末块相同"
        );
    }

    #[test]
    fn missing_file_errors() {
        let err = fast_fingerprint(Path::new("Z:/definitely/missing/file.bin"), 0, 0).unwrap_err();
        assert_eq!(err.code().as_str(), "FINGERPRINT_READ_FAILED");
    }

    /// R-MAIN-09B（最终阻塞，两层互锁第 1 层）：after-open hook 在 File::open 成功后、
    /// 第一次 hash_range 前把文件截断为 0 → 后续 read 遇到 unexpected EOF → 真实
    /// `FINGERPRINT_IO_FAILED`（经 `hash_range` 的 io_err 路径），且 hook 非零触发。
    #[test]
    fn after_open_truncate_yields_fingerprint_io_failed() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("big.bin");
        // 大于 CHUNK_SIZE，保证 hash_range 要真正读取内容（而非一次读满即结束）。
        let big = vec![7u8; 100 * 1024];
        fs::write(&path, &big).unwrap();
        let size = big.len() as u64;

        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let truncate_succeeded = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let calls_hook = calls.clone();
        let truncated_hook = truncate_succeeded.clone();
        let path_hook = path.clone();
        // hook 类型为无返回值闭包；截断失败在测试内部以 panic 暴露（unwrap/expect），
        // 绝不返回错误给 helper 映射成 FINGERPRINT_IO_FAILED。
        let hook = Box::new(move || {
            calls_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let f = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&path_hook)
                .expect("以写模式打开文件必须成功");
            f.set_len(0).expect("截断文件必须成功");
            truncated_hook.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        let err = fast_fingerprint_with_after_open_hook(&path, size, 0, hook).unwrap_err();
        assert_eq!(
            err.code().as_str(),
            "FINGERPRINT_IO_FAILED",
            "打开后截断必须经 hash_range 的真实 read EOF 返回 FINGERPRINT_IO_FAILED"
        );
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "hook 必须非零触发"
        );
        assert!(
            truncate_succeeded.load(std::sync::atomic::Ordering::SeqCst),
            "截断必须成功执行"
        );
        assert_eq!(
            fs::metadata(&path).unwrap().len(),
            0,
            "文件在 hook 后必须为 0 字节"
        );
    }
}
