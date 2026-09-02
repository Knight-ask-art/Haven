//! DownloadService：下载任务的创建、查询和状态机操作。
//!
//! 传输由 `DownloadRunner` 执行；本服务只负责校验领域关系、持久化任务和
//! 通过 CAS 维护状态，不把 UI 传入的状态当作事实。

use std::sync::Arc;

use async_trait::async_trait;
use haven_common::{AppError, ErrorKind};
use haven_domain::contracts::{
    DownloadRepository, EditionRepository, MediaItemRepository, ResourceRepository,
    StorageLocationRepository, WorkRepository,
};
use haven_domain::entities::{DownloadTask, Resource, ResourceLocator, StorageLocation};
use haven_domain::enums::{
    Availability, ContentCategory, DownloadPriority, DownloadState, MediaType, StorageProviderType,
    StorageStatus,
};
use haven_domain::ids::{DownloadTaskId, ResourceId, StorageLocationId};
use haven_domain::settings::{SettingsSection, SettingsValue};

use crate::mapper::time::utc_millis_to_rfc3339;
use crate::services::download_batch::DownloadBatchService;
use crate::services::settings::SettingsService;
use crate::services::source_import::{
    remote_source_mime_compatible, source_key_for_id, validate_remote_source_object,
};
use crate::wire::{
    ContentCategory as ContentCategoryDto, DownloadCreateRequest, DownloadEventData,
    DownloadListRequest, DownloadMutationResultDto, DownloadRevealResultDto, DownloadStateDto,
    DownloadTaskActionRequest, DownloadTaskDto, MediaTypeDto,
};

const SCHEMA_VERSION: u32 = 1;
const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 200;

/// Application 只依赖组合后的下载端口，避免 dyn trait upcasting（MSRV 1.85）。
pub trait DownloadPorts:
    DownloadRepository
    + ResourceRepository
    + StorageLocationRepository
    + MediaItemRepository
    + EditionRepository
    + WorkRepository
    + Send
    + Sync
{
    fn as_download(&self) -> &dyn DownloadRepository;
    fn as_resource(&self) -> &dyn ResourceRepository;
    fn as_storage(&self) -> &dyn StorageLocationRepository;
    fn as_media_item(&self) -> &dyn MediaItemRepository;
    fn as_edition(&self) -> &dyn EditionRepository;
    fn as_work(&self) -> &dyn WorkRepository;
}

impl<T> DownloadPorts for T
where
    T: DownloadRepository
        + ResourceRepository
        + StorageLocationRepository
        + MediaItemRepository
        + EditionRepository
        + WorkRepository
        + Send
        + Sync,
{
    fn as_download(&self) -> &dyn DownloadRepository {
        self
    }
    fn as_resource(&self) -> &dyn ResourceRepository {
        self
    }
    fn as_storage(&self) -> &dyn StorageLocationRepository {
        self
    }
    fn as_media_item(&self) -> &dyn MediaItemRepository {
        self
    }
    fn as_edition(&self) -> &dyn EditionRepository {
        self
    }
    fn as_work(&self) -> &dyn WorkRepository {
        self
    }
}

/// 具体传输实现由 Infrastructure 注入；Command 不直接触碰文件或网络。
pub trait DownloadRunner: Send + Sync {
    fn start(&self, task_id: DownloadTaskId);
}

/// 下载长任务的有序进度出口。具体 Channel 注册与发送由 Tauri Interface 实现。
pub trait DownloadEventSink: Send + Sync {
    /// `error_code` 只在 Worker 终态失败/中断时携带；普通事件为 None。
    fn emit_task(&self, task: &DownloadTask, error_code: Option<&str>);
}

/// 已登记 Offline Resource 的受控文件能力；实现必须拒绝 Offline Root 之外的路径。
#[async_trait]
pub trait OfflineResourceFiles: Send + Sync {
    /// 检查已登记离线资源的文件是否仍然存在且可读取。
    ///
    /// `Ok(false)` 只表示受控离线目录中的文件已丢失；调用方应重新创建下载任务。
    /// 路径越界、权限或其他 IO 错误必须返回错误，不能把安全失败伪装成缺失。
    async fn is_available(
        &self,
        storage: &StorageLocation,
        resource: &Resource,
    ) -> Result<bool, AppError>;
    async fn delete(&self, storage: &StorageLocation, resource: &Resource) -> Result<(), AppError>;
    async fn reveal(&self, storage: &StorageLocation, resource: &Resource) -> Result<(), AppError>;
}

#[derive(Clone)]
pub struct DownloadService {
    ports: Arc<dyn DownloadPorts>,
    runner: Arc<dyn DownloadRunner>,
    offline_files: Arc<dyn OfflineResourceFiles>,
    events: Arc<dyn DownloadEventSink>,
    settings: Arc<SettingsService>,
    batch: Arc<DownloadBatchService>,
}

impl DownloadService {
    pub fn new(
        ports: Arc<dyn DownloadPorts>,
        runner: Arc<dyn DownloadRunner>,
        offline_files: Arc<dyn OfflineResourceFiles>,
        events: Arc<dyn DownloadEventSink>,
        settings: Arc<SettingsService>,
        batch: Arc<DownloadBatchService>,
    ) -> Self {
        Self {
            ports,
            runner,
            offline_files,
            events,
            settings,
            batch,
        }
    }

    pub async fn create(
        &self,
        request: DownloadCreateRequest,
    ) -> Result<DownloadTaskDto, AppError> {
        let source_resource_id: ResourceId = parse_id(&request.source_resource_id, "资源")?;
        let target_storage_id: StorageLocationId =
            parse_id(&request.target_storage_id, "存储位置")?;
        let resource = self
            .ports
            .as_resource()
            .get(source_resource_id)
            .await?
            .ok_or_else(|| not_found("RESOURCE_NOT_FOUND", "下载资源不存在"))?;
        if matches!(
            resource.availability,
            Availability::Missing
                | Availability::StorageUnavailable
                | Availability::SourceUnavailable
        ) {
            return Err(AppError::new(
                "DOWNLOAD_SOURCE_UNAVAILABLE",
                ErrorKind::Validation,
                "下载来源当前不可用",
                true,
            ));
        }
        if matches!(&resource.locator, ResourceLocator::Http { .. }) {
            return Err(AppError::new(
                "DOWNLOAD_SOURCE_UNSUPPORTED",
                ErrorKind::Validation,
                "该资源没有受控的可下载来源",
                false,
            ));
        }
        validate_download_source_object(&resource)?;
        let storage = self
            .ports
            .as_storage()
            .get(target_storage_id)
            .await?
            .ok_or_else(|| not_found("STORAGE_LOCATION_NOT_FOUND", "目标存储位置不存在"))?;
        if storage.provider_type != StorageProviderType::Local
            || storage.status != StorageStatus::Connected
        {
            return Err(AppError::new(
                "DOWNLOAD_TARGET_UNAVAILABLE",
                ErrorKind::Validation,
                "目标存储位置不可用于下载",
                true,
            ));
        }

        let media_item = self
            .ports
            .as_media_item()
            .get(resource.media_item_id)
            .await?
            .ok_or_else(|| not_found("MEDIA_ITEM_NOT_FOUND", "媒体条目不存在"))?;
        let edition = self
            .ports
            .as_edition()
            .get(media_item.edition_id)
            .await?
            .ok_or_else(|| not_found("EDITION_NOT_FOUND", "媒体版本不存在"))?;
        let work = self
            .ports
            .as_work()
            .get(edition.work_id)
            .await?
            .ok_or_else(|| not_found("WORK_NOT_FOUND", "作品不存在"))?;

        if let Some(existing) = self
            .ports
            .as_download()
            .find_active(source_resource_id, target_storage_id)
            .await?
        {
            return self.to_dto(existing).await;
        }

        let existing_offline = {
            let candidates = self
                .ports
                .as_resource()
                .list_by_media_item(media_item.id)
                .await?;
            let mut available = None;
            for candidate in candidates {
                if candidate.storage_location_id != Some(target_storage_id)
                    || candidate.availability != Availability::OfflineAvailable
                    || !matches!(&candidate.locator, ResourceLocator::LocalPath { .. })
                {
                    continue;
                }
                if self
                    .offline_files
                    .is_available(&storage, &candidate)
                    .await?
                {
                    available = Some(candidate);
                    break;
                }
            }
            available
        };

        let now = haven_common::UtcMillis::now();
        if let Some(offline_resource) = existing_offline {
            let bytes_total = offline_resource.size.or(resource.size);
            let task = DownloadTask {
                id: DownloadTaskId::new(),
                work_id: Some(work.id),
                edition_id: Some(edition.id),
                media_item_id: Some(media_item.id),
                source_resource_id,
                target_storage_id,
                offline_resource_id: Some(offline_resource.id),
                state: DownloadState::Completed,
                bytes_total,
                bytes_downloaded: bytes_total.unwrap_or(0),
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
            self.ports.as_download().save(&task).await?;
            self.events.emit_task(&task, None);
            return self.to_dto(task).await;
        }

        let task = DownloadTask {
            id: DownloadTaskId::new(),
            work_id: Some(work.id),
            edition_id: Some(edition.id),
            media_item_id: Some(media_item.id),
            source_resource_id,
            target_storage_id,
            offline_resource_id: None,
            state: DownloadState::Queued,
            bytes_total: resource.size,
            bytes_downloaded: 0,
            speed_bps: None,
            eta_seconds: None,
            batch_id: None,
            priority: DownloadPriority::Normal,
            provider_key: None,
            host_key: None,
            variant_key: String::new(),
            resource_identity: None,
            retry_count: 0,
            not_before: None,
            resumable: None,
            created_at: now,
            updated_at: now,
        };
        self.ports.as_download().save(&task).await?;
        self.events.emit_task(&task, None);
        self.runner.start(task.id);
        self.to_dto(task).await
    }

    pub async fn list(
        &self,
        request: DownloadListRequest,
    ) -> Result<Vec<DownloadTaskDto>, AppError> {
        let limit = request.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let tasks = self.ports.as_download().list(limit).await?;
        let mut result = Vec::with_capacity(tasks.len());
        for task in tasks {
            result.push(self.to_dto(task).await?);
        }
        Ok(result)
    }

    /// 应用启动后恢复上次未正常结束或尚未开始的任务。用户主动暂停的任务保持暂停。
    /// `downloads.autoContinue=false` 时，不自动恢复已中断任务；队列中的新任务
    /// 仍按用户已提交的意图启动，后续可用“暂停”明确阻止它们。
    pub async fn resume_startable(&self) -> Result<u32, AppError> {
        let auto_continue = self.auto_continue_enabled().await;
        let tasks = self.ports.as_download().list(MAX_LIMIT).await?;
        let mut started = 0_u32;
        for task in tasks {
            let start = if should_start_on_boot(task.state, auto_continue) {
                if task.state == DownloadState::Interrupted {
                    self.ports
                        .as_download()
                        .compare_and_set_state(
                            task.id,
                            DownloadState::Interrupted,
                            DownloadState::Queued,
                        )
                        .await?
                } else {
                    true
                }
            } else {
                false
            };
            if start {
                self.runner.start(task.id);
                started = started.saturating_add(1);
            }
        }
        Ok(started)
    }

    async fn auto_continue_enabled(&self) -> bool {
        let Ok(snapshot) = self.settings.get(SettingsSection::Downloads).await else {
            // 设置读取失败时保留既有启动恢复行为，避免把可恢复任务静默留在
            // Interrupted 状态；下一次设置查询仍可重试并覆盖默认值。
            return true;
        };
        match snapshot.value {
            SettingsValue::Downloads(value) => value.auto_continue,
            _ => true,
        }
    }

    pub async fn pause(
        &self,
        request: DownloadTaskActionRequest,
    ) -> Result<DownloadTaskDto, AppError> {
        self.transition(request, Transition::Pause).await
    }

    pub async fn resume(
        &self,
        request: DownloadTaskActionRequest,
    ) -> Result<DownloadTaskDto, AppError> {
        self.transition(request, Transition::Resume).await
    }

    pub async fn cancel(
        &self,
        request: DownloadTaskActionRequest,
    ) -> Result<DownloadTaskDto, AppError> {
        self.transition(request, Transition::Cancel).await
    }

    pub async fn retry(
        &self,
        request: DownloadTaskActionRequest,
    ) -> Result<DownloadTaskDto, AppError> {
        self.transition(request, Transition::Retry).await
    }

    pub async fn remove_record(
        &self,
        request: DownloadTaskActionRequest,
    ) -> Result<DownloadMutationResultDto, AppError> {
        let id: DownloadTaskId = parse_id(&request.task_id, "下载任务")?;
        let task = self.get_task(id).await?;
        if !matches!(
            task.state,
            DownloadState::Completed | DownloadState::Failed | DownloadState::Cancelled
        ) {
            return Err(AppError::new(
                "DOWNLOAD_TASK_NOT_REMOVABLE",
                ErrorKind::Conflict,
                "进行中的下载任务不能从列表移除",
                false,
            ));
        }
        if !self.ports.as_download().delete_terminal(id).await? {
            return Err(AppError::new(
                "DOWNLOAD_TASK_REMOVE_CONFLICT",
                ErrorKind::Conflict,
                "下载任务状态已变化，请刷新后重试",
                true,
            ));
        }
        if let Some(batch_id) = task.batch_id {
            // 删除终态子任务后重算批次计数（防止删除任务导致批次计数与剩余任务错位）。
            let _ = self.batch.reconcile(batch_id).await;
        }
        Ok(DownloadMutationResultDto {
            schema_version: SCHEMA_VERSION,
            task_id: id.to_string(),
            record_removed: true,
            offline_resource_removed: false,
        })
    }

    pub async fn delete_offline(
        &self,
        request: DownloadTaskActionRequest,
    ) -> Result<DownloadMutationResultDto, AppError> {
        let id: DownloadTaskId = parse_id(&request.task_id, "下载任务")?;
        let (_task, resource, storage) = self.offline_context(id).await?;
        self.offline_files.delete(&storage, &resource).await?;
        if !self.ports.as_resource().delete(resource.id).await? {
            return Err(AppError::new(
                "DOWNLOAD_OFFLINE_DELETE_CONFLICT",
                ErrorKind::Conflict,
                "离线资源状态已变化，请刷新后重试",
                true,
            ));
        }
        Ok(DownloadMutationResultDto {
            schema_version: SCHEMA_VERSION,
            task_id: id.to_string(),
            record_removed: false,
            offline_resource_removed: true,
        })
    }

    pub async fn reveal_offline(
        &self,
        request: DownloadTaskActionRequest,
    ) -> Result<DownloadRevealResultDto, AppError> {
        let id: DownloadTaskId = parse_id(&request.task_id, "下载任务")?;
        let (_task, resource, storage) = self.offline_context(id).await?;
        self.offline_files.reveal(&storage, &resource).await?;
        Ok(DownloadRevealResultDto {
            schema_version: SCHEMA_VERSION,
            task_id: id.to_string(),
        })
    }

    async fn get_task(&self, id: DownloadTaskId) -> Result<DownloadTask, AppError> {
        self.ports
            .as_download()
            .get(id)
            .await?
            .ok_or_else(|| not_found("DOWNLOAD_TASK_NOT_FOUND", "下载任务不存在"))
    }

    async fn offline_context(
        &self,
        id: DownloadTaskId,
    ) -> Result<(DownloadTask, Resource, StorageLocation), AppError> {
        let task = self.get_task(id).await?;
        if task.state != DownloadState::Completed {
            return Err(AppError::new(
                "DOWNLOAD_NOT_COMPLETED",
                ErrorKind::Conflict,
                "下载尚未完成，没有可管理的离线文件",
                false,
            ));
        }
        let resource_id = task.offline_resource_id.ok_or_else(|| {
            not_found(
                "DOWNLOAD_OFFLINE_RESOURCE_NOT_FOUND",
                "该下载任务没有可用的离线资源",
            )
        })?;
        let resource = self
            .ports
            .as_resource()
            .get(resource_id)
            .await?
            .ok_or_else(|| {
                not_found(
                    "DOWNLOAD_OFFLINE_RESOURCE_NOT_FOUND",
                    "该下载任务的离线资源不存在",
                )
            })?;
        let storage = self
            .ports
            .as_storage()
            .get(task.target_storage_id)
            .await?
            .ok_or_else(|| not_found("STORAGE_LOCATION_NOT_FOUND", "目标存储位置不存在"))?;
        let media_matches = task
            .media_item_id
            .is_some_and(|media_item_id| media_item_id == resource.media_item_id);
        if resource.storage_location_id != Some(task.target_storage_id)
            || !media_matches
            || resource.availability != Availability::OfflineAvailable
            || storage.provider_type != StorageProviderType::Local
        {
            return Err(AppError::new(
                "DOWNLOAD_OFFLINE_RESOURCE_INVALID",
                ErrorKind::Security,
                "离线资源登记信息无效，已拒绝文件操作",
                false,
            ));
        }
        Ok((task, resource, storage))
    }

    async fn transition(
        &self,
        request: DownloadTaskActionRequest,
        transition: Transition,
    ) -> Result<DownloadTaskDto, AppError> {
        let id: DownloadTaskId = parse_id(&request.task_id, "下载任务")?;
        for _ in 0..3 {
            let current = self
                .ports
                .as_download()
                .get(id)
                .await?
                .ok_or_else(|| not_found("DOWNLOAD_TASK_NOT_FOUND", "下载任务不存在"))?;
            let next = match transition.next(current.state) {
                TransitionResult::Noop => return self.to_dto(current).await,
                TransitionResult::Invalid => {
                    return Err(AppError::new(
                        "DOWNLOAD_STATE_INVALID",
                        ErrorKind::Conflict,
                        "当前下载状态不支持此操作",
                        false,
                    ));
                }
                TransitionResult::Next(next) => next,
            };
            if self
                .ports
                .as_download()
                .compare_and_set_state(id, current.state, next)
                .await?
            {
                if next == DownloadState::Queued {
                    self.runner.start(id);
                }
                if matches!(
                    next,
                    DownloadState::Cancelled | DownloadState::Completed | DownloadState::Failed
                ) {
                    if let Some(batch_id) = current.batch_id {
                        // 子任务进入终态：批次聚合状态/进度由 BatchService 重算。
                        let _ = self.batch.reconcile(batch_id).await;
                    }
                }
                let updated = self
                    .ports
                    .as_download()
                    .get(id)
                    .await?
                    .ok_or_else(|| not_found("DOWNLOAD_TASK_NOT_FOUND", "下载任务不存在"))?;
                self.events.emit_task(&updated, None);
                return self.to_dto(updated).await;
            }
        }
        Err(AppError::new(
            "DOWNLOAD_STATE_CONFLICT",
            ErrorKind::Conflict,
            "下载任务状态已变化，请刷新后重试",
            true,
        ))
    }

    async fn to_dto(&self, task: DownloadTask) -> Result<DownloadTaskDto, AppError> {
        let mut title = "下载任务".to_owned();
        let mut media_type = MediaType::Unknown;
        let mut poster_uri = None;
        if let Some(media_item_id) = task.media_item_id {
            if let Some(media_item) = self.ports.as_media_item().get(media_item_id).await? {
                title = media_item.title;
                media_type = media_item.media_type;
                if let Some(edition) = self.ports.as_edition().get(media_item.edition_id).await? {
                    if let Some(work) = self.ports.as_work().get(edition.work_id).await? {
                        title = if work.canonical_title == title {
                            work.canonical_title
                        } else {
                            format!("{} · {}", work.canonical_title, title)
                        };
                        poster_uri = work
                            .artwork
                            .poster
                            .or(work.artwork.cover)
                            .map(|art| art.uri);
                    }
                }
            }
        }
        let progress_ratio = task
            .bytes_total
            .filter(|total| *total > 0)
            .map(|total| (task.bytes_downloaded as f64 / total as f64).clamp(0.0, 1.0));
        Ok(DownloadTaskDto {
            schema_version: SCHEMA_VERSION,
            task_id: task.id.to_string(),
            work_id: task.work_id.map(|id| id.to_string()),
            edition_id: task.edition_id.map(|id| id.to_string()),
            media_item_id: task.media_item_id.map(|id| id.to_string()),
            source_resource_id: task.source_resource_id.to_string(),
            target_storage_id: task.target_storage_id.to_string(),
            offline_resource_id: task.offline_resource_id.map(|id| id.to_string()),
            title,
            media_type: media_type_dto(media_type),
            category: category_dto(ContentCategory::from_media_type(media_type)),
            poster_uri,
            state: state_dto(task.state),
            bytes_total: task.bytes_total,
            bytes_downloaded: task.bytes_downloaded,
            speed_bps: task.speed_bps,
            eta_seconds: task.eta_seconds,
            progress_ratio,
            batch_id: task.batch_id.map(|id| id.to_string()),
            created_at: utc_millis_to_rfc3339(task.created_at),
            updated_at: utc_millis_to_rfc3339(task.updated_at),
        })
    }
}

/// Fail closed before a DownloadTask is created when a persisted remote
/// resource no longer matches the fixed source allowlist.  The worker repeats
/// the check at execution time; this early check keeps the task table free of
/// identities that could never be safely acquired.
fn validate_download_source_object(resource: &Resource) -> Result<(), AppError> {
    let ResourceLocator::SourceObject {
        source_id,
        remote_id,
    } = &resource.locator
    else {
        return Ok(());
    };
    let Some(source_key) = source_key_for_id(*source_id) else {
        return Err(AppError::new(
            "DOWNLOAD_SOURCE_UNSUPPORTED",
            ErrorKind::Validation,
            "该资源没有受控的可下载来源",
            false,
        ));
    };
    if resource.source_id != Some(*source_id) {
        return Err(AppError::new(
            "DOWNLOAD_SOURCE_UNSUPPORTED",
            ErrorKind::Security,
            "该资源的来源身份无效",
            false,
        ));
    }
    validate_remote_source_object(source_key, resource.resource_type, remote_id).map_err(|_| {
        AppError::new(
            "DOWNLOAD_SOURCE_UNSUPPORTED",
            ErrorKind::Validation,
            "该资源没有受控的可下载来源",
            false,
        )
    })?;
    if !remote_source_mime_compatible(
        source_key,
        resource.resource_type,
        resource.mime_type.as_deref(),
    ) {
        return Err(AppError::new(
            "DOWNLOAD_SOURCE_UNSUPPORTED",
            ErrorKind::Validation,
            "该资源没有受控的可下载格式",
            false,
        ));
    }
    Ok(())
}

fn should_start_on_boot(state: DownloadState, auto_continue: bool) -> bool {
    match state {
        // Queued 任务代表本次运行中已明确提交的下载意图，即使关闭“自动继续”，
        // 也应继续交给并发协调器；该开关只控制上次异常退出的 Interrupted 任务。
        DownloadState::Queued => true,
        DownloadState::Interrupted => auto_continue,
        _ => false,
    }
}

#[derive(Clone, Copy)]
enum Transition {
    Pause,
    Resume,
    Cancel,
    Retry,
}

enum TransitionResult {
    Next(DownloadState),
    Noop,
    Invalid,
}

impl Transition {
    fn next(self, state: DownloadState) -> TransitionResult {
        use DownloadState::*;
        match self {
            Self::Pause => match state {
                Queued | Resolving | Downloading | Interrupted => TransitionResult::Next(Paused),
                Paused => TransitionResult::Noop,
                _ => TransitionResult::Invalid,
            },
            Self::Resume => match state {
                Paused | Interrupted => TransitionResult::Next(Queued),
                Queued | Resolving | Downloading | Verifying => TransitionResult::Noop,
                _ => TransitionResult::Invalid,
            },
            Self::Cancel => match state {
                Cancelled => TransitionResult::Noop,
                Completed | Verifying => TransitionResult::Invalid,
                _ => TransitionResult::Next(Cancelled),
            },
            Self::Retry => match state {
                Failed | Interrupted | Cancelled => TransitionResult::Next(Queued),
                Queued | Resolving | Downloading | Paused | Verifying => TransitionResult::Noop,
                Completed => TransitionResult::Invalid,
            },
        }
    }
}

fn parse_id<T: std::str::FromStr>(value: &str, label: &str) -> Result<T, AppError> {
    value.parse().map_err(|_| {
        AppError::new(
            "INVALID_ID",
            ErrorKind::Validation,
            format!("无效的{label} ID"),
            false,
        )
    })
}

fn not_found(code: &'static str, message: &'static str) -> AppError {
    AppError::new(code, ErrorKind::NotFound, message, false)
}

fn state_dto(value: DownloadState) -> DownloadStateDto {
    match value {
        DownloadState::Queued => DownloadStateDto::Queued,
        DownloadState::Resolving => DownloadStateDto::Resolving,
        DownloadState::Downloading => DownloadStateDto::Downloading,
        DownloadState::Paused => DownloadStateDto::Paused,
        DownloadState::Verifying => DownloadStateDto::Verifying,
        DownloadState::Completed => DownloadStateDto::Completed,
        DownloadState::Failed => DownloadStateDto::Failed,
        DownloadState::Cancelled => DownloadStateDto::Cancelled,
        DownloadState::Interrupted => DownloadStateDto::Interrupted,
    }
}

pub fn download_event_data(task: &DownloadTask, error_code: Option<&str>) -> DownloadEventData {
    DownloadEventData {
        task_id: task.id.to_string(),
        state: state_dto(task.state),
        offline_resource_id: task.offline_resource_id.map(|id| id.to_string()),
        bytes_total: task.bytes_total,
        bytes_downloaded: task.bytes_downloaded,
        speed_bps: task.speed_bps,
        eta_seconds: task.eta_seconds,
        error_code: error_code.map(str::to_owned),
    }
}

fn media_type_dto(value: MediaType) -> MediaTypeDto {
    match value {
        MediaType::Movie => MediaTypeDto::Movie,
        MediaType::Series => MediaTypeDto::Series,
        MediaType::Episode => MediaTypeDto::Episode,
        MediaType::Book => MediaTypeDto::Book,
        MediaType::Document => MediaTypeDto::Document,
        MediaType::Comic => MediaTypeDto::Comic,
        MediaType::Article => MediaTypeDto::Article,
        MediaType::Audio => MediaTypeDto::Audio,
        MediaType::Unknown => MediaTypeDto::Unknown,
    }
}

fn category_dto(value: ContentCategory) -> ContentCategoryDto {
    match value {
        ContentCategory::All => unreachable!("MediaType 不会派生出 All 分类"),
        ContentCategory::Video => ContentCategoryDto::Video,
        ContentCategory::Book => ContentCategoryDto::Book,
        ContentCategory::Comic => ContentCategoryDto::Comic,
        ContentCategory::Periodical => ContentCategoryDto::Periodical,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::enums::ResourceType;
    use haven_domain::ids::MediaItemId;

    #[test]
    fn state_machine_keeps_verification_atomic() {
        assert!(matches!(
            Transition::Pause.next(DownloadState::Verifying),
            TransitionResult::Invalid
        ));
        assert!(matches!(
            Transition::Cancel.next(DownloadState::Verifying),
            TransitionResult::Invalid
        ));
    }

    #[test]
    fn interrupted_tasks_can_resume_and_failed_tasks_can_retry() {
        assert!(matches!(
            Transition::Resume.next(DownloadState::Interrupted),
            TransitionResult::Next(DownloadState::Queued)
        ));
        assert!(matches!(
            Transition::Retry.next(DownloadState::Failed),
            TransitionResult::Next(DownloadState::Queued)
        ));
    }

    #[test]
    fn auto_continue_only_controls_interrupted_tasks_on_boot() {
        assert!(should_start_on_boot(DownloadState::Queued, false));
        assert!(should_start_on_boot(DownloadState::Interrupted, true));
        assert!(!should_start_on_boot(DownloadState::Interrupted, false));
        assert!(!should_start_on_boot(DownloadState::Completed, true));
        assert!(!should_start_on_boot(DownloadState::Paused, true));
    }

    fn remote_resource(
        source_key: &str,
        resource_type: ResourceType,
        remote_id: &str,
        row_source_id: Option<haven_domain::ids::SourceId>,
    ) -> Resource {
        let source_id = crate::services::source_import::stable_source_id(source_key)
            .unwrap_or_else(|_| haven_domain::ids::SourceId::new());
        let mime_type = match source_key {
            "mangadex" => Some("application/vnd.comicbook+zip".to_owned()),
            "arxiv" => Some("application/pdf".to_owned()),
            "opds_gutenberg" => Some("application/epub+zip".to_owned()),
            "europepmc" | "wikisource" => Some("text/html; charset=utf-8".to_owned()),
            _ => None,
        };
        Resource {
            id: ResourceId::new(),
            media_item_id: MediaItemId::new(),
            resource_type,
            source_id: row_source_id.or(Some(source_id)),
            storage_location_id: None,
            locator: ResourceLocator::SourceObject {
                source_id,
                remote_id: remote_id.to_owned(),
            },
            mime_type,
            size: None,
            hash: None,
            availability: Availability::Available,
            availability_source: haven_domain::enums::AvailabilitySource::User,
            modified_ms: None,
            fingerprint_first: None,
            fingerprint_last: None,
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        }
    }

    #[test]
    fn download_rejects_malformed_or_mismatched_source_objects() {
        let valid = remote_resource(
            "mangadex",
            ResourceType::ComicArchive,
            "00000000-0000-0000-0000-000000000001:00000000-0000-0000-0000-000000000002",
            None,
        );
        assert!(validate_download_source_object(&valid).is_ok());

        let unknown = Resource {
            locator: ResourceLocator::SourceObject {
                source_id: haven_domain::ids::SourceId::new(),
                remote_id: "anything".into(),
            },
            ..valid.clone()
        };
        assert_eq!(
            validate_download_source_object(&unknown)
                .unwrap_err()
                .code()
                .as_str(),
            "DOWNLOAD_SOURCE_UNSUPPORTED"
        );

        let mismatch = remote_resource(
            "arxiv",
            ResourceType::PublicationFile,
            "2401.12345",
            Some(haven_domain::ids::SourceId::new()),
        );
        assert!(validate_download_source_object(&mismatch).is_err());

        let url = remote_resource(
            "arxiv",
            ResourceType::PublicationFile,
            "https://evil.invalid/paper.pdf",
            None,
        );
        assert!(validate_download_source_object(&url).is_err());

        let wrong_type = remote_resource(
            "mangadex",
            ResourceType::PublicationFile,
            "00000000-0000-0000-0000-000000000001:00000000-0000-0000-0000-000000000002",
            None,
        );
        assert!(validate_download_source_object(&wrong_type).is_err());

        let mut wrong_mime = valid;
        wrong_mime.mime_type = Some("application/pdf".into());
        assert!(validate_download_source_object(&wrong_mime).is_err());
    }
}
