//! 漫画进度 Subject 兼容层（由 `progress` 通过 path 子模块引入）。

use std::sync::Arc;

use haven_common::AppError;
use haven_domain::comic_identity::ChapterEvidence;
use haven_domain::comic_progress_subject::{ComicProgressSubject, ComicProgressSubjectMember};
use haven_domain::contracts::{
    ComicProgressSubjectRepository, EditionRepository, MediaItemRepository, ProgressRepository,
};
use haven_domain::entities::Progress;
use haven_domain::enums::MediaType;
use haven_domain::ids::MediaItemId;

use crate::services::ports::{ComicProgressSubjectWritePlan, UnitOfWork};

pub trait ComicProgressSubjectPorts:
    ComicProgressSubjectRepository
    + EditionRepository
    + MediaItemRepository
    + ProgressRepository
    + Send
    + Sync
{
}

impl<T> ComicProgressSubjectPorts for T where
    T: ComicProgressSubjectRepository
        + EditionRepository
        + MediaItemRepository
        + ProgressRepository
        + Send
        + Sync
{
}

/// 一次 Subject 解析的只读结果；Progress 保持既有实体，不复制其字段。
#[derive(Debug, Clone)]
pub struct ComicProgressSubjectResolution {
    pub subject: ComicProgressSubject,
    pub member: ComicProgressSubjectMember,
    pub authoritative_progress: Option<Progress>,
}

#[derive(Clone)]
pub struct ComicProgressSubjectService {
    ports: Arc<dyn ComicProgressSubjectPorts>,
    unit_of_work: Arc<dyn UnitOfWork>,
}

impl ComicProgressSubjectService {
    pub fn new(
        ports: Arc<dyn ComicProgressSubjectPorts>,
        unit_of_work: Arc<dyn UnitOfWork>,
    ) -> Self {
        Self {
            ports,
            unit_of_work,
        }
    }

    pub async fn ensure_for_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<ComicProgressSubjectResolution, AppError> {
        // Do these ownership reads before consulting the Subject repository.  In
        // particular this keeps every non-comic Progress path entirely outside
        // the Subject compatibility layer.
        let media_item = MediaItemRepository::get(&*self.ports, media_item_id)
            .await?
            .ok_or_else(media_item_not_found)?;
        if media_item.media_type != MediaType::Comic {
            return Err(AppError::new(
                "COMIC_PROGRESS_SUBJECT_MEDIA_TYPE",
                haven_common::ErrorKind::Validation,
                "漫画进度主体仅适用于 Comic MediaItem",
                false,
            ));
        }
        let edition = EditionRepository::get(&*self.ports, media_item.edition_id)
            .await?
            .ok_or_else(edition_not_found)?;

        let target_progress =
            ProgressRepository::get_for_media_item(&*self.ports, media_item_id).await?;
        let (mut subject, members) = match ComicProgressSubjectRepository::get_for_media_item(
            &*self.ports,
            media_item_id,
        )
        .await?
        {
            Some(member) => {
                let subject = ComicProgressSubjectRepository::get(&*self.ports, member.subject_id)
                    .await?
                    .ok_or_else(subject_not_found)?;
                let members =
                    ComicProgressSubjectRepository::list_members(&*self.ports, subject.id).await?;
                (subject, members)
            }
            None => {
                let now = haven_common::UtcMillis::now();
                let subject =
                    ComicProgressSubject::new(edition.work_id, edition.id, media_item_id, now);
                let mut member = ComicProgressSubjectMember::active(
                    subject.id,
                    media_item_id,
                    vec![ChapterEvidence::SameRemoteIdentity],
                );
                member.relationship = haven_domain::comic_progress_subject::ComicProgressSubjectRelationship::Canonical;
                let members = vec![member];
                self.unit_of_work.run_comic_progress_subject_write(
                    &ComicProgressSubjectWritePlan {
                        subject: subject.clone(),
                        members: members.clone(),
                        page_identity_write: None,
                        progress_writes: Vec::new(),
                        migration_snapshot: None,
                        refresh_receipt: None,
                    },
                )?;
                (subject, members)
            }
        };

        subject
            .load_members(members.clone())
            .map_err(|error| invalid_subject(error.to_string()))?;
        let selected = self.select_authoritative_progress(&subject).await?;

        // A legacy Subject can lack a pointer.  Only add the mapping; existing
        // Progress rows (including their revision/history/markers) are never
        // copied, overwritten, or deleted here.
        if subject.authoritative_progress_media_item_id.is_none() {
            if let Some(progress) = selected.as_ref() {
                subject.authoritative_progress_media_item_id = Some(progress.media_item_id);
                self.unit_of_work.run_comic_progress_subject_write(
                    &ComicProgressSubjectWritePlan {
                        subject: subject.clone(),
                        members: members.clone(),
                        page_identity_write: None,
                        progress_writes: Vec::new(),
                        migration_snapshot: None,
                        refresh_receipt: None,
                    },
                )?;
            }
        }

        let member = members
            .iter()
            .find(|member| member.media_item_id == media_item_id)
            .cloned()
            .ok_or_else(subject_member_not_found)?;
        // When the requested MediaItem already has a Progress it is the
        // caller's current view; it must not be replaced by another member.
        Ok(ComicProgressSubjectResolution {
            subject,
            member,
            authoritative_progress: target_progress.or(selected),
        })
    }

    async fn select_authoritative_progress(
        &self,
        subject: &ComicProgressSubject,
    ) -> Result<Option<Progress>, AppError> {
        let mut progress = Vec::new();
        for media_item_id in subject.active_progress_media_items() {
            if let Some(item) =
                ProgressRepository::get_for_media_item(&*self.ports, media_item_id).await?
            {
                progress.push(item);
            }
        }
        progress.sort_by(|left, right| {
            right
                .last_active_at
                .cmp(&left.last_active_at)
                .then_with(|| {
                    left.media_item_id
                        .to_string()
                        .cmp(&right.media_item_id.to_string())
                })
        });
        Ok(progress.into_iter().next())
    }
}

fn media_item_not_found() -> AppError {
    AppError::new(
        "MEDIA_ITEM_NOT_FOUND",
        haven_common::ErrorKind::NotFound,
        "MediaItem 不存在",
        false,
    )
}

fn edition_not_found() -> AppError {
    AppError::new(
        "EDITION_NOT_FOUND",
        haven_common::ErrorKind::NotFound,
        "Edition 不存在",
        false,
    )
}

fn subject_not_found() -> AppError {
    AppError::new(
        "COMIC_PROGRESS_SUBJECT_NOT_FOUND",
        haven_common::ErrorKind::NotFound,
        "漫画进度主体不存在",
        false,
    )
}

fn subject_member_not_found() -> AppError {
    AppError::new(
        "COMIC_PROGRESS_SUBJECT_MEMBER_NOT_FOUND",
        haven_common::ErrorKind::NotFound,
        "漫画进度主体成员不存在",
        false,
    )
}

fn invalid_subject(message: String) -> AppError {
    AppError::new(
        "COMIC_PROGRESS_SUBJECT_INVALID",
        haven_common::ErrorKind::Validation,
        message,
        false,
    )
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn existing_comic_progress_is_backfilled_without_copying_progress_fields() {
        let _ = super::ComicProgressSubjectService::new;
    }
}
