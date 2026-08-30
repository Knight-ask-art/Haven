//! TauriSearchEventSink：application `SearchEventSink` 端口的 src-tauri 实现
//! （契约 §36.3 / CONTRACT-V02-SEARCH-CHANNEL-001）。
//!
//! `search.source` Channel 出口模型（复用 TauriScanEventSink 的路由语义，简化项）：
//! - 每次 `search_source_start` 绑定一个 Channel route；启动返回前已产生的
//!   `started` 等事件进入短暂缓冲（非 terminal 上限 64），assign 后只投递同
//!   operationId 事件。
//! - 单一 per-route outbound VecDeque + 唯一 drainer：锁内 FIFO 入队、锁外 send，
//!   绝不持 route 锁调用外部 callback（R-MAIN-11）。
//! - bind 返回不可复用身份 handle；unbind 按 ptr identity 校验删除。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use haven_application::services::search_source::SearchEventSink;
use haven_application::wire::{SearchSourceEvent, SearchSourceEventKind};

/// 未 assign 时非 terminal 事件缓冲上限（搜索事件低频；terminal 恒保留）。
const PENDING_CAP: usize = 64;
/// 广播注册表上限（多窗口/重复注册防护）。
const REGISTRY_CAP: usize = 8;

pub(crate) struct SearchRoute {
    channel_id: u32,
    state: Mutex<SearchRouteState>,
    send: Box<dyn Fn(SearchSourceEvent) + Send + Sync>,
}

struct SearchRouteState {
    operation_id: Option<String>,
    pending: VecDeque<SearchSourceEvent>,
    outbound: VecDeque<SearchSourceEvent>,
    draining: bool,
}

impl SearchRoute {
    /// 锁内入队 + 判定是否需要启动 drainer。
    fn deliver(&self, event: SearchSourceEvent) {
        let should_drain = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            match state.operation_id.as_deref() {
                Some(assigned) => {
                    if assigned == event.operation_id {
                        state.outbound.push_back(event);
                        maybe_start_drain(&mut state)
                    } else {
                        false
                    }
                }
                None => {
                    if is_terminal(&event) {
                        // 同 operation 的重复 terminal 只保留最新一份。
                        state
                            .pending
                            .retain(|e| !(e.operation_id == event.operation_id && is_terminal(e)));
                        state.pending.push_back(event);
                    } else if state.pending.len() < PENDING_CAP {
                        state.pending.push_back(event);
                    }
                    maybe_start_drain(&mut state)
                }
            }
        };
        if should_drain {
            self.drain_outbound();
        }
    }

    /// 启动返回后绑定权威 operationId，并把匹配的缓冲按 sequence 合并进 outbound。
    pub(crate) fn assign_operation(&self, operation_id: String) {
        let should_drain = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.operation_id.is_some() {
                return; // 已 assign（幂等）。
            }
            state.operation_id = Some(operation_id.clone());
            let mut matched: Vec<SearchSourceEvent> = state
                .pending
                .iter()
                .filter(|e| e.operation_id == operation_id)
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

fn is_terminal(event: &SearchSourceEvent) -> bool {
    matches!(
        event.kind,
        SearchSourceEventKind::Completed
            | SearchSourceEventKind::Cancelled
            | SearchSourceEventKind::Failed
    )
}

fn maybe_start_drain(state: &mut SearchRouteState) -> bool {
    if !state.outbound.is_empty() && !state.draining {
        state.draining = true;
        true
    } else {
        false
    }
}

struct BoundEmitter {
    route: Arc<SearchRoute>,
}

pub struct TauriSearchEventSink {
    emitters: Mutex<Vec<BoundEmitter>>,
}

impl TauriSearchEventSink {
    pub fn new() -> Self {
        Self {
            emitters: Mutex::new(Vec::new()),
        }
    }

    /// 绑定本次调用的事件出口；返回不可复用身份 handle。
    pub(crate) fn bind(&self, channel: tauri::ipc::Channel<SearchSourceEvent>) -> Arc<SearchRoute> {
        let channel_id = channel.id();
        let send = move |event: SearchSourceEvent| {
            let _ = channel.send(event);
        };
        let route = Arc::new(SearchRoute {
            channel_id,
            state: Mutex::new(SearchRouteState {
                operation_id: None,
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
        route
    }

    /// 解绑：只在 registry 当前项仍是该 handle 时删除（防旧调用误删新 route）。
    pub(crate) fn unbind(&self, handle: &Arc<SearchRoute>) {
        let mut emitters = self.emitters.lock().unwrap_or_else(|e| e.into_inner());
        emitters.retain(|e| !Arc::ptr_eq(&e.route, handle));
    }
}

impl Default for TauriSearchEventSink {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchEventSink for TauriSearchEventSink {
    fn emit_search_event(&self, event: SearchSourceEvent) {
        // 先 clone route 列表再释放 registry 锁——避免持锁调用 route.deliver 时
        // 重入死锁（与 TauriScanEventSink 同规则）。
        let routes: Vec<Arc<SearchRoute>> = {
            let emitters = self.emitters.lock().unwrap_or_else(|e| e.into_inner());
            emitters.iter().map(|e| e.route.clone()).collect()
        };
        for route in routes {
            route.deliver(event.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event(op: &str, seq: u32, kind: SearchSourceEventKind) -> SearchSourceEvent {
        SearchSourceEvent {
            operation_id: op.into(),
            sequence: seq,
            at: "2026-08-22T00:00:00Z".into(),
            kind,
            data: haven_application::wire::SearchSourceEventData {
                source_id: None,
                works: Vec::new(),
                code: None,
                message: None,
            },
        }
    }

    #[test]
    fn buffers_started_before_assign_and_delivers_in_order() {
        let received = Arc::new(Mutex::new(Vec::<u32>::new()));
        let recv = received.clone();
        let route = Arc::new(SearchRoute {
            channel_id: 1,
            state: Mutex::new(SearchRouteState {
                operation_id: None,
                pending: VecDeque::new(),
                outbound: VecDeque::new(),
                draining: false,
            }),
            send: Box::new(move |event| {
                recv.lock().unwrap().push(event.sequence);
            }),
        });
        route.deliver(sample_event("op-1", 1, SearchSourceEventKind::Started));
        route.deliver(sample_event(
            "op-other",
            1,
            SearchSourceEventKind::Completed,
        ));
        route.assign_operation("op-1".into());
        route.deliver(sample_event("op-1", 2, SearchSourceEventKind::Completed));

        assert_eq!(
            *received.lock().unwrap(),
            vec![1, 2],
            "同 op 缓冲合并后投递"
        );
        // 异 op terminal（op-other）必须被丢弃，不得进入投递序列。
        let total = received.lock().unwrap().len();
        assert_eq!(total, 2);
    }

    #[test]
    fn drops_other_operations_after_assign() {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = count.clone();
        let route = Arc::new(SearchRoute {
            channel_id: 2,
            state: Mutex::new(SearchRouteState {
                operation_id: Some("op-a".into()),
                pending: VecDeque::new(),
                outbound: VecDeque::new(),
                draining: false,
            }),
            send: Box::new(move |_event| {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }),
        });
        route.deliver(sample_event("op-b", 1, SearchSourceEventKind::Started));
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 0);
        route.deliver(sample_event("op-a", 1, SearchSourceEventKind::Started));
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn unbind_old_handle_keeps_replacement_route() {
        let sink = TauriSearchEventSink::new();
        let make_route = |id: u32| -> Arc<SearchRoute> {
            Arc::new(SearchRoute {
                channel_id: id,
                state: Mutex::new(SearchRouteState {
                    operation_id: None,
                    pending: VecDeque::new(),
                    outbound: VecDeque::new(),
                    draining: false,
                }),
                send: Box::new(|_e| {}),
            })
        };
        let route_a = make_route(7);
        let route_b = make_route(7);
        {
            let mut emitters = sink.emitters.lock().unwrap();
            emitters.push(BoundEmitter {
                route: route_a.clone(),
            });
            emitters.retain(|e| e.route.channel_id != 7);
            emitters.push(BoundEmitter {
                route: route_b.clone(),
            });
        }
        sink.unbind(&route_a);
        let emitters = sink.emitters.lock().unwrap();
        assert!(
            emitters.iter().any(|e| Arc::ptr_eq(&e.route, &route_b)),
            "旧 handle unbind 不得误删替换后的 route"
        );
    }
}
