//! ScanService 集成测试（BE-SCAN-001，R2 复审修复后）。
//!
//! 放在 tests/ 而非 src/ 内：避免 haven-application（本 crate）与
//! haven-infrastructure（dev-dep）引入的循环依赖导致的 trait 版本冲突。
//! 与 storage_location_integration.rs 同模式。
//!
//! 覆盖复审要求的危险路径：并发 start 竞态、默认限频、取消+错误终态唯一性、
//! 未知 taskId、阶段/警告事件、scanner panic、sequence 投递顺序。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use haven_application::services::scan::{
    CancelOutcome, LibraryScanner, ScanEventSink, ScanObserver, ScanProgress, ScanReport,
    ScanService,
};
use haven_application::services::storage_location::{ScanTarget, StorageLocationService};
use haven_application::wire::{LibraryChangedDto, LibraryScanEvent, ScanPhase};
use haven_common::AppError;
use haven_domain::contracts::WorkRepository;
use haven_domain::ids::StorageLocationId;
use haven_infrastructure::Db;
use haven_infrastructure::db::repos::SqliteRepositories;
use haven_infrastructure::db::uow::SqliteStorageUoW;
use haven_infrastructure::scanner::LocalLibraryScanner;

/// 内存扫描器：可配置返回 report / 是否失败 / 是否 panic / 每文件回调。
struct MockScanner {
    report: ScanReport,
    fail: bool,
    panic: bool,
    calls: Mutex<Vec<StorageLocationId>>,
    observer_saw_cancel: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl LibraryScanner for MockScanner {
    async fn scan(
        &self,
        target: &ScanTarget,
        observer: &dyn ScanObserver,
    ) -> Result<ScanReport, AppError> {
        self.calls
            .lock()
            .unwrap()
            .push(target.storage_location_id());
        if self.panic {
            panic!("scanner panic 注入");
        }
        for i in 1..=5 {
            if observer.is_cancelled() {
                self.observer_saw_cancel.store(true, Ordering::SeqCst);
                return Ok(self.report);
            }
            observer.on_progress(ScanProgress {
                files_seen: i,
                recognized: i,
                new: i,
                current_item: Some(format!("file-{i}.mkv")),
                ..Default::default()
            });
        }
        if self.fail {
            return Err(AppError::new(
                "DATABASE_ERROR",
                haven_common::ErrorKind::Database,
                "扫描失败",
                true,
            ));
        }
        Ok(self.report)
    }
}

/// 内存 Sink：记录所有发出的事件。
struct MockSink {
    scan_events: Mutex<Vec<LibraryScanEvent>>,
    library_changed: Mutex<Vec<LibraryChangedDto>>,
}

impl ScanEventSink for MockSink {
    fn emit_scan_event(&self, event: LibraryScanEvent) {
        self.scan_events.lock().unwrap().push(event);
    }
    fn emit_library_changed(&self, event: LibraryChangedDto) {
        self.library_changed.lock().unwrap().push(event);
    }
}

/// 真实扫描中 remove 的确定性栅栏：首条 ItemIndexed 已在事务内落库后暂停事件投递，
/// 让测试在下一文件开始前完成 remove，再放行扫描器验证 token guard。
///
/// 这避免以大量文件和时间窗口赌调度顺序；生产 sink 不具备此阻塞行为。
struct RemoveDuringScanSink {
    scan_events: Mutex<Vec<LibraryScanEvent>>,
    library_changed: Mutex<Vec<LibraryChangedDto>>,
    first_item: Mutex<Option<mpsc::Sender<()>>>,
    resume: Mutex<Option<mpsc::Receiver<()>>>,
}

impl ScanEventSink for RemoveDuringScanSink {
    fn emit_scan_event(&self, event: LibraryScanEvent) {
        if event.kind == ScanPhase::ItemIndexed {
            if let Some(reached) = self
                .first_item
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                reached.send(()).expect("测试必须仍在等待首条索引事件");
                let resume = self
                    .resume
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take()
                    .expect("首条索引事件只能阻塞一次");
                resume
                    .recv_timeout(Duration::from_secs(5))
                    .expect("remove 完成后测试必须放行扫描器");
            }
        }
        self.scan_events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(event);
    }

    fn emit_library_changed(&self, event: LibraryChangedDto) {
        self.library_changed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(event);
    }
}

async fn make_service(
    scanner: Arc<dyn LibraryScanner>,
    sink: Arc<dyn ScanEventSink>,
    root: &std::path::Path,
) -> (ScanService, StorageLocationId) {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let storage = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db)));
    let id = storage.add_local("测试库".into(), root).await.unwrap();
    let svc = ScanService::with_throttle(
        scanner,
        storage,
        sink,
        Duration::from_millis(0), // 测试不限频
        1,
    );
    (svc, id)
}

fn mock_scanner(report: ScanReport) -> MockScanner {
    MockScanner {
        report,
        fail: false,
        panic: false,
        calls: Mutex::new(vec![]),
        observer_saw_cancel: Arc::new(AtomicBool::new(false)),
    }
}

#[tokio::test]
async fn start_runs_scan_and_emits_terminal_events() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.mkv"), b"x").unwrap();

    let scanner = Arc::new(mock_scanner(ScanReport {
        files_seen: 5,
        recognized: 5,
        new: 5,
        ..Default::default()
    }));
    let sink = Arc::new(MockSink {
        scan_events: Mutex::new(vec![]),
        library_changed: Mutex::new(vec![]),
    });
    let (svc, id) = make_service(scanner.clone(), sink.clone(), dir.path()).await;

    let result = svc.start(id).await.unwrap();
    assert!(!result.already_running, "首次启动 alreadyRunning=false");
    assert_eq!(result.schema_version, 1);

    wait_for_completion(&svc, id).await;

    let events = sink.scan_events.lock().unwrap();
    let kinds: Vec<_> = events.iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&ScanPhase::Started), "必须有 Started 事件");
    assert!(
        kinds.contains(&ScanPhase::Completed),
        "必须有 Completed 事件: {kinds:?}"
    );
    // 终态事件只出现一次。
    assert_eq!(
        kinds.iter().filter(|k| **k == ScanPhase::Completed).count(),
        1,
        "Completed 只能出现一次"
    );
    assert_eq!(
        sink.library_changed.lock().unwrap().len(),
        1,
        "终态后发一次 library.changed"
    );
    assert!(!scanner.calls.lock().unwrap().is_empty(), "扫描器被调用");
}

#[tokio::test]
async fn duplicate_start_is_idempotent_already_running() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.mkv"), b"x").unwrap();

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let scanner = Arc::new(BlockingScanner {
        report: ScanReport::default(),
        rx: Mutex::new(Some(rx)),
    });
    let sink = Arc::new(MockSink {
        scan_events: Mutex::new(vec![]),
        library_changed: Mutex::new(vec![]),
    });
    let (svc, id) = make_service(scanner.clone(), sink.clone(), dir.path()).await;

    let first = svc.start(id).await.unwrap();
    let second = svc.start(id).await.unwrap();
    assert!(second.already_running, "重复启动必须 alreadyRunning=true");
    assert_eq!(second.operation_id, first.operation_id);
    assert_eq!(second.task_id, first.task_id);

    let _ = tx.send(());
    wait_for_completion(&svc, id).await;
}

struct BlockingScanner {
    report: ScanReport,
    rx: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}
#[async_trait::async_trait]
impl LibraryScanner for BlockingScanner {
    async fn scan(
        &self,
        _target: &ScanTarget,
        _observer: &dyn ScanObserver,
    ) -> Result<ScanReport, AppError> {
        let rx = self.rx.lock().unwrap().take();
        if let Some(rx) = rx {
            let _ = rx.await;
        }
        Ok(self.report)
    }
}

/// 并发 start：两个 start 同时发起，必须只有一个真正建任务（R-C02 原子性）。
#[tokio::test]
async fn concurrent_start_does_not_create_duplicate_tasks() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.mkv"), b"x").unwrap();

    // release gate：scanner 在第一个文件阻塞，直到测试放行，保证两个 start 真正并发。
    let release = Arc::new(tokio::sync::Notify::new());
    let scanner = Arc::new(GatedScanner {
        report: ScanReport {
            files_seen: 1,
            ..Default::default()
        },
        release: release.clone(),
        calls: Mutex::new(vec![]),
        observer_saw_cancel: Arc::new(AtomicBool::new(false)),
    });
    let sink = Arc::new(MockSink {
        scan_events: Mutex::new(vec![]),
        library_changed: Mutex::new(vec![]),
    });
    let (svc, id) = make_service(scanner.clone(), sink.clone(), dir.path()).await;

    // 两个 start 并发：第一个建任务，第二个应 alreadyRunning。
    let (r1, r2) = tokio::join!(svc.start(id), svc.start(id));
    let r1 = r1.unwrap();
    let r2 = r2.unwrap();
    let starts = r1.already_running as u8 + r2.already_running as u8;
    assert_eq!(
        starts, 1,
        "并发 start 必须恰好一个新建、一个合并: r1={r1:?} r2={r2:?}"
    );
    assert_eq!(r1.operation_id, r2.operation_id, "合并到同一 operationId");
    assert_eq!(r1.task_id, r2.task_id, "合并到同一 taskId");

    // 放行 scanner，让后台任务结束。
    release.notify_one();
    wait_for_completion(&svc, id).await;

    assert_eq!(
        scanner.calls.lock().unwrap().len(),
        1,
        "并发 start 不得创建重复扫描任务"
    );
}

/// 在第一个文件阻塞直到 release 被通知的 scanner。
struct GatedScanner {
    report: ScanReport,
    release: Arc<tokio::sync::Notify>,
    calls: Mutex<Vec<StorageLocationId>>,
    observer_saw_cancel: Arc<AtomicBool>,
}
#[async_trait::async_trait]
impl LibraryScanner for GatedScanner {
    async fn scan(
        &self,
        target: &ScanTarget,
        observer: &dyn ScanObserver,
    ) -> Result<ScanReport, AppError> {
        self.calls
            .lock()
            .unwrap()
            .push(target.storage_location_id());
        // 第一个文件前阻塞，等测试放行。
        self.release.notified().await;
        if observer.is_cancelled() {
            self.observer_saw_cancel.store(true, Ordering::SeqCst);
            return Ok(self.report);
        }
        observer.on_progress(ScanProgress {
            files_seen: 1,
            recognized: 1,
            new: 1,
            current_item: Some("file-1.mkv".into()),
            ..Default::default()
        });
        Ok(self.report)
    }
}

#[tokio::test]
async fn cancel_is_cooperative_and_returns_real_terminal() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.mkv"), b"x").unwrap();

    let scanner = Arc::new(MockScanner {
        report: ScanReport {
            files_seen: 3,
            ..Default::default()
        },
        fail: false,
        panic: false,
        calls: Mutex::new(vec![]),
        observer_saw_cancel: Arc::new(AtomicBool::new(false)),
    });
    let sink = Arc::new(MockSink {
        scan_events: Mutex::new(vec![]),
        library_changed: Mutex::new(vec![]),
    });
    let (svc, id) = make_service(scanner.clone(), sink.clone(), dir.path()).await;

    let result = svc.start(id).await.unwrap();
    let outcome = svc.cancel(&result.task_id).unwrap();
    assert_eq!(outcome, CancelOutcome::Cancelled);

    wait_for_completion(&svc, id).await;

    assert!(
        scanner.observer_saw_cancel.load(Ordering::SeqCst),
        "扫描器必须观察到取消标志"
    );
    let events = sink.scan_events.lock().unwrap();
    let kinds: Vec<_> = events.iter().map(|e| e.kind).collect();
    assert!(
        kinds.contains(&ScanPhase::Cancelled),
        "必须有 Cancelled 事件: {kinds:?}"
    );
    // Cancelled 只出现一次。
    assert_eq!(
        kinds.iter().filter(|k| **k == ScanPhase::Cancelled).count(),
        1,
        "Cancelled 只能出现一次"
    );

    // 幂等：已结束任务再 cancel 返回真实终态 Cancelled（非 Completed）。
    let again = svc.cancel(&result.task_id).unwrap();
    assert_eq!(
        again,
        CancelOutcome::AlreadyTerminal(ScanPhase::Cancelled),
        "已取消任务 cancel 必须返回真实终态 Cancelled"
    );
}

#[tokio::test]
async fn scan_failure_returns_failed_terminal() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.mkv"), b"x").unwrap();

    let scanner = Arc::new(MockScanner {
        report: ScanReport::default(),
        fail: true,
        panic: false,
        calls: Mutex::new(vec![]),
        observer_saw_cancel: Arc::new(AtomicBool::new(false)),
    });
    let sink = Arc::new(MockSink {
        scan_events: Mutex::new(vec![]),
        library_changed: Mutex::new(vec![]),
    });
    let (svc, id) = make_service(scanner.clone(), sink.clone(), dir.path()).await;

    let result = svc.start(id).await.unwrap();
    wait_for_completion(&svc, id).await;

    let events = sink.scan_events.lock().unwrap();
    let kinds: Vec<_> = events.iter().map(|e| e.kind).collect();
    assert!(
        kinds.contains(&ScanPhase::Failed),
        "扫描失败必须发 Failed 事件: {kinds:?}"
    );
    assert_eq!(
        kinds.iter().filter(|k| **k == ScanPhase::Failed).count(),
        1,
        "Failed 只能出现一次"
    );

    // cancel 已失败任务返回真实终态 Failed。
    let outcome = svc.cancel(&result.task_id).unwrap();
    assert_eq!(
        outcome,
        CancelOutcome::AlreadyTerminal(ScanPhase::Failed),
        "已失败任务 cancel 必须返回真实终态 Failed"
    );
}

/// 取消 + 失败同时发生：终态唯一（取消优先），不双发 Failed+Cancelled。
#[tokio::test]
async fn cancel_and_failure_simultaneous_emits_single_terminal() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.mkv"), b"x").unwrap();

    let scanner = Arc::new(MockScanner {
        report: ScanReport::default(),
        fail: true, // scanner 返回 Err
        panic: false,
        calls: Mutex::new(vec![]),
        observer_saw_cancel: Arc::new(AtomicBool::new(false)),
    });
    let sink = Arc::new(MockSink {
        scan_events: Mutex::new(vec![]),
        library_changed: Mutex::new(vec![]),
    });
    let (svc, id) = make_service(scanner.clone(), sink.clone(), dir.path()).await;

    let result = svc.start(id).await.unwrap();
    // 立即取消（scanner 会返回 Err，但取消标志已设）。
    svc.cancel(&result.task_id).unwrap();
    wait_for_completion(&svc, id).await;

    let events = sink.scan_events.lock().unwrap();
    let kinds: Vec<_> = events.iter().map(|e| e.kind).collect();
    let terminal_count = kinds
        .iter()
        .filter(|k| {
            matches!(
                **k,
                ScanPhase::Completed | ScanPhase::Cancelled | ScanPhase::Failed
            )
        })
        .count();
    assert_eq!(terminal_count, 1, "终态事件只能出现一次: {kinds:?}");
    // 取消优先于失败。
    assert!(
        kinds.contains(&ScanPhase::Cancelled),
        "取消+失败同时发生时终态应为 Cancelled: {kinds:?}"
    );
    assert!(
        !kinds.contains(&ScanPhase::Failed),
        "取消优先时不得发 Failed: {kinds:?}"
    );
}

#[tokio::test]
async fn unknown_storage_location_returns_resource_not_found() {
    let dir = tempfile::TempDir::new().unwrap();
    let scanner = Arc::new(mock_scanner(ScanReport::default()));
    let sink = Arc::new(MockSink {
        scan_events: Mutex::new(vec![]),
        library_changed: Mutex::new(vec![]),
    });
    let (svc, _id) = make_service(scanner.clone(), sink.clone(), dir.path()).await;

    let err = svc.start(StorageLocationId::new()).await.unwrap_err();
    assert_eq!(err.code().as_str(), "RESOURCE_NOT_FOUND");
}

/// 未知 taskId cancel → RESOURCE_NOT_FOUND（不伪造 Completed）。
#[tokio::test]
async fn cancel_unknown_task_returns_resource_not_found() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.mkv"), b"x").unwrap();
    let scanner = Arc::new(mock_scanner(ScanReport::default()));
    let sink = Arc::new(MockSink {
        scan_events: Mutex::new(vec![]),
        library_changed: Mutex::new(vec![]),
    });
    let (svc, _id) = make_service(scanner.clone(), sink.clone(), dir.path()).await;

    let err = svc.cancel("nonexistent-task-id").unwrap_err();
    assert_eq!(
        err.code().as_str(),
        "RESOURCE_NOT_FOUND",
        "未知 taskId 必须返回 RESOURCE_NOT_FOUND，不得伪造 Completed"
    );
}

#[tokio::test]
async fn events_have_monotonic_sequence_per_operation() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.mkv"), b"x").unwrap();

    let scanner = Arc::new(mock_scanner(ScanReport {
        files_seen: 5,
        recognized: 5,
        new: 5,
        ..Default::default()
    }));
    let sink = Arc::new(MockSink {
        scan_events: Mutex::new(vec![]),
        library_changed: Mutex::new(vec![]),
    });
    let (svc, id) = make_service(scanner.clone(), sink.clone(), dir.path()).await;

    svc.start(id).await.unwrap();
    wait_for_completion(&svc, id).await;

    let events = sink.scan_events.lock().unwrap();
    let seqs: Vec<u32> = events.iter().map(|e| e.sequence).collect();
    let mut sorted = seqs.clone();
    sorted.sort();
    assert_eq!(seqs, sorted, "sequence 必须单调递增");
    assert!(seqs.iter().all(|s| *s >= 1), "sequence 从 1 开始");
    assert!(
        events
            .iter()
            .all(|e| e.operation_id == events[0].operation_id),
        "同一任务事件 operationId 一致"
    );
    // library.changed 也在同一 operation 内递增（sequence 延续）。
    let lib_changed = sink.library_changed.lock().unwrap();
    assert!(!lib_changed.is_empty(), "必须有 library.changed");
    assert!(
        lib_changed
            .iter()
            .all(|e| e.operation_id == events[0].operation_id),
        "library.changed operationId 与 scan 事件同源"
    );
}

/// scanner panic → 终态 Failed + library.changed + 任务移出运行表（不永久 Running）。
#[tokio::test]
async fn scanner_panic_emits_failed_and_cleans_up() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.mkv"), b"x").unwrap();

    let scanner = Arc::new(MockScanner {
        report: ScanReport::default(),
        fail: false,
        panic: true,
        calls: Mutex::new(vec![]),
        observer_saw_cancel: Arc::new(AtomicBool::new(false)),
    });
    let sink = Arc::new(MockSink {
        scan_events: Mutex::new(vec![]),
        library_changed: Mutex::new(vec![]),
    });
    let (svc, id) = make_service(scanner.clone(), sink.clone(), dir.path()).await;

    let result = svc.start(id).await.unwrap();
    wait_for_completion(&svc, id).await;

    let events = sink.scan_events.lock().unwrap();
    let kinds: Vec<_> = events.iter().map(|e| e.kind).collect();
    assert!(
        kinds.contains(&ScanPhase::Failed),
        "scanner panic 必须发 Failed 事件: {kinds:?}"
    );
    assert_eq!(
        sink.library_changed.lock().unwrap().len(),
        1,
        "panic 后也发一次 library.changed"
    );
    // 任务已移出运行表（不永久 Running）。
    assert!(!svc.is_running(id), "panic 后任务不得永久停留 Running");
    // cancel 返回真实终态 Failed。
    let outcome = svc.cancel(&result.task_id).unwrap();
    assert_eq!(
        outcome,
        CancelOutcome::AlreadyTerminal(ScanPhase::Failed),
        "panic 后 cancel 返回真实终态 Failed"
    );
}

/// 默认限频参数（200ms + 50 批次）：5 个文件 + 1 起始 + 1 终态，进度事件被合并。
#[tokio::test]
async fn default_throttle_merges_progress_events() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.mkv"), b"x").unwrap();

    let scanner = Arc::new(mock_scanner(ScanReport {
        files_seen: 5,
        recognized: 5,
        new: 5,
        ..Default::default()
    }));
    let sink = Arc::new(MockSink {
        scan_events: Mutex::new(vec![]),
        library_changed: Mutex::new(vec![]),
    });
    // 用默认限频（200ms + 50 批次）。
    let db = Arc::new(Db::open_in_memory().unwrap());
    let storage = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db)));
    let id = storage
        .add_local("测试库".into(), dir.path())
        .await
        .unwrap();
    let svc = ScanService::new(scanner.clone(), storage, sink.clone());

    svc.start(id).await.unwrap();
    wait_for_completion(&svc, id).await;

    let events = sink.scan_events.lock().unwrap();
    let item_indexed_count = events
        .iter()
        .filter(|e| e.kind == ScanPhase::ItemIndexed)
        .count();
    // 5 个文件，但限频（200ms 窗口 + 50 批次）下应远少于 5（快速完成时可能 0-1）。
    assert!(
        item_indexed_count < 5,
        "默认限频必须合并进度事件，实际 ItemIndexed 数: {item_indexed_count}"
    );
}

async fn wait_for_completion(svc: &ScanService, id: StorageLocationId) {
    for _ in 0..300 {
        if !svc.is_running(id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("扫描任务未在 3s 内完成");
}

/// 复审备注 #1：on_phase（Started 之外）与 on_warning 必须原样转发到事件流。
struct PhaseWarningScanner;

#[async_trait::async_trait]
impl LibraryScanner for PhaseWarningScanner {
    async fn scan(
        &self,
        _target: &ScanTarget,
        observer: &dyn ScanObserver,
    ) -> Result<ScanReport, AppError> {
        observer.on_phase(ScanPhase::Detecting);
        observer.on_warning(
            "跳过无法解析的文件".to_string(),
            ScanProgress {
                files_seen: 1,
                errors: 1,
                ..Default::default()
            },
        );
        observer.on_progress(ScanProgress {
            files_seen: 2,
            recognized: 1,
            new: 1,
            current_item: Some("ok.mkv".to_string()),
            ..Default::default()
        });
        Ok(ScanReport {
            files_seen: 2,
            recognized: 1,
            new: 1,
            errors: 1,
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn phase_and_warning_events_are_forwarded() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.mkv"), b"x").unwrap();

    let sink = Arc::new(MockSink {
        scan_events: Mutex::new(vec![]),
        library_changed: Mutex::new(vec![]),
    });
    let db = Arc::new(Db::open_in_memory().unwrap());
    let storage = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db)));
    let id = storage
        .add_local("测试库".into(), dir.path())
        .await
        .unwrap();
    let svc = ScanService::with_throttle(
        Arc::new(PhaseWarningScanner),
        storage,
        sink.clone(),
        Duration::from_millis(0),
        1,
    );

    svc.start(id).await.unwrap();
    wait_for_completion(&svc, id).await;

    let events = sink.scan_events.lock().unwrap();
    let kinds: Vec<_> = events.iter().map(|e| e.kind).collect();
    assert!(
        kinds.contains(&ScanPhase::Detecting),
        "on_phase(Detecting) 必须转发: {kinds:?}"
    );
    let warning = events
        .iter()
        .find(|e| e.kind == ScanPhase::Warning)
        .expect("on_warning 必须转发为 Warning 事件");
    assert_eq!(
        warning.data.message.as_deref(),
        Some("跳过无法解析的文件"),
        "Warning 事件必须携带 message"
    );
    assert!(
        kinds.contains(&ScanPhase::ItemIndexed),
        "on_progress 必须转发为 ItemIndexed: {kinds:?}"
    );
}

/// 复审备注 #1/#5：真实 `LocalLibraryScanner` 经端口扫描临时目录——
/// Enumerating/Indexing 阶段事件、ItemIndexed 只携带文件名（不泄漏本地路径）、
/// 终态 Completed 唯一，且已完成后 cancel 返回真实终态 Completed。
#[tokio::test]
async fn local_scanner_port_scans_real_files() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.mkv"), b"video-bytes").unwrap();

    let sink = Arc::new(MockSink {
        scan_events: Mutex::new(vec![]),
        library_changed: Mutex::new(vec![]),
    });
    // 扫描器与 StorageLocationService 共享同一 Db（扫描写入 Work/Edition/...）。
    let db = Arc::new(Db::open_in_memory().unwrap());
    let storage = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let id = storage
        .add_local("测试库".into(), dir.path())
        .await
        .unwrap();
    let scanner: Arc<dyn LibraryScanner> = Arc::new(LocalLibraryScanner::new(db));
    let svc =
        ScanService::with_throttle(scanner, storage, sink.clone(), Duration::from_millis(0), 1);

    let result = svc.start(id).await.unwrap();
    wait_for_completion(&svc, id).await;

    let events = sink.scan_events.lock().unwrap();
    let kinds: Vec<_> = events.iter().map(|e| e.kind).collect();
    assert!(
        kinds.contains(&ScanPhase::Enumerating),
        "真实扫描器必须发 Enumerating: {kinds:?}"
    );
    assert!(
        kinds.contains(&ScanPhase::Indexing),
        "真实扫描器入库后必须发 Indexing: {kinds:?}"
    );
    assert_eq!(
        kinds.iter().filter(|k| **k == ScanPhase::Completed).count(),
        1,
        "Completed 只能出现一次: {kinds:?}"
    );
    // current_item 只携带文件名——本地完整路径不得进入事件流（C-06 安全边界）。
    let item = events
        .iter()
        .find(|e| e.kind == ScanPhase::ItemIndexed)
        .expect("必须至少有一条 ItemIndexed 进度事件");
    assert_eq!(item.data.current_item.as_deref(), Some("a.mkv"));
    assert!(
        !item
            .data
            .current_item
            .as_deref()
            .unwrap_or_default()
            .contains('\\'),
        "current_item 不得包含路径分隔符"
    );

    // 复审备注 #5：Completed 终态后 cancel 返回真实终态 Completed。
    let outcome = svc.cancel(&result.task_id).unwrap();
    assert_eq!(
        outcome,
        CancelOutcome::AlreadyTerminal(ScanPhase::Completed)
    );
}

/// VERIFY-SLICE-001 曾用临时探针覆盖本场景后删除。这里固化为正式回归：
/// 扫描已提交第一条内容时移除同一位置，后续写事务必须被 token guard 拦为 stale，
/// purge 必须不留任何内容链孤儿。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_location_during_real_scan_fails_stale_and_purges_content() {
    let dir = tempfile::TempDir::new().unwrap();
    // 两个可识别文件：第一个提交并触发栅栏，第二个在 remove 后触发写事务 token guard。
    std::fs::write(dir.path().join("first.mkv"), b"first-video-bytes").unwrap();
    std::fs::write(dir.path().join("second.mkv"), b"second-video-bytes").unwrap();

    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = SqliteRepositories::new(db.clone());
    let storage = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let id = storage
        .add_local("测试库".into(), dir.path())
        .await
        .unwrap();
    let (first_item_tx, first_item_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let sink = Arc::new(RemoveDuringScanSink {
        scan_events: Mutex::new(vec![]),
        library_changed: Mutex::new(vec![]),
        first_item: Mutex::new(Some(first_item_tx)),
        resume: Mutex::new(Some(resume_rx)),
    });
    let scanner: Arc<dyn LibraryScanner> = Arc::new(LocalLibraryScanner::new(db.clone()));
    let scan = ScanService::with_throttle(
        scanner,
        storage.clone(),
        sink.clone(),
        Duration::from_millis(0),
        1,
    );

    let started = scan.start(id).await.unwrap();
    tokio::task::spawn_blocking(move || first_item_rx.recv_timeout(Duration::from_secs(5)))
        .await
        .expect("等待任务不得 panic")
        .expect("真实扫描必须提交首条内容并发出进度");
    assert!(scan.is_running(id), "remove 必须发生在扫描仍运行时");

    storage.remove(id).await.unwrap();
    resume_tx.send(()).unwrap();
    wait_for_completion(&scan, id).await;

    {
        let events = sink.scan_events.lock().unwrap();
        let terminal: Vec<_> = events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    ScanPhase::Completed | ScanPhase::Cancelled | ScanPhase::Failed
                )
            })
            .collect();
        assert_eq!(terminal.len(), 1, "终态事件必须恰好一次: {terminal:?}");
        assert_eq!(terminal[0].kind, ScanPhase::Failed);
        assert_eq!(
            terminal[0].data.message.as_deref(),
            Some("SCAN_TARGET_STALE"),
            "Failed 事件只携带稳定错误码，不携带根路径"
        );
    }
    assert_eq!(
        sink.library_changed.lock().unwrap().len(),
        1,
        "失败终态后 library.changed 必须恰好一次"
    );
    assert_eq!(
        scan.cancel(&started.task_id).unwrap(),
        CancelOutcome::AlreadyTerminal(ScanPhase::Failed),
        "终态后 cancel 必须返回真实 Failed"
    );

    assert!(storage.list().await.unwrap().is_empty(), "位置必须已删除");
    // 此处真实扫描只创建这条 work 内容链；work 能被删除意味着其下 edition、media_item
    // 与 resource 均已按 RESTRICT 外键顺序清理，不可能留下链上孤儿。
    assert!(
        repos.work.list(10, 0).await.unwrap().is_empty(),
        "位置删除后不得残留扫描写入的内容链"
    );
}
