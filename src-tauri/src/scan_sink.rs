//! TauriScanEventSink：application `ScanEventSink` 端口的 src-tauri 实现
//!（BE-SCAN-001 第三步 / ADR-003 §6 依赖方向）。
//!
//! 事件出口模型（契约 §10.3 / §14.4）：
//! - `library.scan` 进度/阶段/警告 → **Channel**（高频，限频由 application 层负责）；
//! - 终态后的 `library.changed` → **Tauri Event**（transport 名连字符适配）。
//!
//! Channel 在每次 `library_scan_start` 调用时由前端创建并传入，sink 维护按任务路由的
//! 注册表：启动返回 taskId 前的事件进入短暂缓冲，绑定后只投递同 taskId 事件。
//!
//! R-MAIN-11 并发语义：
//! - **单一 per-route outbound VecDeque + draining 状态**：`deliver`/`assign_task` 只在
//!   锁内 FIFO 入队并竞争唯一 drainer；drainer 每次锁内 pop、**锁外** `send`，空队列时
//!   锁内清 draining。**绝不持 route mutex 调用外部 callback**。
//! - 未 assign 时：非 terminal pending 最多 256；**terminal 按 operation/task 单独保留**
//!   （至少目标 task terminal 绝不丢）。assign 后按 sequence 稳定合并进 outbound。
//! - **route handle**（Arc 身份）：`bind` 返回不可复用身份 handle；`unbind` 必须收 handle，
//!   只在 registry 当前项仍是该 handle 时删除（防旧调用误删新 route）。
//! - `emit_scan_event` 先 clone route 列表再释放 registry 锁；`emit_library_changed`
//!   先 clone Arc callback 再锁外调用——避免持锁调用外部 callback 的重入死锁。
//!
//! `library.changed` 与扫描事件不同：`app.emit` 是面向全部窗口的广播，由 sink 持有的
//! 单个 emitter 发一次（防 N 出口 = N 份重复事件风暴）。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tauri::Emitter;

use haven_application::services::scan::ScanEventSink;
use haven_application::wire::{LibraryChangedDto, LibraryScanEvent};

use crate::ipc::LIBRARY_CHANGED_TRANSPORT_EVENT;

/// 广播注册表上限（多窗口/重复注册防护；超过后淘汰最旧）。
const REGISTRY_CAP: usize = 8;
/// 未 assign 时非 terminal 事件缓冲上限。
const PENDING_CAP: usize = 256;

/// 一次 `library_scan_start` 绑定的 Channel 出口。
struct BoundEmitter {
    route: Arc<ScanRoute>,
}

/// `library.changed` 单发出口类型。
type LibraryChangedEmitter = Box<dyn Fn(LibraryChangedDto) + Send + Sync>;

/// A per-invoke route that is assigned the authoritative task ID immediately
/// after `ScanService::start` returns. Events emitted in that short startup
/// window are buffered so concurrent starts cannot claim one another's first
/// event. Once assigned, only matching task IDs reach this Channel.
pub(crate) struct ScanRoute {
    channel_id: u32,
    state: Mutex<ScanRouteState>,
    send: Box<dyn Fn(LibraryScanEvent) + Send + Sync>,
}

struct ScanRouteState {
    task_id: Option<String>,
    /// 未 assign 前的缓冲：非 terminal 限 256；terminal 单独按 (operation, task) 保留。
    pending: VecDeque<LibraryScanEvent>,
    /// assign 后用于竞争唯一 drainer 的 outbound FIFO。
    outbound: VecDeque<LibraryScanEvent>,
    /// 是否已有 drainer 在跑（竞争唯一 drainer）。
    draining: bool,
}

impl ScanRoute {
    fn is_assigned_to(&self, task_id: &str) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .task_id
            .as_deref()
            == Some(task_id)
    }
    /// 锁内入队（FIFO）+ 判定是否需要启动 drainer。
    fn deliver(&self, event: LibraryScanEvent) {
        let should_drain = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(expected) = state.task_id.as_deref() {
                // 已 assign：仅同 taskId 事件进入 outbound，其余丢弃。
                if expected != event.data.task_id {
                    return;
                }
                state.outbound.push_back(event);
            } else {
                // 未 assign：terminal 保留（目标 task 终态绝不丢）；非 terminal 限 256。
                if matches!(
                    event.kind,
                    haven_application::wire::ScanPhase::Completed
                        | haven_application::wire::ScanPhase::Cancelled
                        | haven_application::wire::ScanPhase::Failed
                ) {
                    // 只对**同 operation+task 的已保留 terminal** 去重；不得误删非 terminal。
                    state.pending.retain(|e| {
                        !(e.data.task_id == event.data.task_id
                            && e.operation_id == event.operation_id
                            && matches!(
                                e.kind,
                                haven_application::wire::ScanPhase::Completed
                                    | haven_application::wire::ScanPhase::Cancelled
                                    | haven_application::wire::ScanPhase::Failed
                            ))
                    });
                    state.pending.push_back(event);
                } else if state.pending.len() < PENDING_CAP {
                    state.pending.push_back(event);
                }
            }
            maybe_start_drain(&mut state)
        };
        // MutexGuard 已在小作用域末尾真正 drop，再锁外 drain。
        if should_drain {
            self.drain_outbound();
        }
    }

    /// 锁内绑定 task_id、把匹配的 pending（含 terminal）按 sequence 稳定合并进 outbound、
    /// 再判定启动 drainer；live 事件（此后 deliver）只能排在已合并序列之后。
    pub(crate) fn assign_task(&self, task_id: String) {
        let should_drain = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.task_id.is_some() {
                return; // 已 assign（幂等；同 ID 重绑定由 bind 新建 route）。
            }
            state.task_id = Some(task_id.clone());
            // 筛选本 task 的 pending，按 sequence 稳定排序后合并进 outbound（FIFO 尾）。
            let mut matched: Vec<LibraryScanEvent> = state
                .pending
                .iter()
                .filter(|e| e.data.task_id == task_id)
                .cloned()
                .collect();
            matched.sort_by_key(|e| e.sequence);
            for event in matched {
                state.outbound.push_back(event);
            }
            state.pending.clear();
            maybe_start_drain(&mut state)
        };
        if should_drain {
            self.drain_outbound();
        }
    }

    /// **锁外** drainer：每轮短锁 pop；空队列时同一锁内 draining=false 并返回；
    /// 有 event 则释放锁后调用外部 callback `send`。
    fn drain_outbound(&self) {
        loop {
            let has_event = {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                match state.outbound.pop_front() {
                    Some(event) => Some(event),
                    None => {
                        state.draining = false;
                        None
                    }
                }
            };
            match has_event {
                Some(event) => (self.send)(event), // 锁外调用外部 callback
                None => break,
            }
        }
    }
}

/// 锁内纯逻辑：若 outbound 非空且未在 draining，置 draining=true 并返回 true（应启动 drainer）。
fn maybe_start_drain(state: &mut ScanRouteState) -> bool {
    if !state.outbound.is_empty() && !state.draining {
        state.draining = true;
        true
    } else {
        false
    }
}

pub struct TauriScanEventSink {
    emitters: Mutex<Vec<BoundEmitter>>,
    /// `library.changed` 单发出口（AppHandle 以闭包捕获，避免 sink 泛型化）。
    library_changed: Mutex<Option<Arc<LibraryChangedEmitter>>>,
}

impl TauriScanEventSink {
    pub fn new() -> Self {
        Self {
            emitters: Mutex::new(Vec::new()),
            library_changed: Mutex::new(None),
        }
    }

    /// 绑定本次调用的事件出口；返回**不可复用身份 handle**（Arc ptr identity），
    /// `unbind` 必须用该 handle 校验当前 registry 项，防旧调用误删新 route。
    pub(crate) fn bind<R: tauri::Runtime>(
        &self,
        app: tauri::AppHandle<R>,
        channel: tauri::ipc::Channel<LibraryScanEvent>,
    ) -> Arc<ScanRoute> {
        let channel_id = channel.id();
        let send = move |event: LibraryScanEvent| {
            // 发送失败（窗口已关闭等）不影响扫描本身；注册表由 cap 收敛。
            let _ = channel.send(event);
        };
        let app = app.clone();
        let library_changed = move |event: LibraryChangedDto| {
            let _ = app.emit(LIBRARY_CHANGED_TRANSPORT_EVENT, event);
        };
        let route = Arc::new(ScanRoute {
            channel_id,
            state: Mutex::new(ScanRouteState {
                task_id: None,
                pending: VecDeque::new(),
                outbound: VecDeque::new(),
                draining: false,
            }),
            send: Box::new(send),
        });
        let mut emitters = self.emitters.lock().unwrap_or_else(|e| e.into_inner());
        emitters.retain(|e| e.route.channel_id != channel_id);
        emitters.push(BoundEmitter {
            route: route.clone(),
        });
        while emitters.len() > REGISTRY_CAP {
            emitters.remove(0);
        }
        drop(emitters);
        *self
            .library_changed
            .lock()
            .unwrap_or_else(|e| e.into_inner()) =
            Some(Arc::new(Box::new(library_changed) as LibraryChangedEmitter));
        route
    }

    /// 解绑：**必须接收 route handle**，只在 registry 当前项仍是该 handle 时删除。
    /// 防止同 channel_id 被新 route 替换后，旧调用失败误删新 route（R-MAIN-11 #2）。
    pub(crate) fn unbind(&self, handle: &Arc<ScanRoute>) {
        let mut emitters = self.emitters.lock().unwrap_or_else(|e| e.into_inner());
        emitters.retain(|e| !Arc::ptr_eq(&e.route, handle));
    }

    /// 仅测试：注入 `library.changed` 出口（无 Tauri runtime 也可验证单发语义）。
    #[cfg(test)]
    pub(crate) fn set_library_changed_emitter_for_test(&self, f: LibraryChangedEmitter) {
        *self
            .library_changed
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
    }
}

impl Default for TauriScanEventSink {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanEventSink for TauriScanEventSink {
    fn emit_scan_event(&self, event: LibraryScanEvent) {
        // 先 clone route 列表（Arc 克隆），再释放 registry 锁——避免持锁调用 route.deliver
        // 时 deliver 内锁 route 的重入死锁（R-MAIN-11）。
        let routes: Vec<Arc<ScanRoute>> = {
            let emitters = self.emitters.lock().unwrap_or_else(|e| e.into_inner());
            emitters.iter().map(|e| e.route.clone()).collect()
        };
        for route in routes {
            route.deliver(event.clone());
        }
        if matches!(
            event.kind,
            haven_application::wire::ScanPhase::Completed
                | haven_application::wire::ScanPhase::Cancelled
                | haven_application::wire::ScanPhase::Failed
        ) {
            let mut emitters = self.emitters.lock().unwrap_or_else(|e| e.into_inner());
            emitters.retain(|entry| !entry.route.is_assigned_to(&event.data.task_id));
        }
    }

    fn emit_library_changed(&self, event: LibraryChangedDto) {
        // 先 clone Arc callback，再释放锁后调用——避免持锁调用外部 callback 重入死锁。
        let emitter = self
            .library_changed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(emit) = emitter {
            emit(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_application::wire::{ScanEventData, ScanPhase};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn sample_changed(op: &str) -> LibraryChangedDto {
        LibraryChangedDto {
            schema_version: 1,
            at: "2026-08-17T00:00:00Z".into(),
            operation_id: op.into(),
            sequence: 1,
            revision: None,
        }
    }

    fn sample_scan_event(op: &str, task_id: &str, seq: u32, kind: ScanPhase) -> LibraryScanEvent {
        LibraryScanEvent {
            operation_id: op.into(),
            sequence: seq,
            at: "2026-08-17T00:00:00Z".into(),
            kind,
            data: ScanEventData {
                task_id: task_id.into(),
                files_seen: 0,
                recognized: 0,
                new: 0,
                updated: 0,
                skipped: 0,
                errors: 0,
                current_item: None,
                message: None,
            },
        }
    }

    fn sample_index(op: &str, task_id: &str, seq: u32) -> LibraryScanEvent {
        sample_scan_event(op, task_id, seq, ScanPhase::Indexing)
    }

    #[test]
    fn library_changed_emits_exactly_once() {
        let sink = TauriScanEventSink::new();
        let count = Arc::new(AtomicUsize::new(0));
        let counter = count.clone();
        sink.set_library_changed_emitter_for_test(Box::new(move |_event| {
            counter.fetch_add(1, Ordering::SeqCst);
        }));

        sink.emit_library_changed(sample_changed("op-1"));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        sink.emit_library_changed(sample_changed("op-2"));
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn library_changed_without_emitter_is_dropped_not_panicking() {
        let sink = TauriScanEventSink::new();
        sink.emit_library_changed(sample_changed("op-3"));
        sink.emit_scan_event(sample_index("op-4", "t-1", 1));
    }

    /// R-MAIN-11 测试 a：pending seq1 的 send callback 阻塞时，并发 assign 与 live seq2，
    /// 最终严格 [1,2]（单一 drainer 锁外 send 保证 FIFO，不被乱序抢占）。
    #[test]
    fn assign_after_blocked_send_keeps_strict_fifo() {
        let received = Arc::new(Mutex::new(Vec::<u32>::new()));
        let recv = received.clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let barrier_cb = Arc::clone(&barrier);
        let route = ScanRoute {
            channel_id: 1,
            state: Mutex::new(ScanRouteState {
                task_id: None,
                pending: VecDeque::new(),
                outbound: VecDeque::new(),
                draining: false,
            }),
            send: Box::new(move |event| {
                if event.sequence == 1 {
                    // 模拟第一个 event 的 send 阻塞：等并发 assign 已发生。
                    barrier_cb.wait();
                }
                recv.lock().unwrap().push(event.sequence);
            }),
        };
        // 未 assign 缓冲 seq1（非 terminal）。
        let route = Arc::new(route);
        route.deliver(sample_index("op", "task", 1));

        // 并发：assign(task) 与 live seq2。
        let r1 = Arc::clone(&route);
        let t_assign = std::thread::spawn(move || r1.assign_task("task".into()));
        let r2 = Arc::clone(&route);
        let t_live = std::thread::spawn(move || r2.deliver(sample_index("op", "task", 2)));
        // 先阻塞 seq1 的 send 在 barrier（确保 assign 已完成、seq2 已入队）。
        barrier.wait();
        t_assign.join().unwrap();
        t_live.join().unwrap();

        assert_eq!(*received.lock().unwrap(), vec![1, 2], "严格 FIFO [1,2]");
        assert!(route
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .outbound
            .is_empty());
    }

    /// R-MAIN-11 测试 b：256 非 terminal 塞满后，目标 terminal 仍送达且一次。
    #[test]
    fn terminal_retained_after_pending_cap_full() {
        let received = Arc::new(Mutex::new(Vec::<String>::new()));
        let recv = received.clone();
        let route = ScanRoute {
            channel_id: 1,
            state: Mutex::new(ScanRouteState {
                task_id: None,
                pending: VecDeque::new(),
                outbound: VecDeque::new(),
                draining: false,
            }),
            send: Box::new(move |event| {
                recv.lock().unwrap().push(event.data.task_id.clone());
            }),
        };
        // 塞满 256 非 terminal（同 task）。
        for i in 0..256 {
            route.deliver(sample_index("op", "task", i));
        }
        // 追加的非 terminal 被丢弃（cap）。
        route.deliver(sample_index("op", "task", 999));
        // terminal（finish）必须保留且只一次。
        let terminal = sample_scan_event("op", "task", 1000, ScanPhase::Completed);
        route.deliver(terminal.clone());
        route.deliver(terminal); // 重复 terminal 去重（第二次同 operation+task 被替换）
        route.assign_task("task".into());

        let got = received.lock().unwrap();
        assert_eq!(
            got.iter().filter(|t| *t == "task").count(),
            257,
            "256 非 terminal + 1 去重后 terminal 送达"
        );
        assert_eq!(
            got.iter().filter(|t| *t == "task").count(),
            257,
            "terminal 只送达一次"
        );
        assert!(got.iter().position(|t| t == "task").is_some());
    }

    /// R-MAIN-11 测试 c：A/B 同 channel ID，B 替换 A，unbind(A handle) 后 B 仍能收事件。
    #[test]
    fn unbind_old_handle_does_not_remove_replacement_route() {
        // 通过 sink 用同一 channel_id 构造两个 route（bind 内部去重同 id，
        // 但我们直接构造两个 ScanRoute 验证 unbind 的 ptr_eq 语义）。
        let sink = TauriScanEventSink::new();
        let make_route = |id: u32| -> Arc<ScanRoute> {
            Arc::new(ScanRoute {
                channel_id: id,
                state: Mutex::new(ScanRouteState {
                    task_id: Some("task".into()),
                    pending: VecDeque::new(),
                    outbound: VecDeque::new(),
                    draining: false,
                }),
                send: Box::new(|_e| {}),
            })
        };
        let route_a = make_route(42);
        let route_b = make_route(42);
        {
            let mut emitters = sink.emitters.lock().unwrap();
            emitters.push(BoundEmitter {
                route: route_a.clone(),
            });
            // B 替换 A（同 channel_id，bind 语义 remove A + push B）。
            emitters.retain(|e| e.route.channel_id != 42);
            emitters.push(BoundEmitter {
                route: route_b.clone(),
            });
        }
        // 旧调用失败：unbind(A handle) —— 不得删除 B。
        sink.unbind(&route_a);
        {
            let emitters = sink.emitters.lock().unwrap();
            assert!(
                emitters.iter().any(|e| Arc::ptr_eq(&e.route, &route_b)),
                "unbind(A) 后 B 必须仍在 registry"
            );
            assert!(
                !emitters.iter().any(|e| Arc::ptr_eq(&e.route, &route_a)),
                "A 已移除"
            );
        }
    }

    /// R-MAIN-11 测试 d：callback 重入 sink 不死锁（mpsc + recv_timeout 有限等待，
    /// 死锁会被超时捕获而非永久卡在裸 join）。
    #[test]
    fn callback_reentrancy_does_not_deadlock() {
        let sink = Arc::new(TauriScanEventSink::new());
        // send callback：收到 seq1 时重入一次发 seq99；每次 callback 向 mpsc 发其 sequence。
        let (tx, rx) = std::sync::mpsc::channel::<u32>();
        let reentered = Arc::new(AtomicUsize::new(0));
        let sink_ref = Arc::clone(&sink);
        let tx_ref = tx.clone();
        let reentered_ref = Arc::clone(&reentered);
        let route = Arc::new(ScanRoute {
            channel_id: 1,
            state: Mutex::new(ScanRouteState {
                task_id: Some("task".into()),
                pending: VecDeque::new(),
                outbound: VecDeque::new(),
                draining: false,
            }),
            send: Box::new(move |event| {
                if event.sequence != 99 && reentered_ref.swap(1, Ordering::SeqCst) == 0 {
                    // 仅重入一次（seq99）；不可无限递归。
                    let inner = sample_index("op", "task", 99);
                    sink_ref.emit_scan_event(inner);
                }
                let _ = tx_ref.send(event.sequence);
            }),
        });
        {
            let mut emitters = sink.emitters.lock().unwrap();
            emitters.push(BoundEmitter {
                route: route.clone(),
            });
        }
        let sink_thread = Arc::clone(&sink);
        let handle = std::thread::spawn(move || {
            sink_thread.emit_scan_event(sample_index("op", "task", 1));
        });
        // 顺序断言：1 先、99 后（重入在其后）。超时 → panic（死锁）。
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), 99);
        handle.join().unwrap();
        // 重入完成、无死锁；显式触发一条可测事件确保无 panic。
        sink.emit_library_changed(sample_changed("op-fin"));
    }

    /// R-MAIN-11 测试 d2：`library.changed` callback 内重入
    /// `set_library_changed_emitter_for_test`，mpsc + recv_timeout 证明锁外调用不死锁。
    #[test]
    fn library_changed_callback_reentrancy_does_not_deadlock() {
        let sink = Arc::new(TauriScanEventSink::new());
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let sink_ref = Arc::clone(&sink);
        let tx_ref = tx.clone();
        // 该 emitter 在 emit 时重入设置 emitter（锁外调用前提）。
        sink.set_library_changed_emitter_for_test(Box::new(move |_event| {
            let tx_inner = tx_ref.clone();
            sink_ref.set_library_changed_emitter_for_test(Box::new(move |_e2| {
                let _ = tx_inner.send(());
            }));
            let _ = tx_ref.send(());
        }));
        let sink_thread = Arc::clone(&sink);
        let handle = std::thread::spawn(move || {
            sink_thread.emit_library_changed(sample_changed("op"));
        });
        // 有限等待：第一个信号（外层）与第二个信号（重入后 emitter）都要收到，否则死锁。
        rx.recv_timeout(Duration::from_secs(5))
            .expect("外层 emitter 必须超时内送达");
        handle.join().unwrap();
        // 重入设置的新 emitter 应可再触发。
        sink.emit_library_changed(sample_changed("op-2"));
        rx.recv_timeout(Duration::from_secs(5))
            .expect("重入设置的新 emitter 必须送达（锁外调用成立）");
    }

    #[test]
    fn route_assigns_task_after_startup_without_cross_talk() {
        let received = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink_received = received.clone();
        let route = ScanRoute {
            channel_id: 1,
            state: Mutex::new(ScanRouteState {
                task_id: None,
                pending: VecDeque::new(),
                outbound: VecDeque::new(),
                draining: false,
            }),
            send: Box::new(move |event| {
                sink_received.lock().unwrap().push(event.data.task_id);
            }),
        };
        route.deliver(sample_index("op-a", "task-a", 1));
        route.deliver(sample_index("op-b", "task-b", 1));
        route.assign_task("task-b".into());
        route.deliver(sample_index("op-a-2", "task-a", 2));
        route.deliver(sample_index("op-b-2", "task-b", 2));
        assert_eq!(*received.lock().unwrap(), vec!["task-b", "task-b"]);
        assert!(route
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pending
            .is_empty());
        assert!(route
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .outbound
            .is_empty());
    }

    #[test]
    fn terminal_event_unbinds_the_matching_route() {
        let sink = TauriScanEventSink::new();
        let received = Arc::new(Mutex::new(Vec::new()));
        let route = Arc::new(ScanRoute {
            channel_id: 7,
            state: Mutex::new(ScanRouteState {
                task_id: Some("task-terminal".into()),
                pending: VecDeque::new(),
                outbound: VecDeque::new(),
                draining: false,
            }),
            send: {
                let received = Arc::clone(&received);
                Box::new(move |event| received.lock().unwrap().push(event.kind))
            },
        });
        sink.emitters
            .lock()
            .unwrap()
            .push(BoundEmitter { route });

        sink.emit_scan_event(sample_scan_event(
            "op-terminal",
            "task-terminal",
            2,
            ScanPhase::Completed,
        ));

        assert_eq!(received.lock().unwrap().as_slice(), &[ScanPhase::Completed]);
        assert!(sink.emitters.lock().unwrap().is_empty());
    }
}
