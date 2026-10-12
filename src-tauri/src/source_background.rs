//! 来源清理 outbox 的启动恢复与有界重试；不发起来源请求、不读秘密、不探测网络。

use std::sync::Arc;
use std::time::Duration;

use haven_application::services::source_config_cache::SourceConfigCache;
use haven_application::services::source_registry::SourceRegistryService;
use haven_domain::credential::CredentialStore;

pub(crate) fn start_credential_cleanup(
    registry: SourceRegistryService,
    store: Arc<dyn CredentialStore>,
    cache: Arc<dyn SourceConfigCache>,
) {
    tauri::async_runtime::spawn(async move {
        // interval 的首次 tick 立即执行：既恢复启动前留下的义务，也持续重试暂时失败。
        let mut timer = tokio::time::interval(Duration::from_secs(60));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            let registry = registry.clone();
            let store = store.clone();
            let cache = cache.clone();
            // SQLite/keyring 的同步 I/O 留在 blocking worker；等待本轮结束，不重叠扫描。
            let result = tauri::async_runtime::spawn_blocking(move || {
                tauri::async_runtime::block_on(registry.retry_pending_cleanup(
                    store.as_ref(),
                    cache.as_ref(),
                    haven_common::UtcMillis::now(),
                ))
            })
            .await;
            match result {
                Ok(Ok(report)) if report.failed > 0 => {
                    eprintln!("[source-cleanup] SOURCE_CLEANUP_PENDING");
                }
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    // 只有稳定错误码；不输出引用、路径、来源地址、底层错误或秘密。
                    eprintln!("[source-cleanup] {}", error.code().as_str());
                }
                Err(_) => eprintln!("[source-cleanup] SOURCE_CLEANUP_WORKER_FAILED"),
            }
        }
    });
}
