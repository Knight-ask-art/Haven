use std::collections::HashMap;
use std::sync::Mutex;

use haven_application::mapper::time::utc_millis_to_rfc3339;
use haven_application::services::download::{download_event_data, DownloadEventSink};
use haven_application::wire::{DownloadEvent, DownloadEventKind};
use haven_domain::entities::DownloadTask;
use tauri::ipc::Channel;

const MAX_CHANNELS: usize = 16;

pub struct TauriDownloadEventSink {
    channels: Mutex<HashMap<String, Channel<DownloadEvent>>>,
    sequences: Mutex<HashMap<String, u32>>,
}

impl TauriDownloadEventSink {
    pub fn new() -> Self {
        Self {
            channels: Mutex::new(HashMap::new()),
            sequences: Mutex::new(HashMap::new()),
        }
    }

    pub fn bind(&self, subscription_id: String, channel: Channel<DownloadEvent>) {
        let mut channels = self
            .channels
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if channels.len() == MAX_CHANNELS && !channels.contains_key(&subscription_id) {
            if let Some(stale_id) = channels.keys().next().cloned() {
                channels.remove(&stale_id);
            }
        }
        channels.insert(subscription_id, channel);
    }

    pub fn unbind(&self, subscription_id: &str) {
        self.channels
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(subscription_id);
    }
}

impl DownloadEventSink for TauriDownloadEventSink {
    fn emit_task(&self, task: &DownloadTask, error_code: Option<&str>) {
        let operation_id = task.id.to_string();
        let sequence = {
            let mut sequences = self
                .sequences
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let next = sequences.entry(operation_id.clone()).or_default();
            *next = next.saturating_add(1);
            *next
        };
        let event = DownloadEvent {
            operation_id,
            sequence,
            at: utc_millis_to_rfc3339(haven_common::UtcMillis::now()),
            kind: DownloadEventKind::Updated,
            data: download_event_data(task, error_code),
        };
        self.channels
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|_, channel| channel.send(event.clone()).is_ok());
    }
}

impl Default for TauriDownloadEventSink {
    fn default() -> Self {
        Self::new()
    }
}
