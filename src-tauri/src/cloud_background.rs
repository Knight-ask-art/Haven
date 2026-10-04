//! 云盘凭据 outbox 的受控恢复任务；不发起授权、不请求 Drive、不记录凭据。

use std::sync::Arc;
use std::time::Duration;

use haven_application::services::cloud_storage::CloudStorageService;

pub fn start_credential_cleanup(service: Arc<CloudStorageService>) {
    tauri::async_runtime::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(60));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            if let Err(error) = service.sweep_credential_cleanup().await {
                // 失败 outbox 留待下一轮。只有稳定错误码，禁止输出 source / ref / secret。
                eprintln!("[cloud-cleanup] {}", error.code().as_str());
            }
        }
    });
}
