//! 平台监听原语：Windows Named Pipe（含 DACL）与 Unix domain socket（含目录/文件模式）。
//!
//! 契约来源：`docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md` §4.4。
//!
//! 两条平台规则都在这里落地，**不伪造**：
//! - Windows：首实例用 **`CreateNamedPipeW` + `SECURITY_ATTRIBUTES`** 创建，DACL 只含
//!   当前用户 SID 的一个 ACE。后续实例传 null 安全属性——同一 pipe 的所有实例共享
//!   首实例的安全描述符（MSDN 语义），因此只需构造一次。
//! - Unix：先在每用户 runtime 根下以 `0700` 建目录，再取 **每用户活实例锁**
//!   （`flock(LOCK_EX | LOCK_NB)`，锁文件 `0600`），然后 `bind`，最后把 socket 置为
//!   `0600`。目录模式已经把"bind 到 chmod 之间的窗口"关掉——其他用户进不到这个目录。
//!   锁的作用见 [`platform`] 的 `Listener::bind` 与设计 §7.3：它把"**此刻**有没有活的
//!   Haven 实例"变成一次内核判定，因此崩溃（含 `SIGKILL`）留下的 socket 文件能被安全
//!   识别为残留而不是"另一个实例在跑"。
//!
//! 为什么不用"先建后 `SetSecurityInfo`"：`PIPE_ACCESS_DUPLEX` 创建的句柄**不含
//! `WRITE_DAC`**，`SetSecurityInfo` 会以 `ERROR_ACCESS_DENIED(5)` 失败。这个结论是实测
//! 出来的，不是推断。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;

use super::endpoint::BrokerEndpoint;
use super::error::BrokerError;
use super::protocol::{
    decode_frame_length, encode_frame, MAX_FRAME_BYTES, MAX_INFLIGHT_PER_CONNECTION,
};
use super::session::{BrokerAgentApi, BrokerSession, FrameOutcome, ProcessQuota};

/// 无在途请求时的连接空闲上限。
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// 握手（连接后首帧 `hello`）上限。
///
/// 没有它，一个连上却不发首帧的客户端会一直占着一个连接 permit：连接数上限被逐个耗光，
/// 而 Broker 侧看不出任何异常。超时按**连接失败**收尾——断连，不写任何回复。
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// 单次 Broker→client 写出的上限，覆盖 `write_all`、`flush` 与两处 `shutdown`。
///
/// 对端不读时（管道/套接字缓冲写满）写会永久阻塞，连接任务连同它的 permit 就再也回不来。
/// 超时同样按连接失败收尾：**不**把超时伪装成一条错误回复——写不出去正是问题本身，
/// 再写一条"我超时了"只会同样卡住。
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// 单个连接的读写缓冲。
#[cfg_attr(not(windows), allow(dead_code))]
const PIPE_BUFFER_BYTES: u32 = 64 * 1024;

/// 端点绑定是**进程级独占**的（这正是"每用户唯一端点"的设计）。
///
/// 因此同一测试进程里所有会真正 `bind` 的用例必须串行——否则它们会互相把对方挤成
/// `HAVEN_BROKER_ENDPOINT_BUSY`，测试变成随机失败。这不是产品缺陷，是测试需要遵守
/// 的真实约束；用一个显式互斥量把它写出来，而不是靠加 sleep 掩盖。
#[cfg(test)]
pub(crate) fn endpoint_test_guard() -> std::sync::MutexGuard<'static, ()> {
    static GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
    GUARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 绑定后的监听器。
pub struct BoundListener {
    endpoint: BrokerEndpoint,
    inner: platform::Listener,
}

impl std::fmt::Debug for BoundListener {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BoundListener")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl BoundListener {
    pub fn endpoint(&self) -> &BrokerEndpoint {
        &self.endpoint
    }
}

/// 在给定端点上开始监听。
///
/// 端点被占用时返回 `HAVEN_BROKER_ENDPOINT_BUSY`——**fail closed**：不接管、不删除、
/// 不改名、不换端点。
pub async fn bind(endpoint: &BrokerEndpoint) -> Result<BoundListener, BrokerError> {
    let inner = platform::Listener::bind(endpoint).await?;
    Ok(BoundListener {
        endpoint: endpoint.clone(),
        inner,
    })
}

/// 监听循环。`shutdown` 置真时立即停止接收新连接，并让在途连接尽快收尾。
///
/// `serving` 是**对外相位**，由平台实现在"确定不再 accept"的那一刻置 `false`。之后
/// `connection_tasks` 的 abort/drain（Unix 侧还要删 socket 文件）可能还要跑一会儿，这段
/// 时间里 join handle 仍未结束，但端点上已经没有人在 accept——[`super::AgentBrokerManager`]
/// 只看 `JoinHandle::is_finished` 会把这段窗口误投影成 `listening`。
pub async fn serve(
    listener: BoundListener,
    api: Arc<dyn BrokerAgentApi>,
    quota: Arc<ProcessQuota>,
    app_version: String,
    shutdown: watch::Receiver<bool>,
    serving: watch::Sender<bool>,
) {
    listener
        .inner
        .serve(api, quota, app_version, shutdown, serving)
        .await;
}

/// 读帧任务的单向输出。
///
/// 读任务只负责完整帧的 framing，不解析协议、不写响应。这样主连接任务可以在不取消
/// 一个正在读取半个长度前缀的 future 的前提下，同时等待 shutdown、请求完成与下一帧。
async fn read_frames<R>(mut reader: R, sender: mpsc::Sender<String>)
where
    R: AsyncRead + Unpin,
{
    loop {
        let mut prefix = [0u8; 4];
        if reader.read_exact(&mut prefix).await.is_err() {
            return;
        }
        let Some(len) = decode_frame_length(prefix) else {
            // 载荷上限违反后，字节流不可再解释；直接结束读任务，不发送错误帧。
            return;
        };
        let mut buffer = vec![0u8; len];
        if reader.read_exact(&mut buffer).await.is_err() {
            return;
        }
        let Ok(text) = String::from_utf8(buffer) else {
            // UTF-8 失败同样不尝试从不可信字节流中构造响应。
            return;
        };
        if sender.send(text).await.is_err() {
            return;
        }
    }
}

struct WorkerCompletion {
    id: i64,
    outcome: FrameOutcome,
}

/// 写出一个已经由 [`BrokerSession`] 构造好的响应。所有写入都在连接主任务中串行执行。
///
/// 每一次写出都套 `write_timeout`；`Err(())` 表示连接失败，调用方直接断开。
async fn write_outcome<W>(
    writer: &mut W,
    outcome: FrameOutcome,
    write_timeout: Duration,
) -> Result<bool, ()>
where
    W: AsyncWrite + Unpin,
{
    let (reply, close) = match outcome {
        FrameOutcome::Reply(text) => (Some(text), false),
        FrameOutcome::ReplyAndClose(text) => (Some(text), true),
        FrameOutcome::Close => (None, true),
    };
    if let Some(reply) = reply {
        let frame = encode_frame(&reply);
        tokio::time::timeout(write_timeout, async {
            writer.write_all(&frame).await?;
            writer.flush().await
        })
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
    }
    if close {
        tokio::time::timeout(write_timeout, writer.shutdown())
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;
    }
    Ok(close)
}

/// 断开连接前回收读任务、所有请求任务和 Session 中的在途登记。
async fn cleanup_connection(
    session: &Arc<BrokerSession>,
    reader_task: &mut tokio::task::JoinHandle<()>,
    tasks: &mut JoinSet<WorkerCompletion>,
    task_ids: &mut HashMap<i64, tokio::task::AbortHandle>,
) {
    reader_task.abort();
    let _ = reader_task.await;

    tasks.abort_all();
    while tasks.join_next().await.is_some() {}

    for id in task_ids.drain().map(|(id, _)| id) {
        session.release(id).await;
    }
}

/// 连接驱动的三个时间参数。
///
/// 打成结构体而不是位置参数：三者都是 `Duration`，错位传参不会有编译错误，只会在生产
/// 上把 5 秒的握手窗口换成 30 分钟。测试用 `..Default::default()` 只覆盖关心的一项。
#[derive(Clone, Copy)]
struct ConnectionTimeouts {
    handshake: Duration,
    write: Duration,
    idle: Duration,
}

impl Default for ConnectionTimeouts {
    fn default() -> Self {
        Self {
            handshake: HANDSHAKE_TIMEOUT,
            write: WRITE_TIMEOUT,
            idle: IDLE_TIMEOUT,
        }
    }
}

/// 单连接处理：独立读任务 + 并发请求任务 + 单一写出者。
///
/// 读写都是**有界**的：长度前缀先过 [`decode_frame_length`]，超过帧上限直接断连（不回复）。
/// `BrokerSession::prepare` 只做快速判定，`BrokerSession::execute` 在独立任务中执行，因而
/// 同一连接可以真实保持最多 [`MAX_INFLIGHT_PER_CONNECTION`] 条在途请求；取消、20 秒超时、
/// 客户端断线和 Broker shutdown 都会回收对应任务与 Session 登记。
///
/// 时间上界有三处，缺一不可：握手（[`HANDSHAKE_TIMEOUT`]）、每次写出（[`WRITE_TIMEOUT`]）、
/// 空闲（[`IDLE_TIMEOUT`]）。任何一处缺失都会让一条不配合的连接永久占住 permit。
pub(crate) async fn drive_connection<S>(
    stream: S,
    api: Arc<dyn BrokerAgentApi>,
    quota: Arc<ProcessQuota>,
    app_version: String,
    shutdown: watch::Receiver<bool>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    drive_connection_with_timeouts(
        stream,
        api,
        quota,
        app_version,
        shutdown,
        ConnectionTimeouts::default(),
    )
    .await;
}

/// [`drive_connection`] 的可注入版本：超时全部由 `timeouts` 给出，测试才能把窗口压到毫秒级
/// 而不必真等 5 秒。
async fn drive_connection_with_timeouts<S>(
    stream: S,
    api: Arc<dyn BrokerAgentApi>,
    quota: Arc<ProcessQuota>,
    app_version: String,
    mut shutdown: watch::Receiver<bool>,
    timeouts: ConnectionTimeouts,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let session = Arc::new(BrokerSession::new(api, quota, app_version));
    let (reader, mut writer) = tokio::io::split(stream);
    let (frame_sender, mut frame_receiver) = mpsc::channel(MAX_INFLIGHT_PER_CONNECTION + 1);
    let mut reader_task = tokio::spawn(read_frames(reader, frame_sender));

    // 握手必须先完成；之后才进入并发分派循环。首帧有固定上限：连上却不发 `hello`
    // 的客户端不能一直占着这个连接。超时与"读任务先结束"一样按连接失败收尾，直接
    // 断开——握手没完成，没有合法的帧 id 可以回。
    let handshake = tokio::select! {
        biased;
        _ = shutdown.changed() => {
            reader_task.abort();
            let _ = reader_task.await;
            return;
        }
        frame = tokio::time::timeout(timeouts.handshake, frame_receiver.recv()) => frame,
    };
    let Ok(Some(first_frame)) = handshake else {
        reader_task.abort();
        let _ = reader_task.await;
        return;
    };

    match session.prepare(&first_frame).await {
        super::session::Prepared::Immediate(outcome) => {
            match write_outcome(&mut writer, outcome, timeouts.write).await {
                Ok(true) | Err(()) => {
                    reader_task.abort();
                    let _ = reader_task.await;
                    return;
                }
                Ok(false) => {}
            }
        }
        // hello 当前不会产生 Dispatch。若未来协议演进改变这一点，宁可释放并关闭，
        // 也不在尚未握手的连接上启动 Application 调用。
        super::session::Prepared::Dispatch(dispatch) => {
            session.release(dispatch.id).await;
            reader_task.abort();
            let _ = reader_task.await;
            return;
        }
    }

    let mut tasks = JoinSet::<WorkerCompletion>::new();
    let mut task_ids = HashMap::new();
    let mut last_activity = tokio::time::Instant::now();

    loop {
        let idle_deadline = last_activity + timeouts.idle;
        tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            // `join_next` **排在 `frame_receiver.recv()` 之前**：已完成请求的响应必须优先
            // 写出。反过来的话，一条持续灌帧的连接能让 recv 永远就绪，join 分支被无限期
            // 饿死——请求早就执行完了，客户端却迟迟拿不到结果。帧不会因此丢失：它们留在
            // 有界通道里，下一轮照样被读到；完成数也不超过在途上限，所以不会反过来饿死收帧。
            joined = tasks.join_next(), if !tasks.is_empty() => {
                match joined {
                    Some(Ok(completion)) => {
                        task_ids.remove(&completion.id);
                        session.release(completion.id).await;
                        last_activity = tokio::time::Instant::now();
                        let outcome = completion.outcome;
                        match write_outcome(&mut writer, outcome, timeouts.write).await {
                            Ok(true) | Err(()) => break,
                            Ok(false) => {}
                        }
                    }
                    // 一个请求任务 panic 时不再继续接受同一连接上的输入；下面的统一清理
                    // 会 abort/release 其余请求，避免留下不可见的在途登记。
                    Some(Err(_)) => break,
                    None => {}
                }
            }
            frame = frame_receiver.recv() => {
                let Some(frame) = frame else { break; };
                last_activity = tokio::time::Instant::now();
                match session.prepare(&frame).await {
                    super::session::Prepared::Immediate(outcome) => {
                        match write_outcome(&mut writer, outcome, timeouts.write).await {
                            Ok(true) | Err(()) => break,
                            Ok(false) => {}
                        }
                    }
                    super::session::Prepared::Dispatch(dispatch) => {
                        let id = dispatch.id;
                        let cancel = session.cancel_receiver(id).await;
                        let session_for_task = session.clone();
                        let abort = tasks.spawn(async move {
                            WorkerCompletion {
                                id,
                                outcome: session_for_task.execute(dispatch, cancel).await,
                            }
                        });
                        task_ids.insert(id, abort);
                    }
                }
            }
            _ = tokio::time::sleep_until(idle_deadline), if task_ids.is_empty() => break,
        }
    }

    cleanup_connection(&session, &mut reader_task, &mut tasks, &mut task_ids).await;
    // 收尾的 shutdown 同样有上限：对端不再读取时，连接任务也必须能结束并交回 permit。
    let _ = tokio::time::timeout(timeouts.write, writer.shutdown()).await;
}

/// 编译期断言：驱动循环依赖帧上限为正。
const _: () = assert!(MAX_FRAME_BYTES > 0);

#[cfg(unix)]
mod platform {
    use super::*;
    use std::ffi::OsStr;
    use std::os::unix::fs::{
        DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt,
    };
    use std::os::unix::io::AsRawFd;
    use std::path::{Path, PathBuf};

    /// 运行目录与端点/锁文件的权限：只有属主可进入、可读写。
    const DIR_MODE: u32 = 0o700;
    const FILE_MODE: u32 = 0o600;

    /// 端点 liveness probe 的上限。
    ///
    /// AF_UNIX 的 `connect` 要么立刻成功、要么立刻失败；唯一可能等待的情形是**对端 backlog
    /// 已满**——那本身就已经说明有人在 accept。给一个固定上限是为了让"探测结果不确定"成为
    /// 一个有界的事实，而不是让 `enable` 无限期挂在 `bind` 里。
    const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

    /// 在一段**同步**窗口内收紧 umask，并在所有返回路径（含提前 `return`）恢复。
    ///
    /// 目录/socket 随后仍会显式 chmod 到 0700/0600；umask 的作用是把 bind 前的创建窗口
    /// 也纳入同一安全边界。
    ///
    /// **不得跨 `await` 持有**：umask 是进程级状态，而任务可以在任意 `await` 点被挂起，
    /// 于是同一进程里其它任务的创建操作会意外落到 077 下。这个约束只能避免 Tokio 任务在
    /// 等待期间泄漏；多线程 runtime 的其它 OS 线程仍可能在同步窗口内运行，所以窗口必须
    /// 尽量短，且这里选择的 077 只会让其它文件更严格而不会放宽权限。调用方必须把 guard
    /// 建在只含同步调用的作用域里（见 [`Listener::bind`]），需要异步的步骤一律放在 guard
    /// 之外。
    struct RestrictiveUmask(libc::mode_t);

    impl RestrictiveUmask {
        fn new() -> Self {
            // SAFETY：umask 是 POSIX 进程级原语；旧值由 guard 保存并在 Drop 中恢复。
            Self(unsafe { libc::umask(0o077) })
        }
    }

    impl Drop for RestrictiveUmask {
        fn drop(&mut self) {
            // SAFETY：恢复 bind 前保存的进程 umask。
            unsafe { libc::umask(self.0) };
        }
    }

    /// 端点不可用的统一错误（不含路径、不含 errno）。
    fn endpoint_unavailable(message: &str) -> BrokerError {
        BrokerError::new("HAVEN_BROKER_ENDPOINT_UNAVAILABLE", message, false)
    }

    /// 建出运行目录并收紧到 `0700`。
    ///
    /// 必须在创建锁文件与 socket **之前**完成：目录不可进入之后，锁文件与 socket 的创建
    /// 窗口才落在同一个安全边界内（其它用户进不来）。
    fn ensure_runtime_dir(parent: &Path) -> Result<(), BrokerError> {
        // `create_dir_all` 会跟随中间路径上的符号链接。先做一次受 Haven 管理的路径后缀
        // 检查，再在创建后复核；HOME 或 XDG_RUNTIME_DIR 本身可能是系统提供的 symlink，
        // 不应因为用户环境的合法路径别名而被拒绝。真正使用最终目录时还会用
        // `O_DIRECTORY | O_NOFOLLOW` 重新打开它，避免最后一段在检查后被替换成链接。
        reject_managed_symlink_components(parent)?;

        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(DIR_MODE);
        builder
            .create(parent)
            .map_err(|_| endpoint_unavailable("无法创建 Haven 运行目录。"))?;

        reject_managed_symlink_components(parent)?;

        // 不用按路径 set_permissions：最终目录必须通过 O_NOFOLLOW 打开，并且必须是当前
        // 用户拥有的普通目录。这样即使 `parent` 本身曾经是 symlink，也不会把目标目录
        // chmod 成 0700 或把 socket 建到意外位置。
        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(parent)
            .map_err(|_| endpoint_unavailable("无法打开 Haven 运行目录。"))?;
        let metadata = directory
            .metadata()
            .map_err(|_| endpoint_unavailable("无法检查 Haven 运行目录。"))?;
        // SAFETY：`geteuid` 无参数、无副作用。
        let uid = unsafe { libc::geteuid() };
        if !metadata.file_type().is_dir() || metadata.uid() != uid {
            return Err(endpoint_unavailable(
                "Haven 运行目录不是当前用户拥有的目录。",
            ));
        }
        directory
            .set_permissions(std::fs::Permissions::from_mode(DIR_MODE))
            .map_err(|_| endpoint_unavailable("无法收紧 Haven 运行目录权限。"))
    }

    /// 检查 Haven 自己管理的目录后缀都是真实目录，而不是 symlink 或其它 inode。
    ///
    /// 这不是对同用户恶意进程的完整竞态防护（Unix 路径解析本身需要逐段 `openat` 才能
    /// 消除那类竞态），但它拒绝了稳定存在的 Haven 管理目录重定向；最终目录的
    /// `O_NOFOLLOW` 打开又把最敏感的最后一段钉在了真实目录上。端点本来就不把同用户进程
    /// 当作可信方，这里的目标是 fail closed，避免 Haven 主动 chmod 或写入一个明显无关的
    /// 目标目录。
    fn reject_managed_symlink_components(path: &Path) -> Result<(), BrokerError> {
        let mut managed = vec![path];
        // 回退路径是 `$HOME/.haven/run`：HOME 可以是系统合法的 symlink，但 `.haven` 与
        // `run` 是 Haven 自己创建和管理的目录，不能被替换成重定向。
        let fallback_segments = super::super::endpoint::FALLBACK_DIR_SEGMENTS;
        if path.file_name() == Some(OsStr::new(fallback_segments[1]))
            && path.parent().and_then(Path::file_name) == Some(OsStr::new(fallback_segments[0]))
        {
            if let Some(haven_dir) = path.parent() {
                managed.push(haven_dir);
            }
        }

        for current in managed {
            match std::fs::symlink_metadata(current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(endpoint_unavailable("Haven 运行目录路径不得包含符号链接。"));
                }
                Ok(metadata) if !metadata.file_type().is_dir() => {
                    return Err(endpoint_unavailable("Haven 运行目录路径包含非目录项。"));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(endpoint_unavailable("无法检查 Haven 运行目录路径。")),
            }
        }
        Ok(())
    }

    /// 取**每用户活实例排他锁**。
    ///
    /// 锁文件本身没有内容意义，值全在"这个打开文件描述上有没有 `flock`"。锁由内核随进程
    /// 消亡（含 `SIGKILL`）释放，因此它回答的是"**此刻**有没有活的 Haven 实例"；"锁文件
    /// 存不存在"只能回答"曾经有没有实例"，那正是崩溃后再也开不起来的原因。
    fn acquire_instance_lock(lock_path: &Path) -> Result<std::fs::File, BrokerError> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(FILE_MODE)
            // 不跟随最后一段的符号链接：锁必须下在我们自己在这个目录里创建的文件上。
            .custom_flags(libc::O_NOFOLLOW)
            .open(lock_path)
            .map_err(|_| endpoint_unavailable("无法锁定 Haven 运行端点。"))?;

        // `O_NOFOLLOW` 之后仍然核一次最终拿到的对象：只接受"当前用户拥有的普通文件"。
        // 类型或属主不对说明这个路径被别的东西占了，不去锁它。
        let metadata = file
            .metadata()
            .map_err(|_| endpoint_unavailable("无法锁定 Haven 运行端点。"))?;
        // SAFETY：`geteuid` 无参数、无副作用。
        let uid = unsafe { libc::geteuid() };
        if !metadata.file_type().is_file() || metadata.uid() != uid {
            return Err(endpoint_unavailable("无法锁定 Haven 运行端点。"));
        }
        // umask 之外再钉一次：锁文件同样只给属主。
        file.set_permissions(std::fs::Permissions::from_mode(FILE_MODE))
            .map_err(|_| endpoint_unavailable("无法锁定 Haven 运行端点。"))?;

        lock_exclusive(&file)?;
        Ok(file)
    }

    /// `flock(LOCK_EX | LOCK_NB)`：**非阻塞**取排他锁，三种结果对应三个不同的事实。
    ///
    /// - 拿到锁 → 这个用户下此刻没有别的 Haven 在提供 Broker；
    /// - `EWOULDBLOCK` → 有，而且它是**活**的 → `HAVEN_BROKER_ENDPOINT_BUSY`；
    /// - 其它 errno（描述符不可锁、锁表耗尽…）→ 说不清楚，同样 fail closed，但报的是
    ///   "不可用"而不是"另一个实例在跑"：把未知失败说成已知事实是另一种不诚实。
    fn lock_exclusive(file: &std::fs::File) -> Result<(), BrokerError> {
        let fd = file.as_raw_fd();
        loop {
            // SAFETY：`fd` 由 `file` 持有且在本调用期间有效；`flock` 只改这个打开文件描述
            // 上的锁状态，既不读也不写文件内容。
            if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(());
            }
            // SAFETY：紧接失败的调用读取线程局部 errno。
            match std::io::Error::last_os_error().raw_os_error() {
                Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN => {
                    return Err(BrokerError::endpoint_busy());
                }
                // 非阻塞调用理论上不该被信号打断，但重试的代价是零。
                Some(libc::EINTR) => continue,
                _ => return Err(endpoint_unavailable("无法锁定 Haven 运行端点。")),
            }
        }
    }

    /// 端点 liveness probe：**只有** connect 明确回答"这个地址上没有 listener"才为真。
    ///
    /// - 连上了 → 有人在 accept：那是别人的**活**端点，不是可以清理的残留；
    /// - 超时 / 权限不足 / 认不出的 errno → 结果不确定。
    ///
    /// 两种情况都返回假，调用方保持 `HAVEN_BROKER_ENDPOINT_BUSY` 且**不删除**任何东西：
    /// fail closed 的方向永远是"不接管"。
    async fn stale_socket_is_recoverable(socket_path: &Path) -> bool {
        match tokio::time::timeout(PROBE_TIMEOUT, tokio::net::UnixStream::connect(socket_path))
            .await
        {
            Ok(Ok(_live)) => false,
            Ok(Err(error)) => matches!(
                error.kind(),
                // "有文件、没人听"——这是"陈旧 socket"唯一的权威定义。
                std::io::ErrorKind::ConnectionRefused
                    // 探测期间文件已经不在了：没有东西要删，直接去重试 bind。
                    | std::io::ErrorKind::NotFound
            ),
            Err(_elapsed) => false,
        }
    }

    /// 清掉**我们自己的**陈旧端点文件。
    ///
    /// 两道约束缺一不可：
    /// - 只删调用方传来的这一个固定端点路径——不扫描目录、不动锁文件、不回退到别的路径；
    /// - 只有 `lstat` 确认它**确实是 socket 文件**才删。常规文件、目录、符号链接一律原样
    ///   保留并 fail closed：那说明这个路径被别的东西占了，不是我们崩溃留下的残留。刻意用
    ///   `symlink_metadata`：绝不跟随符号链接去删它的**目标**。
    fn remove_stale_socket(socket_path: &Path) -> Result<(), BrokerError> {
        match std::fs::symlink_metadata(socket_path) {
            Ok(metadata) if metadata.file_type().is_socket() => std::fs::remove_file(socket_path)
                .map_err(|_| endpoint_unavailable("无法移除陈旧 Haven 端点。")),
            // 已经不在了：没有东西要删。
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            // 不是 socket 文件，或者连看都看不到：不删，fail closed。这里不能报
            // ENDPOINT_BUSY，因为普通文件/目录并不证明另一个 Haven 实例正在监听。
            _ => Err(endpoint_unavailable("本地端点路径已被非 socket 对象占用。")),
        }
    }

    /// 在 liveness probe 前先钉住端点的 inode 类型。
    ///
    /// `connect` 对普通文件、目录或符号链接的错误码并不构成稳定的类型判断；先做一次
    /// `lstat`，让非 socket 占用在任何 probe 结果下都归为 `ENDPOINT_UNAVAILABLE`。路径在
    /// probe 后还会由 `remove_stale_socket` 再检查一次，保留 fail closed 的二次闸门。
    fn validate_stale_socket_candidate(socket_path: &Path) -> Result<(), BrokerError> {
        match std::fs::symlink_metadata(socket_path) {
            Ok(metadata) if metadata.file_type().is_socket() => Ok(()),
            // 端点可能在 bind 报 EADDRINUSE 后已被其原持有者收尾移除；让 probe 的 ENOENT
            // 决定是否进行一次无害的重绑。
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Ok(_) => Err(endpoint_unavailable("本地端点路径已被非 socket 对象占用。")),
            Err(_) => Err(endpoint_unavailable("无法检查本地端点路径。")),
        }
    }

    pub(super) struct Listener {
        listener: tokio::net::UnixListener,
        socket_path: PathBuf,
        /// 活实例排他锁的载体。
        ///
        /// **有意持有到 `serve` 的收尾结束**（含删除 socket 文件）：只要它还活着，同用户的
        /// 第二个 Haven 就拿不到锁，因而不会把我们"正在收尾"的窗口误判成"端点可以接管"。
        /// 锁文件本身不删——它只是一个空的载体，留着不会让任何人误判有人在听。
        _lock: std::fs::File,
    }

    impl Listener {
        /// 绑定 Unix 端点：**先取活实例锁，再碰端点**。
        ///
        /// 顺序不能反。锁是"这个用户下此刻有没有别的 Haven 在提供 Broker"的唯一判据：
        /// - 拿不到锁 ⇒ 另一个**活**实例存在 ⇒ `HAVEN_BROKER_ENDPOINT_BUSY`，一个文件都
        ///   不动；
        /// - 拿到锁 ⇒ 不可能有活的 Haven ⇒ 端点路径上若还留着 socket 文件，它只可能是上一
        ///   个 Haven 崩溃 / 被 `SIGKILL` 留下的（内核已经替我们释放了它的锁）。
        ///
        /// 没有这一层，"陈旧 socket 文件"与"另一个实例的活端点"在 `bind` 看来完全一样
        /// （都是 `EADDRINUSE`），只能一律 fail closed——那正是崩溃之后 Haven 再也开不起来
        /// 的原因。反过来，只有锁也不够：锁保证的是"没有别的 Haven"，不保证这个路径上没有
        /// 别的程序，所以删除之前仍然要 probe 一次，而且只重试一次。
        ///
        /// umask 按**同步窗口**分两段收紧，中间的 liveness probe 在 guard 之外完成：umask 是
        /// 进程级的，跨 `await` 持有就会泄漏给同进程的其它任务。
        pub(super) async fn bind(endpoint: &BrokerEndpoint) -> Result<Self, BrokerError> {
            let BrokerEndpoint::UnixSocket(socket_path) = endpoint else {
                return Err(BrokerError::internal());
            };
            let parent = socket_path
                .parent()
                .ok_or_else(|| BrokerError::invalid_argument("端点缺少父目录。"))?;
            let lock_path = super::super::endpoint::unix_lock_file_at(socket_path)
                .ok_or_else(|| BrokerError::invalid_argument("端点缺少父目录。"))?;

            // 同步窗口一：收紧 umask，完成目录、锁文件与初次 bind。
            //
            // guard 只活在这个块里——块内没有任何 `await`，因此它不可能被挂起着带到别的任务
            // （含提前 `return` 的失败路径：离开块时 guard 先于一切恢复 umask）。锁本身是
            // `File`，不依赖 guard，从这里开始由 `lock` 持有到 probe / 重试 / 收尾结束。
            let lock = {
                let _umask = RestrictiveUmask::new();

                // 目录先收紧到 0700，再创建锁文件与 socket。
                ensure_runtime_dir(parent)?;

                let lock = acquire_instance_lock(&lock_path)?;

                // 初次 bind 也不能把最终一段的符号链接交给内核决定是否跟随。不存在的
                // 路径与真实 socket 会继续进入各自的正常分支；普通文件、目录和 symlink
                // 在触碰 bind 之前就 fail closed。
                validate_stale_socket_candidate(socket_path)?;

                match tokio::net::UnixListener::bind(socket_path) {
                    Ok(listener) => return Self::finish(listener, socket_path, lock),
                    Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => lock,
                    Err(_) => return Err(endpoint_unavailable("无法在本地端点监听。")),
                }
            };

            validate_stale_socket_candidate(socket_path)?;

            // guard 已经释放：probe 是真异步的，必须在这里做。
            //
            // 我们持有排他锁 ⇒ 没有活的 Haven 实例在听。端点却 bind 不上：要么是崩溃残留，
            // 要么有别的东西占着这个路径。删不删由 probe 说了算。
            if !stale_socket_is_recoverable(socket_path).await {
                // probe 的结果不稳定时仍重新核对类型：即使 connect 返回了超时或权限错误，
                // 普通文件/目录/符号链接也不能被报成"另一个 Haven 实例正在监听"。
                validate_stale_socket_candidate(socket_path)?;
                return Err(BrokerError::endpoint_busy());
            }

            // 同步窗口二：删除残留与重试 bind 同样在收紧的 umask 下进行。用块包住 guard，
            // 让未来插入 await 时在结构上不会把进程级 umask 带出这个同步窗口。
            {
                let _umask = RestrictiveUmask::new();
                remove_stale_socket(socket_path)?;
                // **只重试一次**：第二次仍然被占，说明有一个不受这把锁约束的监听者。
                // 再删下去就不是"清理自己的残留"，而是"接管别人的端点"了。
                match tokio::net::UnixListener::bind(socket_path) {
                    Ok(listener) => Self::finish(listener, socket_path, lock),
                    Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                        Err(BrokerError::endpoint_busy())
                    }
                    Err(_) => Err(endpoint_unavailable("无法在本地端点监听。")),
                }
            }
        }

        /// 收紧 socket 文件权限并组装监听器；锁随后由结构体一直持有到收尾结束。
        fn finish(
            listener: tokio::net::UnixListener,
            socket_path: &Path,
            lock: std::fs::File,
        ) -> Result<Self, BrokerError> {
            if std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(FILE_MODE))
                .is_err()
            {
                let _ = std::fs::remove_file(socket_path);
                return Err(endpoint_unavailable("无法收紧 Haven 本地端点权限。"));
            }
            Ok(Self {
                listener,
                socket_path: socket_path.to_path_buf(),
                _lock: lock,
            })
        }

        pub(super) async fn serve(
            self,
            api: Arc<dyn BrokerAgentApi>,
            quota: Arc<ProcessQuota>,
            app_version: String,
            mut shutdown: watch::Receiver<bool>,
            serving: watch::Sender<bool>,
        ) {
            let connections = Arc::new(tokio::sync::Semaphore::new(
                super::super::protocol::MAX_CONNECTIONS,
            ));
            let mut connection_tasks = JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.changed() => break,
                    joined = connection_tasks.join_next(), if !connection_tasks.is_empty() => {
                        let _ = joined;
                    }
                    result = self.listener.accept() => {
                        let Ok((stream, _addr)) = result else {
                            break;
                        };
                        let Ok(permit) = connections.clone().try_acquire_owned() else {
                            // 连接数超限：直接拒绝新连接（不排队），符合协议 §5.1。
                            drop(stream);
                            continue;
                        };
                        let api = api.clone();
                        let quota = quota.clone();
                        let app_version = app_version.clone();
                        let shutdown = shutdown.clone();
                        connection_tasks.spawn(async move {
                            let _permit = permit;
                            drive_connection(stream, api, quota, app_version, shutdown).await;
                        });
                    }
                }
            }
            // 到这里已经退出 accept 循环（shutdown 广播、accept 出错或监听器失效），不会再
            // 接收新连接。**先宣告相位再收尾**：下面的 abort/drain 可能持续一段时间，这段
            // 时间里 join handle 仍未结束，管理端若只看它就会把"正在断开连接"报成在监听。
            // 用 `send_replace` 而不是 `send`：相位是既成事实，不取决于当下有没有接收端。
            let _ = serving.send_replace(false);
            connection_tasks.abort_all();
            while connection_tasks.join_next().await.is_some() {}
            // 关闭时移除 socket 文件（只可能是我们自己创建的那个）。
            let _ = std::fs::remove_file(&self.socket_path);
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use tokio::net::windows::named_pipe::NamedPipeServer;

    /// 监听器直接持有首个 pipe 实例。
    ///
    /// 曾经想过用 thread-local 在 `bind`/`serve` 之间传递句柄——那是错的：两者由
    /// `tokio::spawn` 调度，可能落在不同 worker 线程上，thread-local 会读到空值并让监听
    /// 静默失效。`NamedPipeServer` 本身是 `Send`，直接放进结构体即可。
    pub(super) struct Listener {
        name: String,
        first: NamedPipeServer,
    }

    impl Listener {
        /// 测试用：读回首实例句柄以断言其 DACL。
        #[cfg(test)]
        pub(super) fn first_instance(&self) -> &NamedPipeServer {
            &self.first
        }
    }

    impl Listener {
        pub(super) async fn bind(endpoint: &BrokerEndpoint) -> Result<Self, BrokerError> {
            let BrokerEndpoint::NamedPipe(name) = endpoint else {
                return Err(BrokerError::internal());
            };
            let sid = super::super::endpoint::current_user_sid_string()?;
            let first = create_instance(name, true, Some(&sid))?;
            Ok(Self {
                name: name.clone(),
                first,
            })
        }

        pub(super) async fn serve(
            self,
            api: Arc<dyn BrokerAgentApi>,
            quota: Arc<ProcessQuota>,
            app_version: String,
            mut shutdown: watch::Receiver<bool>,
            serving: watch::Sender<bool>,
        ) {
            let Self { name, first } = self;
            let mut server = first;
            let connections = Arc::new(tokio::sync::Semaphore::new(
                super::super::protocol::MAX_CONNECTIONS,
            ));
            let mut connection_tasks = JoinSet::new();

            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.changed() => break,
                    joined = connection_tasks.join_next(), if !connection_tasks.is_empty() => {
                        let _ = joined;
                    }
                    connected = server.connect() => {
                        if connected.is_err() {
                            break;
                        }
                        // 先补一个等待实例，再处理已连接的那个，避免漏掉紧随其后的连接。
                        let Ok(next) = create_instance(&name, false, None) else {
                            break;
                        };
                        let current = std::mem::replace(&mut server, next);

                        let Ok(permit) = connections.clone().try_acquire_owned() else {
                            drop(current);
                            continue;
                        };
                        let api = api.clone();
                        let quota = quota.clone();
                        let app_version = app_version.clone();
                        let shutdown = shutdown.clone();
                        connection_tasks.spawn(async move {
                            let _permit = permit;
                            drive_connection(current, api, quota, app_version, shutdown).await;
                        });
                    }
                }
            }
            // 到这里已经退出 accept 循环（shutdown 广播、`connect` 出错或补等待实例失败），
            // 不会再接收新连接。**先宣告相位再收尾**：下面的 abort/drain 可能持续一段时间，
            // 这段时间里 join handle 仍未结束，管理端若只看它就会把"正在断开连接"报成在监听。
            // 用 `send_replace` 而不是 `send`：相位是既成事实，不取决于当下有没有接收端。
            let _ = serving.send_replace(false);
            connection_tasks.abort_all();
            while connection_tasks.join_next().await.is_some() {}
        }
    }

    /// 创建一个 pipe 实例。
    ///
    /// `first = true` 时同时加两样东西：
    /// - `SECURITY_ATTRIBUTES`：只含当前用户 SID 的 DACL（后续实例共享它，传 `null`）；
    /// - **`FILE_FLAG_FIRST_PIPE_INSTANCE`**：这是"每用户唯一端点"的关键。没有它，
    ///   `PIPE_UNLIMITED_INSTANCES` 会让**第二个 Haven 实例**也能用同一个 pipe 名建实例，
    ///   于是两个 Haven 同时对外提供 Broker——而要恰好就是这种情况。带上这个标志后，
    ///   已存在同名 pipe 时 `CreateNamedPipeW` 以 `ERROR_ACCESS_DENIED` 失败，
    ///   被归类为 `HAVEN_BROKER_ENDPOINT_BUSY`（fail closed，不接管）。这条归类**只对首实例
    ///   成立**：后续实例不带该标志，其 `ERROR_ACCESS_DENIED` 归为
    ///   `HAVEN_BROKER_ENDPOINT_UNAVAILABLE`，见 [`classify_create_error`]。
    ///
    /// 句柄以 `FILE_FLAG_OVERLAPPED` 创建后交给 tokio，由它负责异步 `ConnectNamedPipe`。
    fn create_instance(
        name: &str,
        first: bool,
        sid: Option<&str>,
    ) -> Result<NamedPipeServer, BrokerError> {
        use windows_sys::Win32::Foundation::{GetLastError, INVALID_HANDLE_VALUE};
        // `PIPE_ACCESS_DUPLEX` 在 windows-sys 里归在 Storage::FileSystem（它是 winbase.h 的
        // FILE_* 家族），其余 PIPE_* 模式常量在 System::Pipes。
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX,
        };
        use windows_sys::Win32::System::Pipes::{
            CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
            PIPE_WAIT,
        };

        let wide: Vec<u16> = OsStr::new(name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        // 安全描述符与其 ACL 必须在调用期间保持存活。
        let security = match sid {
            Some(sid) => Some(build_security_attributes(sid)?),
            None => None,
        };
        let sa_ptr = security
            .as_ref()
            .map(|value| &value.attributes as *const _)
            .unwrap_or(std::ptr::null());

        // SAFETY：`wide` 与 `security` 在本调用期间存活；`sa_ptr` 要么为空，要么指向同一
        // 作用域内已初始化的 `SECURITY_ATTRIBUTES`。
        let open_mode = if first {
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE
        } else {
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED
        };
        let handle = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                PIPE_BUFFER_BYTES,
                PIPE_BUFFER_BYTES,
                0,
                sa_ptr,
            )
        };

        if handle == INVALID_HANDLE_VALUE {
            // SAFETY：紧接失败的调用读取线程局部错误码。
            let code = unsafe { GetLastError() };
            return Err(classify_create_error(code, first));
        }

        // SAFETY：句柄由 `CreateNamedPipeW` 以 OVERLAPPED 语义创建，所有权在此移交 tokio。
        unsafe { NamedPipeServer::from_raw_handle(handle.cast()) }.map_err(|_| {
            BrokerError::new(
                "HAVEN_BROKER_ENDPOINT_UNAVAILABLE",
                "无法在本地端点监听。",
                false,
            )
        })
    }

    /// 持有 ACL 与安全描述符缓冲区（`SECURITY_ATTRIBUTES` 只持裸指针，必须有人保活）。
    struct SecurityAttributes {
        _acl: Vec<u8>,
        _descriptor: Box<windows_sys::Win32::Security::SECURITY_DESCRIPTOR>,
        attributes: windows_sys::Win32::Security::SECURITY_ATTRIBUTES,
    }

    /// 构造**只含一个 ACE（给定 SID）**的 DACL 与自相对安全描述符。
    fn build_security_attributes(sid: &str) -> Result<SecurityAttributes, BrokerError> {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
        use windows_sys::Win32::Security::{
            AddAccessAllowedAce, GetLengthSid, InitializeAcl, InitializeSecurityDescriptor,
            SetSecurityDescriptorDacl, ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, PSID,
            SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR,
        };
        use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;

        // SAFETY：所有指针都指向本函数持有、在返回前填充完毕的缓冲区；`sid_ptr` 由
        // `ConvertStringSidToSidW` 分配并在使用后 `LocalFree`。
        unsafe {
            let wide: Vec<u16> = OsStr::new(sid)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let mut sid_ptr: PSID = std::ptr::null_mut();
            if ConvertStringSidToSidW(wide.as_ptr(), &mut sid_ptr) == 0 || sid_ptr.is_null() {
                return Err(BrokerError::internal());
            }

            let sid_len = GetLengthSid(sid_ptr) as usize;
            // ACL = 头部 + 一个 ACCESS_ALLOWED_ACE（SidStart 已含 4 字节）+ SID 余量
            let acl_size = std::mem::size_of::<ACL>() + std::mem::size_of::<ACCESS_ALLOWED_ACE>()
                - std::mem::size_of::<u32>()
                + sid_len;
            let mut acl = vec![0u8; acl_size];
            let acl_ptr = acl.as_mut_ptr().cast::<ACL>();

            let built = InitializeAcl(acl_ptr, acl_size as u32, ACL_REVISION) != 0
                && AddAccessAllowedAce(acl_ptr, ACL_REVISION, FILE_ALL_ACCESS, sid_ptr) != 0;
            LocalFree(sid_ptr.cast());
            if !built {
                return Err(BrokerError::new(
                    "HAVEN_BROKER_ENDPOINT_UNAVAILABLE",
                    "无法为本地端点构造访问控制表。",
                    false,
                ));
            }

            let mut descriptor: Box<SECURITY_DESCRIPTOR> =
                Box::new(std::mem::zeroed::<SECURITY_DESCRIPTOR>());
            let descriptor_ptr: *mut SECURITY_DESCRIPTOR = &mut *descriptor;
            // SECURITY_DESCRIPTOR_REVISION 在 windows-sys 里没有导出常量，其值恒为 1。
            const SECURITY_DESCRIPTOR_REVISION: u32 = 1;
            // 第三个参数 TRUE(1)=存在 DACL；第四个 FALSE(0)=不是默认 DACL。
            if InitializeSecurityDescriptor(descriptor_ptr.cast(), SECURITY_DESCRIPTOR_REVISION)
                == 0
                || SetSecurityDescriptorDacl(descriptor_ptr.cast(), 1, acl_ptr, 0) == 0
            {
                return Err(BrokerError::new(
                    "HAVEN_BROKER_ENDPOINT_UNAVAILABLE",
                    "无法为本地端点构造安全描述符。",
                    false,
                ));
            }

            Ok(SecurityAttributes {
                attributes: SECURITY_ATTRIBUTES {
                    nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: descriptor_ptr.cast(),
                    bInheritHandle: 0,
                },
                _acl: acl,
                _descriptor: descriptor,
            })
        }
    }

    /// Win32 错误码 → 稳定 Broker 错误。
    ///
    /// `ERROR_ACCESS_DENIED` 只在**首实例**（`first = true`，即带
    /// `FILE_FLAG_FIRST_PIPE_INSTANCE` 的那一次）才是"同名 pipe 已存在"的证据：MSDN 把
    /// "拒绝创建"记为该标志唯一的失败原因。后续实例不带这个标志，同一错误码来自安全
    /// 描述符或参数层，把它归类成 `ENDPOINT_BUSY` 等于把"本实例创不出端点"谎报成
    /// "另有实例在跑"——用户会去找一个并不存在的第二实例，而真正的问题（DACL 构造被拒）
    /// 被掩盖。因此后续实例一律 fail closed 成 `ENDPOINT_UNAVAILABLE`。
    ///
    /// 两种归类都不可重试：它们都表示"此刻不该接管"。
    pub(super) fn classify_create_error(code: u32, first: bool) -> BrokerError {
        const ERROR_ACCESS_DENIED: u32 = 5;
        const ERROR_PIPE_BUSY: u32 = 231;
        match code {
            // 端点已被另一个实例持有：**fail closed**，不接管、不删除、不改名。
            ERROR_PIPE_BUSY => BrokerError::endpoint_busy(),
            ERROR_ACCESS_DENIED if first => BrokerError::endpoint_busy(),
            _ => BrokerError::new(
                "HAVEN_BROKER_ENDPOINT_UNAVAILABLE",
                "无法在本地端点监听。",
                false,
            ),
        }
    }

    /// 读回 pipe 的 DACL 中出现的 SID 列表，供测试断言"只授当前用户"。
    #[cfg(test)]
    pub(super) fn read_dacl_ace_sids(server: &NamedPipeServer) -> Vec<String> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::{
            ConvertSidToStringSidW, GetSecurityInfo, SE_KERNEL_OBJECT,
        };
        use windows_sys::Win32::Security::{
            AclSizeInformation, GetAclInformation, ACL, ACL_SIZE_INFORMATION,
            DACL_SECURITY_INFORMATION, PSID,
        };

        // SAFETY：`GetSecurityInfo` 分配的缓冲区在读取完成后由 `LocalFree` 释放。
        unsafe {
            let mut dacl: *mut ACL = std::ptr::null_mut();
            let mut sd: PSID = std::ptr::null_mut();
            let status = GetSecurityInfo(
                server.as_raw_handle().cast(),
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut sd,
            );
            if status != 0 || dacl.is_null() {
                return Vec::new();
            }

            let mut size_info: ACL_SIZE_INFORMATION = std::mem::zeroed();
            if GetAclInformation(
                dacl,
                (&mut size_info as *mut ACL_SIZE_INFORMATION).cast(),
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            ) == 0
            {
                LocalFree(sd.cast());
                return Vec::new();
            }

            let mut sids = Vec::new();
            for index in 0..size_info.AceCount {
                let mut ace: *mut std::ffi::c_void = std::ptr::null_mut();
                if windows_sys::Win32::Security::GetAce(dacl, index, &mut ace) == 0 {
                    continue;
                }
                // ACE 头部之后是 4 字节 access mask，再之后才是 SID。
                let sid_ptr = ace
                    .cast::<u8>()
                    .add(std::mem::size_of::<windows_sys::Win32::Security::ACE_HEADER>() + 4)
                    .cast::<std::ffi::c_void>();
                let mut text: *mut u16 = std::ptr::null_mut();
                if ConvertSidToStringSidW(sid_ptr, &mut text) != 0 && !text.is_null() {
                    let mut len = 0usize;
                    while *text.add(len) != 0 {
                        len += 1;
                    }
                    sids.push(String::from_utf16_lossy(std::slice::from_raw_parts(
                        text, len,
                    )));
                    LocalFree(text.cast());
                }
            }
            LocalFree(sd.cast());
            sids
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use super::*;

    pub(super) struct Listener;

    impl Listener {
        pub(super) async fn bind(_endpoint: &BrokerEndpoint) -> Result<Self, BrokerError> {
            // 本平台没有受支持的本地端点原语：明确 fail closed，不伪造。
            Err(BrokerError::new(
                "HAVEN_BROKER_ENDPOINT_UNSUPPORTED",
                "当前平台不支持外部 Agent 接入的本地端点。",
                false,
            ))
        }

        /// 本平台没有监听循环可跑：没用过相位也无所谓——任务立刻结束，管理端的
        /// `is_finished` 判定已经覆盖这种"从未进入 serve"的情况。
        pub(super) async fn serve(
            self,
            _api: Arc<dyn BrokerAgentApi>,
            _quota: Arc<ProcessQuota>,
            _app_version: String,
            _shutdown: watch::Receiver<bool>,
            _serving: watch::Sender<bool>,
        ) {
        }
    }
}

#[cfg(test)]
mod tests {
    // 端点是进程级独占资源；测试必须把这个同步 guard 持有到异步 bind/serve 流程结束，
    // 否则并发用例会互相抢端点。这里是有意的测试串行化，不是生产代码的锁跨 await。
    #![allow(clippy::await_holding_lock)]

    use super::*;
    use serde_json::json;
    use tokio::sync::Notify;

    struct EchoApi;

    #[async_trait::async_trait]
    impl BrokerAgentApi for EchoApi {
        async fn context(&self) -> Result<super::super::session::ContextPayload, BrokerError> {
            Ok(json!({ "revision": "rev-1" }))
        }

        async fn setting_sources(&self) -> Result<serde_json::Value, BrokerError> {
            Ok(json!({"schemaVersion": 1, "section": "reading", "revision": null, "layers": []}))
        }

        async fn resource_preference(
            &self,
            _request: &super::super::protocol::ResourcePreferencePayload,
        ) -> Result<serde_json::Value, BrokerError> {
            Ok(
                json!({"schemaVersion": 1, "contextId": "0196f0d2-0000-7000-8000-0000000000c1", "contextHash": "a".repeat(64), "targetScope": "edition", "editionId": "0196f0d2-0000-7000-8000-0000000000c1", "mediaItemId": null, "revision": null, "reading": null, "comic": null}),
            )
        }

        async fn library_summary(&self, _limit: u32) -> Result<serde_json::Value, BrokerError> {
            Ok(
                json!({"schemaVersion": 1, "counts": {"works": 0, "editions": 0, "mediaItems": 0, "favorites": 0, "inProgress": 0}, "categories": [], "recent": [], "truncated": false}),
            )
        }

        async fn media_capabilities(
            &self,
            _media_item_id: Option<&str>,
            _limit: u32,
        ) -> Result<serde_json::Value, BrokerError> {
            Ok(json!({"schemaVersion": 1, "items": [], "truncated": false}))
        }

        async fn onboarding(&self) -> Result<serde_json::Value, BrokerError> {
            Ok(
                json!({"schemaVersion": 1, "completedSteps": [], "nextStep": null, "hasStorageLocation": false, "hasLibraryContent": false, "hasAiProvider": false}),
            )
        }

        async fn create_proposal(
            &self,
            _session_id: haven_domain::ids::AgentSessionId,
            _request_id: haven_domain::ids::AgentRequestId,
            _request: &haven_application::wire::AgentSettingsProposalCreateRequest,
        ) -> Result<haven_application::wire::AgentSettingsProposalDto, BrokerError> {
            Err(BrokerError::capability_unavailable())
        }

        async fn create_resource_proposal(
            &self,
            _session_id: haven_domain::ids::AgentSessionId,
            _request_id: haven_domain::ids::AgentRequestId,
            _request: &haven_application::wire::AgentResourcePreferenceProposalCreateRequest,
        ) -> Result<haven_application::wire::AgentResourcePreferenceProposalDto, BrokerError>
        {
            Err(BrokerError::capability_unavailable())
        }
    }

    /// 调度顺序用例专用 API。
    ///
    /// 两个请求各自先报"已进入"，再等各自的放行信号——测试因此能精确控制"哪个 worker 什么
    /// 时候完成"。`context` 返回一个**大到写不完**的载荷：驱动会一直卡在写它上面，这段
    /// 时间里驱动的 select 根本不会被轮询（这正是构造"两个事件同时就绪"的关键）。
    struct GatedApi {
        entered: mpsc::UnboundedSender<&'static str>,
        release_context: Notify,
        release_proposal: Notify,
        big: String,
    }

    #[async_trait::async_trait]
    impl BrokerAgentApi for GatedApi {
        async fn context(&self) -> Result<super::super::session::ContextPayload, BrokerError> {
            let _ = self.entered.send("context");
            self.release_context.notified().await;
            Ok(json!({ "revision": "rev-1", "pad": self.big }))
        }

        async fn setting_sources(&self) -> Result<serde_json::Value, BrokerError> {
            Ok(json!({"schemaVersion": 1, "section": "reading", "revision": null, "layers": []}))
        }

        async fn resource_preference(
            &self,
            _request: &super::super::protocol::ResourcePreferencePayload,
        ) -> Result<serde_json::Value, BrokerError> {
            Ok(
                json!({"schemaVersion": 1, "contextId": "0196f0d2-0000-7000-8000-0000000000c1", "contextHash": "a".repeat(64), "targetScope": "edition", "editionId": "0196f0d2-0000-7000-8000-0000000000c1", "mediaItemId": null, "revision": null, "reading": null, "comic": null}),
            )
        }

        async fn library_summary(&self, _limit: u32) -> Result<serde_json::Value, BrokerError> {
            Ok(
                json!({"schemaVersion": 1, "counts": {"works": 0, "editions": 0, "mediaItems": 0, "favorites": 0, "inProgress": 0}, "categories": [], "recent": [], "truncated": false}),
            )
        }

        async fn media_capabilities(
            &self,
            _media_item_id: Option<&str>,
            _limit: u32,
        ) -> Result<serde_json::Value, BrokerError> {
            Ok(json!({"schemaVersion": 1, "items": [], "truncated": false}))
        }

        async fn onboarding(&self) -> Result<serde_json::Value, BrokerError> {
            Ok(
                json!({"schemaVersion": 1, "completedSteps": [], "nextStep": null, "hasStorageLocation": false, "hasLibraryContent": false, "hasAiProvider": false}),
            )
        }

        async fn create_proposal(
            &self,
            _session_id: haven_domain::ids::AgentSessionId,
            _request_id: haven_domain::ids::AgentRequestId,
            _request: &haven_application::wire::AgentSettingsProposalCreateRequest,
        ) -> Result<haven_application::wire::AgentSettingsProposalDto, BrokerError> {
            let _ = self.entered.send("create_proposal");
            self.release_proposal.notified().await;
            Err(BrokerError::capability_unavailable())
        }

        async fn create_resource_proposal(
            &self,
            _session_id: haven_domain::ids::AgentSessionId,
            _request_id: haven_domain::ids::AgentRequestId,
            _request: &haven_application::wire::AgentResourcePreferenceProposalCreateRequest,
        ) -> Result<haven_application::wire::AgentResourcePreferenceProposalDto, BrokerError>
        {
            Err(BrokerError::capability_unavailable())
        }
    }

    fn hello_frame() -> Vec<u8> {
        encode_frame(r#"{"type":"hello","protocol_version":1,"client":{"name":"test"}}"#)
    }

    fn context_frame(id: i64) -> Vec<u8> {
        encode_frame(&format!(r#"{{"type":"context","id":{id},"payload":{{}}}}"#))
    }

    fn cancel_frame(id: i64) -> Vec<u8> {
        encode_frame(&format!(r#"{{"type":"cancel","id":{id}}}"#))
    }

    /// 一条能通过 `prepare` 全部校验的 `create_proposal` 帧（字段形状与协议层一致）。
    fn create_proposal_frame(id: i64) -> Vec<u8> {
        let hash = "a".repeat(64);
        encode_frame(&format!(
            r#"{{"type":"create_proposal","id":{id},"payload":{{"section":"reading","context_id":"ctx-1","context_hash":"{hash}","base_revision":null,"patch":{{"font_size":"large"}}}}}}"#
        ))
    }

    async fn read_one<S>(stream: &mut S) -> serde_json::Value
    where
        S: AsyncReadExt + Unpin,
    {
        let mut prefix = [0u8; 4];
        stream.read_exact(&mut prefix).await.unwrap();
        let len = decode_frame_length(prefix).unwrap();
        let mut buffer = vec![0u8; len];
        stream.read_exact(&mut buffer).await.unwrap();
        serde_json::from_slice(&buffer).unwrap()
    }

    fn spawn_driver<S>(
        stream: S,
    ) -> (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    )
    where
        S: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
    {
        spawn_driver_with_timeouts(stream, ConnectionTimeouts::default())
    }

    /// 可注入超时的驱动。用例因此能把窗口压到毫秒级，不必真等 [`HANDSHAKE_TIMEOUT`]。
    fn spawn_driver_with_timeouts<S>(
        stream: S,
        timeouts: ConnectionTimeouts,
    ) -> (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    )
    where
        S: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
    {
        spawn_driver_with_api(stream, Arc::new(EchoApi), timeouts)
    }

    /// 同上，但连 API 一起注入：调度顺序用例需要一个完成时机由测试精确控制的实现。
    fn spawn_driver_with_api<S>(
        stream: S,
        api: Arc<dyn BrokerAgentApi>,
        timeouts: ConnectionTimeouts,
    ) -> (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    )
    where
        S: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
    {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = tokio::spawn(drive_connection_with_timeouts(
            stream,
            api,
            Arc::new(ProcessQuota::default()),
            "0.1.0-beta.1".to_owned(),
            shutdown_rx,
            timeouts,
        ));
        (shutdown_tx, handle)
    }

    #[tokio::test]
    async fn drive_connection_handshakes_then_serves_a_request() {
        // 用内存 duplex 流替代真实 socket：驱动逻辑与平台原语解耦，因此任何平台都能测。
        let (mut client, server) = tokio::io::duplex(8 * 1024);
        let (shutdown_tx, handle) = spawn_driver(server);

        client.write_all(&hello_frame()).await.unwrap();
        let welcome = read_one(&mut client).await;
        assert_eq!(welcome["type"], "welcome");
        assert_eq!(
            welcome["granted_requests"],
            json!([
                "context",
                "setting_sources",
                "resource_preference",
                "library_summary",
                "media_capabilities",
                "onboarding",
                "create_proposal",
                "create_resource_proposal"
            ])
        );

        client.write_all(&context_frame(9)).await.unwrap();
        let reply = read_one(&mut client).await;
        assert_eq!(reply["type"], "ok");
        assert_eq!(reply["id"], 9);
        assert_eq!(reply["payload"]["revision"], "rev-1");

        let _ = shutdown_tx.send(true);
        let _ = handle.await;
    }

    /// **调度顺序**：已完成 worker 的响应必须优先于帧通道里已经排队的帧。
    ///
    /// 构造方式刻意不依赖"谁先被调度"这种时间竞态，而是把两个事件**同时**变成就绪：
    /// 1. 两个请求都先卡在 API 里（`GatedApi` 会先报"已进入"）；
    /// 2. 放行第一个，它返回一个远大于 duplex 容量的载荷——驱动卡在写里，这段时间内它的
    ///    `select!` 根本不会被轮询；
    /// 3. 在驱动仍被钉住时排入一帧（`cancel`，立即回复），再放行第二个 worker 并让它跑完；
    /// 4. 客户端开始读大回复 → 驱动写完后回到 `select!`，此时"worker 已完成"和"通道里有帧"
    ///    同时就绪。
    ///
    /// 于是先写哪一个完全由 `biased` 的分支顺序决定：`join_next` 在前就是 `id: 2` 先到，
    /// `frame_receiver.recv()` 在前就是 `id: 9` 先到。整个断言因此是确定性的，不靠 sleep。
    #[tokio::test]
    async fn completed_worker_is_written_before_already_queued_frames() {
        /// 大回复的载荷：必须远大于 duplex 容量，否则驱动不会被写阻塞。
        const BIG_PAD: usize = 64 * 1024;
        const CAPACITY: usize = 1024;

        let (entered_tx, mut entered_rx) = mpsc::unbounded_channel::<&'static str>();
        let api = Arc::new(GatedApi {
            entered: entered_tx,
            release_context: Notify::new(),
            release_proposal: Notify::new(),
            big: "x".repeat(BIG_PAD),
        });

        let (mut client, server) = tokio::io::duplex(CAPACITY);
        let (shutdown_tx, handle) = spawn_driver_with_api(
            server,
            api.clone(),
            ConnectionTimeouts {
                // 大回复的写出被客户端有意拖住，窗口要留够；其余超时用默认值。
                write: std::time::Duration::from_secs(10),
                ..ConnectionTimeouts::default()
            },
        );

        client.write_all(&hello_frame()).await.unwrap();
        assert_eq!(read_one(&mut client).await["type"], "welcome");

        // ---- 两个请求都必须在第一个完成之前被分派出去 ----
        // "已进入"信号来自 worker 内部，因此等到两个信号就意味着两个 worker 都已经在跑。
        // 否则第二个请求可能还躺在帧通道里，测的就不是调度顺序了。
        client.write_all(&context_frame(1)).await.unwrap();
        client.write_all(&create_proposal_frame(2)).await.unwrap();
        let mut entered = [false, false];
        for _ in 0..2 {
            match entered_rx.recv().await.expect("两个请求都必须进入 API") {
                "context" => entered[0] = true,
                "create_proposal" => entered[1] = true,
                other => panic!("未知的进入信号：{other}"),
            }
        }
        assert_eq!(entered, [true, true], "两个 worker 都必须已经在跑");

        // ---- 放行第一个：它返回大载荷，驱动开始写并卡住 ----
        api.release_context.notify_one();
        let mut prefix = [0u8; 4];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_exact(&mut prefix),
        )
        .await
        .expect("驱动必须开始写第一条的回复")
        .unwrap();
        let big_len = decode_frame_length(prefix).expect("长度前缀必须合法");
        assert!(
            big_len > CAPACITY,
            "回复必须大于 duplex 容量，否则驱动不会被写阻塞：{big_len}"
        );

        // ---- 驱动被钉在写出里：排入一帧，再放行第二个 worker ----
        client.write_all(&cancel_frame(9)).await.unwrap();
        api.release_proposal.notify_one();
        // 让第二个 worker 真正跑完（当前线程运行时下，worker 只在 main 让出时才被轮询；
        // 驱动此刻仍在写里，不可能抢先去取通道里的帧）。
        for _ in 0..64 {
            tokio::task::yield_now().await;
        }

        // ---- 读完大回复：驱动回到 select，两个事件同时就绪 ----
        let mut rest = vec![0u8; big_len];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_exact(&mut rest),
        )
        .await
        .expect("客户端读完后驱动必须能写完大回复")
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&rest).unwrap()["id"],
            1,
            "先写完的必须是第一条"
        );

        let first = read_one(&mut client).await;
        let second = read_one(&mut client).await;
        assert_eq!(
            first["id"], 2,
            "已完成的 worker 必须先写出；实际先到的是 {first}"
        );
        assert_eq!(second["id"], 9, "排队的帧随后处理；实际第二帧是 {second}");

        let _ = shutdown_tx.send(true);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }

    #[tokio::test]
    async fn drive_connection_closes_on_oversized_frame_without_reply() {
        let (mut client, server) = tokio::io::duplex(64);
        let (_shutdown_tx, handle) = spawn_driver(server);

        let too_big = (MAX_FRAME_BYTES as u32 + 1).to_be_bytes();
        let _ = client.write_all(&too_big).await;
        let mut buffer = [0u8; 1];
        let read = client.read(&mut buffer).await;
        assert!(matches!(read, Ok(0) | Err(_)), "必须断连，得到 {read:?}");
        let _ = handle.await;
    }

    #[tokio::test]
    async fn drive_connection_stops_on_shutdown_signal() {
        let (_client, server) = tokio::io::duplex(64);
        let (shutdown_tx, handle) = spawn_driver(server);
        shutdown_tx.send(true).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("关闭信号必须让连接任务结束")
            .unwrap();
    }

    /// 握手超时：客户端连上却不发首帧时，驱动必须在短时间内自己收尾。
    ///
    /// 窗口由注入的毫秒级参数给出（不等真实的 [`HANDSHAKE_TIMEOUT`]），整条路径只用内存
    /// duplex，因此**不依赖 Named Pipe / Unix socket**，任何平台都能跑。
    #[tokio::test]
    async fn drive_connection_ends_when_handshake_times_out() {
        let (mut client, server) = tokio::io::duplex(64);
        let (_shutdown_tx, handle) = spawn_driver_with_timeouts(
            server,
            ConnectionTimeouts {
                handshake: std::time::Duration::from_millis(20),
                ..ConnectionTimeouts::default()
            },
        );

        // 一个字节都不发：驱动只能等握手窗口，等不到就必须断连。
        let mut buffer = [0u8; 1];
        let read =
            tokio::time::timeout(std::time::Duration::from_secs(2), client.read(&mut buffer))
                .await
                .expect("握手超时必须断连，而不是把连接一直挂着");
        assert!(
            matches!(read, Ok(0) | Err(_)),
            "握手未完成时必须断连且不写回复，得到 {read:?}"
        );

        tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("握手超时后连接任务必须结束，permit 随之释放")
            .unwrap();
    }

    /// 写出超时：对端不读时 welcome 写不出去，驱动必须在短时间内收尾而不是永远卡在写上。
    ///
    /// duplex 容量刻意小于一帧：客户端只发 `hello`、之后**不读**，welcome 会填满缓冲并阻塞，
    /// 只有 [`WRITE_TIMEOUT`] 能把它结束掉。
    #[tokio::test]
    async fn drive_connection_ends_when_write_stalls() {
        let (mut client, server) = tokio::io::duplex(64);
        let (_shutdown_tx, handle) = spawn_driver_with_timeouts(
            server,
            ConnectionTimeouts {
                write: std::time::Duration::from_millis(20),
                ..ConnectionTimeouts::default()
            },
        );

        // hello 帧略大于容量，但读任务在持续消费，所以这一次写能完成。
        client.write_all(&hello_frame()).await.unwrap();
        // 此后不再读取：welcome 的写端必然阻塞。

        tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("写出超时后连接任务必须结束，permit 随之释放")
            .unwrap();
    }

    /// 空闲超时：握手完成、收到 `welcome` 之后不再发任何请求，服务端必须自己断连。
    ///
    /// 窗口同样经 [`ConnectionTimeouts`] 注入，idle 压到毫秒级——用例既不真等
    /// [`IDLE_TIMEOUT`]（30 分钟），也不改动生产常量与协议。整条路径只有内存 duplex，
    /// 因此不依赖 Named Pipe / Unix socket，任何平台都能跑。
    ///
    /// 前半段是**对照**：客户端行为完全相同，只是用生产默认超时，连接就不该在观测窗口内
    /// 被关掉。没有它，"连接关了"也可能来自别的路径；有了它，后半段的关闭才能归因到 idle
    /// 定时器本身。
    #[tokio::test]
    async fn drive_connection_ends_when_idle_times_out() {
        // ---- 对照：idle 远大于观测窗口时连接必须保持打开 ----
        {
            let (mut client, server) = tokio::io::duplex(8 * 1024);
            let (shutdown_tx, handle) = spawn_driver(server);

            client.write_all(&hello_frame()).await.unwrap();
            assert_eq!(read_one(&mut client).await["type"], "welcome");

            // 收到 welcome 之后一个字节都不再发：这正是"空闲"的定义。
            let mut buffer = [0u8; 1];
            let read = tokio::time::timeout(
                std::time::Duration::from_millis(100),
                client.read(&mut buffer),
            )
            .await;
            assert!(
                read.is_err(),
                "生产默认 idle 下，观测窗口内连接不该被关闭：{read:?}"
            );

            let _ = shutdown_tx.send(true);
            tokio::time::timeout(std::time::Duration::from_secs(2), handle)
                .await
                .expect("对照连接收到 shutdown 后必须结束")
                .expect("对照连接任务不得 panic");
        }

        // ---- 正题：idle 压到毫秒级，同一种客户端行为必须让连接自己收尾 ----
        let (mut client, server) = tokio::io::duplex(8 * 1024);
        // `_shutdown_tx` 必须绑在具名变量上活到用例结束：一旦它被 drop，
        // `shutdown.changed()` 会立刻返回 `Err`，连接就会因为停机而不是空闲而结束。
        let (_shutdown_tx, handle) = spawn_driver_with_timeouts(
            server,
            ConnectionTimeouts {
                idle: std::time::Duration::from_millis(20),
                ..ConnectionTimeouts::default()
            },
        );

        client.write_all(&hello_frame()).await.unwrap();
        assert_eq!(read_one(&mut client).await["type"], "welcome");
        // 此后一个字节都不再发，也不发 shutdown：驱动只剩 idle 这一条出路。

        let mut buffer = [0u8; 1];
        let read =
            tokio::time::timeout(std::time::Duration::from_secs(2), client.read(&mut buffer))
                .await
                .expect("空闲超时必须让服务端主动断连，而不是把连接一直挂着");
        assert!(
            matches!(read, Ok(0) | Err(_)),
            "空闲超时必须断连且不写回复，得到 {read:?}"
        );

        tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("空闲超时后连接任务必须结束，permit 随之释放")
            .unwrap();
    }

    /// 真实端点往返：绑定 → 连接 → 握手 → 请求，外加 V4 的权限与收尾断言。
    ///
    /// 三个前提任一不成立时**明确跳过**（打印原因）而不是静默通过：端点不可解析，或端点被
    /// 本机另一个**活** Haven 占着（那种情况下第一件事就是不该去动它）。
    #[cfg(unix)]
    #[tokio::test]
    async fn unix_endpoint_round_trip() {
        use std::os::unix::fs::{FileTypeExt, PermissionsExt};

        let _serialized = endpoint_test_guard();
        let Ok(endpoint) = super::super::endpoint::resolve_endpoint() else {
            eprintln!("skip: 本机既无 XDG_RUNTIME_DIR 也无 HOME，端点不可解析");
            return;
        };
        let BrokerEndpoint::UnixSocket(path) = &endpoint else {
            return;
        };
        let listener = match bind(&endpoint).await {
            Ok(listener) => listener,
            // 本机已有活 Haven 在提供 Broker：本用例不该去动它的端点。
            Err(error) if error.code() == "HAVEN_BROKER_ENDPOINT_BUSY" => {
                eprintln!("skip: 端点已被另一个活实例占用（HAVEN_BROKER_ENDPOINT_BUSY）");
                return;
            }
            // 例如 XDG_RUNTIME_DIR 只读之类：环境不允许，不谎称通过。
            Err(error) => {
                eprintln!("skip: 本机环境无法绑定端点（{}）", error.code());
                return;
            }
        };

        // ---- V4：绑定成功的那一刻，目录已经是 0700、socket 已经是 0600 ----
        let parent = path.parent().expect("端点必须有父目录");
        assert_eq!(
            std::fs::metadata(parent).unwrap().permissions().mode() & 0o777,
            0o700,
            "运行目录必须是 0700"
        );
        let socket_metadata = std::fs::symlink_metadata(path).unwrap();
        assert!(
            socket_metadata.file_type().is_socket(),
            "端点必须是一个 socket 文件"
        );
        assert_eq!(
            socket_metadata.permissions().mode() & 0o777,
            0o600,
            "端点 socket 必须是 0600"
        );

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (serving_tx, serving_rx) = watch::channel(true);
        let server = tokio::spawn(serve(
            listener,
            Arc::new(EchoApi),
            Arc::new(ProcessQuota::default()),
            "0.1.0-beta.1".to_owned(),
            shutdown_rx,
            serving_tx,
        ));

        let mut client = tokio::net::UnixStream::connect(path)
            .await
            .expect("必须能连上刚绑定的端点");
        client.write_all(&hello_frame()).await.unwrap();
        assert_eq!(read_one(&mut client).await["type"], "welcome");
        client.write_all(&context_frame(1)).await.unwrap();
        let reply = read_one(&mut client).await;
        assert_eq!(reply["type"], "ok");
        assert_eq!(reply["id"], 1);

        drop(client);
        let _ = shutdown_tx.send(true);
        tokio::time::timeout(std::time::Duration::from_secs(2), server)
            .await
            .expect("关闭信号后 serve 必须结束")
            .expect("serve 不得 panic");
        // serve 在收尾之前必须先宣告"不再 accept"——这是管理端不把收尾窗口误报成
        // `listening` 的唯一依据。
        assert!(!*serving_rx.borrow(), "serve 结束前必须先宣告停止接收");
        // 收尾必须把 socket 文件从这个固定路径上移除：留着它，下一次 enable 就得靠
        // §7.3 的陈旧恢复去收拾，而那是给"崩溃"准备的路，不是给正常关闭准备的。
        assert!(
            std::fs::symlink_metadata(path).is_err(),
            "serve 收尾必须移除端点 socket 文件"
        );
        // 锁文件**不删**：它只是锁的载体，留着不会让任何人误判有人在听。
        assert!(
            super::super::endpoint::unix_lock_file_at(path)
                .expect("端点必须有父目录")
                .exists(),
            "锁文件不随监听结束删除"
        );
    }

    /// Windows 真实端点往返 + **DACL 断言**：pipe 的 DACL 只含当前用户 SID。
    #[cfg(windows)]
    #[tokio::test]
    async fn windows_endpoint_round_trip_and_dacl_grants_only_current_user() {
        use tokio::net::windows::named_pipe::ClientOptions;

        let _serialized = endpoint_test_guard();

        let Ok(endpoint) = super::super::endpoint::resolve_endpoint() else {
            return;
        };
        let BrokerEndpoint::NamedPipe(name) = &endpoint else {
            return;
        };
        let Ok(listener) = bind(&endpoint).await else {
            return; // 端点被占用
        };

        // ---- DACL：恰好一个 ACE，且是当前用户 ----
        let sids = platform::read_dacl_ace_sids(listener.inner.first_instance());
        let expected = super::super::endpoint::current_user_sid_string().unwrap();
        assert_eq!(
            sids,
            vec![expected.clone()],
            "DACL 必须**恰好**含当前用户 SID 一个 ACE，实际 {sids:?}"
        );
        for forbidden in [
            "S-1-5-18",     // SYSTEM
            "S-1-1-0",      // Everyone
            "S-1-5-11",     // Authenticated Users
            "S-1-5-7",      // Anonymous
            "S-1-5-32-545", // Users
        ] {
            assert!(
                !sids.iter().any(|sid| sid == forbidden),
                "DACL 不得包含 {forbidden}"
            );
        }

        // ---- 往返 ----
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (serving_tx, serving_rx) = watch::channel(true);
        let server = tokio::spawn(serve(
            listener,
            Arc::new(EchoApi),
            Arc::new(ProcessQuota::default()),
            "0.1.0-beta.1".to_owned(),
            shutdown_rx,
            serving_tx,
        ));

        let mut client = ClientOptions::new()
            .open(std::ffi::OsStr::new(name))
            .expect("必须能连上刚绑定的端点");
        client.write_all(&hello_frame()).await.unwrap();
        assert_eq!(read_one(&mut client).await["type"], "welcome");
        client.write_all(&context_frame(1)).await.unwrap();
        let reply = read_one(&mut client).await;
        assert_eq!(reply["type"], "ok");
        assert_eq!(reply["id"], 1);

        drop(client);
        let _ = shutdown_tx.send(true);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), server).await;
        // 同上：Windows 侧同样必须在收尾前宣告"不再 accept"。
        assert!(!*serving_rx.borrow(), "serve 结束前必须先宣告停止接收");
    }

    /// 第二个实例必须在同一端点上 **fail closed**，且不干扰第一个实例。
    #[tokio::test]
    async fn second_bind_on_same_endpoint_is_rejected() {
        let _serialized = endpoint_test_guard();
        let Ok(endpoint) = super::super::endpoint::resolve_endpoint() else {
            return;
        };
        let Ok(first) = bind(&endpoint).await else {
            return;
        };
        match bind(&endpoint).await {
            Ok(_) => panic!("第二次绑定必须失败，且不得接管端点"),
            Err(error) => {
                assert_eq!(error.code(), "HAVEN_BROKER_ENDPOINT_BUSY");
                assert!(!error.retryable());
            }
        }
        drop(first);
    }

    /// `ERROR_ACCESS_DENIED` 只在首实例才是「端点已被另一个实例占用」的证据（设计 §7.4）。
    ///
    /// 首实例带 `FILE_FLAG_FIRST_PIPE_INSTANCE`，同名 pipe 已存在时唯一的失败原因就是它；
    /// 后续实例不带该标志，同一错误码来自安全描述符层——把它报成「另一个实例在跑」会让
    /// 用户去找一个并不存在的第二实例，而真正的原因（本实例创不出端点）被掩盖。
    /// 两种归类都必须 fail closed：都不可重试。
    #[cfg(windows)]
    #[test]
    fn access_denied_is_busy_only_for_the_first_pipe_instance() {
        const ERROR_ACCESS_DENIED: u32 = 5;
        const ERROR_PIPE_BUSY: u32 = 231;
        const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;

        let first = platform::classify_create_error(ERROR_ACCESS_DENIED, true);
        assert_eq!(first.code(), "HAVEN_BROKER_ENDPOINT_BUSY");
        assert!(!first.retryable());

        let later = platform::classify_create_error(ERROR_ACCESS_DENIED, false);
        assert_eq!(
            later.code(),
            "HAVEN_BROKER_ENDPOINT_UNAVAILABLE",
            "后续实例的 ERROR_ACCESS_DENIED 不是「另一个实例在跑」的证据"
        );
        assert!(!later.retryable());

        assert_eq!(
            platform::classify_create_error(ERROR_PIPE_BUSY, false).code(),
            "HAVEN_BROKER_ENDPOINT_BUSY"
        );
        // 认不出的错误码在两个方向上都不得被说成「最后一个实例在跑」。
        assert_eq!(
            platform::classify_create_error(ERROR_NOT_ENOUGH_MEMORY, true).code(),
            "HAVEN_BROKER_ENDPOINT_UNAVAILABLE"
        );
    }

    /// 即使 bind 失败，进程级 umask 也必须恢复；不能把 077 泄漏给后续文件创建。
    #[cfg(unix)]
    #[tokio::test]
    async fn unix_bind_restores_umask_on_endpoint_busy() {
        struct RestoreUmask(libc::mode_t);

        impl Drop for RestoreUmask {
            fn drop(&mut self) {
                // SAFETY：恢复本测试进入前的进程级 umask。
                unsafe { libc::umask(self.0) };
            }
        }

        let _serialized = endpoint_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("haven").join("agent-v1.sock");
        if socket.to_string_lossy().len() > super::super::endpoint::SOCKET_PATH_MAX_LEN {
            return;
        }
        let endpoint = BrokerEndpoint::UnixSocket(socket);
        let previous = unsafe { libc::umask(0o027) };
        let _restore = RestoreUmask(previous);
        let Ok(first) = bind(&endpoint).await else {
            panic!("第一个 bind 必须成功，才能验证 busy 路径的 umask 恢复");
        };

        let error = match bind(&endpoint).await {
            Ok(_) => panic!("第二次 bind 必须失败"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "HAVEN_BROKER_ENDPOINT_BUSY");
        let current = unsafe {
            let mask = libc::umask(0);
            libc::umask(mask);
            mask
        };
        assert_eq!(current, 0o027, "失败路径必须恢复 bind 前的 umask");
        drop(first);
    }

    /// 成功路径同样必须恢复进程级 umask，不能只保护 endpoint busy 分支。
    #[cfg(unix)]
    #[tokio::test]
    async fn unix_bind_restores_umask_on_success() {
        struct RestoreUmask(libc::mode_t);

        impl Drop for RestoreUmask {
            fn drop(&mut self) {
                // SAFETY：恢复本测试进入前的进程级 umask。
                unsafe { libc::umask(self.0) };
            }
        }

        let _serialized = endpoint_test_guard();
        let previous = unsafe { libc::umask(0o027) };
        let _restore = RestoreUmask(previous);
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("haven").join("agent-v1.sock");
        if socket.to_string_lossy().len() > super::super::endpoint::SOCKET_PATH_MAX_LEN {
            return;
        }
        let endpoint = BrokerEndpoint::UnixSocket(socket);
        let listener = bind(&endpoint).await.expect("空目录必须能绑定");
        let current = unsafe {
            let mask = libc::umask(0);
            libc::umask(mask);
            mask
        };
        assert_eq!(current, 0o027, "成功路径必须恢复 bind 前的 umask");
        drop(listener);
    }

    /// Unix 端点的**活实例锁**与**陈旧端点恢复**（设计 §7.3 / §7.4）。
    ///
    /// 这组用例在 `tempfile` 造出的**专属端点**上跑，而不是 `resolve_endpoint()` 的每用户
    /// 真实端点：它们要制造的正是"上一个 Haven 被 SIGKILL"这种现场，在真实端点上做就等于
    /// 破坏本机正在运行的 Haven。端点的形态与合法性由 `endpoint.rs` 的纯函数用例覆盖，
    /// 真实端点的往返、权限与收尾由 `unix_endpoint_round_trip` 覆盖。
    #[cfg(unix)]
    mod unix_endpoint {
        use super::*;
        use crate::agent_broker::endpoint::{
            LOCK_FILE_NAME, SOCKET_FILE_NAME, SOCKET_PATH_MAX_LEN,
        };
        use std::os::unix::fs::{FileTypeExt, PermissionsExt};
        use std::path::{Path, PathBuf};
        use tempfile::TempDir;

        /// `<tempdir>/haven/agent-v1.sock`。
        ///
        /// 返回 `None` 表示临时目录路径太长、装不下 `sun_path`——此时明确跳过，而不是产生一个
        /// 与实现无关的失败。`TempDir` 由调用方持有：它一 drop，整棵目录树就没了。
        fn endpoint_in_tempdir() -> Option<(TempDir, BrokerEndpoint, PathBuf)> {
            let dir = tempfile::tempdir().ok()?;
            let socket = dir.path().join("haven").join(SOCKET_FILE_NAME);
            if socket.to_string_lossy().len() > SOCKET_PATH_MAX_LEN {
                eprintln!("skip: 临时目录路径过长，装不下 sun_path：{socket:?}");
                return None;
            }
            Some((dir, BrokerEndpoint::UnixSocket(socket.clone()), socket))
        }

        /// `<tempdir>/.haven/run/agent-v1.sock`，覆盖 `$HOME` 回退形态。
        fn fallback_endpoint_in_tempdir() -> Option<(TempDir, BrokerEndpoint, PathBuf)> {
            let dir = tempfile::tempdir().ok()?;
            let socket = dir.path().join(".haven").join("run").join(SOCKET_FILE_NAME);
            if socket.to_string_lossy().len() > SOCKET_PATH_MAX_LEN {
                eprintln!("skip: 临时目录路径过长，装不下 sun_path：{socket:?}");
                return None;
            }
            Some((dir, BrokerEndpoint::UnixSocket(socket.clone()), socket))
        }

        fn lock_path(socket: &Path) -> PathBuf {
            socket
                .parent()
                .expect("端点必须有父目录")
                .join(LOCK_FILE_NAME)
        }

        fn assert_is_socket(path: &Path) {
            assert!(
                std::fs::symlink_metadata(path)
                    .unwrap_or_else(|error| panic!("{path:?} 必须存在：{error}"))
                    .file_type()
                    .is_socket(),
                "{path:?} 必须仍然是 socket 文件"
            );
        }

        /// 上个实例被 SIGKILL 之后，下一次 enable 必须恢复自己的陈旧端点，而不是把
        /// `EADDRINUSE` 一律当成"另一个实例在跑"。
        ///
        /// 现场用"正常绑定一次再 drop"制造，它与 SIGKILL 在两个关键点上等价：进程级 fd 关闭
        /// 让内核释放 `flock`，而收尾代码没有机会跑，于是 socket 文件留在路径上
        /// （`BoundListener` 自己不删文件，只有 `serve` 的收尾才删）。
        #[tokio::test]
        async fn stale_socket_from_a_killed_instance_is_recovered() {
            let _serialized = endpoint_test_guard();
            let Some((_dir, endpoint, socket)) = endpoint_in_tempdir() else {
                return;
            };
            let first = bind(&endpoint).await.expect("空目录必须能绑定");
            drop(first);

            // 前提：现场确实是"有残留"，否则这个用例证明不了任何东西。
            assert_is_socket(&socket);
            assert!(
                lock_path(&socket).exists(),
                "前提：锁文件必须还留着（它本来就不该被删）"
            );

            let second = bind(&endpoint)
                .await
                .expect("自己的陈旧 socket 不得被误报成另一个实例");
            assert_eq!(second.endpoint(), &endpoint, "恢复后端点字符串不变");
            assert_is_socket(&socket);
        }

        /// 活实例的端点**不得**被删除、接管或打断。
        ///
        /// 这里刻意用"不持锁的监听者"占住路径：第二实例因此会先拿到锁、再撞上 `EADDRINUSE`，
        /// 于是"删不删"完全由 liveness probe 决定——正是要钉住的那一步。
        #[tokio::test]
        async fn live_socket_is_never_deleted() {
            let _serialized = endpoint_test_guard();
            let Some((_dir, endpoint, socket)) = endpoint_in_tempdir() else {
                return;
            };
            std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
            let _live = std::os::unix::net::UnixListener::bind(&socket).expect("必须能建活监听者");

            let error = bind(&endpoint).await.expect_err("活端点不得被接管");
            assert_eq!(error.code(), "HAVEN_BROKER_ENDPOINT_BUSY");
            assert!(!error.retryable());
            assert_is_socket(&socket);
            // 更强的断言：它**仍然在**接受连接，而不是"文件还在但已经不可用"。
            assert!(
                std::os::unix::net::UnixStream::connect(&socket).is_ok(),
                "活端点必须原样可用"
            );
        }

        /// **锁才是"活实例"的判据**，不是 socket 文件。
        ///
        /// 把 socket 文件从路径上移走：如果没有这把锁，第二实例此刻能 bind 成功，于是两个
        /// Haven 会同时认为自己在提供 Broker——正是多实例设计要禁止的那种情况。
        #[tokio::test]
        async fn instance_lock_alone_rejects_a_second_instance() {
            let _serialized = endpoint_test_guard();
            let Some((_dir, endpoint, socket)) = endpoint_in_tempdir() else {
                return;
            };
            let first = bind(&endpoint).await.expect("空目录必须能绑定");
            std::fs::remove_file(&socket).expect("必须能移走 socket 文件");
            assert!(
                std::fs::symlink_metadata(&socket).is_err(),
                "前提：路径上已经没有 socket 文件"
            );

            let error = bind(&endpoint)
                .await
                .expect_err("活实例持有的锁必须挡住第二实例");
            assert_eq!(error.code(), "HAVEN_BROKER_ENDPOINT_BUSY");
            drop(first);
        }

        /// 锁文件：`0600`、普通文件，而且**不随监听结束删除**（它只是锁的载体）。
        #[tokio::test]
        async fn lock_file_is_owner_only_and_never_removed() {
            let _serialized = endpoint_test_guard();
            let Some((_dir, endpoint, socket)) = endpoint_in_tempdir() else {
                return;
            };
            let listener = bind(&endpoint).await.expect("空目录必须能绑定");

            assert_eq!(
                std::fs::metadata(socket.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700,
                "运行目录必须是 0700"
            );
            let metadata = std::fs::metadata(lock_path(&socket)).expect("绑定后必须有锁文件");
            assert!(metadata.file_type().is_file(), "锁文件必须是普通文件");
            assert_eq!(
                metadata.permissions().mode() & 0o777,
                0o600,
                "锁文件必须是 0600"
            );

            drop(listener);
            assert!(
                lock_path(&socket).exists(),
                "锁文件不随监听结束删除：删它既不释放锁也不表达任何东西"
            );
        }

        /// 符号链接形式的端点路径：不跟随、不删除、fail closed。
        ///
        /// 构造是"别处的一个**活**监听者 + 指向它的符号链接"：probe 会（跟随链接）连上它，
        /// 于是结论是"有人在听"。无论走哪条分支，符号链接与它的目标都必须原样保留。
        #[tokio::test]
        async fn symlinked_endpoint_path_is_not_followed_or_deleted() {
            let _serialized = endpoint_test_guard();
            let Some((dir, endpoint, socket)) = endpoint_in_tempdir() else {
                return;
            };
            std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
            let target = dir.path().join("elsewhere.sock");
            let _live =
                std::os::unix::net::UnixListener::bind(&target).expect("必须能建目标监听者");
            std::os::unix::fs::symlink(&target, &socket).expect("必须能建符号链接");

            let error = bind(&endpoint).await.expect_err("不得穿过符号链接接管端点");
            assert_eq!(error.code(), "HAVEN_BROKER_ENDPOINT_UNAVAILABLE");
            assert!(
                std::fs::symlink_metadata(&socket)
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "符号链接本身必须原样保留"
            );
            assert_is_socket(&target);
        }

        /// 悬空符号链接也必须在 probe 之前被识别并保留，不能因为 `ENOENT` 而被当成 stale socket。
        #[tokio::test]
        async fn dangling_symlink_at_endpoint_path_is_refused() {
            let _serialized = endpoint_test_guard();
            let Some((dir, endpoint, socket)) = endpoint_in_tempdir() else {
                return;
            };
            std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
            let missing_target = dir.path().join("missing.sock");
            std::os::unix::fs::symlink(&missing_target, &socket).unwrap();

            let error = bind(&endpoint)
                .await
                .expect_err("悬空端点符号链接必须 fail closed");
            assert_eq!(error.code(), "HAVEN_BROKER_ENDPOINT_UNAVAILABLE");
            assert!(
                std::fs::symlink_metadata(&socket)
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "悬空符号链接本身必须原样保留"
            );
        }

        /// 端点路径上的目录不能被 probe 的错误码误判成另一个 Haven 实例，也不能被删除。
        #[tokio::test]
        async fn directory_at_endpoint_path_is_refused() {
            let _serialized = endpoint_test_guard();
            let Some((_dir, endpoint, socket)) = endpoint_in_tempdir() else {
                return;
            };
            std::fs::create_dir_all(&socket).unwrap();

            let error = bind(&endpoint)
                .await
                .expect_err("端点路径被目录占用时必须 fail closed");
            assert_eq!(error.code(), "HAVEN_BROKER_ENDPOINT_UNAVAILABLE");
            assert!(
                std::fs::metadata(&socket).unwrap().file_type().is_dir(),
                "端点路径上的目录不得被删除"
            );
        }

        /// 运行目录本身是符号链接时必须拒绝：不能 chmod 目标目录，也不能把 socket 写到目标目录。
        #[tokio::test]
        async fn symlinked_runtime_dir_is_refused() {
            let _serialized = endpoint_test_guard();
            let Some((dir, endpoint, socket)) = endpoint_in_tempdir() else {
                return;
            };
            let parent = socket.parent().expect("端点必须有父目录");
            let target = dir.path().join("redirect-target");
            std::fs::create_dir(&target).unwrap();
            std::os::unix::fs::symlink(&target, parent).unwrap();

            let error = bind(&endpoint)
                .await
                .expect_err("运行目录是符号链接时必须 fail closed");
            assert_eq!(error.code(), "HAVEN_BROKER_ENDPOINT_UNAVAILABLE");
            assert!(
                std::fs::symlink_metadata(parent)
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "运行目录符号链接本身必须原样保留"
            );
            assert!(
                std::fs::symlink_metadata(target.join(SOCKET_FILE_NAME)).is_err(),
                "不得把 socket 写入符号链接目标目录"
            );
        }

        /// `$HOME`/`XDG_RUNTIME_DIR` 本身可以是合法的系统路径别名；只要 Haven 管理的后缀
        /// 目录是真实目录，就不能因为中间的环境根路径是 symlink 而误拒。
        #[tokio::test]
        async fn configured_runtime_root_symlink_is_allowed() {
            let _serialized = endpoint_test_guard();
            let Some((dir, _unused_endpoint, _unused_socket)) = endpoint_in_tempdir() else {
                return;
            };
            let real_root = dir.path().join("real-runtime");
            let configured_root = dir.path().join("configured-runtime");
            std::fs::create_dir(&real_root).unwrap();
            std::os::unix::fs::symlink(&real_root, &configured_root).unwrap();
            let socket = configured_root.join("haven").join(SOCKET_FILE_NAME);
            let endpoint = BrokerEndpoint::UnixSocket(socket.clone());

            let listener = bind(&endpoint)
                .await
                .expect("环境根路径的合法 symlink 不应阻止绑定");
            assert_is_socket(&socket);
            drop(listener);
        }

        /// 回退路径中由 Haven 管理的 `.haven` 目录是 symlink 时必须拒绝。
        #[tokio::test]
        async fn fallback_managed_runtime_symlink_is_refused() {
            let _serialized = endpoint_test_guard();
            let Some((dir, endpoint, socket)) = fallback_endpoint_in_tempdir() else {
                return;
            };
            let target = dir.path().join("fallback-target");
            std::fs::create_dir(&target).unwrap();
            let haven_dir = socket
                .parent()
                .and_then(Path::parent)
                .expect("回退端点必须有 .haven 父目录");
            std::os::unix::fs::symlink(&target, haven_dir).unwrap();

            let error = bind(&endpoint)
                .await
                .expect_err("回退路径的 .haven symlink 必须 fail closed");
            assert_eq!(error.code(), "HAVEN_BROKER_ENDPOINT_UNAVAILABLE");
            assert!(
                std::fs::symlink_metadata(haven_dir)
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                ".haven 符号链接本身必须原样保留"
            );
        }

        /// 锁文件路径上的符号链接：`O_NOFOLLOW` 拒绝，且**不穿过它**去动目标文件。
        #[tokio::test]
        async fn symlinked_lock_file_is_refused() {
            let _serialized = endpoint_test_guard();
            let Some((dir, endpoint, socket)) = endpoint_in_tempdir() else {
                return;
            };
            std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
            let victim = dir.path().join("victim");
            std::fs::write(&victim, b"untouched").unwrap();
            std::os::unix::fs::symlink(&victim, lock_path(&socket)).unwrap();

            let error = bind(&endpoint)
                .await
                .expect_err("锁文件是符号链接时必须 fail closed");
            assert_eq!(error.code(), "HAVEN_BROKER_ENDPOINT_UNAVAILABLE");
            assert!(!error.retryable());
            assert_eq!(
                std::fs::read(&victim).unwrap(),
                b"untouched",
                "不得穿过符号链接去动它的目标"
            );
            assert!(
                std::fs::symlink_metadata(lock_path(&socket))
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "符号链接本身也不得被替换或删除"
            );
            assert!(
                std::fs::symlink_metadata(&socket).is_err(),
                "锁都拿不到时不得创建端点"
            );
        }

        /// 端点路径上不是 socket 文件：不删、fail closed。
        ///
        /// 平台把这种占用报成 `EADDRINUSE` 还是别的 errno 由内核决定，因此这里钉的是**性质**
        /// 而不是具体码：无论走哪条分支都必须 fail closed，且那个文件必须原样保留。
        #[tokio::test]
        async fn non_socket_at_endpoint_path_is_never_removed() {
            let _serialized = endpoint_test_guard();
            let Some((_dir, endpoint, socket)) = endpoint_in_tempdir() else {
                return;
            };
            std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
            std::fs::write(&socket, b"not a socket").unwrap();

            let error = bind(&endpoint)
                .await
                .expect_err("端点路径被非 socket 文件占着时必须 fail closed");
            assert_eq!(error.code(), "HAVEN_BROKER_ENDPOINT_UNAVAILABLE");
            assert!(!error.retryable());
            assert_eq!(
                std::fs::read(&socket).unwrap(),
                b"not a socket",
                "端点路径上的非 socket 文件不得被删除"
            );
        }
    }
}
