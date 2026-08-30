//! ScanService：`library_scan_start` / `scan_cancel`（BE-SCAN-001 / SLICE-SCAN-001）。
//!
//! 架构见 `docs/adr/ADR-003-library-scan-tasksupervisor.md`。
//!
//! 规则（契约 §14.4 / §14.5 / §10.2.1 / §10.3）：
//! - `start(storageLocationId)` 只登记任务 + 立即返回；扫描在独立后台任务跑。
//! - 同一 storageLocationId 已有运行/启动中任务 → 返回既有 operationId/taskId +
//!   alreadyRunning=true（R-C02 幂等）。登记用 Starting 占位 + 单一临界区，避免并发竞态。
//! - 进度通过 `library.scan` Channel 推送，**限频**（默认 200ms 窗口 + 50 批次阈值）。
//! - 协作式取消：每个文件边界检查取消标志；已索引文件不回滚。
//! - `scan_cancel(taskId)` 幂等：运行中 → 设置取消标志；已结束 → 返回真实终态；
//!   未知 taskId → `RESOURCE_NOT_FOUND`（不伪造 Completed）。
//! - 终态事件只发一次（completed/cancelled/failed 三选一，取消优先于失败）。
//! - 终态后发一次 `library.changed`（带完整 envelope：operationId/sequence/revision）。
//!
//! 依赖方向：本模块在 application 层，只依赖 domain 契约 + wire DTO；
//! `LibraryScanner` / `ScanEventSink` 端口由 infrastructure / src-tauri 实现。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::ids::StorageLocationId;

use crate::mapper::utc_millis_to_rfc3339;
use crate::services::storage_location::{ScanTarget, StorageLocationService};
use crate::wire::{LibraryChangedDto, LibraryScanEvent, ScanEventData, ScanPhase, ScanStartResult};

/// 扫描统计（application 级；infra 的 `LocalLibraryScanner` 实现端口时转换）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanReport {
    pub files_seen: u64,
    pub recognized: u64,
    pub new: u64,
    pub updated: u64,
    pub skipped: u64,
    pub errors: u64,
}

/// 单文件进度快照（传给 `ScanObserver::on_progress`）。`current_item` 为拥有型，
/// 适配 `LocalLibraryScanner` 的 WalkDir 局部路径（不泄漏字符串生命周期）。
#[derive(Debug, Clone, Default)]
pub struct ScanProgress {
    pub files_seen: u64,
    pub recognized: u64,
    pub new: u64,
    pub updated: u64,
    pub skipped: u64,
    pub errors: u64,
    pub current_item: Option<String>,
}

/// 扫描观察者端口：扫描器在文件边界回调进度 + 阶段切换 + 警告 + 检查取消。
/// `Send + Sync`：观察者在后台任务中跨线程使用。**实现方不得并发调用**（同一
/// 扫描任务内串行回调）；sequence 投递顺序由 `TaskObserver` 内部 Mutex 保证。
pub trait ScanObserver: Send + Sync {
    /// 阶段切换（started/enumerating/detecting/fingerprinting/indexing）。
    fn on_phase(&self, phase: ScanPhase);
    /// 每个文件处理完后回调当前累计进度（限频由 observer 实现决定）。
    fn on_progress(&self, progress: ScanProgress);
    /// 单文件级警告（解析失败/格式不支持等；不终止扫描）。
    fn on_warning(&self, message: String, progress: ScanProgress);
    /// 协作式取消检查：true → 扫描器在当前文件边界优雅停止。
    fn is_cancelled(&self) -> bool;
}

/// 扫描器端口：`LocalLibraryScanner`（infra）实现，application 不直接依赖 infra。
/// 取消时返回 `Ok(report)`（已索引部分保留），扫描器不得在取消时返回 Err。
#[async_trait::async_trait]
pub trait LibraryScanner: Send + Sync {
    async fn scan(
        &self,
        target: &ScanTarget,
        observer: &dyn ScanObserver,
    ) -> Result<ScanReport, AppError>;
}

/// 扫描事件 Sink 端口：src-tauri 实现为 Tauri Channel 发送。
/// `Send + Sync`：后台任务跨线程调用。
pub trait ScanEventSink: Send + Sync {
    /// 发送一条 `library.scan` Channel 事件（envelope 由 application 构造）。
    fn emit_scan_event(&self, event: LibraryScanEvent);
    /// 扫描终态后发一次 `library.changed`（envelope 由 application 构造，含
    /// operationId/sequence/revision；sink 不得篡改）。
    fn emit_library_changed(&self, event: LibraryChangedDto);
}

/// `scan_cancel` 结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelOutcome {
    /// 任务正在运行，已设置取消标志。
    Cancelled,
    /// 任务已结束，返回真实终态阶段。
    AlreadyTerminal(ScanPhase),
}

/// 任务生命周期阶段（内部，比 `ScanPhase` 多一个 Starting）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskState {
    Starting,
    Running,
    Completed,
    Cancelled,
    Failed,
}

/// 运行中（或启动中）的扫描任务。
struct RunningTask {
    operation_id: String,
    task_id: String,
    cancel: Arc<AtomicBool>,
    state: Arc<Mutex<TaskState>>,
}

/// 已结束任务的终态记录（保留以供 `scan_cancel` 返回真实终态）。
struct TerminalRecord {
    phase: ScanPhase,
    inserted_at: Instant,
}

/// 终态记录容量上限（超过后淘汰最旧；防无界增长）。
const TERMINAL_HISTORY_CAP: usize = 256;

/// 事件限频器：按时间窗口 + 批次阈值合并进度事件。
struct Throttle {
    last_emit: Mutex<Option<Instant>>,
    window: Duration,
    batch_threshold: u64,
    since_last: AtomicU64,
}

impl Throttle {
    fn new(window: Duration, batch_threshold: u64) -> Self {
        Self {
            last_emit: Mutex::new(None),
            window,
            batch_threshold,
            since_last: AtomicU64::new(0),
        }
    }

    /// 判断是否应发事件。若发，记录时间并重置计数。
    fn should_emit(&self, force: bool) -> bool {
        let mut last = self.last_emit.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let elapsed_ok = last
            .map(|t| now.duration_since(t) >= self.window)
            .unwrap_or(true);
        let count = self.since_last.load(Ordering::Relaxed);
        let batch_ok = count >= self.batch_threshold;
        if force || elapsed_ok || batch_ok {
            *last = Some(now);
            self.since_last.store(0, Ordering::Relaxed);
            true
        } else {
            self.since_last.store(count + 1, Ordering::Relaxed);
            false
        }
    }
}

/// 后台任务使用的观察者：持有取消标志 + sink + 限频器 + 任务元数据。
/// sequence 分配与 sink 投递在同一 Mutex 内完成，保证同一 operation 内严格递增投递。
struct TaskObserver {
    operation_id: String,
    task_id: String,
    cancel: Arc<AtomicBool>,
    sink: Arc<dyn ScanEventSink>,
    throttle: Throttle,
    /// sequence 分配 + sink 投递的串行化锁（同 operation 内严格递增投递）。
    emit_lock: Mutex<u32>,
}

impl TaskObserver {
    fn emit(&self, kind: ScanPhase, progress: ScanProgress, message: Option<String>) {
        let mut seq = self.emit_lock.lock().unwrap_or_else(|e| e.into_inner());
        *seq += 1;
        let event = LibraryScanEvent {
            operation_id: self.operation_id.clone(),
            sequence: *seq,
            at: utc_millis_to_rfc3339(UtcMillis::now()),
            kind,
            data: ScanEventData {
                task_id: self.task_id.clone(),
                files_seen: progress.files_seen,
                recognized: progress.recognized,
                new: progress.new,
                updated: progress.updated,
                skipped: progress.skipped,
                errors: progress.errors,
                current_item: progress.current_item,
                message,
            },
        };
        self.sink.emit_scan_event(event);
    }

    fn emit_library_changed(&self, revision: Option<String>) {
        let mut seq = self.emit_lock.lock().unwrap_or_else(|e| e.into_inner());
        *seq += 1;
        let event = LibraryChangedDto {
            schema_version: 1,
            at: utc_millis_to_rfc3339(UtcMillis::now()),
            operation_id: self.operation_id.clone(),
            sequence: *seq,
            revision,
        };
        self.sink.emit_library_changed(event);
    }
}

impl ScanObserver for TaskObserver {
    fn on_phase(&self, phase: ScanPhase) {
        // 阶段切换强制发（不限频）。
        self.throttle.should_emit(true);
        self.emit(phase, ScanProgress::default(), None);
    }

    fn on_progress(&self, progress: ScanProgress) {
        if self.throttle.should_emit(false) {
            self.emit(ScanPhase::ItemIndexed, progress, None);
        }
    }

    fn on_warning(&self, message: String, progress: ScanProgress) {
        // 警告强制发（单文件级失败需可见）。
        self.throttle.should_emit(true);
        self.emit(ScanPhase::Warning, progress, Some(message));
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
}

/// `Arc<TaskObserver>` 转发实现（scanner 接收 `&dyn ScanObserver`，调用方持 `Arc`）。
impl ScanObserver for Arc<TaskObserver> {
    fn on_phase(&self, phase: ScanPhase) {
        (**self).on_phase(phase)
    }
    fn on_progress(&self, progress: ScanProgress) {
        (**self).on_progress(progress)
    }
    fn on_warning(&self, message: String, progress: ScanProgress) {
        (**self).on_warning(message, progress)
    }
    fn is_cancelled(&self) -> bool {
        (**self).is_cancelled()
    }
}

/// TaskSupervisor：集中管理运行中任务 + 终态记录。
/// - 运行表按 storageLocationId 索引（R-C02 幂等：同位置只一个任务）。
/// - 终态记录按 taskId 索引（cancel 返回真实终态；容量上限淘汰最旧）。
struct TaskSupervisor {
    running: Mutex<HashMap<StorageLocationId, Arc<RunningTask>>>,
    terminal: Mutex<HashMap<String, TerminalRecord>>,
}

impl TaskSupervisor {
    fn new() -> Self {
        Self {
            running: Mutex::new(HashMap::new()),
            terminal: Mutex::new(HashMap::new()),
        }
    }

    /// 原子登记 Starting 占位：若该位置已有 Starting/Running 任务，返回既有任务
    /// （R-C02 幂等）；否则插入 Starting 占位并返回 None。
    /// 调用方在 get_scan_target 失败时必须调 `abort_starting` 清理占位。
    fn try_register(&self, storage_location_id: StorageLocationId) -> RegisterResult {
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(task) = running.get(&storage_location_id) {
            let state = *task.state.lock().unwrap_or_else(|e| e.into_inner());
            if state == TaskState::Starting || state == TaskState::Running {
                return RegisterResult::AlreadyRunning(
                    task.operation_id.clone(),
                    task.task_id.clone(),
                );
            }
            // 终态残留（理论上后台任务会移除，防御性清理）。
        }
        let operation_id = new_id("op");
        let task_id = new_id("task");
        let task = Arc::new(RunningTask {
            operation_id: operation_id.clone(),
            task_id: task_id.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
            state: Arc::new(Mutex::new(TaskState::Starting)),
        });
        running.insert(storage_location_id, task);
        RegisterResult::Registered(operation_id, task_id)
    }

    /// get_scan_target 失败时清理 Starting 占位。
    fn abort_starting(&self, storage_location_id: StorageLocationId) {
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(task) = running.get(&storage_location_id) {
            let state = *task.state.lock().unwrap_or_else(|e| e.into_inner());
            if state == TaskState::Starting {
                running.remove(&storage_location_id);
            }
        }
    }

    /// 取 Starting 占位的 task Arc（用于转 Running + spawn 后台）。
    fn get_task(&self, storage_location_id: StorageLocationId) -> Option<Arc<RunningTask>> {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&storage_location_id)
            .cloned()
    }

    /// 后台任务结束：转终态 + 移出运行表 + 记入终态历史。
    fn finalize(&self, storage_location_id: StorageLocationId, task_id: &str, phase: ScanPhase) {
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        // 条件删除：仅当当前运行任务的 task_id 匹配时才移除（防盲删仍在运行的新任务）。
        if let Some(task) = running.get(&storage_location_id) {
            if task.task_id == task_id {
                running.remove(&storage_location_id);
            }
        }
        let mut terminal = self.terminal.lock().unwrap_or_else(|e| e.into_inner());
        if terminal.len() >= TERMINAL_HISTORY_CAP {
            // 淘汰最旧（线性扫描；容量小，可接受）。
            if let Some(oldest) = terminal
                .iter()
                .min_by_key(|(_, r)| r.inserted_at)
                .map(|(k, _)| k.clone())
            {
                terminal.remove(&oldest);
            }
        }
        terminal.insert(
            task_id.to_string(),
            TerminalRecord {
                phase,
                inserted_at: Instant::now(),
            },
        );
    }

    /// cancel：运行中 → 设置取消标志；已结束 → 返回真实终态；未知 → None。
    fn cancel(&self, task_id: &str) -> Option<CancelOutcome> {
        let running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        for task in running.values() {
            if task.task_id == task_id {
                let state = *task.state.lock().unwrap_or_else(|e| e.into_inner());
                if state == TaskState::Starting || state == TaskState::Running {
                    task.cancel.store(true, Ordering::Release);
                    return Some(CancelOutcome::Cancelled);
                }
                // 终态但仍在运行表（finalize 未跑完）→ 返回对应终态。
                return Some(CancelOutcome::AlreadyTerminal(state_to_phase(state)));
            }
        }
        drop(running);
        let terminal = self.terminal.lock().unwrap_or_else(|e| e.into_inner());
        terminal
            .get(task_id)
            .map(|r| CancelOutcome::AlreadyTerminal(r.phase))
    }

    /// 该 storageLocationId 是否有运行/启动中任务（测试用）。
    fn is_running(&self, storage_location_id: StorageLocationId) -> bool {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&storage_location_id)
            .map(|t| {
                let s = *t.state.lock().unwrap_or_else(|e| e.into_inner());
                s == TaskState::Starting || s == TaskState::Running
            })
            .unwrap_or(false)
    }
}

enum RegisterResult {
    Registered(String, String),
    AlreadyRunning(String, String),
}

/// 扫描 Completed 后钩子类型（V2-F enrichment 入队）。
type CompletedHook =
    Arc<dyn Fn() -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// `library_scan_start` / `scan_cancel` 服务。
#[derive(Clone)]
pub struct ScanService {
    scanner: Arc<dyn LibraryScanner>,
    storage: StorageLocationService,
    sink: Arc<dyn ScanEventSink>,
    supervisor: Arc<TaskSupervisor>,
    throttle_window: Duration,
    throttle_batch: u64,
    /// 扫描 Completed 后的钩子（V2-F enrichment 入队）；None 表示无后续动作。
    on_completed: Option<CompletedHook>,
}

impl ScanService {
    pub fn new(
        scanner: Arc<dyn LibraryScanner>,
        storage: StorageLocationService,
        sink: Arc<dyn ScanEventSink>,
    ) -> Self {
        Self {
            scanner,
            storage,
            sink,
            supervisor: Arc::new(TaskSupervisor::new()),
            throttle_window: Duration::from_millis(200),
            throttle_batch: 50,
            on_completed: None,
        }
    }

    /// 测试/调优构造：注入限频参数（生产用 `new` 的默认值）。
    pub fn with_throttle(
        scanner: Arc<dyn LibraryScanner>,
        storage: StorageLocationService,
        sink: Arc<dyn ScanEventSink>,
        window: Duration,
        batch: u64,
    ) -> Self {
        Self {
            scanner,
            storage,
            sink,
            supervisor: Arc::new(TaskSupervisor::new()),
            throttle_window: window,
            throttle_batch: batch,
            on_completed: None,
        }
    }

    /// 扫描 Completed 后的钩子（V2-F enrichment）。重复调用覆盖。
    pub fn set_on_completed<F>(&mut self, hook: F)
    where
        F: Fn() -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync + 'static,
    {
        self.on_completed = Some(Arc::new(hook));
    }

    /// `library_scan_start`：登记任务 + 立即返回。扫描在后台跑。
    pub async fn start(
        &self,
        storage_location_id: StorageLocationId,
    ) -> Result<ScanStartResult, AppError> {
        // R-C02 原子登记：Starting 占位 + 单一临界区，避免并发竞态。
        let (operation_id, task_id, already_running) =
            match self.supervisor.try_register(storage_location_id) {
                RegisterResult::AlreadyRunning(op, task) => (op, task, true),
                RegisterResult::Registered(op, task) => (op, task, false),
            };

        if already_running {
            return Ok(ScanStartResult {
                schema_version: 1,
                operation_id,
                task_id,
                already_running: true,
            });
        }

        // 获取扫描目标（校验 storageLocationId 存在 + Connected + 路径可达）。
        // 失败映射为契约错误码并清理 Starting 占位。
        let target = match self.storage.get_scan_target(storage_location_id).await {
            Ok(t) => t,
            Err(e) => {
                self.supervisor.abort_starting(storage_location_id);
                return Err(e);
            }
        };

        // 转 Running + 取 task Arc。占位在当前调用图下不可能消失（只有 owner 的
        // abort_starting 会删 Starting；finalize 按 taskId 条件删除），但这是
        // async 命令路径——以稳定错误码兜底而非 panic（复审备注 #2）。
        let task = match self.supervisor.get_task(storage_location_id) {
            Some(task) => task,
            None => {
                // retryable 与冻结错误目录对齐（catalog.json：INTERNAL_ERROR → false；
                // 原实现 true 与目录漂移，前端会按可重试渲染重试 UI）。
                return Err(AppError::new(
                    "INTERNAL_ERROR",
                    ErrorKind::Internal,
                    "扫描任务登记意外丢失，请重新发起扫描",
                    false,
                ));
            }
        };
        *task.state.lock().unwrap_or_else(|e| e.into_inner()) = TaskState::Running;

        let observer = Arc::new(TaskObserver {
            operation_id: operation_id.clone(),
            task_id: task_id.clone(),
            cancel: task.cancel.clone(),
            sink: self.sink.clone(),
            throttle: Throttle::new(self.throttle_window, self.throttle_batch),
            emit_lock: Mutex::new(0),
        });

        // 发 started 事件（强制发，不限频）。
        observer.on_phase(ScanPhase::Started);

        let scanner = self.scanner.clone();
        let supervisor = self.supervisor.clone();
        let cancel = task.cancel.clone();
        let state = task.state.clone();
        let task_id_clone = task_id.clone();
        let on_completed = self.on_completed.clone();

        // 后台执行扫描。用内层 tokio::spawn 包裹 scanner 调用，外层任务 await 其
        // JoinHandle：scanner/sink panic 时 JoinHandle 返回 Err(JoinError)，外层
        // 据此转 Failed 并保证终态事件 + library.changed + 终态登记都执行（统一出口）。
        tokio::spawn(async move {
            let inner = tokio::spawn(run_scan_inner(
                scanner,
                target,
                observer.clone(),
                cancel.clone(),
            ));
            let outcome = match inner.await {
                Ok(scan_outcome) => scan_outcome,
                Err(_join_err) => ScanOutcome::Panicked,
            };
            // 终态裁决：取消优先于失败（取消时 scanner 应返回 Ok，但防御性处理）。
            let (phase, final_progress, message) = match outcome {
                ScanOutcome::Done(report) => {
                    if cancel.load(Ordering::Acquire) {
                        (ScanPhase::Cancelled, report, None)
                    } else {
                        (ScanPhase::Completed, report, None)
                    }
                }
                ScanOutcome::Cancelled(report) => (ScanPhase::Cancelled, report, None),
                ScanOutcome::Failed(report, err) => {
                    if cancel.load(Ordering::Acquire) {
                        (ScanPhase::Cancelled, report, None)
                    } else {
                        let msg = err.code().as_str().to_string();
                        (ScanPhase::Failed, report, Some(msg))
                    }
                }
                ScanOutcome::Panicked => (
                    ScanPhase::Failed,
                    ScanReport::default(),
                    Some("scan task panicked".to_string()),
                ),
            };
            *state.lock().unwrap_or_else(|e| e.into_inner()) = phase_to_state(phase);

            // 终态事件只发一次（统一出口，无重复）。
            if phase == ScanPhase::Failed {
                observer.emit(phase, final_progress.into(), message);
            } else {
                observer.emit(phase, final_progress.into(), None);
            }

            // 终态后发一次 library.changed（带完整 envelope）。
            observer.emit_library_changed(None);

            // Completed 后触发 enrichment 流水线（契约 §36.8）；失败只记日志，
            // 不影响扫描终态（匹配失败不回滚扫描）。
            if phase == ScanPhase::Completed {
                if let Some(hook) = on_completed.as_ref() {
                    hook().await;
                }
            }

            // 终态登记 + 移出运行表。
            supervisor.finalize(storage_location_id, &task_id_clone, phase);
        });

        Ok(ScanStartResult {
            schema_version: 1,
            operation_id,
            task_id,
            already_running: false,
        })
    }

    /// `scan_cancel`：协作式取消。幂等——运行中 → Cancelled；已结束 → 真实终态；
    /// 未知 taskId → `RESOURCE_NOT_FOUND`（不伪造 Completed）。
    pub fn cancel(&self, task_id: &str) -> Result<CancelOutcome, AppError> {
        match self.supervisor.cancel(task_id) {
            Some(outcome) => Ok(outcome),
            None => Err(AppError::new(
                "RESOURCE_NOT_FOUND",
                ErrorKind::NotFound,
                format!("扫描任务不存在: {task_id}"),
                false,
            )),
        }
    }

    /// 该 storageLocationId 是否有运行中任务（测试与可观测性用）。
    pub fn is_running(&self, storage_location_id: StorageLocationId) -> bool {
        self.supervisor.is_running(storage_location_id)
    }
}

/// 扫描执行结果（区分取消/失败/panic，供终态裁决）。
enum ScanOutcome {
    Done(ScanReport),
    Cancelled(ScanReport),
    Failed(ScanReport, AppError),
    Panicked,
}

/// 内层扫描 future（由 `tokio::spawn` 驱动，panic 由外层 JoinHandle 捕获）。
/// 正常路径：scanner 返回 Ok → 据取消标志判 Done/Cancelled；返回 Err → Failed。
async fn run_scan_inner(
    scanner: Arc<dyn LibraryScanner>,
    target: ScanTarget,
    observer: Arc<TaskObserver>,
    cancel: Arc<AtomicBool>,
) -> ScanOutcome {
    match scanner.scan(&target, &observer as &dyn ScanObserver).await {
        Ok(report) => {
            if cancel.load(Ordering::Acquire) {
                ScanOutcome::Cancelled(report)
            } else {
                ScanOutcome::Done(report)
            }
        }
        Err(e) => ScanOutcome::Failed(ScanReport::default(), e),
    }
}

fn phase_to_state(phase: ScanPhase) -> TaskState {
    match phase {
        ScanPhase::Started
        | ScanPhase::Enumerating
        | ScanPhase::Detecting
        | ScanPhase::Fingerprinting
        | ScanPhase::Indexing
        | ScanPhase::ItemIndexed
        | ScanPhase::Warning => TaskState::Running,
        ScanPhase::Completed => TaskState::Completed,
        ScanPhase::Cancelled => TaskState::Cancelled,
        ScanPhase::Failed => TaskState::Failed,
    }
}

/// `TaskState` → 契约 `ScanPhase`。**仅终态三值可达**（复审裁决）：调用方
/// `TaskSupervisor::cancel` 对 Starting/Running 走取消标志分支，只有
/// Completed/Cancelled/Failed 会落到本函数；前两个映射是纯防御性兜底。
fn state_to_phase(state: TaskState) -> ScanPhase {
    match state {
        TaskState::Starting => ScanPhase::Started,
        TaskState::Running => ScanPhase::Indexing,
        TaskState::Completed => ScanPhase::Completed,
        TaskState::Cancelled => ScanPhase::Cancelled,
        TaskState::Failed => ScanPhase::Failed,
    }
}

impl From<ScanReport> for ScanProgress {
    fn from(r: ScanReport) -> Self {
        ScanProgress {
            files_seen: r.files_seen,
            recognized: r.recognized,
            new: r.new,
            updated: r.updated,
            skipped: r.skipped,
            errors: r.errors,
            current_item: None,
        }
    }
}

/// 生成 opaque id（时间戳 + 纳秒后缀，保证唯一性）。
fn new_id(prefix: &str) -> String {
    format!(
        "{}-{:016x}-{:x}",
        prefix,
        UtcMillis::now().0 as u64,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0)
    )
}
