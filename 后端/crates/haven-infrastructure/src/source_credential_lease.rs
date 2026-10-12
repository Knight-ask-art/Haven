//! 来源凭据外部 I/O 的**跨进程**独占租约（文件锁实现）。
//!
//! 端口与理由见 `haven_application::services::source_registry::SourceCredentialLease`。
//! 这里只提供真实实现：对一把**只由数据库目录决定**的专用锁文件做非阻塞排它锁。
//!
//! 为什么是文件锁而不是内存互斥量：需要被仲裁的是「同一台机器上两个都认为自己是唯一
//! 写者」的进程（例如用户开了两份应用，或一次崩溃残留的进程仍在收尾）。内存互斥量在
//! 这种场景下给出的是一个假的保证。
//!
//! 纪律：
//! - 锁文件路径**只**从 `Db::path()` 的父目录派生；调用方不能传任意路径进来，因此不存在
//!   「前端/配置指定锁文件」的入口。内存数据库（没有文件路径）明确失败，绝不退化成
//!   进程内的假锁——那会把「跨进程独占」悄悄降级成「本进程独占」。
//! - 父目录要**规范化**（`std::fs::canonicalize`）之后才拼锁文件名：相对路径、`.`/`..`
//!   与目录符号链接都只是同一个目录的别名，只按调用方传来的字面路径拼名字，会让同一台
//!   机器上的两个写者各自持有一把「不同」的锁文件，跨进程独占就退化成假保证。规范化
//!   失败按可重试的本机 I/O 错误处理（`SOURCE_CREDENTIAL_LEASE_IO_FAILED`），不带内存回退。
//! - 每次获取都**新开一个句柄**（`read+write+create`，`truncate(false)`），`Drop` 时解锁
//!   并关闭句柄；绝不 unlink / 重建这个文件（重建会让两个写者各自持有一把「新」锁）。
//! - 竞争立刻返回可重试的 `SOURCE_CREDENTIAL_BUSY`：不轮询、不等待、不重试。
//! - 所有错误文案都不含路径、引用或查询串。

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use fs2::FileExt;

use haven_application::services::source_registry::{
    SourceCredentialLease, SourceCredentialLeaseGuard, source_credential_busy,
    source_credential_lease_io_failed, source_credential_lease_unavailable,
};
use haven_common::AppError;

use crate::db::Db;

/// 专用锁文件名。它与 SQLite 文件同目录，但不与任何 SQLite 文件同名。
const LEASE_FILE_NAME: &str = "source-credential-lease.lock";

/// 基于数据库目录的跨进程租约。
pub struct FileSourceCredentialLease {
    path: PathBuf,
}

impl FileSourceCredentialLease {
    /// 从**已经打开的后端数据库**构造：路径不由调用方给出。
    ///
    /// 内存数据库（`Db::open_in_memory`）没有文件目录，返回
    /// `SOURCE_CREDENTIAL_LEASE_UNAVAILABLE`：那种装配要么显式注入进程内租约（测试 /
    /// 无盘装配），要么 fail closed，而不是拿一把假锁冒充跨进程独占。
    pub fn from_db(db: &Db) -> Result<Self, AppError> {
        Ok(Self {
            path: lock_file_path(db)?,
        })
    }

    /// 只读诊断：本租约仲裁的那把锁文件。
    ///
    /// 它是给装配自检与测试观测用的（「两个装配面确实在争同一把锁吗」），不构成任何
    /// 写入口：没有 `&mut self`，也没有从任意路径构造租约的构造函数。
    pub fn lock_file(&self) -> &Path {
        &self.path
    }

    fn open_handle(&self) -> Result<File, AppError> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.path)
            .map_err(|error| {
                // 文案不含路径：本地目录结构不属于用户可见的错误信息。
                source_credential_lease_io_failed().with_source(error)
            })
    }
}

impl std::fmt::Debug for FileSourceCredentialLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileSourceCredentialLease")
            .finish_non_exhaustive()
    }
}

/// 持有一个已加锁的文件句柄。`Drop` 解锁；进程退出时操作系统同样会释放。
struct FileLeaseGuard {
    file: File,
}

impl SourceCredentialLeaseGuard for FileLeaseGuard {}

impl std::fmt::Debug for FileLeaseGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 不打印内部的 `File`：它的 `Debug` 会带上锁文件路径，而本地路径不属于可回显的
        // 诊断信息（守卫的 `Debug` 契约要求不含路径、引用与秘密）。
        f.debug_struct("FileLeaseGuard").finish_non_exhaustive()
    }
}

impl Drop for FileLeaseGuard {
    fn drop(&mut self) {
        // 显式解锁 + 随后的关闭句柄，两条路径都会释放锁；重复解锁是幂等的。
        let _ = FileExt::unlock(&self.file);
    }
}

#[async_trait]
impl SourceCredentialLease for FileSourceCredentialLease {
    async fn acquire(&self) -> Result<Box<dyn SourceCredentialLeaseGuard>, AppError> {
        let file = self.open_handle()?;
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(Box::new(FileLeaseGuard { file })),
            Err(error) if is_contended(&error) => Err(source_credential_busy()),
            Err(error) => Err(source_credential_lease_io_failed().with_source(error)),
        }
    }
}

/// 竞争判定。
///
/// `WouldBlock` 是跨平台共通的表达，一律按竞争处理。裸错误码只**在它确实表示锁竞争**的
/// 平台上才认：`ERROR_LOCK_VIOLATION` = 33 是 Windows 专有的，POSIX 上 33 是 `EDOM`
/// ——在那里把 33 当竞争，会把一个真实的错误说成「稍后重试就好」。
fn is_contended(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        if error.raw_os_error() == Some(33) {
            return true;
        }
    }
    false
}

fn lock_file_path(db: &Db) -> Result<PathBuf, AppError> {
    let db_path = db
        .path()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(source_credential_lease_unavailable)?;
    let dir = db_path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    // 规范化父目录：`haven.db`、`./haven.db`、`sub/../haven.db` 与经过目录符号链接的路径
    // 都是同一个数据库目录的别名，必须收敛到同一把锁文件。规范化失败（目录已消失等）
    // 按可重试的本机 I/O 故障返回，不退化成「各自一把锁」的假独占。
    let canonical_dir = std::fs::canonicalize(dir)
        .map_err(|error| source_credential_lease_io_failed().with_source(error))?;
    Ok(canonical_dir.join(LEASE_FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_db(dir: &Path) -> Db {
        Db::open(&dir.join("haven.db")).expect("打开文件数据库")
    }

    #[tokio::test]
    async fn memory_database_has_no_cross_process_lease() {
        let db = Db::open_in_memory().unwrap();
        let error =
            FileSourceCredentialLease::from_db(&db).expect_err("内存库不得拿到一个跨进程租约");
        assert_eq!(error.code().as_str(), "SOURCE_CREDENTIAL_LEASE_UNAVAILABLE");
        assert!(!error.retryable(), "装配错误不可重试");
    }

    #[tokio::test]
    async fn lock_file_io_errors_are_reported_as_retryable_environment_failures() {
        let dir = tempfile::tempdir().unwrap();
        let lease = FileSourceCredentialLease::from_db(&file_db(dir.path())).unwrap();
        // 目录占用了锁文件名：下一次 open 会收到平台 I/O 错误，而不是锁竞争。
        std::fs::create_dir(lease.lock_file()).unwrap();

        let error = lease
            .acquire()
            .await
            .expect_err("目录不能作为可写的租约锁文件打开");
        assert_eq!(error.code().as_str(), "SOURCE_CREDENTIAL_LEASE_IO_FAILED");
        assert!(error.retryable(), "本机文件系统恢复后允许重试");
        assert!(
            !error.user_message().contains("source-credential-lease"),
            "用户可见文案不得回显锁文件路径"
        );
    }

    /// 两个**独立构造**的租约实例共用同一把锁文件：这正是跨进程场景的最小复现。
    #[tokio::test]
    async fn two_instances_on_the_same_database_contend_and_release() {
        let dir = tempfile::tempdir().unwrap();
        let db = file_db(dir.path());
        let first = FileSourceCredentialLease::from_db(&db).unwrap();
        let second = FileSourceCredentialLease::from_db(&db).unwrap();
        assert_eq!(first.lock_file(), second.lock_file());

        let held = first.acquire().await.expect("首次加锁必须成功");
        let error = second
            .acquire()
            .await
            .expect_err("同一把锁的第二个持有者必须立刻拿到 BUSY");
        assert_eq!(error.code().as_str(), "SOURCE_CREDENTIAL_BUSY");
        assert!(error.retryable());
        assert!(
            !error.user_message().contains("source-credential-lease"),
            "错误文案不得回显锁文件路径"
        );

        drop(held);
        let again = second
            .acquire()
            .await
            .expect("释放之后第二个实例必须能加锁");
        drop(again);
    }

    /// 同一个租约对象连续获取两次也必须竞争：每次获取都新开一个句柄，锁不是重入的。
    #[tokio::test]
    async fn acquiring_twice_from_the_same_instance_contends() {
        let dir = tempfile::tempdir().unwrap();
        let lease = FileSourceCredentialLease::from_db(&file_db(dir.path())).unwrap();
        let held = lease.acquire().await.unwrap();
        assert_eq!(
            lease.acquire().await.unwrap_err().code().as_str(),
            "SOURCE_CREDENTIAL_BUSY"
        );
        drop(held);
        assert!(lease.acquire().await.is_ok());
    }

    /// 锁文件被保留下来：不 unlink、不重建，避免两个写者各持一把「新」锁。
    #[tokio::test]
    async fn lock_file_is_stable_and_never_recreated_empty() {
        let dir = tempfile::tempdir().unwrap();
        let db = file_db(dir.path());
        let lease = FileSourceCredentialLease::from_db(&db).unwrap();
        let path = lease.lock_file().to_path_buf();
        {
            let _held = lease.acquire().await.unwrap();
        }
        assert!(path.exists(), "锁文件必须仍然存在");
        let _held = lease.acquire().await.unwrap();
        assert_eq!(lease.lock_file(), path);
    }

    /// 锁文件与数据库文件同目录（规范化之后的真实目录），且不与数据库文件同名。
    #[tokio::test]
    async fn lock_file_lives_next_to_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let lease = FileSourceCredentialLease::from_db(&file_db(dir.path())).unwrap();
        let canonical_dir = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(lease.lock_file().parent().unwrap(), canonical_dir);
        assert_eq!(
            lease.lock_file().file_name().unwrap().to_string_lossy(),
            LEASE_FILE_NAME
        );
    }

    /// 同一个数据库目录的**别名**（这里是含 `..` 的路径）必须收敛到同一把锁文件，并且
    /// 真的互相排斥：只按字面路径拼锁文件名，同一台机器上的两个写者就会各持一把「不同」
    /// 的锁，跨进程独占退化成假保证。
    #[tokio::test]
    async fn aliased_database_paths_share_one_lock_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("nested")).unwrap();
        let direct = FileSourceCredentialLease::from_db(&file_db(dir.path())).unwrap();
        let aliased_path = dir.path().join("nested").join("..").join("haven.db");
        let aliased =
            FileSourceCredentialLease::from_db(&Db::open(&aliased_path).unwrap()).unwrap();
        assert_eq!(
            direct.lock_file(),
            aliased.lock_file(),
            "同一个目录的不同写法必须指向同一把锁文件"
        );

        let held = direct.acquire().await.expect("首次加锁必须成功");
        assert_eq!(
            aliased.acquire().await.unwrap_err().code().as_str(),
            "SOURCE_CREDENTIAL_BUSY",
            "别名路径必须与真实路径争同一把锁"
        );
        drop(held);
        assert!(
            aliased.acquire().await.is_ok(),
            "释放之后别名路径必须能加锁"
        );
    }

    /// `WouldBlock` 是跨平台共通的竞争表达，任何平台上都按竞争处理。
    #[test]
    fn would_block_is_contention_on_every_platform() {
        let error = std::io::Error::new(std::io::ErrorKind::WouldBlock, "locked elsewhere");
        assert!(is_contended(&error));
    }

    /// 裸错误码 33 是 **Windows 专有**的 `ERROR_LOCK_VIOLATION`；POSIX 上 33 是 `EDOM`，
    /// 把它当成竞争就是把一个真实错误说成「稍后重试就好」。两个平台各钉一边。
    #[cfg(windows)]
    #[test]
    fn windows_raw_lock_violation_is_contention_and_other_codes_are_not() {
        assert!(
            is_contended(&std::io::Error::from_raw_os_error(33)),
            "Windows 33 = ERROR_LOCK_VIOLATION，必须按竞争处理"
        );
        assert!(
            !is_contended(&std::io::Error::from_raw_os_error(5)),
            "ERROR_ACCESS_DENIED 不是竞争"
        );
    }

    #[cfg(unix)]
    #[test]
    fn posix_error_33_is_not_mistaken_for_lock_contention() {
        assert!(
            !is_contended(&std::io::Error::from_raw_os_error(33)),
            "POSIX 33 = EDOM：不得被误判成锁竞争"
        );
    }
}
