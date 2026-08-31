//! 本地下载 Worker。
//!
//! v0.1 只消费后端已经索引且明确为本地文件的 Resource，并复制到目标 Local
//! Storage 的离线目录。远程 URL、流媒体和需要 Source Resolver 的资源会进入
//! Failed，而不是把“可播放”错误地伪装成“可下载”。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use haven_application::services::download_batch::DownloadBatchService;
use haven_application::services::ports::RemoteAcquisitionPort;
use haven_application::services::settings::SettingsService;
use haven_application::services::{DownloadEventSink, DownloadRunner, OfflineResourceFiles};
use haven_common::{AppError, ErrorKind};
use haven_domain::contracts::{DownloadRepository, ResourceRepository, StorageLocationRepository};
use haven_domain::entities::{DownloadTask, Resource, ResourceLocator};
use haven_domain::enums::{
    Availability, AvailabilitySource, DownloadPriority, DownloadState, StorageProviderType,
    StorageStatus,
};
use haven_domain::ids::DownloadTaskId;
use haven_domain::settings::{
    DownloadConcurrency, DownloadSpeedLimit, SettingsSection, SettingsValue,
};
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::Notify;

use crate::db::repos::SqliteRepositories;

const CHUNK_SIZE: usize = 1024 * 1024;
const DISK_SPACE_RESERVE_BYTES: u64 = 16 * 1024 * 1024;
const SPACE_RECHECK_INTERVAL: Duration = Duration::from_secs(1);
const MAX_WORKER_RETRIES: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DownloadWorkerFailure {
    code: &'static str,
    retryable: bool,
}

impl DownloadWorkerFailure {
    const fn new(code: &'static str, retryable: bool) -> Self {
        Self { code, retryable }
    }
}

fn io_failure(error: &std::io::Error, not_found_code: &'static str) -> DownloadWorkerFailure {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => {
            DownloadWorkerFailure::new("DOWNLOAD_PERMISSION_DENIED", false)
        }
        std::io::ErrorKind::StorageFull => {
            DownloadWorkerFailure::new("DOWNLOAD_DISK_SPACE_LOW", false)
        }
        std::io::ErrorKind::NotFound => DownloadWorkerFailure::new(
            not_found_code,
            !matches!(
                not_found_code,
                "DOWNLOAD_DIRECTORY_UNAVAILABLE" | "DOWNLOAD_SOURCE_UNAVAILABLE"
            ),
        ),
        _ => DownloadWorkerFailure::new("DOWNLOAD_IO_FAILED", true),
    }
}

fn repository_failure() -> DownloadWorkerFailure {
    DownloadWorkerFailure::new("DOWNLOAD_IO_FAILED", true)
}

fn has_enough_space(available: u64, required: u64, reserve: u64) -> bool {
    available >= required.saturating_add(reserve)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DownloadPolicy {
    max_concurrent: usize,
    speed_limit_bps: Option<u64>,
}

impl Default for DownloadPolicy {
    fn default() -> Self {
        Self {
            max_concurrent: DownloadConcurrency::Three.as_usize(),
            speed_limit_bps: DownloadSpeedLimit::Unlimited.as_bytes_per_second(),
        }
    }
}

async fn load_download_policy(settings: &SettingsService) -> DownloadPolicy {
    let Ok(snapshot) = settings.get(SettingsSection::Downloads).await else {
        return DownloadPolicy::default();
    };
    let SettingsValue::Downloads(value) = snapshot.value else {
        return DownloadPolicy::default();
    };
    DownloadPolicy {
        max_concurrent: value.concurrent_tasks.as_usize(),
        speed_limit_bps: value.speed_limit.as_bytes_per_second(),
    }
}

/// 三层调度准入（裁决 D 题）：
/// - 全局并发上限：来自设置（≤4 默认，`DownloadConcurrency`）。
/// - 每 Host 并发上限：`HOST_MAX_CONCURRENT`（默认 2）。
/// - 每 Provider TokenBucket + DRR 权重（High=8 / Normal=4 / Low=1）：
///   bucket 容量 = 权重，补液速率 = 权重 × `BUCKET_REFILL_PER_SEC_PER_WEIGHT`，
///   权重高的 Provider 以更高速率获得槽位，保证 8:4:1 的公平份额。
const HOST_MAX_CONCURRENT: usize = 2;
const BUCKET_REFILL_PER_SEC_PER_WEIGHT: f64 = 0.5;

fn priority_weight(priority: DownloadPriority) -> f64 {
    match priority {
        DownloadPriority::High => 8.0,
        DownloadPriority::Normal => 4.0,
        DownloadPriority::Low => 1.0,
    }
}

/// TokenBucket 补液：`min(capacity, tokens + elapsed_secs * rate_per_weight * weight)`。
/// 纯函数便于测试；容量恒等于权重。
fn refill_bucket(tokens: f64, elapsed_secs: f64, weight: f64) -> f64 {
    (tokens + elapsed_secs * BUCKET_REFILL_PER_SEC_PER_WEIGHT * weight).min(weight)
}

/// 进程内下载准入控制。设置变更会在下一个任务获得槽位时生效，
/// 不强制中断已经运行的任务。
struct DownloadScheduler {
    global_active: Mutex<usize>,
    host_active: Mutex<std::collections::HashMap<String, usize>>,
    provider_buckets: Mutex<std::collections::HashMap<String, ProviderBucket>>,
    notify: Notify,
}

struct ProviderBucket {
    tokens: f64,
    last_refill: Instant,
}

impl DownloadScheduler {
    fn new() -> Self {
        Self {
            global_active: Mutex::new(0),
            host_active: Mutex::new(std::collections::HashMap::new()),
            provider_buckets: Mutex::new(std::collections::HashMap::new()),
            notify: Notify::new(),
        }
    }

    async fn acquire(
        self: &Arc<Self>,
        task: &DownloadTask,
        settings: &SettingsService,
    ) -> DownloadPermit {
        let provider = task
            .provider_key
            .clone()
            .unwrap_or_else(|| "default".into());
        let host = task.host_key.clone().unwrap_or_else(|| "default".into());
        let weight = priority_weight(task.priority);
        loop {
            // 先建立通知 future，再检查计数，避免释放发生在检查与 await 之间时丢唤醒。
            let notified = self.notify.notified();
            let policy = load_download_policy(settings).await;
            let acquired = {
                let mut global = self
                    .global_active
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let mut hosts = self
                    .host_active
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let mut buckets = self
                    .provider_buckets
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let bucket = buckets
                    .entry(provider.clone())
                    .or_insert_with(|| ProviderBucket {
                        tokens: weight,
                        last_refill: Instant::now(),
                    });
                let elapsed = bucket.last_refill.elapsed().as_secs_f64();
                bucket.tokens = refill_bucket(bucket.tokens, elapsed, weight);
                bucket.last_refill = Instant::now();
                let host_count = hosts.get(&host).copied().unwrap_or(0);
                if *global < policy.max_concurrent
                    && host_count < HOST_MAX_CONCURRENT
                    && bucket.tokens >= 1.0
                {
                    *global += 1;
                    *hosts.entry(host.clone()).or_insert(0) += 1;
                    bucket.tokens -= 1.0;
                    true
                } else {
                    false
                }
            };
            if acquired {
                return DownloadPermit {
                    scheduler: Arc::clone(self),
                    provider,
                    host,
                };
            }
            notified.await;
        }
    }

    fn release(&self, _provider: &str, host: &str) {
        let mut global = self
            .global_active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *global = global.saturating_sub(1);
        drop(global);
        let mut hosts = self
            .host_active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(count) = hosts.get_mut(host) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                hosts.remove(host);
            }
        }
        drop(hosts);
        self.notify.notify_one();
    }
}

struct DownloadPermit {
    scheduler: Arc<DownloadScheduler>,
    provider: String,
    host: String,
}

impl Drop for DownloadPermit {
    fn drop(&mut self) {
        self.scheduler.release(&self.provider, &self.host);
    }
}

#[derive(Clone)]
pub struct LocalDownloadRunner {
    repos: Arc<SqliteRepositories>,
    active: Arc<Mutex<HashSet<DownloadTaskId>>>,
    scheduler: Arc<DownloadScheduler>,
    settings: Arc<SettingsService>,
    events: Arc<dyn DownloadEventSink>,
    batch: Arc<DownloadBatchService>,
    remote: Arc<dyn RemoteAcquisitionPort>,
}

#[derive(Clone, Default)]
pub struct LocalOfflineResourceFiles;

/// The result of promoting a downloaded file into the resource index.  The
/// `created` bit is important for compensation: an existing valid offline
/// resource must never be deleted just because a later task-state CAS failed,
/// while a resource created by this worker must not be left orphaned.
#[derive(Debug, Clone, Copy)]
struct OfflineResourceHandle {
    id: haven_domain::ids::ResourceId,
    created: bool,
}

#[derive(Clone, Copy)]
struct UnavailableRemoteAcquisition;

#[async_trait]
impl RemoteAcquisitionPort for UnavailableRemoteAcquisition {
    async fn acquire(
        &self,
        _source_key: &str,
        _remote_id: &str,
        _destination: &Path,
    ) -> Result<haven_application::services::ports::RemoteAcquiredFile, AppError> {
        Err(AppError::new(
            "DOWNLOAD_SOURCE_UNAVAILABLE",
            ErrorKind::Network,
            "远端来源当前不可用",
            true,
        ))
    }
}

#[async_trait]
impl OfflineResourceFiles for LocalOfflineResourceFiles {
    async fn is_available(
        &self,
        storage: &haven_domain::entities::StorageLocation,
        resource: &Resource,
    ) -> Result<bool, AppError> {
        match registered_offline_path(storage, resource).await {
            Ok(path) => {
                let metadata = fs::metadata(path).await.map_err(|error| {
                    AppError::new(
                        "DOWNLOAD_OFFLINE_FILE_CHECK_FAILED",
                        ErrorKind::Io,
                        "无法确认离线文件状态",
                        true,
                    )
                    .with_source(error)
                })?;
                Ok(metadata.is_file() && metadata.len() > 0)
            }
            Err(error) if error.code().as_str() == "DOWNLOAD_OFFLINE_FILE_NOT_FOUND" => Ok(false),
            Err(error) => Err(error),
        }
    }

    async fn delete(
        &self,
        storage: &haven_domain::entities::StorageLocation,
        resource: &Resource,
    ) -> Result<(), AppError> {
        let path = registered_offline_path(storage, resource).await?;
        fs::remove_file(&path).await.map_err(|error| {
            AppError::new(
                "DOWNLOAD_OFFLINE_DELETE_FAILED",
                ErrorKind::Io,
                "无法删除离线文件",
                true,
            )
            .with_source(error)
        })?;
        match fs::metadata(&path).await {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Ok(_) => Err(AppError::new(
                "DOWNLOAD_OFFLINE_DELETE_VERIFY_FAILED",
                ErrorKind::Io,
                "离线文件删除后仍然存在",
                true,
            )),
            Err(error) => Err(AppError::new(
                "DOWNLOAD_OFFLINE_DELETE_VERIFY_FAILED",
                ErrorKind::Io,
                "无法确认离线文件已删除",
                true,
            )
            .with_source(error)),
        }
    }

    async fn reveal(
        &self,
        storage: &haven_domain::entities::StorageLocation,
        resource: &Resource,
    ) -> Result<(), AppError> {
        let path = registered_offline_path(storage, resource).await?;
        tokio::task::spawn_blocking(move || reveal_with_system(&path))
            .await
            .map_err(|error| {
                AppError::new(
                    "DOWNLOAD_REVEAL_FAILED",
                    ErrorKind::Internal,
                    "无法打开文件所在位置",
                    true,
                )
                .with_source(error)
            })?
    }
}

impl LocalDownloadRunner {
    pub fn new(
        repos: Arc<SqliteRepositories>,
        settings: Arc<SettingsService>,
        events: Arc<dyn DownloadEventSink>,
        batch: Arc<DownloadBatchService>,
    ) -> Self {
        Self::new_with_remote(
            repos,
            settings,
            events,
            batch,
            Arc::new(UnavailableRemoteAcquisition),
        )
    }

    pub fn new_with_remote(
        repos: Arc<SqliteRepositories>,
        settings: Arc<SettingsService>,
        events: Arc<dyn DownloadEventSink>,
        batch: Arc<DownloadBatchService>,
        remote: Arc<dyn RemoteAcquisitionPort>,
    ) -> Self {
        Self {
            repos,
            active: Arc::new(Mutex::new(HashSet::new())),
            scheduler: Arc::new(DownloadScheduler::new()),
            settings,
            events,
            batch,
            remote,
        }
    }

    async fn run(&self, task_id: DownloadTaskId) {
        // 先取任务以获取 provider/host/priority（三层调度入参），再申请准入槽位。
        let Some(task) = self.repos.download.get(task_id).await.ok().flatten() else {
            self.active
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&task_id);
            return;
        };
        let _permit = self.scheduler.acquire(&task, &self.settings).await;
        let policy = load_download_policy(&self.settings).await;
        let mut failure = None;
        for attempt in 0..=MAX_WORKER_RETRIES {
            match self.run_inner(task_id, policy).await {
                Ok(()) => break,
                Err(error) if error.retryable && attempt < MAX_WORKER_RETRIES => {
                    if !self.requeue_after_failure(task_id).await {
                        failure = Some(error);
                        break;
                    }
                    let delay = if attempt == 0 {
                        Duration::from_millis(250)
                    } else {
                        Duration::from_secs(1)
                    };
                    tokio::time::sleep(delay).await;
                }
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        if let Some(error) = failure {
            // Worker 错误只影响任务状态；错误详情不写日志，避免把本地路径带出。
            self.fail_task(task_id).await;
            if let Ok(Some(task)) = self.repos.download.get(task_id).await {
                self.events.emit_task(&task, Some(error.code));
            }
        } else if let Ok(Some(task)) = self.repos.download.get(task_id).await {
            self.events.emit_task(&task, None);
        }
        // 子任务进入终态后重算批次聚合（Completed/Failed/Cancelled 幂等收敛）。
        if let Ok(Some(task)) = self.repos.download.get(task_id).await {
            if let Some(batch_id) = task.batch_id {
                if matches!(
                    task.state,
                    DownloadState::Completed | DownloadState::Failed | DownloadState::Cancelled
                ) {
                    let _ = self.batch.reconcile(batch_id).await;
                }
            }
        }
        self.active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&task_id);
        // Resume 可能与旧 Worker 退出同时发生。旧 Worker 释放去重标记后再次检查，
        // 确保该竞态不会把已转回 Queued 的任务永久遗留在队列中。
        if matches!(
            self.repos.download.get(task_id).await,
            Ok(Some(task)) if task.state == DownloadState::Queued
        ) {
            self.start(task_id);
        }
    }

    async fn run_inner(
        &self,
        task_id: DownloadTaskId,
        policy: DownloadPolicy,
    ) -> Result<(), DownloadWorkerFailure> {
        if !self
            .repos
            .download
            .compare_and_set_state(task_id, DownloadState::Queued, DownloadState::Resolving)
            .await
            .map_err(|_| repository_failure())?
        {
            return Ok(());
        }
        let task = self
            .repos
            .download
            .get(task_id)
            .await
            .map_err(|_| repository_failure())?
            .ok_or_else(|| DownloadWorkerFailure::new("DOWNLOAD_SOURCE_UNAVAILABLE", false))?;
        let resource = self
            .repos
            .resource
            .get(task.source_resource_id)
            .await
            .map_err(|_| repository_failure())?
            .ok_or_else(|| DownloadWorkerFailure::new("DOWNLOAD_SOURCE_UNAVAILABLE", false))?;
        let storage = self
            .repos
            .storage_location
            .get(task.target_storage_id)
            .await
            .map_err(|_| repository_failure())?
            .ok_or_else(|| DownloadWorkerFailure::new("DOWNLOAD_DIRECTORY_UNAVAILABLE", false))?;
        if storage.provider_type != StorageProviderType::Local
            || storage.status != StorageStatus::Connected
        {
            return Err(DownloadWorkerFailure::new(
                "DOWNLOAD_DIRECTORY_UNAVAILABLE",
                false,
            ));
        }
        if let ResourceLocator::SourceObject {
            source_id,
            remote_id,
        } = &resource.locator
        {
            return self
                .run_remote_inner(task_id, &task, &resource, &storage, *source_id, remote_id)
                .await;
        }
        let source_path = match &resource.locator {
            ResourceLocator::LocalPath { path } => {
                resolve_registered_local_source(&self.repos, &resource, path).await?
            }
            ResourceLocator::StorageObject {
                provider_id,
                path_hint,
                ..
            } => {
                let source_storage = self
                    .repos
                    .storage_location
                    .get(*provider_id)
                    .await
                    .map_err(|_| DownloadWorkerFailure::new("DOWNLOAD_SOURCE_UNAVAILABLE", false))?
                    .ok_or_else(|| {
                        DownloadWorkerFailure::new("DOWNLOAD_SOURCE_UNAVAILABLE", false)
                    })?;
                let relative = path_hint.as_deref().ok_or_else(|| {
                    DownloadWorkerFailure::new("DOWNLOAD_SOURCE_UNAVAILABLE", false)
                })?;
                validate_relative_object_path(relative).map_err(|_| {
                    DownloadWorkerFailure::new("DOWNLOAD_SOURCE_UNAVAILABLE", false)
                })?;
                let root = fs::canonicalize(&source_storage.root_ref)
                    .await
                    .map_err(|_| {
                        DownloadWorkerFailure::new("DOWNLOAD_SOURCE_UNAVAILABLE", false)
                    })?;
                let candidate = root.join(relative);
                let canonical = fs::canonicalize(candidate).await.map_err(|_| {
                    DownloadWorkerFailure::new("DOWNLOAD_SOURCE_UNAVAILABLE", false)
                })?;
                if !canonical.starts_with(&root) || canonical == root {
                    return Err(DownloadWorkerFailure::new(
                        "DOWNLOAD_SOURCE_UNAVAILABLE",
                        false,
                    ));
                }
                canonical
            }
            ResourceLocator::Http { .. } | ResourceLocator::SourceObject { .. } => {
                return Err(DownloadWorkerFailure::new(
                    "DOWNLOAD_SOURCE_UNAVAILABLE",
                    false,
                ));
            }
        };
        let metadata = fs::metadata(&source_path)
            .await
            .map_err(|error| io_failure(&error, "DOWNLOAD_SOURCE_UNAVAILABLE"))?;
        if !metadata.is_file() {
            return Err(DownloadWorkerFailure::new(
                "DOWNLOAD_SOURCE_UNAVAILABLE",
                false,
            ));
        }
        let total = resource.size.unwrap_or(metadata.len());
        if total != metadata.len() {
            return Err(DownloadWorkerFailure::new(
                "DOWNLOAD_SOURCE_UNAVAILABLE",
                false,
            ));
        }

        let offline_dir = download_category_dir(Path::new(&storage.root_ref), &resource);
        fs::create_dir_all(&offline_dir)
            .await
            .map_err(|error| io_failure(&error, "DOWNLOAD_DIRECTORY_UNAVAILABLE"))?;
        let extension = safe_extension(&source_path);
        let final_path = offline_dir.join(format!("{}.{}", task_id, extension));
        let part_path = offline_dir.join(format!("{}.part", task_id));

        match fs::metadata(&final_path).await {
            Ok(final_meta) if final_meta.len() == total => {
                let offline = match self
                    .ensure_offline_resource(&resource, task.target_storage_id, &final_path, total)
                    .await
                {
                    Ok(handle) => handle,
                    Err(error) => {
                        let _ = fs::remove_file(&final_path).await;
                        return Err(error);
                    }
                };
                let resolving = match self
                    .repos
                    .download
                    .compare_and_set_state(
                        task_id,
                        DownloadState::Resolving,
                        DownloadState::Verifying,
                    )
                    .await
                {
                    Ok(value) => value,
                    Err(_) => {
                        self.compensate_failed_promotion(&final_path, offline).await;
                        return Err(repository_failure());
                    }
                };
                if !resolving {
                    self.compensate_failed_promotion(&final_path, offline).await;
                    return Ok(());
                }
                let associated = match self
                    .repos
                    .download
                    .associate_offline_resource(task_id, DownloadState::Verifying, offline.id)
                    .await
                {
                    Ok(value) => value,
                    Err(_) => {
                        self.compensate_failed_promotion(&final_path, offline).await;
                        return Err(repository_failure());
                    }
                };
                if !associated {
                    self.compensate_failed_promotion(&final_path, offline).await;
                    return Err(repository_failure());
                }
                let completed = match self
                    .repos
                    .download
                    .compare_and_set_state(
                        task_id,
                        DownloadState::Verifying,
                        DownloadState::Completed,
                    )
                    .await
                {
                    Ok(value) => value,
                    Err(_) => {
                        self.compensate_failed_promotion(&final_path, offline).await;
                        return Err(repository_failure());
                    }
                };
                if !completed {
                    self.compensate_failed_promotion(&final_path, offline).await;
                }
                return Ok(());
            }
            Ok(_) => fs::remove_file(&final_path)
                .await
                .map_err(|error| io_failure(&error, "DOWNLOAD_DIRECTORY_UNAVAILABLE"))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_failure(&error, "DOWNLOAD_DIRECTORY_UNAVAILABLE")),
        }

        let mut partial = match fs::metadata(&part_path).await {
            Ok(meta) if meta.len() <= total => meta.len(),
            Ok(_) => {
                fs::remove_file(&part_path)
                    .await
                    .map_err(|error| io_failure(&error, "DOWNLOAD_DIRECTORY_UNAVAILABLE"))?;
                0
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(io_failure(&error, "DOWNLOAD_DIRECTORY_UNAVAILABLE")),
        };
        self.ensure_space(&offline_dir, total.saturating_sub(partial))
            .await?;
        let mut source = File::open(&source_path)
            .await
            .map_err(|error| io_failure(&error, "DOWNLOAD_SOURCE_UNAVAILABLE"))?;
        source
            .seek(std::io::SeekFrom::Start(partial))
            .await
            .map_err(|error| io_failure(&error, "DOWNLOAD_SOURCE_UNAVAILABLE"))?;
        let mut output = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&part_path)
            .await
            .map_err(|error| io_failure(&error, "DOWNLOAD_DIRECTORY_UNAVAILABLE"))?;
        if !self
            .repos
            .download
            .compare_and_set_state(
                task_id,
                DownloadState::Resolving,
                DownloadState::Downloading,
            )
            .await
            .map_err(|_| repository_failure())?
        {
            self.remove_partial_if_cancelled(task_id, &part_path).await;
            return Ok(());
        }
        self.repos
            .download
            .update_progress(
                task_id,
                DownloadState::Downloading,
                Some(total),
                partial,
                None,
                None,
            )
            .await
            .map_err(|_| repository_failure())?;

        let mut buffer = vec![0_u8; CHUNK_SIZE];
        let transfer_started = Instant::now();
        let transfer_start_bytes = partial;
        let mut last_event_at = Instant::now() - Duration::from_millis(100);
        let mut last_space_check = Instant::now();
        loop {
            let state = self
                .repos
                .download
                .get(task_id)
                .await
                .map_err(|_| repository_failure())?
                .ok_or_else(repository_failure)?
                .state;
            match state {
                DownloadState::Downloading => {}
                DownloadState::Cancelled => {
                    drop(output);
                    let _ = fs::remove_file(&part_path).await;
                    return Ok(());
                }
                // Paused/Interrupted 保留 .part，之后 Resume 会继续。
                _ => return Ok(()),
            }
            let read = source
                .read(&mut buffer)
                .await
                .map_err(|error| io_failure(&error, "DOWNLOAD_SOURCE_UNAVAILABLE"))?;
            if read == 0 {
                break;
            }
            output
                .write_all(&buffer[..read])
                .await
                .map_err(|error| io_failure(&error, "DOWNLOAD_IO_FAILED"))?;
            partial = partial.saturating_add(read as u64);
            if last_space_check.elapsed() >= SPACE_RECHECK_INTERVAL {
                self.ensure_space(&offline_dir, total.saturating_sub(partial))
                    .await?;
                last_space_check = Instant::now();
            }
            if let Some(limit) = policy.speed_limit_bps {
                throttle_transfer(
                    transfer_started,
                    partial.saturating_sub(transfer_start_bytes),
                    limit,
                )
                .await;
            }
            let elapsed = transfer_started.elapsed().as_secs_f64();
            let speed_bps = (elapsed > 0.0).then(|| {
                ((partial.saturating_sub(transfer_start_bytes)) as f64 / elapsed).round() as u64
            });
            let eta_seconds = speed_bps
                .filter(|speed| *speed > 0)
                .map(|speed| total.saturating_sub(partial).div_ceil(speed));
            self.repos
                .download
                .update_progress(
                    task_id,
                    DownloadState::Downloading,
                    Some(total),
                    partial,
                    speed_bps,
                    eta_seconds,
                )
                .await
                .map_err(|_| repository_failure())?;
            if last_event_at.elapsed() >= Duration::from_millis(100) || partial == total {
                let mut snapshot = task.clone();
                snapshot.state = DownloadState::Downloading;
                snapshot.bytes_total = Some(total);
                snapshot.bytes_downloaded = partial;
                snapshot.speed_bps = speed_bps;
                snapshot.eta_seconds = eta_seconds;
                self.events.emit_task(&snapshot, None);
                last_event_at = Instant::now();
            }
        }
        output
            .flush()
            .await
            .map_err(|error| io_failure(&error, "DOWNLOAD_IO_FAILED"))?;
        output
            .sync_all()
            .await
            .map_err(|error| io_failure(&error, "DOWNLOAD_IO_FAILED"))?;
        drop(output);
        if partial != total {
            return Err(DownloadWorkerFailure::new("DOWNLOAD_PARTIAL_INVALID", true));
        }
        if !self
            .repos
            .download
            .compare_and_set_state(
                task_id,
                DownloadState::Downloading,
                DownloadState::Verifying,
            )
            .await
            .map_err(|_| repository_failure())?
        {
            self.remove_partial_if_cancelled(task_id, &part_path).await;
            return Ok(());
        }
        let part_meta = fs::metadata(&part_path)
            .await
            .map_err(|error| io_failure(&error, "DOWNLOAD_PARTIAL_INVALID"))?;
        if part_meta.len() != total {
            return Err(DownloadWorkerFailure::new("DOWNLOAD_PARTIAL_INVALID", true));
        }
        // 同一目录内 rename 是原子的；完成态之前不暴露最终文件。
        fs::rename(&part_path, &final_path)
            .await
            .map_err(|error| io_failure(&error, "DOWNLOAD_DIRECTORY_UNAVAILABLE"))?;
        let offline = match self
            .ensure_offline_resource(&resource, task.target_storage_id, &final_path, total)
            .await
        {
            Ok(handle) => handle,
            Err(error) => {
                let _ = fs::remove_file(&final_path).await;
                return Err(error);
            }
        };
        let associated = match self
            .repos
            .download
            .associate_offline_resource(task_id, DownloadState::Verifying, offline.id)
            .await
        {
            Ok(value) => value,
            Err(_) => {
                self.compensate_failed_promotion(&final_path, offline).await;
                return Err(repository_failure());
            }
        };
        if !associated {
            self.compensate_failed_promotion(&final_path, offline).await;
            return Err(repository_failure());
        }
        let completed = match self
            .repos
            .download
            .compare_and_set_state(task_id, DownloadState::Verifying, DownloadState::Completed)
            .await
        {
            Ok(value) => value,
            Err(_) => {
                self.compensate_failed_promotion(&final_path, offline).await;
                return Err(repository_failure());
            }
        };
        if !completed {
            self.compensate_failed_promotion(&final_path, offline).await;
        }
        Ok(())
    }

    async fn run_remote_inner(
        &self,
        task_id: DownloadTaskId,
        task: &DownloadTask,
        resource: &Resource,
        storage: &haven_domain::entities::StorageLocation,
        source_id: haven_domain::ids::SourceId,
        remote_id: &str,
    ) -> Result<(), DownloadWorkerFailure> {
        let source_key =
            haven_application::services::source_import::source_key_for_id(source_id)
                .ok_or_else(|| DownloadWorkerFailure::new("DOWNLOAD_SOURCE_UNAVAILABLE", false))?;
        let offline_dir = download_category_dir(Path::new(&storage.root_ref), resource);
        fs::create_dir_all(&offline_dir)
            .await
            .map_err(|error| io_failure(&error, "DOWNLOAD_DIRECTORY_UNAVAILABLE"))?;
        let extension = resource_extension(resource);
        let final_path = offline_dir.join(format!("{}.{}", task_id, extension));
        let part_path = offline_dir.join(format!("{}.part", task_id));

        // A previous successful run may have completed the file before the
        // process was interrupted. Reuse it only after checking its expected
        // container/magic; a truncated or unrelated file must be reacquired.
        if let Ok(meta) = fs::metadata(&final_path).await {
            if meta.is_file()
                && meta.len() > 0
                && validate_remote_payload(&final_path, resource).await.is_ok()
            {
                let total = meta.len();
                let offline = match self
                    .ensure_offline_resource(resource, task.target_storage_id, &final_path, total)
                    .await
                {
                    Ok(handle) => handle,
                    Err(error) => {
                        let _ = fs::remove_file(&final_path).await;
                        return Err(error);
                    }
                };
                let resolving = match self
                    .repos
                    .download
                    .compare_and_set_state(
                        task_id,
                        DownloadState::Resolving,
                        DownloadState::Verifying,
                    )
                    .await
                {
                    Ok(value) => value,
                    Err(_) => {
                        self.compensate_failed_promotion(&final_path, offline).await;
                        return Err(repository_failure());
                    }
                };
                if !resolving {
                    self.compensate_failed_promotion(&final_path, offline).await;
                    return Ok(());
                }
                let associated = match self
                    .repos
                    .download
                    .associate_offline_resource(task_id, DownloadState::Verifying, offline.id)
                    .await
                {
                    Ok(value) => value,
                    Err(_) => {
                        self.compensate_failed_promotion(&final_path, offline).await;
                        return Err(repository_failure());
                    }
                };
                if !associated {
                    self.compensate_failed_promotion(&final_path, offline).await;
                    return Err(repository_failure());
                }
                let completed = match self
                    .repos
                    .download
                    .compare_and_set_state(
                        task_id,
                        DownloadState::Verifying,
                        DownloadState::Completed,
                    )
                    .await
                {
                    Ok(value) => value,
                    Err(_) => {
                        self.compensate_failed_promotion(&final_path, offline).await;
                        return Err(repository_failure());
                    }
                };
                if !completed {
                    self.compensate_failed_promotion(&final_path, offline).await;
                }
                return Ok(());
            }
            let _ = fs::remove_file(&final_path).await;
        }

        // Remote providers currently acquire a complete object rather than a
        // byte-range resume. Remove stale worker/provider partials before a new
        // attempt so Windows rename cannot collide with an old `.part` file.
        match fs::remove_file(&part_path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(io_failure(&error, "DOWNLOAD_DIRECTORY_UNAVAILABLE"));
            }
        }
        self.ensure_space(&offline_dir, resource.size.unwrap_or(0))
            .await?;
        if !self
            .repos
            .download
            .compare_and_set_state(
                task_id,
                DownloadState::Resolving,
                DownloadState::Downloading,
            )
            .await
            .map_err(|_| repository_failure())?
        {
            return Ok(());
        }

        // Poll the task state while the provider is awaiting network IO. The
        // future is dropped on cancellation, which aborts reqwest's in-flight
        // request; providers also clean their own temporary file on errors.
        let acquisition = self.remote.acquire(source_key, remote_id, &part_path);
        tokio::pin!(acquisition);
        let acquired = loop {
            tokio::select! {
                result = &mut acquisition => match result {
                    Ok(acquired) => break acquired,
                    Err(error) => {
                        let _ = fs::remove_file(&part_path).await;
                        return Err(map_remote_error(error));
                    }
                },
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    let state = self
                        .repos
                        .download
                        .get(task_id)
                        .await
                        .map_err(|_| repository_failure())?
                        .ok_or_else(repository_failure)?
                        .state;
                    if state == DownloadState::Cancelled {
                        let _ = fs::remove_file(&part_path).await;
                        return Ok(());
                    }
                }
            }
        };
        let current = self
            .repos
            .download
            .get(task_id)
            .await
            .map_err(|_| repository_failure())?
            .ok_or_else(repository_failure)?;
        if current.state != DownloadState::Downloading {
            let _ = fs::remove_file(&part_path).await;
            return Ok(());
        }
        let total = acquired.size_bytes;
        if total == 0 {
            let _ = fs::remove_file(&part_path).await;
            return Err(DownloadWorkerFailure::new(
                "DOWNLOAD_PARTIAL_INVALID",
                false,
            ));
        }
        if !remote_mime_matches(resource, &acquired.mime)
            || validate_remote_payload(&part_path, resource).await.is_err()
        {
            let _ = fs::remove_file(&part_path).await;
            return Err(DownloadWorkerFailure::new(
                "DOWNLOAD_PARTIAL_INVALID",
                false,
            ));
        }
        // Providers write the complete object to `.part`; recheck the reserve
        // after that write so an unknown-size source cannot consume the last
        // available space while still being promoted to an Offline Resource.
        if let Err(error) = self.ensure_space(&offline_dir, 0).await {
            let _ = fs::remove_file(&part_path).await;
            return Err(error);
        }
        self.repos
            .download
            .update_progress(
                task_id,
                DownloadState::Downloading,
                Some(total),
                total,
                None,
                None,
            )
            .await
            .map_err(|_| repository_failure())?;
        if !self
            .repos
            .download
            .compare_and_set_state(
                task_id,
                DownloadState::Downloading,
                DownloadState::Verifying,
            )
            .await
            .map_err(|_| repository_failure())?
        {
            let _ = fs::remove_file(&part_path).await;
            return Ok(());
        }
        let part_meta = fs::metadata(&part_path)
            .await
            .map_err(|error| io_failure(&error, "DOWNLOAD_PARTIAL_INVALID"))?;
        if part_meta.len() != total {
            let _ = fs::remove_file(&part_path).await;
            return Err(DownloadWorkerFailure::new(
                "DOWNLOAD_PARTIAL_INVALID",
                false,
            ));
        }
        fs::rename(&part_path, &final_path).await.map_err(|error| {
            let _ = std::fs::remove_file(&part_path);
            io_failure(&error, "DOWNLOAD_DIRECTORY_UNAVAILABLE")
        })?;
        let offline = match self
            .ensure_offline_resource(resource, task.target_storage_id, &final_path, total)
            .await
        {
            Ok(handle) => handle,
            Err(error) => {
                let _ = fs::remove_file(&final_path).await;
                return Err(error);
            }
        };
        let associated = match self
            .repos
            .download
            .associate_offline_resource(task_id, DownloadState::Verifying, offline.id)
            .await
        {
            Ok(value) => value,
            Err(_) => {
                self.compensate_failed_promotion(&final_path, offline).await;
                return Err(repository_failure());
            }
        };
        if !associated {
            self.compensate_failed_promotion(&final_path, offline).await;
            return Err(repository_failure());
        }
        let completed = match self
            .repos
            .download
            .compare_and_set_state(task_id, DownloadState::Verifying, DownloadState::Completed)
            .await
        {
            Ok(value) => value,
            Err(_) => {
                self.compensate_failed_promotion(&final_path, offline).await;
                return Err(repository_failure());
            }
        };
        if !completed {
            self.compensate_failed_promotion(&final_path, offline).await;
        }
        Ok(())
    }

    async fn ensure_offline_resource(
        &self,
        source: &Resource,
        target_storage_id: haven_domain::ids::StorageLocationId,
        final_path: &Path,
        total: u64,
    ) -> Result<OfflineResourceHandle, DownloadWorkerFailure> {
        let resources = self
            .repos
            .resource
            .list_by_media_item(source.media_item_id)
            .await
            .map_err(|_| repository_failure())?;
        let final_string = final_path.to_string_lossy().into_owned();
        if let Some(existing) = resources.iter().find(|item| {
            matches!(&item.locator, ResourceLocator::LocalPath { path } if path == &final_string)
        }) {
            return Ok(OfflineResourceHandle {
                id: existing.id,
                created: false,
            });
        }
        let now = haven_common::UtcMillis::now();
        let mut offline = source.clone();
        offline.id = haven_domain::ids::ResourceId::new();
        offline.storage_location_id = Some(target_storage_id);
        offline.locator = ResourceLocator::LocalPath { path: final_string };
        offline.size = Some(total);
        offline.availability = Availability::OfflineAvailable;
        offline.availability_source = AvailabilitySource::User;
        offline.created_at = now;
        offline.updated_at = now;
        let offline_id = offline.id;
        self.repos
            .resource
            .save(&offline)
            .await
            .map_err(|_| repository_failure())?;
        Ok(OfflineResourceHandle {
            id: offline_id,
            created: true,
        })
    }

    /// Compensate a promotion that could not be associated with a completed
    /// task.  Only artifacts created by this worker are removed; a pre-existing
    /// valid offline resource may be shared by a later retry and is retained.
    async fn compensate_failed_promotion(&self, final_path: &Path, offline: OfflineResourceHandle) {
        if offline.created {
            let _ = self.repos.resource.delete(offline.id).await;
            let _ = fs::remove_file(final_path).await;
        }
    }

    async fn ensure_space(
        &self,
        offline_dir: &Path,
        required_bytes: u64,
    ) -> Result<(), DownloadWorkerFailure> {
        let directory = offline_dir.to_path_buf();
        let available = tokio::task::spawn_blocking(move || fs2::available_space(directory))
            .await
            .map_err(|_| DownloadWorkerFailure::new("DOWNLOAD_DIRECTORY_UNAVAILABLE", true))?
            .map_err(|error| io_failure(&error, "DOWNLOAD_DIRECTORY_UNAVAILABLE"))?;
        if has_enough_space(available, required_bytes, DISK_SPACE_RESERVE_BYTES) {
            Ok(())
        } else {
            Err(DownloadWorkerFailure::new("DOWNLOAD_DISK_SPACE_LOW", false))
        }
    }

    async fn requeue_after_failure(&self, task_id: DownloadTaskId) -> bool {
        for state in [
            DownloadState::Resolving,
            DownloadState::Downloading,
            DownloadState::Verifying,
        ] {
            if self
                .repos
                .download
                .compare_and_set_state(task_id, state, DownloadState::Queued)
                .await
                .unwrap_or(false)
            {
                return true;
            }
        }
        false
    }

    async fn fail_task(&self, task_id: DownloadTaskId) {
        for state in [
            DownloadState::Resolving,
            DownloadState::Downloading,
            DownloadState::Verifying,
        ] {
            if self
                .repos
                .download
                .compare_and_set_state(task_id, state, DownloadState::Failed)
                .await
                .unwrap_or(false)
            {
                break;
            }
        }
    }

    async fn remove_partial_if_cancelled(&self, task_id: DownloadTaskId, part_path: &Path) {
        if matches!(
            self.repos.download.get(task_id).await,
            Ok(Some(task)) if task.state == DownloadState::Cancelled
        ) {
            let _ = fs::remove_file(part_path).await;
        }
    }
}

/// Resolve a LocalPath only through the StorageLocation that owns the
/// resource. Canonicalization happens before containment checking so `..`
/// components and symlink escapes are rejected identically. This helper is
/// intentionally shared by the worker path instead of accepting an arbitrary
/// filesystem path from the Wire layer.
async fn resolve_registered_local_source(
    repos: &SqliteRepositories,
    resource: &Resource,
    path: &str,
) -> Result<PathBuf, DownloadWorkerFailure> {
    let source_storage_id = resource
        .storage_location_id
        .ok_or_else(|| DownloadWorkerFailure::new("DOWNLOAD_SOURCE_UNAVAILABLE", false))?;
    let source_storage = repos
        .storage_location
        .get(source_storage_id)
        .await
        .map_err(|_| DownloadWorkerFailure::new("DOWNLOAD_SOURCE_UNAVAILABLE", false))?
        .ok_or_else(|| DownloadWorkerFailure::new("DOWNLOAD_SOURCE_UNAVAILABLE", false))?;
    if source_storage.provider_type != StorageProviderType::Local
        || !matches!(
            source_storage.status,
            StorageStatus::Connected | StorageStatus::ReadOnly
        )
    {
        return Err(DownloadWorkerFailure::new(
            "DOWNLOAD_SOURCE_UNAVAILABLE",
            false,
        ));
    }

    let root = fs::canonicalize(&source_storage.root_ref)
        .await
        .map_err(|_| DownloadWorkerFailure::new("DOWNLOAD_SOURCE_UNAVAILABLE", false))?;
    let root_metadata = fs::metadata(&root)
        .await
        .map_err(|_| DownloadWorkerFailure::new("DOWNLOAD_SOURCE_UNAVAILABLE", false))?;
    if !root_metadata.is_dir() {
        return Err(DownloadWorkerFailure::new(
            "DOWNLOAD_SOURCE_UNAVAILABLE",
            false,
        ));
    }
    let raw_file = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        root.join(path)
    };
    let canonical = fs::canonicalize(raw_file)
        .await
        .map_err(|_| DownloadWorkerFailure::new("DOWNLOAD_SOURCE_UNAVAILABLE", false))?;
    if canonical == root || !canonical.starts_with(&root) {
        return Err(DownloadWorkerFailure::new(
            "DOWNLOAD_SOURCE_UNAVAILABLE",
            false,
        ));
    }
    let metadata = fs::metadata(&canonical)
        .await
        .map_err(|_| DownloadWorkerFailure::new("DOWNLOAD_SOURCE_UNAVAILABLE", false))?;
    if !metadata.is_file() {
        return Err(DownloadWorkerFailure::new(
            "DOWNLOAD_SOURCE_UNAVAILABLE",
            false,
        ));
    }
    Ok(canonical)
}

impl DownloadRunner for LocalDownloadRunner {
    fn start(&self, task_id: DownloadTaskId) {
        let should_start = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(task_id);
        if !should_start {
            return;
        }
        let runner = self.clone();
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            runner
                .active
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&task_id);
            return;
        };
        handle.spawn(async move { runner.run(task_id).await });
    }
}

async fn throttle_transfer(started: Instant, bytes: u64, limit_bps: u64) {
    let delay = required_transfer_delay(started, bytes, limit_bps);
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
}

fn required_transfer_delay(started: Instant, bytes: u64, limit_bps: u64) -> Duration {
    if limit_bps == 0 || bytes == 0 {
        return Duration::ZERO;
    }
    let expected = Duration::from_secs_f64(bytes as f64 / limit_bps as f64);
    expected.saturating_sub(started.elapsed())
}

fn safe_extension(path: &Path) -> String {
    let raw = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("bin");
    let filtered: String = raw
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(12)
        .collect();
    if filtered.is_empty() {
        "bin".into()
    } else {
        filtered.to_ascii_lowercase()
    }
}

fn resource_extension(resource: &Resource) -> &'static str {
    match resource.resource_type {
        haven_domain::enums::ResourceType::ComicArchive => "cbz",
        haven_domain::enums::ResourceType::PublicationFile => {
            if resource
                .mime_type
                .as_deref()
                .is_some_and(|mime| mime.to_ascii_lowercase().contains("epub"))
            {
                "epub"
            } else {
                "pdf"
            }
        }
        haven_domain::enums::ResourceType::ArticleSnapshot => "html",
        _ => "bin",
    }
}

fn zip_magic(header: &[u8]) -> bool {
    header.starts_with(b"PK\x03\x04")
        || header.starts_with(b"PK\x05\x06")
        || header.starts_with(b"PK\x07\x08")
}

fn normalized_mime(value: &str) -> &str {
    value.split(';').next().unwrap_or(value).trim()
}

fn remote_mime_matches(resource: &Resource, actual: &str) -> bool {
    let actual = normalized_mime(actual).to_ascii_lowercase();
    match resource.resource_type {
        haven_domain::enums::ResourceType::ComicArchive => {
            actual == "application/vnd.comicbook+zip" || actual == "application/zip"
        }
        haven_domain::enums::ResourceType::PublicationFile => {
            let expected = resource
                .mime_type
                .as_deref()
                .map(normalized_mime)
                .unwrap_or_default()
                .to_ascii_lowercase();
            if expected.contains("epub") {
                actual.contains("epub") || actual == "application/zip"
            } else if expected.contains("pdf") {
                actual == "application/pdf"
            } else {
                actual == "application/pdf"
                    || actual.contains("epub")
                    || actual == "application/zip"
            }
        }
        haven_domain::enums::ResourceType::ArticleSnapshot => {
            actual == "text/html" || actual == "application/xhtml+xml"
        }
        _ => false,
    }
}

async fn validate_remote_payload(path: &Path, resource: &Resource) -> Result<(), ()> {
    let mut file = File::open(path).await.map_err(|_| ())?;
    let mut header = [0_u8; 8];
    let read = file.read(&mut header).await.map_err(|_| ())?;
    if read == 0 {
        return Err(());
    }
    let mime = resource.mime_type.as_deref().unwrap_or_default();
    let valid_magic = match resource.resource_type {
        haven_domain::enums::ResourceType::ComicArchive => zip_magic(&header[..read]),
        haven_domain::enums::ResourceType::PublicationFile
            if mime.to_ascii_lowercase().contains("epub") =>
        {
            zip_magic(&header[..read])
        }
        haven_domain::enums::ResourceType::PublicationFile => header[..read].starts_with(b"%PDF-"),
        haven_domain::enums::ResourceType::ArticleSnapshot => header[0] == b'<',
        _ => false,
    };
    valid_magic.then_some(()).ok_or(())
}

fn validate_relative_object_path(path: &str) -> Result<(), ()> {
    if path.is_empty()
        || path.contains('\\')
        || Path::new(path).is_absolute()
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(());
    }
    Ok(())
}

fn download_category_dir(root: &Path, resource: &Resource) -> PathBuf {
    let category = match resource.resource_type {
        haven_domain::enums::ResourceType::ComicArchive
        | haven_domain::enums::ResourceType::ImageSequence
        | haven_domain::enums::ResourceType::RemoteChapter
        | haven_domain::enums::ResourceType::RemotePageSet => "comics",
        haven_domain::enums::ResourceType::PublicationFile => {
            if resource
                .mime_type
                .as_deref()
                .is_some_and(|mime| mime.to_ascii_lowercase().contains("epub"))
            {
                "books"
            } else {
                "articles"
            }
        }
        haven_domain::enums::ResourceType::ArticleSnapshot => "articles",
        _ => "books",
    };
    root.join(category)
}

fn map_remote_error(error: AppError) -> DownloadWorkerFailure {
    match error.code().as_str() {
        "DOWNLOAD_DISK_SPACE_LOW" => DownloadWorkerFailure::new("DOWNLOAD_DISK_SPACE_LOW", false),
        "DOWNLOAD_DIRECTORY_UNAVAILABLE" | "STORAGE_ERROR" => {
            DownloadWorkerFailure::new("DOWNLOAD_DIRECTORY_UNAVAILABLE", true)
        }
        "SECURITY_POLICY_DENIED" | "INVALID_ARGUMENT" => {
            DownloadWorkerFailure::new("DOWNLOAD_SOURCE_UNAVAILABLE", false)
        }
        _ => DownloadWorkerFailure::new("DOWNLOAD_SOURCE_UNAVAILABLE", error.retryable()),
    }
}

async fn registered_offline_path(
    storage: &haven_domain::entities::StorageLocation,
    resource: &Resource,
) -> Result<PathBuf, AppError> {
    if storage.provider_type != StorageProviderType::Local
        || resource.storage_location_id != Some(storage.id)
        || resource.availability != Availability::OfflineAvailable
    {
        return Err(invalid_offline_path());
    }
    let ResourceLocator::LocalPath { path } = &resource.locator else {
        return Err(invalid_offline_path());
    };
    let canonical_file = fs::canonicalize(path).await.map_err(|error| {
        AppError::new(
            "DOWNLOAD_OFFLINE_FILE_NOT_FOUND",
            ErrorKind::NotFound,
            "离线文件不存在",
            false,
        )
        .with_source(error)
    })?;
    let root = Path::new(&storage.root_ref);
    let mut contained = false;
    for candidate in [
        root.join(".haven").join("offline"),
        root.join("books"),
        root.join("comics"),
        root.join("articles"),
    ] {
        if let Ok(canonical_root) = fs::canonicalize(candidate).await {
            if canonical_file.starts_with(&canonical_root) && canonical_file != canonical_root {
                contained = true;
                break;
            }
        }
    }
    if !contained {
        return Err(invalid_offline_path());
    }
    let metadata = fs::metadata(&canonical_file).await.map_err(|error| {
        AppError::new(
            "DOWNLOAD_OFFLINE_FILE_NOT_FOUND",
            ErrorKind::NotFound,
            "离线文件不存在",
            false,
        )
        .with_source(error)
    })?;
    if !metadata.is_file() {
        return Err(invalid_offline_path());
    }
    Ok(canonical_file)
}

fn invalid_offline_path() -> AppError {
    AppError::new(
        "DOWNLOAD_OFFLINE_PATH_INVALID",
        ErrorKind::Security,
        "离线资源路径不在已登记的离线目录中",
        false,
    )
}

fn reveal_with_system(path: &Path) -> Result<(), AppError> {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer.exe")
        .arg(format!("/select,{}", path.display()))
        .spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .spawn();

    result.map(|_| ()).map_err(|error| {
        AppError::new(
            "DOWNLOAD_REVEAL_FAILED",
            ErrorKind::Io,
            "无法打开文件所在位置",
            true,
        )
        .with_source(error)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use haven_application::services::DownloadService;
    use haven_application::services::download_batch::DownloadBatchService;
    use haven_application::services::ports::RemoteAcquiredFile;
    use haven_application::services::settings::{SettingsService, SettingsTxPorts, SettingsUoW};
    use haven_application::wire::{DownloadCreateRequest, DownloadStateDto};
    use haven_domain::contracts::{
        DownloadBatchRepository, EditionRepository, MediaItemRepository, WorkRepository,
    };
    use haven_domain::entities::{
        ArtworkSet, DownloadBatch, DownloadTask, Edition, MediaIndex, MediaItem, Resource,
        StorageLocation, Work,
    };
    use haven_domain::enums::{
        BatchState, MediaItemStatus, MediaType, ResourceType, WorkStatus, WorkType,
    };
    use haven_domain::ids::{
        DownloadBatchId, DownloadTaskId, EditionId, MediaItemId, ResourceId, StorageLocationId,
        WorkId,
    };
    use haven_domain::settings::{
        DownloadConcurrency, DownloadPatch, DownloadSpeedLimit, SettingsPatch, SettingsSection,
    };
    use std::io::{Cursor, Write as _};
    use tokio::sync::Notify;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    struct NoopDownloadEventSink;

    impl DownloadEventSink for NoopDownloadEventSink {
        fn emit_task(&self, _task: &DownloadTask, _error_code: Option<&str>) {}
    }

    #[derive(Default)]
    struct CountingDownloadRunner {
        starts: AtomicUsize,
    }

    impl DownloadRunner for CountingDownloadRunner {
        fn start(&self, _task_id: DownloadTaskId) {
            self.starts.fetch_add(1, Ordering::Relaxed);
        }
    }

    struct FailingSettingsUow;

    impl SettingsUoW for FailingSettingsUow {
        fn run(
            &self,
            _f: &dyn Fn(&dyn SettingsTxPorts) -> Result<(), AppError>,
        ) -> Result<(), AppError> {
            Err(AppError::new(
                "DATABASE_ERROR",
                ErrorKind::Database,
                "测试设置读取失败",
                true,
            ))
        }

        fn run_read(
            &self,
            _f: &dyn Fn(&dyn SettingsTxPorts) -> Result<(), AppError>,
        ) -> Result<(), AppError> {
            Err(AppError::new(
                "DATABASE_ERROR",
                ErrorKind::Database,
                "测试设置读取失败",
                true,
            ))
        }
    }

    fn settings_service(db: Arc<crate::Db>) -> Arc<SettingsService> {
        Arc::new(SettingsService::new(Arc::new(
            crate::db::repos::SqliteSettingsUoW::new(db),
        )))
    }

    /// 调度器测试用最小任务（仅携带调度键与优先级）。
    fn scheduling_task(
        provider_key: Option<String>,
        host_key: Option<String>,
        priority: DownloadPriority,
    ) -> DownloadTask {
        DownloadTask {
            id: DownloadTaskId::new(),
            work_id: None,
            edition_id: None,
            media_item_id: None,
            source_resource_id: ResourceId::new(),
            target_storage_id: StorageLocationId::new(),
            offline_resource_id: None,
            state: DownloadState::Queued,
            bytes_total: None,
            bytes_downloaded: 0,
            speed_bps: None,
            eta_seconds: None,
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
            batch_id: None,
            priority,
            provider_key,
            host_key,
            variant_key: String::new(),
            resource_identity: None,
            retry_count: 0,
            not_before: None,
            resumable: None,
        }
    }

    #[test]
    fn disk_space_check_is_overflow_safe_and_keeps_reserve() {
        assert!(has_enough_space(128, 64, 64));
        assert!(!has_enough_space(127, 64, 64));
        assert!(has_enough_space(u64::MAX, u64::MAX, u64::MAX));
        assert!(has_enough_space(16, 0, 16));
        assert!(!has_enough_space(15, 0, 16));
    }

    #[tokio::test]
    async fn local_source_path_is_bound_to_storage_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        let outside = temp.path().join("outside.bin");
        std::fs::create_dir_all(&root).unwrap();
        let inside = root.join("inside.bin");
        std::fs::write(&inside, b"inside").unwrap();
        std::fs::write(&outside, b"outside").unwrap();

        let db = Arc::new(crate::Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db));
        let storage_id = StorageLocationId::new();
        repos
            .storage_location
            .save(&StorageLocation {
                id: storage_id,
                provider_type: StorageProviderType::Local,
                display_name: "源目录".into(),
                root_ref: root.to_string_lossy().into_owned(),
                credential_ref: None,
                status: StorageStatus::Connected,
                created_at: haven_common::UtcMillis(1),
                updated_at: haven_common::UtcMillis(1),
            })
            .await
            .unwrap();
        let resource = Resource {
            id: ResourceId::new(),
            media_item_id: MediaItemId::new(),
            resource_type: ResourceType::LocalFile,
            source_id: None,
            storage_location_id: Some(storage_id),
            locator: ResourceLocator::LocalPath {
                path: inside.to_string_lossy().into_owned(),
            },
            mime_type: None,
            size: None,
            hash: None,
            availability: Availability::Available,
            availability_source: AvailabilitySource::User,
            modified_ms: None,
            fingerprint_first: None,
            fingerprint_last: None,
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        };

        assert!(
            resolve_registered_local_source(&repos, &resource, "inside.bin")
                .await
                .is_ok()
        );
        assert!(
            resolve_registered_local_source(&repos, &resource, &outside.to_string_lossy())
                .await
                .is_err()
        );
        assert!(
            resolve_registered_local_source(&repos, &resource, "..")
                .await
                .is_err()
        );
        assert!(
            resolve_registered_local_source(&repos, &resource, &root.to_string_lossy())
                .await
                .is_err()
        );
    }

    #[test]
    fn io_errors_map_to_stable_codes_without_platform_text() {
        let permission = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "private path");
        assert_eq!(
            io_failure(&permission, "DOWNLOAD_DIRECTORY_UNAVAILABLE").code,
            "DOWNLOAD_PERMISSION_DENIED"
        );
        assert!(!io_failure(&permission, "DOWNLOAD_DIRECTORY_UNAVAILABLE").retryable);

        let full = std::io::Error::new(std::io::ErrorKind::StorageFull, "disk full");
        assert_eq!(
            io_failure(&full, "DOWNLOAD_IO_FAILED").code,
            "DOWNLOAD_DISK_SPACE_LOW"
        );
        assert!(!io_failure(&full, "DOWNLOAD_IO_FAILED").retryable);

        let missing = std::io::Error::new(std::io::ErrorKind::NotFound, "C:\\secret\\file");
        let failure = io_failure(&missing, "DOWNLOAD_DIRECTORY_UNAVAILABLE");
        assert_eq!(failure.code, "DOWNLOAD_DIRECTORY_UNAVAILABLE");
        assert!(!failure.retryable);
    }

    #[test]
    fn retry_policy_only_retries_transient_io_and_partial_failures() {
        assert!(DownloadWorkerFailure::new("DOWNLOAD_IO_FAILED", true).retryable);
        assert!(DownloadWorkerFailure::new("DOWNLOAD_PARTIAL_INVALID", true).retryable);
        assert!(!DownloadWorkerFailure::new("DOWNLOAD_DISK_SPACE_LOW", false).retryable);
        assert!(!DownloadWorkerFailure::new("DOWNLOAD_PERMISSION_DENIED", false).retryable);
    }

    #[tokio::test]
    async fn download_policy_defaults_and_reads_persisted_values() {
        let db = Arc::new(crate::Db::open_in_memory().unwrap());
        let settings = settings_service(db);

        assert_eq!(
            load_download_policy(&settings).await,
            DownloadPolicy::default(),
            "未保存设置必须使用安全默认策略"
        );

        settings
            .update(
                SettingsSection::Downloads,
                None,
                SettingsPatch::Downloads(DownloadPatch {
                    concurrent_tasks: Some(DownloadConcurrency::Five),
                    speed_limit: Some(DownloadSpeedLimit::Mbps2),
                    auto_continue: None,
                }),
            )
            .await
            .unwrap();

        assert_eq!(
            load_download_policy(&settings).await,
            DownloadPolicy {
                max_concurrent: 5,
                speed_limit_bps: Some(2 * 1024 * 1024),
            }
        );
    }

    #[tokio::test]
    async fn download_policy_falls_back_when_settings_read_fails() {
        let settings = SettingsService::new(Arc::new(FailingSettingsUow));
        assert_eq!(
            load_download_policy(&settings).await,
            DownloadPolicy::default()
        );
    }

    #[tokio::test]
    async fn scheduler_respects_global_limit_and_releases_waiting_task() {
        let db = Arc::new(crate::Db::open_in_memory().unwrap());
        let settings = settings_service(db);
        settings
            .update(
                SettingsSection::Downloads,
                None,
                SettingsPatch::Downloads(DownloadPatch {
                    concurrent_tasks: Some(DownloadConcurrency::One),
                    speed_limit: None,
                    auto_continue: None,
                }),
            )
            .await
            .unwrap();

        let scheduler = Arc::new(DownloadScheduler::new());
        let task = scheduling_task(None, None, DownloadPriority::Normal);
        let first = scheduler.acquire(&task, &settings).await;
        assert_eq!(
            *scheduler
                .global_active
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
            1
        );

        let waiting_scheduler = Arc::clone(&scheduler);
        let waiting_settings = Arc::clone(&settings);
        let waiting_task = scheduling_task(None, None, DownloadPriority::Normal);
        let (started_tx, mut started_rx) = tokio::sync::oneshot::channel();
        let waiting = tokio::spawn(async move {
            let _ = started_tx.send(());
            waiting_scheduler
                .acquire(&waiting_task, &waiting_settings)
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), &mut started_rx)
            .await
            .expect("等待任务应启动")
            .expect("等待任务通知不应丢失");
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            *scheduler
                .global_active
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
            1,
            "并发上限为 1 时第二个任务必须等待"
        );

        drop(first);
        let second = tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("释放槽位后等待任务应继续")
            .expect("等待任务不应 panic");
        assert_eq!(
            *scheduler
                .global_active
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
            1
        );
        drop(second);
        assert_eq!(
            *scheduler
                .global_active
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
            0
        );
    }

    #[tokio::test]
    async fn scheduler_enforces_per_host_limit() {
        let db = Arc::new(crate::Db::open_in_memory().unwrap());
        let settings = settings_service(db);
        settings
            .update(
                SettingsSection::Downloads,
                None,
                SettingsPatch::Downloads(DownloadPatch {
                    concurrent_tasks: Some(DownloadConcurrency::Five),
                    speed_limit: None,
                    auto_continue: None,
                }),
            )
            .await
            .unwrap();

        let scheduler = Arc::new(DownloadScheduler::new());
        let same_host = |_i: u32| {
            scheduling_task(
                Some("provider-a".into()),
                Some("host-1".into()),
                DownloadPriority::Normal,
            )
        };
        // 同 Host 两个任务直接通过（HOST_MAX_CONCURRENT=2）
        let first = scheduler.acquire(&same_host(1), &settings).await;
        let second = scheduler.acquire(&same_host(2), &settings).await;
        assert_eq!(
            *scheduler
                .global_active
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
            2
        );
        assert_eq!(
            scheduler
                .host_active
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get("host-1")
                .copied()
                .unwrap_or(0),
            2
        );

        // 第三个同 Host 任务必须等待
        let waiting_scheduler = Arc::clone(&scheduler);
        let waiting_settings = Arc::clone(&settings);
        let third_task = same_host(3);
        let (started_tx, mut started_rx) = tokio::sync::oneshot::channel();
        let waiting = tokio::spawn(async move {
            let _ = started_tx.send(());
            waiting_scheduler
                .acquire(&third_task, &waiting_settings)
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), &mut started_rx)
            .await
            .expect("等待任务应启动")
            .expect("等待任务通知不应丢失");
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            scheduler
                .host_active
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get("host-1")
                .copied()
                .unwrap_or(0),
            2,
            "同 Host 第 3 个任务必须等待（host 上限 2）"
        );

        // 不同 Host 的任务不受影响
        let other_host = scheduling_task(
            Some("provider-b".into()),
            Some("host-2".into()),
            DownloadPriority::Normal,
        );
        let other = scheduler.acquire(&other_host, &settings).await;
        assert_eq!(
            *scheduler
                .global_active
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
            3
        );
        drop(other);
        drop(second);
        drop(first);
        let third = tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("释放同 Host 槽位后等待任务应继续")
            .expect("等待任务不应 panic");
        drop(third);
    }

    #[test]
    fn drr_weights_follow_8_4_1_and_bucket_refill_is_weighted() {
        assert_eq!(priority_weight(DownloadPriority::High), 8.0);
        assert_eq!(priority_weight(DownloadPriority::Normal), 4.0);
        assert_eq!(priority_weight(DownloadPriority::Low), 1.0);

        let high = refill_bucket(0.0, 1.0, 8.0);
        let low = refill_bucket(0.0, 1.0, 1.0);
        assert!(high > low, "高优先级桶必须按权重更快补液");
        assert!((high - 4.0).abs() < 1e-9, "8 权重 1 秒补液 4 token");
        assert!(refill_bucket(7.9, 100.0, 8.0) <= 8.0, "桶容量不得超过权重");
    }

    #[test]
    fn throttle_delay_respects_target_rate_without_sleeping_when_already_on_budget() {
        let started = Instant::now();
        let delay = required_transfer_delay(started, 1024, 1024);
        assert!(
            delay <= Duration::from_secs(1),
            "1 KiB at 1 KiB/s 的目标等待不应超过 1 秒"
        );
        assert!(
            delay >= Duration::from_millis(900),
            "限速计算应接近目标速率，实际等待 {delay:?}"
        );

        let elapsed_started = Instant::now() - Duration::from_secs(2);
        assert_eq!(
            required_transfer_delay(elapsed_started, 1024, 1024),
            Duration::ZERO,
            "累计耗时已经超过目标速率时不得额外等待"
        );
        assert_eq!(required_transfer_delay(started, 0, 1024), Duration::ZERO);
        assert_eq!(required_transfer_delay(started, 1024, 0), Duration::ZERO);
    }

    #[tokio::test]
    async fn local_worker_creates_verified_offline_resource() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("source.epub");
        let target_root = temp.path().join("target");
        std::fs::create_dir_all(&target_root).unwrap();
        std::fs::write(&source_path, b"haven offline content").unwrap();

        let db = Arc::new(crate::Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let now = haven_common::UtcMillis::now();
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let storage_id = StorageLocationId::new();
        let source_storage_id = StorageLocationId::new();
        let source_id = ResourceId::new();

        repos
            .work
            .save(&Work {
                id: work_id,
                canonical_title: "离线测试".into(),
                original_title: None,
                sort_title: None,
                description: None,
                work_type: WorkType::Fiction,
                release_year: None,
                language: None,
                director: None,
                actor: None,
                status: WorkStatus::Completed,
                rating_value: None,
                rating_scale: None,
                artwork: ArtworkSet::default(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .edition
            .save(&Edition {
                id: edition_id,
                work_id,
                title: "测试版本".into(),
                subtitle: None,
                edition_type: MediaType::Book,
                release_date: None,
                language: None,
                region: None,
                publisher_or_studio: None,
                description: None,
                artwork: ArtworkSet::default(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .media_item
            .save(&MediaItem {
                id: media_item_id,
                edition_id,
                parent_id: None,
                media_type: MediaType::Book,
                title: "正文".into(),
                index: MediaIndex::Custom {
                    label: "正文".into(),
                    ordinal: Some(1.0),
                },
                duration_ms: None,
                page_count: None,
                chapter_count: None,
                published_at: None,
                status: MediaItemStatus::Available,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .storage_location
            .save(&StorageLocation {
                id: storage_id,
                provider_type: StorageProviderType::Local,
                display_name: "离线目录".into(),
                root_ref: target_root.to_string_lossy().into_owned(),
                credential_ref: None,
                status: StorageStatus::Connected,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .storage_location
            .save(&StorageLocation {
                id: source_storage_id,
                provider_type: StorageProviderType::Local,
                display_name: "源文件目录".into(),
                root_ref: temp.path().to_string_lossy().into_owned(),
                credential_ref: None,
                status: StorageStatus::Connected,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .resource
            .save(&Resource {
                id: source_id,
                media_item_id,
                resource_type: ResourceType::PublicationFile,
                source_id: None,
                storage_location_id: Some(source_storage_id),
                locator: ResourceLocator::LocalPath {
                    path: source_path.to_string_lossy().into_owned(),
                },
                mime_type: Some("application/epub+zip".into()),
                size: Some(21),
                hash: None,
                availability: Availability::Available,
                availability_source: AvailabilitySource::User,
                modified_ms: None,
                fingerprint_first: None,
                fingerprint_last: None,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();

        let task = DownloadTask {
            id: DownloadTaskId::new(),
            work_id: Some(work_id),
            edition_id: Some(edition_id),
            media_item_id: Some(media_item_id),
            source_resource_id: source_id,
            target_storage_id: storage_id,
            offline_resource_id: None,
            state: DownloadState::Queued,
            bytes_total: Some(21),
            bytes_downloaded: 0,
            speed_bps: None,
            eta_seconds: None,
            created_at: now,
            updated_at: now,
            batch_id: None,
            priority: haven_domain::enums::DownloadPriority::Normal,
            provider_key: None,
            host_key: None,
            variant_key: String::new(),
            resource_identity: None,
            retry_count: 0,
            not_before: None,
            resumable: None,
        };
        repos.download.save(&task).await.unwrap();

        let settings = Arc::new(SettingsService::new(Arc::new(
            crate::db::repos::SqliteSettingsUoW::new(db.clone()),
        )));
        LocalDownloadRunner::new(
            repos.clone(),
            settings,
            Arc::new(NoopDownloadEventSink),
            Arc::new(DownloadBatchService::new(repos.clone())),
        )
        .run(task.id)
        .await;

        let completed = repos.download.get(task.id).await.unwrap().unwrap();
        assert_eq!(completed.state, DownloadState::Completed);
        assert!(completed.offline_resource_id.is_some());
        assert_eq!(completed.bytes_downloaded, 21);
        let resources = repos
            .resource
            .list_by_media_item(media_item_id)
            .await
            .unwrap();
        let offline = resources
            .iter()
            .find(|resource| resource.id != source_id)
            .expect("下载完成后应生成新的 Offline Resource");
        assert_eq!(offline.availability, Availability::OfflineAvailable);
        let ResourceLocator::LocalPath { path } = &offline.locator else {
            panic!("Offline Resource 必须使用本地路径")
        };
        assert_eq!(std::fs::read(path).unwrap(), b"haven offline content");
        assert!(
            !target_root
                .join(".haven")
                .join("offline")
                .join(format!("{}.part", task.id))
                .exists()
        );

        assert!(repos.download.delete_terminal(task.id).await.unwrap());
        let runner = Arc::new(CountingDownloadRunner::default());
        let service = DownloadService::new(
            repos.clone(),
            runner.clone(),
            Arc::new(LocalOfflineResourceFiles),
            Arc::new(NoopDownloadEventSink),
            settings_service(db.clone()),
            Arc::new(DownloadBatchService::new(repos.clone())),
        );
        let recreated = service
            .create(DownloadCreateRequest {
                source_resource_id: source_id.to_string(),
                target_storage_id: storage_id.to_string(),
            })
            .await
            .unwrap();

        assert_eq!(recreated.state, DownloadStateDto::Completed);
        assert_eq!(recreated.offline_resource_id, Some(offline.id.to_string()));
        assert_eq!(runner.starts.load(Ordering::Relaxed), 0);
        let resources = repos
            .resource
            .list_by_media_item(media_item_id)
            .await
            .unwrap();
        assert_eq!(
            resources
                .iter()
                .filter(|resource| resource.availability == Availability::OfflineAvailable)
                .count(),
            1,
            "已有 Offline Resource 时不得再次复制文件",
        );

        // 索引仍在但文件已被用户移除时，不能继续伪造 Completed；下一次创建
        // 必须重新排队，让 Worker 按正常下载流程恢复文件。
        std::fs::remove_file(path).unwrap();
        let recreated_task_id: DownloadTaskId = recreated.task_id.parse().unwrap();
        assert!(
            repos
                .download
                .delete_terminal(recreated_task_id)
                .await
                .unwrap()
        );
        let retry_runner = Arc::new(CountingDownloadRunner::default());
        let retry_service = DownloadService::new(
            repos.clone(),
            retry_runner.clone(),
            Arc::new(LocalOfflineResourceFiles),
            Arc::new(NoopDownloadEventSink),
            settings_service(db.clone()),
            Arc::new(DownloadBatchService::new(repos.clone())),
        );
        let retry = retry_service
            .create(DownloadCreateRequest {
                source_resource_id: source_id.to_string(),
                target_storage_id: storage_id.to_string(),
            })
            .await
            .unwrap();
        assert_eq!(retry.state, DownloadStateDto::Queued);
        assert_eq!(retry_runner.starts.load(Ordering::Relaxed), 1);
    }

    #[derive(Clone)]
    struct TestRemoteAcquisition {
        payload: Vec<u8>,
        mime: String,
        failure: Option<(&'static str, bool)>,
        started: Option<Arc<Notify>>,
        release: Option<Arc<Notify>>,
    }

    impl TestRemoteAcquisition {
        fn success(payload: Vec<u8>, mime: &str) -> Self {
            Self {
                payload,
                mime: mime.to_owned(),
                failure: None,
                started: None,
                release: None,
            }
        }

        fn failure(code: &'static str, retryable: bool) -> Self {
            Self {
                payload: Vec::new(),
                mime: String::new(),
                failure: Some((code, retryable)),
                started: None,
                release: None,
            }
        }

        fn blocking(
            payload: Vec<u8>,
            mime: &str,
            started: Arc<Notify>,
            release: Arc<Notify>,
        ) -> Self {
            Self {
                payload,
                mime: mime.to_owned(),
                failure: None,
                started: Some(started),
                release: Some(release),
            }
        }
    }

    #[async_trait]
    impl RemoteAcquisitionPort for TestRemoteAcquisition {
        async fn acquire(
            &self,
            _source_key: &str,
            _remote_id: &str,
            destination: &Path,
        ) -> Result<RemoteAcquiredFile, AppError> {
            if let Some((code, retryable)) = self.failure {
                return Err(AppError::new(
                    code,
                    ErrorKind::Network,
                    "测试远端来源失败",
                    retryable,
                ));
            }
            if let (Some(started), Some(release)) = (&self.started, &self.release) {
                tokio::fs::write(destination, b"partial")
                    .await
                    .map_err(|_| {
                        AppError::new("STORAGE_ERROR", ErrorKind::Storage, "测试写入失败", true)
                    })?;
                started.notify_one();
                release.notified().await;
            }
            tokio::fs::write(destination, &self.payload)
                .await
                .map_err(|_| {
                    AppError::new("STORAGE_ERROR", ErrorKind::Storage, "测试写入失败", true)
                })?;
            Ok(RemoteAcquiredFile {
                size_bytes: self.payload.len() as u64,
                mime: self.mime.clone(),
            })
        }
    }

    fn zip_payload(file_name: &str, content: &[u8]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(file_name, SimpleFileOptions::default())
            .unwrap();
        writer.write_all(content).unwrap();
        writer.finish().unwrap().into_inner()
    }

    async fn seed_remote_download(
        source_key: &str,
        media_type: MediaType,
        resource_type: ResourceType,
        mime_type: &str,
        remote_id: &str,
    ) -> (
        tempfile::TempDir,
        Arc<crate::Db>,
        Arc<SqliteRepositories>,
        Arc<SettingsService>,
        DownloadTask,
        Resource,
    ) {
        let target_root = tempfile::tempdir().unwrap();
        let db = Arc::new(crate::Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let now = haven_common::UtcMillis::now();
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let storage_id = StorageLocationId::new();
        let resource_id = ResourceId::new();
        let source_id =
            haven_application::services::source_import::stable_source_id(source_key).unwrap();

        repos
            .work
            .save(&Work {
                id: work_id,
                canonical_title: "远端下载测试".into(),
                original_title: None,
                sort_title: None,
                description: None,
                work_type: if media_type == MediaType::Article {
                    WorkType::Article
                } else {
                    WorkType::Fiction
                },
                release_year: None,
                language: None,
                director: None,
                actor: None,
                status: WorkStatus::Completed,
                rating_value: None,
                rating_scale: None,
                artwork: ArtworkSet::default(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .edition
            .save(&Edition {
                id: edition_id,
                work_id,
                title: "远端测试版本".into(),
                subtitle: None,
                edition_type: media_type,
                release_date: None,
                language: None,
                region: None,
                publisher_or_studio: None,
                description: None,
                artwork: ArtworkSet::default(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .media_item
            .save(&MediaItem {
                id: media_item_id,
                edition_id,
                parent_id: None,
                media_type,
                title: "远端正文".into(),
                index: match media_type {
                    MediaType::Comic => MediaIndex::Chapter {
                        volume: None,
                        chapter: 1.0,
                    },
                    MediaType::Article => MediaIndex::Article { ordinal: Some(1) },
                    _ => MediaIndex::Custom {
                        label: "正文".into(),
                        ordinal: Some(1.0),
                    },
                },
                duration_ms: None,
                page_count: None,
                chapter_count: None,
                published_at: None,
                status: MediaItemStatus::Available,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .storage_location
            .save(&StorageLocation {
                id: storage_id,
                provider_type: StorageProviderType::Local,
                display_name: "远端下载目录".into(),
                root_ref: target_root.path().to_string_lossy().into_owned(),
                credential_ref: None,
                status: StorageStatus::Connected,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();

        let resource = Resource {
            id: resource_id,
            media_item_id,
            resource_type,
            source_id: Some(source_id),
            storage_location_id: None,
            locator: ResourceLocator::SourceObject {
                source_id,
                remote_id: remote_id.to_owned(),
            },
            mime_type: Some(mime_type.to_owned()),
            size: None,
            hash: None,
            availability: Availability::Available,
            availability_source: AvailabilitySource::User,
            modified_ms: None,
            fingerprint_first: None,
            fingerprint_last: None,
            created_at: now,
            updated_at: now,
        };
        repos.resource.save(&resource).await.unwrap();

        let task = DownloadTask {
            id: DownloadTaskId::new(),
            work_id: Some(work_id),
            edition_id: Some(edition_id),
            media_item_id: Some(media_item_id),
            source_resource_id: resource_id,
            target_storage_id: storage_id,
            offline_resource_id: None,
            state: DownloadState::Queued,
            bytes_total: None,
            bytes_downloaded: 0,
            speed_bps: None,
            eta_seconds: None,
            created_at: now,
            updated_at: now,
            batch_id: None,
            priority: DownloadPriority::Normal,
            provider_key: None,
            host_key: None,
            variant_key: String::new(),
            resource_identity: None,
            retry_count: 0,
            not_before: None,
            resumable: None,
        };
        repos.download.save(&task).await.unwrap();
        // Keep the database handle for tests that need to inject a precise
        // state transition between resource association and completion CAS.
        // Production code never exposes this handle through the service API.
        let settings = settings_service(db.clone());
        (target_root, db, repos, settings, task, resource)
    }

    #[tokio::test]
    async fn remote_worker_acquires_each_supported_type_into_the_expected_category() {
        let cases = [
            (
                "mangadex",
                MediaType::Comic,
                ResourceType::ComicArchive,
                "application/vnd.comicbook+zip",
                "manga:chapter",
                "cbz",
                "comics",
                zip_payload("page-0001.jpg", b"jpg"),
            ),
            (
                "opds_gutenberg",
                MediaType::Book,
                ResourceType::PublicationFile,
                "application/epub+zip",
                "https://www.gutenberg.org/ebooks/84.opds",
                "epub",
                "books",
                zip_payload("mimetype", b"application/epub+zip"),
            ),
            (
                "arxiv",
                MediaType::Article,
                ResourceType::PublicationFile,
                "application/pdf",
                "2401.12345",
                "pdf",
                "articles",
                b"%PDF-1.7\nminimal test pdf".to_vec(),
            ),
            (
                "wikisource",
                MediaType::Article,
                ResourceType::ArticleSnapshot,
                "text/html; charset=utf-8",
                "测试页面",
                "html",
                "articles",
                b"<!doctype html><article>test</article>".to_vec(),
            ),
        ];

        for (
            source_key,
            media_type,
            resource_type,
            mime_type,
            remote_id,
            extension,
            category,
            payload,
        ) in cases
        {
            let (target_root, _db, repos, settings, task, resource) =
                seed_remote_download(source_key, media_type, resource_type, mime_type, remote_id)
                    .await;
            let runner = LocalDownloadRunner::new_with_remote(
                repos.clone(),
                settings,
                Arc::new(NoopDownloadEventSink),
                Arc::new(DownloadBatchService::new(repos.clone())),
                Arc::new(TestRemoteAcquisition::success(payload.clone(), mime_type)),
            );
            runner.run(task.id).await;

            let completed = repos.download.get(task.id).await.unwrap().unwrap();
            assert_eq!(completed.state, DownloadState::Completed, "{source_key}");
            assert!(completed.offline_resource_id.is_some(), "{source_key}");
            assert_eq!(
                completed.bytes_downloaded,
                payload.len() as u64,
                "{source_key}"
            );
            let final_path = target_root
                .path()
                .join(category)
                .join(format!("{}.{}", task.id, extension));
            assert_eq!(
                tokio::fs::read(&final_path).await.unwrap(),
                payload,
                "{source_key}"
            );
            assert!(!final_path.with_extension("part").exists(), "{source_key}");
            assert!(
                !final_path.with_extension("provider-part").exists(),
                "{source_key}"
            );

            let resources = repos
                .resource
                .list_by_media_item(resource.media_item_id)
                .await
                .unwrap();
            let offline = resources
                .iter()
                .find(|candidate| candidate.id != resource.id)
                .expect("远端下载必须创建 Offline Resource");
            assert_eq!(offline.availability, Availability::OfflineAvailable);
            assert!(matches!(offline.locator, ResourceLocator::LocalPath { .. }));
        }
    }

    #[tokio::test]
    async fn remote_worker_rejects_provider_failure_and_mismatched_mime_without_residue() {
        let (target_root, _db, repos, settings, task, resource) = seed_remote_download(
            "wikisource",
            MediaType::Article,
            ResourceType::ArticleSnapshot,
            "text/html; charset=utf-8",
            "失败页面",
        )
        .await;
        let runner = LocalDownloadRunner::new_with_remote(
            repos.clone(),
            settings,
            Arc::new(NoopDownloadEventSink),
            Arc::new(DownloadBatchService::new(repos.clone())),
            Arc::new(TestRemoteAcquisition::failure("SOURCE_UNAVAILABLE", false)),
        );
        runner.run(task.id).await;
        let failed = repos.download.get(task.id).await.unwrap().unwrap();
        assert_eq!(failed.state, DownloadState::Failed);
        let expected_dir = target_root.path().join("articles");
        assert!(!expected_dir.join(format!("{}.part", task.id)).exists());
        assert_eq!(
            repos
                .resource
                .list_by_media_item(resource.media_item_id)
                .await
                .unwrap()
                .len(),
            1
        );

        let (target_root, _db, repos, settings, task, resource) = seed_remote_download(
            "wikisource",
            MediaType::Article,
            ResourceType::ArticleSnapshot,
            "text/html; charset=utf-8",
            "错误类型",
        )
        .await;
        let runner = LocalDownloadRunner::new_with_remote(
            repos.clone(),
            settings,
            Arc::new(NoopDownloadEventSink),
            Arc::new(DownloadBatchService::new(repos.clone())),
            Arc::new(TestRemoteAcquisition::success(
                b"<!doctype html><article>test</article>".to_vec(),
                "application/pdf",
            )),
        );
        runner.run(task.id).await;
        let failed = repos.download.get(task.id).await.unwrap().unwrap();
        assert_eq!(failed.state, DownloadState::Failed);
        assert!(
            !target_root
                .path()
                .join("articles")
                .join(format!("{}.part", task.id))
                .exists()
        );
        assert_eq!(
            repos
                .resource
                .list_by_media_item(resource.media_item_id)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn remote_worker_cancellation_drops_provider_and_removes_partial_file() {
        let (target_root, _db, repos, settings, task, resource) = seed_remote_download(
            "mangadex",
            MediaType::Comic,
            ResourceType::ComicArchive,
            "application/vnd.comicbook+zip",
            "manga:chapter",
        )
        .await;
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let started_wait = started.notified();
        let runner = Arc::new(LocalDownloadRunner::new_with_remote(
            repos.clone(),
            settings,
            Arc::new(NoopDownloadEventSink),
            Arc::new(DownloadBatchService::new(repos.clone())),
            Arc::new(TestRemoteAcquisition::blocking(
                zip_payload("page-0001.jpg", b"jpg"),
                "application/vnd.comicbook+zip",
                started.clone(),
                release.clone(),
            )),
        ));
        let running = {
            let runner = runner.clone();
            tokio::spawn(async move { runner.run(task.id).await })
        };
        tokio::time::timeout(Duration::from_secs(1), started_wait)
            .await
            .expect("远端 Provider 应进入等待状态");
        assert!(
            repos
                .download
                .compare_and_set_state(
                    task.id,
                    DownloadState::Downloading,
                    DownloadState::Cancelled
                )
                .await
                .unwrap()
        );
        release.notify_one();
        running.await.unwrap();

        let cancelled = repos.download.get(task.id).await.unwrap().unwrap();
        assert_eq!(cancelled.state, DownloadState::Cancelled);
        let articles = target_root.path().join("comics");
        assert!(!articles.join(format!("{}.part", task.id)).exists());
        assert!(!articles.join(format!("{}.cbz", task.id)).exists());
        assert_eq!(
            repos
                .resource
                .list_by_media_item(resource.media_item_id)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn completion_cas_failure_compensates_new_offline_resource_after_association() {
        let (target_root, db, repos, settings, task, resource) = seed_remote_download(
            "wikisource",
            MediaType::Article,
            ResourceType::ArticleSnapshot,
            "text/html; charset=utf-8",
            "cas-race",
        )
        .await;

        // Inject a deterministic state race at the exact boundary under test:
        // the association update succeeds, then another actor cancels the
        // task before the Verifying -> Completed CAS.  This is a SQLite
        // trigger only in the test database; production has no such trigger.
        let task_id = task.id;
        db.lock()
            .execute_batch(&format!(
                "CREATE TRIGGER test_cancel_after_offline_association
                 AFTER UPDATE OF offline_resource_id ON download_tasks
                 WHEN NEW.id = '{task_id}'
                   AND NEW.offline_resource_id IS NOT NULL
                   AND NEW.state = 'verifying'
                 BEGIN
                   UPDATE download_tasks
                      SET state = 'cancelled', updated_at = NEW.updated_at
                    WHERE id = NEW.id AND state = 'verifying';
                 END;"
            ))
            .unwrap();

        let payload = b"<!doctype html><article>cas-race</article>".to_vec();
        let runner = LocalDownloadRunner::new_with_remote(
            repos.clone(),
            settings,
            Arc::new(NoopDownloadEventSink),
            Arc::new(DownloadBatchService::new(repos.clone())),
            Arc::new(TestRemoteAcquisition::success(
                payload.clone(),
                "text/html; charset=utf-8",
            )),
        );
        runner.run(task.id).await;

        let cancelled = repos.download.get(task.id).await.unwrap().unwrap();
        assert_eq!(cancelled.state, DownloadState::Cancelled);
        assert_eq!(cancelled.offline_resource_id, None);

        let final_path = target_root
            .path()
            .join("articles")
            .join(format!("{}.html", task.id));
        assert!(
            !final_path.exists(),
            "完成 CAS 失败后不得留下未关联的最终文件"
        );
        assert!(
            !final_path.with_extension("part").exists(),
            "完成 CAS 失败后不得留下 .part"
        );
        assert_eq!(
            repos
                .resource
                .list_by_media_item(resource.media_item_id)
                .await
                .unwrap()
                .len(),
            1,
            "当前 Worker 创建的 Offline Resource 必须一并补偿删除"
        );
    }

    #[tokio::test]
    async fn offline_file_delete_requires_registered_offline_root() {
        let temp = tempfile::tempdir().unwrap();
        let target_root = temp.path().join("target");
        let offline_root = target_root.join(".haven").join("offline");
        std::fs::create_dir_all(&offline_root).unwrap();
        let inside = offline_root.join("inside.bin");
        let outside = target_root.join("outside.bin");
        std::fs::write(&inside, b"inside").unwrap();
        std::fs::write(&outside, b"outside").unwrap();
        let now = haven_common::UtcMillis::now();
        let storage_id = StorageLocationId::new();
        let storage = StorageLocation {
            id: storage_id,
            provider_type: StorageProviderType::Local,
            display_name: "离线目录".into(),
            root_ref: target_root.to_string_lossy().into_owned(),
            credential_ref: None,
            status: StorageStatus::Connected,
            created_at: now,
            updated_at: now,
        };
        let media_item_id = MediaItemId::new();
        let resource_for = |path: &Path| Resource {
            id: ResourceId::new(),
            media_item_id,
            resource_type: ResourceType::LocalFile,
            source_id: None,
            storage_location_id: Some(storage_id),
            locator: ResourceLocator::LocalPath {
                path: path.to_string_lossy().into_owned(),
            },
            mime_type: None,
            size: None,
            hash: None,
            availability: Availability::OfflineAvailable,
            availability_source: AvailabilitySource::User,
            modified_ms: None,
            fingerprint_first: None,
            fingerprint_last: None,
            created_at: now,
            updated_at: now,
        };
        let files = LocalOfflineResourceFiles;

        let error = files
            .delete(&storage, &resource_for(&outside))
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "DOWNLOAD_OFFLINE_PATH_INVALID");
        assert!(outside.exists(), "Offline Root 之外的文件不得被删除");

        files
            .delete(&storage, &resource_for(&inside))
            .await
            .unwrap();
        assert!(!inside.exists(), "删除成功后必须确认文件已经不存在");
    }

    #[tokio::test]
    async fn batch_reconcile_derives_partial_completed_and_bytes() {
        let db = Arc::new(crate::Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let now = haven_common::UtcMillis::now();
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let storage_id = StorageLocationId::new();
        let source_id = ResourceId::new();
        repos
            .work
            .save(&Work {
                id: work_id,
                canonical_title: "批次测试".into(),
                original_title: None,
                sort_title: None,
                description: None,
                work_type: WorkType::Fiction,
                release_year: None,
                language: None,
                director: None,
                actor: None,
                status: WorkStatus::Completed,
                rating_value: None,
                rating_scale: None,
                artwork: ArtworkSet::default(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .edition
            .save(&Edition {
                id: edition_id,
                work_id,
                title: "版本".into(),
                subtitle: None,
                edition_type: MediaType::Book,
                release_date: None,
                language: None,
                region: None,
                publisher_or_studio: None,
                description: None,
                artwork: ArtworkSet::default(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .media_item
            .save(&MediaItem {
                id: media_item_id,
                edition_id,
                parent_id: None,
                media_type: MediaType::Book,
                title: "正文".into(),
                index: MediaIndex::Custom {
                    label: "正文".into(),
                    ordinal: Some(1.0),
                },
                duration_ms: None,
                page_count: None,
                chapter_count: None,
                published_at: None,
                status: MediaItemStatus::Available,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .storage_location
            .save(&StorageLocation {
                id: storage_id,
                provider_type: StorageProviderType::Local,
                display_name: "离线目录".into(),
                root_ref: std::env::temp_dir().to_string_lossy().into_owned(),
                credential_ref: None,
                status: StorageStatus::Connected,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .resource
            .save(&Resource {
                id: source_id,
                media_item_id,
                resource_type: ResourceType::LocalFile,
                source_id: None,
                storage_location_id: None,
                locator: ResourceLocator::LocalPath {
                    path: "dummy.bin".into(),
                },
                mime_type: None,
                size: Some(1),
                hash: None,
                availability: Availability::Available,
                availability_source: AvailabilitySource::User,
                modified_ms: None,
                fingerprint_first: None,
                fingerprint_last: None,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();

        let batch_id = DownloadBatchId::new();
        repos
            .download_batch
            .save(&DownloadBatch {
                id: batch_id,
                title: "整本书".into(),
                category: haven_domain::enums::ContentCategory::Book,
                subject_type: "edition".into(),
                subject_id: edition_id.to_string(),
                target_storage_id: storage_id,
                state: BatchState::Queued,
                total_tasks: 0,
                completed_tasks: 0,
                total_bytes: None,
                completed_bytes: 0,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();

        let make_task = |id: DownloadTaskId, state: DownloadState, bytes: u64| DownloadTask {
            id,
            work_id: Some(work_id),
            edition_id: Some(edition_id),
            media_item_id: Some(media_item_id),
            source_resource_id: source_id,
            target_storage_id: storage_id,
            offline_resource_id: None,
            state,
            bytes_total: Some(bytes),
            bytes_downloaded: if state == DownloadState::Completed {
                bytes
            } else {
                0
            },
            speed_bps: None,
            eta_seconds: None,
            created_at: now,
            updated_at: now,
            batch_id: Some(batch_id),
            priority: haven_domain::enums::DownloadPriority::Normal,
            provider_key: None,
            host_key: None,
            variant_key: String::new(),
            resource_identity: None,
            retry_count: 0,
            not_before: None,
            resumable: None,
        };
        let task_a = DownloadTaskId::new();
        let task_b = DownloadTaskId::new();
        let task_c = DownloadTaskId::new();
        repos
            .download
            .save(&make_task(task_a, DownloadState::Completed, 100))
            .await
            .unwrap();
        repos
            .download
            .save(&make_task(task_b, DownloadState::Failed, 200))
            .await
            .unwrap();
        repos
            .download
            .save(&make_task(task_c, DownloadState::Queued, 300))
            .await
            .unwrap();

        let batch_service = DownloadBatchService::new(repos.clone());
        batch_service.reconcile(batch_id).await.unwrap();
        let batch = repos.download_batch.get(batch_id).await.unwrap().unwrap();
        assert_eq!(batch.total_tasks, 3);
        assert_eq!(batch.completed_tasks, 1);
        assert_eq!(
            batch.state,
            BatchState::Queued,
            "仍有 Queued 子任务且无活跃传输"
        );
        assert_eq!(batch.total_bytes, Some(600));
        assert_eq!(batch.completed_bytes, 100);

        // 全部终态（1 完成 + 2 失败）→ PartialCompleted；字节进度按完成任务计。
        repos
            .download
            .save(&make_task(task_c, DownloadState::Failed, 300))
            .await
            .unwrap();
        batch_service.reconcile(batch_id).await.unwrap();
        let batch = repos.download_batch.get(batch_id).await.unwrap().unwrap();
        assert_eq!(batch.state, BatchState::PartialCompleted);
        assert_eq!(batch.completed_tasks, 1);
        assert_eq!(batch.completed_bytes, 100);

        // 全部取消 → Cancelled
        repos
            .download
            .save(&make_task(task_a, DownloadState::Cancelled, 100))
            .await
            .unwrap();
        repos
            .download
            .save(&make_task(task_b, DownloadState::Cancelled, 200))
            .await
            .unwrap();
        repos
            .download
            .save(&make_task(task_c, DownloadState::Cancelled, 300))
            .await
            .unwrap();
        batch_service.reconcile(batch_id).await.unwrap();
        let batch = repos.download_batch.get(batch_id).await.unwrap().unwrap();
        assert_eq!(batch.state, BatchState::Cancelled);
        assert_eq!(batch.completed_tasks, 0);
    }

    #[tokio::test]
    async fn batch_reconcile_is_noop_for_missing_batch() {
        let db = Arc::new(crate::Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let batch_service = DownloadBatchService::new(repos.clone());
        batch_service
            .reconcile(DownloadBatchId::new())
            .await
            .unwrap();
    }
}
