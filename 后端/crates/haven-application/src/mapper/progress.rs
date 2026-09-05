//! Progress → ProgressSummaryDto 映射（契约 §12、§22.6）。
//!
//! 规则：completion 转闭合 wire 枚举（NOTE-2）；revision 保留持久层 opaque token；
//! updatedAt 仅转 RFC 3339 展示时间；
//! locator 映射失败时整体失败（不产出残缺摘要）。
//! NOTE：progressRatio 是持久化 percentage 的投影；percentage 在 Service.save
//! 时由 Locator 派生（DOMAIN_MODEL §30），此处不重算。

use haven_common::AppError;
use haven_domain::entities::Progress;
use haven_domain::enums::CompletionState;

use crate::mapper::locator::locator_to_dto;
use crate::mapper::time::utc_millis_to_rfc3339;
use crate::wire::{CompletionWire, ProgressSummaryDto};

pub fn progress_summary(progress: &Progress) -> Result<ProgressSummaryDto, AppError> {
    let revision = progress
        .revision
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::new(
                "PROGRESS_REVISION_MISSING",
                haven_common::ErrorKind::Database,
                "Progress 缺少持久化 revision",
                false,
            )
        })?;
    Ok(ProgressSummaryDto {
        media_item_id: progress.media_item_id.to_string(),
        completion: CompletionWire::from(progress.completion),
        progress_ratio: progress.percentage.map(|v| v as f64),
        revision: revision.to_owned(),
        updated_at: utc_millis_to_rfc3339(progress.updated_at),
        locator: locator_to_dto(&progress.locator)?,
        keyframe_uri: progress.keyframe_uri.clone(),
    })
}

pub fn completion_wire(completion: CompletionState) -> CompletionWire {
    completion.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::ids::{EditionId, MediaItemId, ProgressId, WorkId};
    use haven_domain::locator::{Locator, VideoLocator};

    fn sample_progress() -> Progress {
        Progress {
            id: ProgressId::new(),
            work_id: WorkId::new(),
            edition_id: EditionId::new(),
            media_item_id: MediaItemId::new(),
            locator: Locator::Video(VideoLocator {
                media_item_id: MediaItemId::new(),
                position_ms: 60_000,
            }),
            completion: CompletionState::InProgress,
            percentage: Some(0.5),
            last_active_at: haven_common::UtcMillis(1_700_000_000_000),
            updated_at: haven_common::UtcMillis(1_700_000_000_000),
            revision: Some("opaque-revision".into()),
            keyframe_uri: None,
        }
    }

    #[test]
    fn summary_projects_wire_fields() {
        let p = sample_progress();
        let dto = progress_summary(&p).unwrap();
        assert_eq!(dto.completion, crate::wire::CompletionWire::InProgress);
        assert_eq!(dto.progress_ratio, Some(0.5));
        assert_eq!(dto.revision, p.revision.clone().unwrap());
        assert_eq!(dto.updated_at, "2023-11-14T22:13:20Z");
        assert_eq!(dto.media_item_id, p.media_item_id.to_string());
        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("\"mediaItemId\""), "{json}");
        assert!(json.contains("\"progressRatio\":0.5"), "{json}");
    }
}
