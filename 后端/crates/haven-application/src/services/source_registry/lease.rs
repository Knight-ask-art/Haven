//! 自定义源凭据**外部 I/O** 的独占租约（跨进程写者仲裁）。
//!
//! 为什么需要它：外部系统凭据库与 SQLite 设置行不可能在同一个事务里。此前的实现只靠
//! 「暂存记录 + 清理队列」把中断的写入留给清理扫描，于是留下一个真实的无主窗口：
//!
//! 1. 写入者 W 的暂存记录被扫描按有效期或容量压力回收，转入清理队列并被删除；
//! 2. W 仍然活着，仍然会完成它那次**外部写入**（keystore 的 `set` 不参与任何事务）；
//! 3. W 在补偿删除之前崩溃/被取消。
//!
//! 结果是一个既没有来源记录指向、也不在任何队列里的槽位——没有任何清理所有者，而且
//! 没有任何时间量能证明它不会再被写一次。「足够长的 TTL」不是证明，只是把窗口变窄。
//!
//! 因此这里引入一个**跨进程可证明的**独占租约：所有会触碰自定义源凭据外部条目的操作
//! （写入、清除、来源移除与清理扫描）都必须在整个「暂存 → 外部写入 → 采纳/补偿」或
//! 「撤销 → 外部删除 → 出队」区间内持有一份租约。持租约期间不存在第二个合规写者，
//! 所以扫描看到的暂存记录只可能属于**已经不会再继续**的写入。
//!
//! 边界（不夸大）：
//! - 这是一个**外部 I/O 的所有权**机制，不是事务。它不替代 `SettingsUoW` 的条件写，
//!   也不让 keystore 与数据库变成原子；它只保证同一时刻只有一个合规的外部写者。
//! - 它只覆盖遵守本租约的路径。系统凭据库仍可被本进程之外的程序改写，那不在本契约内。
//! - **不递归获取**：公开的变更/清理入口各获取一次，私有 helper 只接收已经持有的守卫。
//!   递归获取会立刻在同一个写者身上自我竞争（非重入）。
//! - 受信任的 `CredentialStore` 实现**不得**在自己的 future 完成或取消之后继续后台写入
//!   （例如把 `set` 甩给 `spawn_blocking` 后立刻返回）。租约的覆盖范围止于本调用栈，
//!   任何在 future 结束之后仍然存活的副作用都会跑到租约之外。当前 Windows 实现在
//!   future 内同步完成系统调用，不派生任何后台任务。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use haven_common::{AppError, ErrorKind};

/// 已经取得租约的守卫。
///
/// 只有 `Drop` 会释放：没有「显式释放了但外部写入还在飞」的中间状态。守卫必须随调用栈
/// 一起穿过整个外部 I/O 区间，因此实现必须满足三条：
///
/// - `Send`：守卫本身要在 `async fn` 里跨 await 点存活；
/// - `Sync`：公开入口取得守卫之后，把它**按引用**交给私有 helper（`&dyn
///   SourceCredentialLeaseGuard`）。一个持有该引用的 future 只有在被引用对象是 `Sync`
///   时才是 `Send`，而 `Send` 的 future 正是 `tokio::spawn` 的前提。少了这一条，「写入
///   整个区间持租约」就只能写成不跨 await 的同步代码，等于放弃租约的覆盖范围；
/// - `Debug`：只用于诊断与测试断言（`Result::expect_err` / `unwrap_err` 要求成功值可
///   `Debug`）。实现**不得**把锁文件路径、引用或秘密写进 `Debug` 输出。
pub trait SourceCredentialLeaseGuard: Send + Sync + std::fmt::Debug {}

/// 外部凭据 I/O 的独占租约端口。
///
/// 实现必须：
/// - **非阻塞**：有人持有时立刻返回 `SOURCE_CREDENTIAL_BUSY`（可重试），不做无界等待或轮询；
/// - 返回的守卫在 `Drop` 时释放；
/// - 未注入实现时返回 `SOURCE_CREDENTIAL_LEASE_UNAVAILABLE`；锁文件暂时不可访问时返回可重试的
///   `SOURCE_CREDENTIAL_LEASE_IO_FAILED`；错误文案里不出现任何路径、引用或查询串。
///
/// 生产装配必须注入真实实现（文件锁）。未注入时所有凭据变更与清理**fail closed**：
/// 只读注册表仍然可用，但没有任何入口会去写外部凭据库。
#[async_trait]
pub trait SourceCredentialLease: Send + Sync {
    /// 尝试取得租约；被占用时立即返回可重试的 `SOURCE_CREDENTIAL_BUSY`。
    async fn acquire(&self) -> Result<Box<dyn SourceCredentialLeaseGuard>, AppError>;
}

/// 另一个合规写者正在持有租约：本次操作没有产生任何外部副作用，可以直接重试。
pub fn source_credential_busy() -> AppError {
    AppError::new(
        "SOURCE_CREDENTIAL_BUSY",
        ErrorKind::Conflict,
        "另一个来源凭据操作正在进行，请稍后重试",
        true,
    )
}

/// 组装面没有注入租约：凭据变更与清理无法证明独占性，因此 fail closed。
pub fn source_credential_lease_unavailable() -> AppError {
    AppError::new(
        "SOURCE_CREDENTIAL_LEASE_UNAVAILABLE",
        ErrorKind::Internal,
        "当前装配没有可用的来源凭据写者租约",
        false,
    )
}

/// 租约已正确装配，但锁文件遇到本机 I/O 错误：保留稳定分类并允许环境恢复后重试。
pub fn source_credential_lease_io_failed() -> AppError {
    AppError::new(
        "SOURCE_CREDENTIAL_LEASE_IO_FAILED",
        ErrorKind::Io,
        "无法访问来源凭据操作锁，请检查应用数据目录后重试",
        true,
    )
}

/// **进程内**租约：只在一个进程内保证独占。
///
/// 它存在的理由是「无盘装配」——内存数据库（测试、`Db::open_in_memory`）本来就不与任何
/// 其他进程共享事实，一把进程内的锁就是它正确的独占范围。绝不要用它给文件数据库做
/// 「跨进程「的声明：那是一个假的保证。文件数据库的装配必须使用
/// `haven_infrastructure::FileSourceCredentialLease`。
///
/// 它不会被任何 builder 自动启用：只有显式注入才会生效。
#[derive(Clone, Default)]
pub struct InMemorySourceCredentialLease {
    held: Arc<Mutex<bool>>,
}

impl InMemorySourceCredentialLease {
    pub fn new() -> Self {
        Self::default()
    }
}

struct InMemoryLeaseGuard {
    held: Arc<Mutex<bool>>,
}

impl SourceCredentialLeaseGuard for InMemoryLeaseGuard {}

impl std::fmt::Debug for InMemoryLeaseGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 只报身份：租约状态与任何外部事实都不属于可回显的诊断信息。
        f.debug_struct("InMemoryLeaseGuard").finish_non_exhaustive()
    }
}

impl Drop for InMemoryLeaseGuard {
    fn drop(&mut self) {
        *self.held.lock().unwrap_or_else(|error| error.into_inner()) = false;
    }
}

#[async_trait]
impl SourceCredentialLease for InMemorySourceCredentialLease {
    async fn acquire(&self) -> Result<Box<dyn SourceCredentialLeaseGuard>, AppError> {
        let mut held = self.held.lock().unwrap_or_else(|error| error.into_inner());
        if *held {
            return Err(source_credential_busy());
        }
        *held = true;
        Ok(Box::new(InMemoryLeaseGuard {
            held: self.held.clone(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_lease_is_exclusive_and_released_on_drop() {
        let lease = InMemorySourceCredentialLease::new();
        let first = lease.acquire().await.expect("首次获取必须成功");

        let error = lease
            .acquire()
            .await
            .expect_err("同一租约的第二次获取必须被拒绝");
        assert_eq!(error.code().as_str(), "SOURCE_CREDENTIAL_BUSY");
        assert!(error.retryable(), "占用是可重试状态，而不是永久失败");

        drop(first);
        let second = lease.acquire().await.expect("释放之后必须可以重新获取");
        drop(second);
    }

    #[tokio::test]
    async fn two_clones_share_one_lease() {
        let lease = InMemorySourceCredentialLease::new();
        let other = lease.clone();
        let held = lease.acquire().await.unwrap();
        assert_eq!(
            other.acquire().await.unwrap_err().code().as_str(),
            "SOURCE_CREDENTIAL_BUSY"
        );
        drop(held);
        assert!(other.acquire().await.is_ok());
    }
}
