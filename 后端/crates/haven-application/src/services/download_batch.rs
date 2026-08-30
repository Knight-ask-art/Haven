//! DownloadBatchService：批次聚合状态派生与进度归一（DOMAIN_MODEL §45）。
//!
//! 规则（高级模型裁决 D 题）：
//! - Batch 只聚合子任务，不直接拥有网络执行权；子任务进入三层调度器。
//! - 终态推导：全部终态时——全 Completed → Completed；全 Failed → Failed；
//!   全 Cancelled → Cancelled；混合（Failed/Cancelled 存在且非全部同态）→ PartialCompleted。
//! - progressRatio：全部子任务 bytes_total 已知 → 按字节；否则按完成数/总数。
//! - 本服务由 Worker / DownloadService 在子任务进入终态后调用（幂等重放安全）。

use std::sync::Arc;

use haven_common::AppError;
use haven_domain::contracts::{DownloadBatchRepository, DownloadRepository};
use haven_domain::entities::DownloadTask;
use haven_domain::enums::{BatchState, DownloadState};
use haven_domain::ids::DownloadBatchId;

/// Application 端口组合：Batch 聚合需要任务 + 批次两个仓储。
pub trait DownloadBatchPorts: DownloadRepository + DownloadBatchRepository + Send + Sync {
    fn as_download(&self) -> &dyn DownloadRepository;
    fn as_batch(&self) -> &dyn DownloadBatchRepository;
}

impl<T> DownloadBatchPorts for T
where
    T: DownloadRepository + DownloadBatchRepository + Send + Sync,
{
    fn as_download(&self) -> &dyn DownloadRepository {
        self
    }
    fn as_batch(&self) -> &dyn DownloadBatchRepository {
        self
    }
}

#[derive(Clone)]
pub struct DownloadBatchService {
    ports: Arc<dyn DownloadBatchPorts>,
}

impl DownloadBatchService {
    pub fn new(ports: Arc<dyn DownloadBatchPorts>) -> Self {
        Self { ports }
    }

    /// 按当前子任务集合重算批次聚合值并落库。调用方在子任务进入终态后触发；
    /// 重复调用是幂等的（只收敛到同一派生结果）。
    pub async fn reconcile(&self, batch_id: DownloadBatchId) -> Result<(), AppError> {
        let Some(mut batch) = self.ports.as_batch().get(batch_id).await? else {
            return Ok(());
        };
        let tasks = self.ports.as_download().list_by_batch(batch_id).await?;
        if tasks.is_empty() {
            return Ok(());
        }
        let (completed, failed, cancelled, active, all_totals_known, total_bytes, completed_bytes) =
            aggregate_tasks(&tasks);
        batch.total_tasks = tasks.len() as u32;
        batch.completed_tasks = completed;
        batch.total_bytes = if all_totals_known {
            Some(total_bytes)
        } else {
            None
        };
        batch.completed_bytes = if all_totals_known { completed_bytes } else { 0 };
        batch.state = derive_batch_state(completed, failed, cancelled, active, tasks.len() as u32);
        batch.updated_at = haven_common::UtcMillis::now();
        self.ports.as_batch().save(&batch).await
    }
}

#[cfg(test)]
fn is_active(state: DownloadState) -> bool {
    matches!(
        state,
        DownloadState::Resolving
            | DownloadState::Downloading
            | DownloadState::Verifying
            | DownloadState::Paused
            | DownloadState::Interrupted
    )
}

/// 聚合子任务计数与字节进度。返回
/// (completed, failed, cancelled, active, all_totals_known, total_bytes, completed_bytes)。
fn aggregate_tasks(tasks: &[DownloadTask]) -> (u32, u32, u32, u32, bool, u64, u64) {
    let mut completed = 0u32;
    let mut failed = 0u32;
    let mut cancelled = 0u32;
    let mut active = 0u32;
    let mut all_totals_known = true;
    let mut total_bytes = 0u64;
    let mut completed_bytes = 0u64;
    for task in tasks {
        match task.state {
            DownloadState::Completed => {
                completed += 1;
                completed_bytes = completed_bytes.saturating_add(task.bytes_downloaded);
            }
            DownloadState::Failed => failed += 1,
            DownloadState::Cancelled => cancelled += 1,
            DownloadState::Resolving
            | DownloadState::Downloading
            | DownloadState::Verifying
            | DownloadState::Paused
            | DownloadState::Interrupted => active += 1,
            DownloadState::Queued => {}
        }
        match task.bytes_total {
            Some(bytes) => total_bytes = total_bytes.saturating_add(bytes),
            None => all_totals_known = false,
        }
    }
    (
        completed,
        failed,
        cancelled,
        active,
        all_totals_known,
        total_bytes,
        completed_bytes,
    )
}

/// 批次状态派生：子任务全终态时按同态/混合判定；否则按是否存在活跃子任务。
fn derive_batch_state(
    completed: u32,
    failed: u32,
    cancelled: u32,
    active: u32,
    total: u32,
) -> BatchState {
    let terminal = completed + failed + cancelled;
    if terminal == total {
        if completed == total {
            BatchState::Completed
        } else if failed == total {
            BatchState::Failed
        } else if cancelled == total {
            BatchState::Cancelled
        } else {
            BatchState::PartialCompleted
        }
    } else if active > 0 {
        BatchState::Downloading
    } else {
        BatchState::Queued
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(state: DownloadState, bytes_total: Option<u64>, bytes_downloaded: u64) -> DownloadTask {
        DownloadTask {
            id: haven_domain::ids::DownloadTaskId::new(),
            work_id: None,
            edition_id: None,
            media_item_id: None,
            source_resource_id: haven_domain::ids::ResourceId::new(),
            target_storage_id: haven_domain::ids::StorageLocationId::new(),
            offline_resource_id: None,
            state,
            bytes_total,
            bytes_downloaded,
            speed_bps: None,
            eta_seconds: None,
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
            batch_id: None,
            priority: haven_domain::enums::DownloadPriority::Normal,
            provider_key: None,
            host_key: None,
            variant_key: String::new(),
            resource_identity: None,
            retry_count: 0,
            not_before: None,
            resumable: None,
        }
    }

    #[test]
    fn derive_batch_state_covers_terminal_combinations() {
        assert_eq!(derive_batch_state(3, 0, 0, 0, 3), BatchState::Completed);
        assert_eq!(derive_batch_state(0, 3, 0, 0, 3), BatchState::Failed);
        assert_eq!(derive_batch_state(0, 0, 3, 0, 3), BatchState::Cancelled);
        assert_eq!(
            derive_batch_state(2, 1, 0, 0, 3),
            BatchState::PartialCompleted
        );
        assert_eq!(
            derive_batch_state(1, 0, 2, 0, 3),
            BatchState::PartialCompleted
        );
        assert_eq!(derive_batch_state(1, 0, 0, 1, 3), BatchState::Downloading);
        assert_eq!(derive_batch_state(0, 0, 0, 0, 3), BatchState::Queued);
    }

    #[test]
    fn aggregate_bytes_when_all_totals_known_else_counts() {
        let tasks = vec![
            task(DownloadState::Completed, Some(100), 100),
            task(DownloadState::Failed, Some(200), 50),
            task(DownloadState::Queued, Some(300), 0),
        ];
        let (completed, failed, cancelled, active, known, total, done) = aggregate_tasks(&tasks);
        assert_eq!((completed, failed, cancelled, active), (1, 1, 0, 0));
        assert!(known);
        assert_eq!(total, 600);
        assert_eq!(done, 100);

        let unknown = vec![
            task(DownloadState::Completed, None, 10),
            task(DownloadState::Completed, Some(50), 50),
        ];
        let (_, _, _, _, known, total, done) = aggregate_tasks(&unknown);
        assert!(!known, "任一子任务缺 bytes_total 时必须按计数");
        // 函数层只聚合已知字节；reconcile 在 not all known 时把 total_bytes/completed_bytes 归零。
        assert_eq!(total, 50);
        assert_eq!(done, 60);
    }

    #[test]
    fn active_states_include_paused_and_interrupted() {
        for state in [
            DownloadState::Resolving,
            DownloadState::Downloading,
            DownloadState::Verifying,
            DownloadState::Paused,
            DownloadState::Interrupted,
        ] {
            assert!(is_active(state), "{state:?} 应视为活跃");
        }
        for state in [
            DownloadState::Queued,
            DownloadState::Completed,
            DownloadState::Failed,
            DownloadState::Cancelled,
        ] {
            assert!(!is_active(state), "{state:?} 不应视为活跃");
        }
    }
}
