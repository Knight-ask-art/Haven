use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use haven_application::services::reader_search::ReaderSearchEventSink;
use haven_application::wire::{ReaderSearchEvent, ReaderSearchEventKind};

const PENDING_CAP: usize = 64;
const REGISTRY_CAP: usize = 8;

pub(crate) struct ReaderSearchRoute {
    channel_id: u32,
    state: Mutex<ReaderSearchRouteState>,
    send: Box<dyn Fn(ReaderSearchEvent) + Send + Sync>,
}

struct ReaderSearchRouteState {
    operation_id: Option<String>,
    pending: VecDeque<ReaderSearchEvent>,
    outbound: VecDeque<ReaderSearchEvent>,
    draining: bool,
}

impl ReaderSearchRoute {
    fn deliver(&self, event: ReaderSearchEvent) {
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

    pub(crate) fn assign_operation(&self, operation_id: String) {
        let should_drain = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.operation_id.is_some() {
                return;
            }
            state.operation_id = Some(operation_id.clone());
            let mut matched: Vec<ReaderSearchEvent> = state
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
                Some(event) => (self.send)(event),
                None => break,
            }
        }
    }
}

fn is_terminal(event: &ReaderSearchEvent) -> bool {
    matches!(
        event.kind,
        ReaderSearchEventKind::Completed
            | ReaderSearchEventKind::Cancelled
            | ReaderSearchEventKind::Failed
    )
}

fn maybe_start_drain(state: &mut ReaderSearchRouteState) -> bool {
    if !state.outbound.is_empty() && !state.draining {
        state.draining = true;
        true
    } else {
        false
    }
}

struct BoundEmitter {
    route: Arc<ReaderSearchRoute>,
}

pub struct TauriReaderSearchEventSink {
    emitters: Mutex<Vec<BoundEmitter>>,
}

impl TauriReaderSearchEventSink {
    pub fn new() -> Self {
        Self {
            emitters: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn bind(
        &self,
        channel: tauri::ipc::Channel<ReaderSearchEvent>,
    ) -> Arc<ReaderSearchRoute> {
        let channel_id = channel.id();
        let send = move |event: ReaderSearchEvent| {
            let _ = channel.send(event);
        };
        let route = Arc::new(ReaderSearchRoute {
            channel_id,
            state: Mutex::new(ReaderSearchRouteState {
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

    pub(crate) fn unbind(&self, handle: &Arc<ReaderSearchRoute>) {
        let mut emitters = self.emitters.lock().unwrap_or_else(|e| e.into_inner());
        emitters.retain(|e| !Arc::ptr_eq(&e.route, handle));
    }
}

impl Default for TauriReaderSearchEventSink {
    fn default() -> Self {
        Self::new()
    }
}

impl ReaderSearchEventSink for TauriReaderSearchEventSink {
    fn emit(&self, event: ReaderSearchEvent) {
        let routes: Vec<Arc<ReaderSearchRoute>> = {
            let emitters = self.emitters.lock().unwrap_or_else(|e| e.into_inner());
            emitters.iter().map(|e| e.route.clone()).collect()
        };
        for route in routes {
            route.deliver(event.clone());
        }
    }
}
