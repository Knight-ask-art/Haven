//! 漫画进度 Subject 兼容层（由 `progress` 通过 path 子模块引入）。

use std::sync::Arc;

use haven_common::AppError;
use haven_domain::comic_identity::ChapterEvidence;
use haven_domain::comic_progress_subject::{
    ComicProgressSubject, ComicProgressSubjectMember, ComicProgressSubjectState,
};
use haven_domain::contracts::{
    ComicProgressSubjectRepository, EditionRepository, MediaItemRepository, ProgressRepository,
};
use haven_domain::entities::Progress;
use haven_domain::enums::MediaType;
use haven_domain::ids::MediaItemId;

use crate::services::ports::{
    ComicProgressSubjectWritePlan, ComicProgressSubjectWritePrecondition, UnitOfWork,
};

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

/// 懒回填的最大尝试次数（含首次）。每次尝试都重新读取聚合、在**新的**
/// Immediate 事务里做受检写入；冲突后重读重算，而不是无限自递归或重放
/// 已经失效的旧计划。
const MAX_ENSURE_FOR_MEDIA_ITEM_ATTEMPTS: usize = 3;

/// 单次回填尝试的结果。`RetryAfterConflict` 只由真正的 Immediate 事务内
/// CAS 冲突产生：该次尝试没有任何写入落库，调用方必须在有界预算内重读。
///
/// `Resolved` 装箱只为避免把整个聚合（约 544 字节）撑进每次尝试的返回值；
/// 它在成功路径上会立刻被解引用返回给调用方。
enum EnsureOutcome {
    Resolved(Box<ComicProgressSubjectResolution>),
    RetryAfterConflict(AppError),
}

/// 并发冲突的稳定错误码；Application 只据它区分"可重读"与"必须传播"。
fn is_subject_conflict(error: &AppError) -> bool {
    error.code().as_str() == "COMIC_PROGRESS_SUBJECT_CONFLICT"
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
        let mut attempts_remaining = MAX_ENSURE_FOR_MEDIA_ITEM_ATTEMPTS;
        loop {
            match self.ensure_for_media_item_once(media_item_id).await? {
                EnsureOutcome::Resolved(resolution) => return Ok(*resolution),
                EnsureOutcome::RetryAfterConflict(error) => {
                    attempts_remaining -= 1;
                    if attempts_remaining == 0 {
                        // 预算耗尽仍冲突：明确报冲突，不返回部分结果。
                        return Err(error);
                    }
                }
            }
        }
    }

    async fn ensure_for_media_item_once(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<EnsureOutcome, AppError> {
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
                let mut subject =
                    ComicProgressSubject::new(edition.work_id, edition.id, media_item_id, now);
                // `new` defaults to the canonical MediaItem as a convenient
                // mapping seed.  A normal unread legacy Comic has no Progress,
                // however, so it must persist no dangling authority pointer.
                subject.authoritative_progress_media_item_id = target_progress
                    .as_ref()
                    .map(|progress| progress.media_item_id);
                let mut member = ComicProgressSubjectMember::active(
                    subject.id,
                    media_item_id,
                    vec![ChapterEvidence::SameRemoteIdentity],
                );
                member.relationship = haven_domain::comic_progress_subject::ComicProgressSubjectRelationship::Canonical;
                let members = vec![member];
                let create = self.unit_of_work.run_checked_comic_progress_subject_write(
                    &ComicProgressSubjectWritePlan {
                        subject: subject.clone(),
                        members: members.clone(),
                        page_identity_write: None,
                        progress_writes: Vec::new(),
                        migration_snapshot: None,
                        refresh_receipt: None,
                    },
                    &ComicProgressSubjectWritePrecondition::AbsentActiveMember { media_item_id },
                );
                match create {
                    Ok(_) => (subject, members),
                    Err(error) if is_subject_conflict(&error) => {
                        // 受检创建在事务内发现同一 MediaItem 已有 active 成员：
                        // 只有重新读到**合法**的 active winner（仍在 Active 且
                        // 可读）才恢复；无 winner 或其他读取错误继续传播。
                        let Some((winner_subject, winner_members)) =
                            self.read_active_winner(media_item_id).await?
                        else {
                            return Err(error);
                        };
                        (winner_subject, winner_members)
                    }
                    Err(error) => return Err(error),
                }
            }
        };

        subject
            .load_members(members.clone())
            .map_err(|error| invalid_subject(error.to_string()))?;
        let authoritative_progress = if let Some(authoritative_media_item_id) =
            subject.authoritative_progress_media_item_id
        {
            // Validate every persisted pointer before applying the requested
            // MediaItem's view preference.  A dangling pointer is fail-closed;
            // a target-owned Progress may win the returned view only after the
            // Subject's unique authority has been proven to exist.
            Some(
                ProgressRepository::get_for_media_item(&*self.ports, authoritative_media_item_id)
                    .await?
                    .ok_or_else(authoritative_progress_missing)?,
            )
        } else {
            let selected = self.select_authoritative_progress(&subject).await?;
            // A legacy Subject can lack a pointer.  Only add the mapping;
            // existing Progress rows (including revision/history/markers) are
            // never copied, overwritten, or deleted here.
            if let Some(progress) = selected.as_ref() {
                let expected_subject = subject.clone();
                subject.authoritative_progress_media_item_id = Some(progress.media_item_id);
                let update = self.unit_of_work.run_checked_comic_progress_subject_write(
                    &ComicProgressSubjectWritePlan {
                        subject: subject.clone(),
                        members: members.clone(),
                        page_identity_write: None,
                        progress_writes: Vec::new(),
                        migration_snapshot: None,
                        refresh_receipt: None,
                    },
                    &ComicProgressSubjectWritePrecondition::ExactSnapshot {
                        subject: expected_subject,
                        members: members.clone(),
                        require_authoritative_progress_none: true,
                    },
                );
                if let Err(error) = update {
                    if is_subject_conflict(&error) {
                        // The checked write did not mutate anything. Re-read and
                        // recompute under the bounded retry budget instead of
                        // replaying the stale destructive plan or recursing
                        // without a bound.
                        return Ok(EnsureOutcome::RetryAfterConflict(error));
                    }
                    return Err(error);
                }
            }
            selected
        };

        let member = members
            .iter()
            .find(|member| {
                member.media_item_id == media_item_id
                    && member.state
                        == haven_domain::comic_progress_subject::ComicProgressSubjectMemberState::Active
            })
            .cloned()
            .ok_or_else(subject_member_not_found)?;
        // When the requested MediaItem already has a Progress it is the
        // caller's current view; it must not be replaced by another member.
        Ok(EnsureOutcome::Resolved(Box::new(
            ComicProgressSubjectResolution {
                subject,
                member,
                authoritative_progress: target_progress.or(authoritative_progress),
            },
        )))
    }

    /// 重新读取同一 MediaItem 的并发创建 winner。
    ///
    /// 只有仍处于 Active（未 redirect）且完整可读的 Subject 才算合法 winner；
    /// 否则返回 None，让调用方传播原始冲突而不是把一个不可用的聚合当成结果。
    async fn read_active_winner(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Option<(ComicProgressSubject, Vec<ComicProgressSubjectMember>)>, AppError> {
        let Some(winner) =
            ComicProgressSubjectRepository::get_for_media_item(&*self.ports, media_item_id).await?
        else {
            return Ok(None);
        };
        let winner_subject = ComicProgressSubjectRepository::get(&*self.ports, winner.subject_id)
            .await?
            .ok_or_else(subject_not_found)?;
        if winner_subject.state != ComicProgressSubjectState::Active
            || winner_subject.redirect_subject_id.is_some()
        {
            return Ok(None);
        }
        let winner_members =
            ComicProgressSubjectRepository::list_members(&*self.ports, winner.subject_id).await?;
        Ok(Some((winner_subject, winner_members)))
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

fn authoritative_progress_missing() -> AppError {
    AppError::new(
        "COMIC_PROGRESS_SUBJECT_AUTHORITATIVE_PROGRESS_MISSING",
        haven_common::ErrorKind::NotFound,
        "漫画进度主体的权威 Progress 不存在",
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
    use super::*;
    use crate::services::ports::{
        ComicProgressSubjectWritePrecondition, ComicProgressSubjectWriteResult, FavoriteTxPorts,
        UnitOfWork,
    };
    use haven_common::UtcMillis;
    use haven_domain::comic_identity::ChapterEvidence;
    use haven_domain::comic_progress_subject::ComicProgressSubjectRelationship;
    use haven_domain::entities::{Edition, MediaIndex, MediaItem, Resource, Work};
    use haven_domain::enums::{CompletionState, MediaItemStatus};
    use haven_domain::ids::{ComicProgressSubjectId, EditionId, ProgressId, WorkId};
    use haven_domain::locator::{ComicLocator, Locator};
    use haven_infrastructure::Db;
    use haven_infrastructure::db::repos::SqliteRepositories;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ---- 真实 SQLite fixture -------------------------------------------------
    //
    // 本 crate 的测试目标与 `haven-infrastructure` 之间是 dev-dependency 环：
    // `SqliteUnitOfWork` 实现的是 infra 侧链接的那一份 `haven_application` 的
    // `UnitOfWork`，无法当作本 crate 的 `dyn UnitOfWork` 使用。因此这里用真实
    // `SqliteRepositories` 读写真实 SQLite，只用测试内的 UnitOfWork 复现"事务内
    // CAS 发现并发变更"这一信号；真正的 Immediate 事务、事务内重读与
    // `save_on_conn` 原子性由 `haven-infrastructure` 的 `SqliteUnitOfWork` 测试覆盖。

    /// Work + Edition + MediaItem，全部走真实 Repository 写入。
    async fn seed_media_chain(
        repos: &SqliteRepositories,
        title: &str,
        media_type: MediaType,
        media_item_id: MediaItemId,
    ) -> (WorkId, EditionId) {
        use haven_domain::contracts::WorkRepository;
        use haven_domain::enums::{WorkStatus, WorkType};

        let now = UtcMillis(1);
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        WorkRepository::save(
            repos,
            &Work {
                id: work_id,
                canonical_title: title.to_owned(),
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
                artwork: Default::default(),
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .unwrap();
        EditionRepository::save(
            repos,
            &Edition {
                id: edition_id,
                work_id,
                title: format!("{title} 版本"),
                subtitle: None,
                edition_type: media_type,
                release_date: None,
                language: None,
                region: None,
                publisher_or_studio: None,
                description: None,
                artwork: Default::default(),
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .unwrap();
        let (index, duration_ms, page_count) = match media_type {
            MediaType::Comic => (
                MediaIndex::Chapter {
                    volume: None,
                    chapter: 1.0,
                },
                None,
                Some(10),
            ),
            _ => (MediaIndex::Movie, Some(1_000), None),
        };
        MediaItemRepository::save(
            repos,
            &MediaItem {
                id: media_item_id,
                edition_id,
                parent_id: None,
                media_type,
                title: "第 1 话".to_owned(),
                index,
                duration_ms,
                page_count,
                chapter_count: None,
                published_at: None,
                status: MediaItemStatus::Available,
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .unwrap();
        (work_id, edition_id)
    }

    async fn seed_comic_chain(
        repos: &SqliteRepositories,
        title: &str,
        media_item_id: MediaItemId,
    ) -> (WorkId, EditionId) {
        seed_media_chain(repos, title, MediaType::Comic, media_item_id).await
    }

    /// 在同一 Edition 下追加一个 Comic MediaItem。
    async fn add_comic_media_item(
        repos: &SqliteRepositories,
        edition_id: EditionId,
        chapter: f32,
    ) -> MediaItemId {
        let media_item_id = MediaItemId::new();
        MediaItemRepository::save(
            repos,
            &MediaItem {
                id: media_item_id,
                edition_id,
                parent_id: None,
                media_type: MediaType::Comic,
                title: format!("第 {chapter} 话"),
                index: MediaIndex::Chapter {
                    volume: None,
                    chapter,
                },
                duration_ms: None,
                page_count: Some(10),
                chapter_count: None,
                published_at: None,
                status: MediaItemStatus::Available,
                created_at: UtcMillis(1),
                updated_at: UtcMillis(1),
            },
        )
        .await
        .unwrap();
        media_item_id
    }

    /// 为 fixture MediaItem 保存一条 Comic Progress，返回持久化 revision。
    async fn seed_progress(
        repos: &SqliteRepositories,
        work_id: WorkId,
        edition_id: EditionId,
        media_item_id: MediaItemId,
        at: i64,
    ) -> String {
        let progress = Progress {
            id: ProgressId::new(),
            work_id,
            edition_id,
            media_item_id,
            locator: Locator::Comic(ComicLocator {
                chapter_item_id: media_item_id,
                page_index: 3,
                page_progression: Some(0.4),
            }),
            completion: CompletionState::InProgress,
            percentage: Some(0.4),
            last_active_at: UtcMillis(at),
            updated_at: UtcMillis(at),
            revision: None,
            keyframe_uri: Some("data:image/png;base64,fixture".to_owned()),
        };
        ProgressRepository::save_if_revision(repos, &progress, None)
            .await
            .unwrap()
            .expect("fixture Progress 必须取得 revision")
    }

    /// 合法 active 成员（relationship 非 candidate、confidence=high）。
    fn active_subject_member(
        subject_id: ComicProgressSubjectId,
        media_item_id: MediaItemId,
        relationship: ComicProgressSubjectRelationship,
        at: i64,
    ) -> ComicProgressSubjectMember {
        let mut member = ComicProgressSubjectMember::active(
            subject_id,
            media_item_id,
            vec![ChapterEvidence::SameRemoteIdentity],
        );
        member.relationship = relationship;
        member.created_at = UtcMillis(at);
        member.updated_at = UtcMillis(at);
        member
    }

    // ---- 冲突注入测试替身 ----------------------------------------------------

    /// 受检写入的哪一类前置条件需要被"插队"。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum RaceTrigger {
        /// 首次创建：`AbsentActiveMember`。
        FirstCreate,
        /// 权威 pointer 回填：`ExactSnapshot` + pointer-none。
        PointerBackfill,
    }

    /// 首次创建的前置条件（`AbsentActiveMember`）。
    fn is_first_create(precondition: &ComicProgressSubjectWritePrecondition) -> bool {
        matches!(
            precondition,
            ComicProgressSubjectWritePrecondition::AbsentActiveMember { .. }
        )
    }

    /// 权威 pointer 回填的前置条件（`ExactSnapshot` + pointer-none）。
    fn is_pointer_backfill(precondition: &ComicProgressSubjectWritePrecondition) -> bool {
        matches!(
            precondition,
            ComicProgressSubjectWritePrecondition::ExactSnapshot {
                require_authoritative_progress_none: true,
                ..
            }
        )
    }

    /// 只在指定前置条件上返回 `COMIC_PROGRESS_SUBJECT_CONFLICT` 的 UnitOfWork。
    ///
    /// 它不做任何写入，只复现"受检写入在自己的事务里发现并发变更"这一信号，
    /// 用于把 Application 的有界重试与失败关闭行为钉在真实 SQLite 读取之上。
    struct ConflictUnitOfWork {
        trigger: RaceTrigger,
        checked_writes: AtomicUsize,
    }

    impl ConflictUnitOfWork {
        fn for_trigger(trigger: RaceTrigger) -> Arc<Self> {
            Arc::new(Self {
                trigger,
                checked_writes: AtomicUsize::new(0),
            })
        }

        fn checked_writes(&self) -> usize {
            self.checked_writes.load(Ordering::SeqCst)
        }

        fn matches(&self, precondition: &ComicProgressSubjectWritePrecondition) -> bool {
            match self.trigger {
                RaceTrigger::FirstCreate => is_first_create(precondition),
                RaceTrigger::PointerBackfill => is_pointer_backfill(precondition),
            }
        }
    }

    impl UnitOfWork for ConflictUnitOfWork {
        fn run_favorite(
            &self,
            _f: &dyn Fn(&dyn FavoriteTxPorts) -> Result<(), AppError>,
        ) -> Result<(), AppError> {
            Err(unavailable_write())
        }

        fn run_source_import(
            &self,
            _provider: &str,
            _external_id: &str,
            _work: &Work,
            _edition: &Edition,
            _items: &[MediaItem],
            _resources: &[Resource],
        ) -> Result<(), AppError> {
            Err(unavailable_write())
        }

        fn run_comic_progress_subject_write(
            &self,
            _plan: &ComicProgressSubjectWritePlan,
        ) -> Result<ComicProgressSubjectWriteResult, AppError> {
            Err(unavailable_write())
        }

        fn run_checked_comic_progress_subject_write(
            &self,
            _plan: &ComicProgressSubjectWritePlan,
            precondition: &ComicProgressSubjectWritePrecondition,
        ) -> Result<ComicProgressSubjectWriteResult, AppError> {
            if self.matches(precondition) {
                self.checked_writes.fetch_add(1, Ordering::SeqCst);
                return Err(subject_conflict_error());
            }
            Err(unavailable_write())
        }
    }

    fn subject_conflict_error() -> AppError {
        AppError::new(
            "COMIC_PROGRESS_SUBJECT_CONFLICT",
            haven_common::ErrorKind::Conflict,
            "漫画进度主体已被并发更新，请重新读取",
            false,
        )
    }

    fn unavailable_write() -> AppError {
        AppError::new(
            "COMIC_PROGRESS_SUBJECT_UOW_UNAVAILABLE",
            haven_common::ErrorKind::Internal,
            "测试用 UnitOfWork 不执行该写入",
            false,
        )
    }

    #[test]
    fn comic_progress_subject_backfill_resolution_retains_the_existing_progress_entity() {
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let mut subject = ComicProgressSubject::new(
            work_id,
            edition_id,
            media_item_id,
            haven_common::UtcMillis(1),
        );
        let mut member = ComicProgressSubjectMember::active(
            subject.id,
            media_item_id,
            vec![ChapterEvidence::SameRemoteIdentity],
        );
        member.relationship = ComicProgressSubjectRelationship::Canonical;
        subject.attach_member(member.clone()).unwrap();
        let progress = Progress {
            id: ProgressId::new(),
            work_id,
            edition_id,
            media_item_id,
            locator: Locator::Comic(ComicLocator {
                chapter_item_id: media_item_id,
                page_index: 7,
                page_progression: Some(0.7),
            }),
            completion: CompletionState::InProgress,
            percentage: Some(0.7),
            last_active_at: haven_common::UtcMillis(3),
            updated_at: haven_common::UtcMillis(4),
            revision: Some("existing-revision".to_owned()),
            keyframe_uri: Some("data:image/png;base64,existing".to_owned()),
        };
        let resolution = ComicProgressSubjectResolution {
            subject,
            member,
            authoritative_progress: Some(progress.clone()),
        };
        assert_eq!(resolution.authoritative_progress, Some(progress));
        assert_eq!(
            resolution.subject.authoritative_progress_media_item_id,
            Some(media_item_id)
        );
    }

    /// 首次创建的受检写入冲突后，只有重新读到同一 MediaItem 的合法 active
    /// winner 才允许恢复。这里数据库里没有任何 winner：必须**立刻**传播
    /// `COMIC_PROGRESS_SUBJECT_CONFLICT`，不得重试、不得写入部分聚合，也不得
    /// 把任意聚合当成结果返回。
    #[tokio::test]
    async fn comic_progress_subject_backfill_create_conflict_without_winner_propagates_and_writes_nothing()
     {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db));
        let media_item_id = MediaItemId::new();
        seed_comic_chain(&repos, "无 winner", media_item_id).await;
        let unit_of_work = ConflictUnitOfWork::for_trigger(RaceTrigger::FirstCreate);
        let service = ComicProgressSubjectService::new(repos.clone(), unit_of_work.clone());

        let error = service
            .ensure_for_media_item(media_item_id)
            .await
            .expect_err("无合法 active winner 时必须传播冲突");
        assert_eq!(error.code().as_str(), "COMIC_PROGRESS_SUBJECT_CONFLICT");
        assert_eq!(error.kind(), haven_common::ErrorKind::Conflict);
        assert_eq!(
            unit_of_work.checked_writes(),
            1,
            "创建冲突没有 winner 时必须在首次尝试后立即传播，不得进入重试预算"
        );
        assert!(
            ComicProgressSubjectRepository::get_for_media_item(&*repos, media_item_id)
                .await
                .unwrap()
                .is_none(),
            "冲突传播后不得留下 Subject/member 部分写入"
        );
        assert!(
            ProgressRepository::get_for_media_item(&*repos, media_item_id)
                .await
                .unwrap()
                .is_none()
        );
    }

    /// pointer 回填的受检写入持续冲突时，重算必须**有界**：每次尝试都重新读取
    /// 真实 SQLite 聚合，耗尽 `MAX_ENSURE_FOR_MEDIA_ITEM_ATTEMPTS` 后返回冲突，
    /// 且期间没有任何写入落库——旧聚合（pointer=None、两个 active 成员、两条
    /// Progress revision）必须原样可读。
    #[tokio::test]
    async fn comic_progress_subject_backfill_pointer_conflict_retry_is_bounded_and_keeps_old_aggregate()
     {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db));
        let first_media = MediaItemId::new();
        let (work, edition) = seed_comic_chain(&repos, "有界重试", first_media).await;
        let second_media = add_comic_media_item(&repos, edition, 2.0).await;
        // 第一个成员有更新的 last_active_at：若允许重放旧计划，pointer 会指向它。
        let first_revision = seed_progress(&repos, work, edition, first_media, 200).await;
        let second_revision = seed_progress(&repos, work, edition, second_media, 100).await;

        // 已存在但缺少权威 pointer 的 Subject（两个 active 成员）。
        let mut subject = ComicProgressSubject::new(work, edition, first_media, UtcMillis(1));
        subject.authoritative_progress_media_item_id = None;
        let canonical = ComicProgressSubjectRelationship::Canonical;
        let equivalent = ComicProgressSubjectRelationship::Equivalent;
        let first_member = active_subject_member(subject.id, first_media, canonical, 1);
        let second_member = active_subject_member(subject.id, second_media, equivalent, 2);
        subject.attach_member(first_member.clone()).unwrap();
        subject.attach_member(second_member.clone()).unwrap();
        let stored = subject.clone();
        ComicProgressSubjectRepository::save_subject(&*repos, &stored)
            .await
            .unwrap();
        assert_eq!(
            ComicProgressSubjectRepository::get(&*repos, stored.id)
                .await
                .unwrap()
                .unwrap(),
            stored
        );

        let unit_of_work = ConflictUnitOfWork::for_trigger(RaceTrigger::PointerBackfill);
        let service = ComicProgressSubjectService::new(repos.clone(), unit_of_work.clone());
        let error = service
            .ensure_for_media_item(first_media)
            .await
            .expect_err("预算耗尽后必须返回冲突，而不是部分结果");
        assert_eq!(error.code().as_str(), "COMIC_PROGRESS_SUBJECT_CONFLICT");
        assert_eq!(
            unit_of_work.checked_writes(),
            MAX_ENSURE_FOR_MEDIA_ITEM_ATTEMPTS,
            "受检写入的尝试次数必须等于有界预算，不得无限重试"
        );
        assert_eq!(
            ComicProgressSubjectRepository::get(&*repos, stored.id)
                .await
                .unwrap()
                .unwrap(),
            stored,
            "重试期间不得改写 Subject（含 pointer 与 updated_at）"
        );
        assert_eq!(
            ComicProgressSubjectRepository::list_members(&*repos, stored.id)
                .await
                .unwrap(),
            vec![first_member, second_member],
            "重试期间不得丢成员，也不得把计划写入落库"
        );
        assert_eq!(
            ProgressRepository::get_for_media_item(&*repos, first_media)
                .await
                .unwrap()
                .unwrap()
                .revision
                .as_deref(),
            Some(first_revision.as_str())
        );
        assert_eq!(
            ProgressRepository::get_for_media_item(&*repos, second_media)
                .await
                .unwrap()
                .unwrap()
                .revision
                .as_deref(),
            Some(second_revision.as_str())
        );
    }

    /// 非 Comic MediaItem 必须在接触 Subject 仓储之前被拒绝：即使 UnitOfWork
    /// 会在任何受检写入上报告冲突，也不得发生一次 Subject 写入，Progress 保持
    /// 原样。
    #[tokio::test]
    async fn comic_progress_subject_backfill_rejects_non_comic_media_item_before_any_subject_write()
    {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db));
        let media_item_id = MediaItemId::new();
        let (work, edition) =
            seed_media_chain(&repos, "非漫画", MediaType::Movie, media_item_id).await;
        let revision = {
            let progress = Progress {
                id: ProgressId::new(),
                work_id: work,
                edition_id: edition,
                media_item_id,
                locator: Locator::Comic(ComicLocator {
                    chapter_item_id: media_item_id,
                    page_index: 3,
                    page_progression: Some(0.4),
                }),
                completion: CompletionState::InProgress,
                percentage: Some(0.4),
                last_active_at: UtcMillis(1),
                updated_at: UtcMillis(1),
                revision: None,
                keyframe_uri: None,
            };
            ProgressRepository::save_if_revision(&*repos, &progress, None)
                .await
                .unwrap()
                .expect("fixture Progress 必须取得 revision")
        };
        let unit_of_work = ConflictUnitOfWork::for_trigger(RaceTrigger::FirstCreate);
        let service = ComicProgressSubjectService::new(repos.clone(), unit_of_work.clone());

        let error = service
            .ensure_for_media_item(media_item_id)
            .await
            .expect_err("非 Comic MediaItem 必须被拒绝");
        assert_eq!(error.code().as_str(), "COMIC_PROGRESS_SUBJECT_MEDIA_TYPE");
        assert_eq!(unit_of_work.checked_writes(), 0);
        assert!(
            ComicProgressSubjectRepository::get_for_media_item(&*repos, media_item_id)
                .await
                .unwrap()
                .is_none(),
            "非 Comic 读取路径不得进入 Subject 兼容层"
        );
        assert_eq!(
            ProgressRepository::get_for_media_item(&*repos, media_item_id)
                .await
                .unwrap()
                .unwrap()
                .revision
                .as_deref(),
            Some(revision.as_str())
        );
    }

    /// 缺少 MediaItem 时必须在任何 Subject 访问前报 NOT_FOUND。
    #[tokio::test]
    async fn comic_progress_subject_backfill_rejects_missing_media_item() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db));
        let unit_of_work = ConflictUnitOfWork::for_trigger(RaceTrigger::FirstCreate);
        let service = ComicProgressSubjectService::new(repos, unit_of_work.clone());
        let error = service
            .ensure_for_media_item(MediaItemId::new())
            .await
            .expect_err("不存在的 MediaItem 必须报 NOT_FOUND");
        assert_eq!(error.code().as_str(), "MEDIA_ITEM_NOT_FOUND");
        assert_eq!(unit_of_work.checked_writes(), 0);
    }
}
