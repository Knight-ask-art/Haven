//! 漫画进度 Subject 兼容层（由 `progress` 通过 path 子模块引入）。

use std::sync::Arc;

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::comic_identity::{
    ChapterEvidence, ChapterMatch, ChapterMatchKind, ChapterSourceIdentity, ChapterSourceRef,
    ComicProgressMigrationSnapshot, MatchConfidence, PageIdentity, ProgressMigrationMode,
    ProgressMigrationState, compare_chapters, compare_chapters_within_media_item,
    compare_edition_profiles, migrate_page_index,
};
use haven_domain::comic_progress_subject::{
    ComicProgressSubject, ComicProgressSubjectMember, ComicProgressSubjectMemberState,
    ComicProgressSubjectRelationship, ComicProgressSubjectState, select_subject_survivor,
};
use haven_domain::contracts::{
    ChapterSourceRepository, ComicPageIdentityRepository, ComicProgressSubjectRepository,
    EditionRepository, MediaItemRepository, ProgressRepository,
};
use haven_domain::entities::{Edition, MediaItem, Progress};
use haven_domain::enums::MediaType;
use haven_domain::ids::{
    ComicProgressMigrationId, ComicProgressSubjectId, MediaItemId, ProgressId,
};
use haven_domain::locator::{ComicLocator, Locator};

use crate::mapper::progress::progress_summary;
use crate::services::comic_progress_migration::{
    COMIC_PROGRESS_MIGRATION_ALGORITHM_VERSION, ComicProgressMigrationReceipt,
    ComicProgressMigrationResult, ComicProgressMigrationStatus, ComicProgressReceiptInput,
    migration_receipt, migration_result_with_receipt, no_target_page_migration,
    progress_snapshot_view, revision_conflict, translated_progress,
};
use crate::services::ports::{
    ComicProgressSubjectMergeExpected, ComicProgressSubjectMergePlan,
    ComicProgressSubjectWritePlan, ComicProgressSubjectWritePrecondition,
    ComicProgressWriteCandidate, UnitOfWork,
};

pub trait ComicProgressSubjectPorts:
    ComicProgressSubjectRepository
    + ComicPageIdentityRepository
    + ChapterSourceRepository
    + EditionRepository
    + MediaItemRepository
    + ProgressRepository
    + Send
    + Sync
{
}

impl<T> ComicProgressSubjectPorts for T where
    T: ComicProgressSubjectRepository
        + ComicPageIdentityRepository
        + ChapterSourceRepository
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

/// 当前漫画 MediaItem 视角的进度，以及一次显式物化时产生的可撤销 Receipt。
/// `progress` 的 locator 永远绑定请求的 MediaItem；Subject 只保存权威行指针，
/// 不把页面位置等 payload 复制进 Subject。
#[derive(Debug, Clone)]
pub struct ComicProgressSubjectProgress {
    pub progress: Option<Progress>,
    pub migration_receipt: Option<ComicProgressMigrationReceipt>,
}

/// 两条已登记章节来源在 Subject 层的连续性结论。
///
/// 这里返回的是关系事实和匹配证据，不携带 Progress payload；页面位置仍然
/// 由 Progress/Locator 保存，低置信度关系也不会因为返回该结果而参与 active
/// 进度解析。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComicProgressSubjectReconcileResult {
    pub subject_id: ComicProgressSubjectId,
    pub relationship: ComicProgressSubjectRelationship,
    pub confidence: MatchConfidence,
    pub evidence: Vec<ChapterEvidence>,
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

    /// 根据两条已经登记的章节来源身份，建立或解析漫画阅读连续性。
    ///
    /// 远端章节 ID 只是来源事实：只要章节属于同一 Work/Edition，且领域比较
    /// 给出了高置信度内容证据，不同的远端 ID 也会进入同一个 active Subject。
    /// 元数据或部分页面证据不足时只落 Candidate，不会让两个 MediaItem 共享
    /// active Progress。两个已有 Subject 的合并也在一个 Immediate UoW 中完成，
    /// loser 保留为 Redirected，既有 Progress/History/Marker 和来源引用不动。
    pub async fn reconcile_chapters(
        &self,
        source: ChapterSourceIdentity,
        target: ChapterSourceIdentity,
    ) -> Result<ComicProgressSubjectReconcileResult, AppError> {
        let source_ref = ChapterSourceRepository::get(&*self.ports, &source)
            .await?
            .ok_or_else(|| chapter_source_ref_not_found("源章节来源身份不存在"))?;
        let target_ref = ChapterSourceRepository::get(&*self.ports, &target)
            .await?
            .ok_or_else(|| chapter_source_ref_not_found("目标章节来源身份不存在"))?;
        let (source_item, source_edition) =
            self.load_comic_context(source_ref.media_item_id).await?;
        let (target_item, target_edition) =
            self.load_comic_context(target_ref.media_item_id).await?;
        ensure_comic_edition(&source_edition)?;
        ensure_comic_edition(&target_edition)?;
        if source_edition.work_id != target_edition.work_id {
            return Err(comic_work_mismatch());
        }
        if source_item.edition_id != target_item.edition_id {
            return Err(edition_conflict());
        }

        let edition_match = compare_edition_profiles(
            &source_ref.metadata.edition_profile,
            &target_ref.metadata.edition_profile,
        );
        if matches!(
            edition_match.kind,
            haven_domain::comic_identity::EditionMatchKind::Distinct
        ) {
            return Err(edition_conflict());
        }

        let source_pages = ComicPageIdentityRepository::list(&*self.ports, source_item.id).await?;
        let target_pages = if source_item.id == target_item.id {
            source_pages.clone()
        } else {
            ComicPageIdentityRepository::list(&*self.ports, target_item.id).await?
        };
        let match_result = if source_item.id == target_item.id {
            compare_chapters_within_media_item(
                &source_ref.identity,
                &source_ref.metadata,
                &source_pages,
                &target_ref.identity,
                &target_ref.metadata,
                &target_pages,
            )
        } else {
            compare_chapters(
                &source_ref.identity,
                &source_ref.metadata,
                &source_pages,
                &target_ref.identity,
                &target_ref.metadata,
                &target_pages,
            )
        };
        if match_result.kind == ChapterMatchKind::Unrelated {
            return Err(edition_conflict());
        }

        let relationship = if can_activate_reconciliation(&match_result) {
            if source_item.id == target_item.id {
                ComicProgressSubjectRelationship::Canonical
            } else {
                ComicProgressSubjectRelationship::Equivalent
            }
        } else {
            ComicProgressSubjectRelationship::Candidate
        };
        let subject_id = self
            .persist_chapter_reconciliation(
                &source_ref,
                &target_ref,
                source_edition.work_id,
                source_edition.id,
                &match_result,
                relationship,
            )
            .await?;

        Ok(ComicProgressSubjectReconcileResult {
            subject_id,
            relationship,
            confidence: match_result.confidence,
            evidence: match_result.evidence,
        })
    }

    /// 在已经完成章节关系收敛后，把一次跨 MediaItem 的 Progress 写入同一个
    /// Subject/Progress/Snapshot Immediate 事务。
    ///
    /// `ComicProgressMigrationService` 仍负责计算 ChapterMatch、PageMigration
    /// 和完整快照；本方法只负责把目标行接到当前 Subject，并用源/目标 revision
    /// 做事务内 CAS。Candidate member 不会被提升为 active，也不会改变 Subject
    /// 的 authoritative pointer。
    pub async fn apply_migration_progress(
        &self,
        source_progress: Progress,
        target_progress: Option<Progress>,
        new_progress: Progress,
        snapshot: ComicProgressMigrationSnapshot,
    ) -> Result<String, AppError> {
        let source_media_item_id = source_progress.media_item_id;
        let target_media_item_id = new_progress.media_item_id;
        let Some((source_subject, _)) = self
            .read_subject_for_media_item(source_media_item_id)
            .await?
            .or(self
                .read_subject_for_media_item(target_media_item_id)
                .await?)
        else {
            return Err(subject_not_found());
        };
        let mut subject = source_subject.clone();
        let members = subject.members().to_vec();
        let target_is_active = members.iter().any(|member| {
            member.media_item_id == target_media_item_id
                && member.state == ComicProgressSubjectMemberState::Active
                && member.participates_in_active_progress()
        });
        if target_is_active {
            set_authoritative_pointer(&mut subject, Some(target_media_item_id))?;
        }

        let source_revision = progress_revision(&source_progress)?.to_owned();
        let target_revision = target_progress
            .as_ref()
            .map(progress_revision)
            .transpose()?
            .map(str::to_owned);
        let expected_target_revision = target_revision.clone();
        let result = self.unit_of_work.run_checked_comic_progress_subject_write(
            &ComicProgressSubjectWritePlan {
                subject,
                members: members.clone(),
                page_identity_write: None,
                progress_writes: vec![ComicProgressWriteCandidate {
                    progress: new_progress,
                    expected_revision: expected_target_revision,
                }],
                migration_snapshot: Some(snapshot),
                refresh_receipt: None,
            },
            &ComicProgressSubjectWritePrecondition::ExactSnapshot {
                subject: Box::new(source_subject),
                members,
                require_authoritative_progress_none: false,
                require_progress_absent_for_media_item: target_progress
                    .is_none()
                    .then_some(target_media_item_id),
                require_progress_revision_for_media_item: (source_media_item_id
                    != target_media_item_id)
                    .then_some((source_media_item_id, source_revision)),
            },
        )?;
        result
            .applied_progress_revisions
            .into_iter()
            .next()
            .ok_or_else(|| invalid_subject_write("漫画迁移未返回目标 Progress revision"))
    }

    async fn persist_chapter_reconciliation(
        &self,
        source_ref: &ChapterSourceRef,
        target_ref: &ChapterSourceRef,
        work_id: haven_domain::ids::WorkId,
        edition_id: haven_domain::ids::EditionId,
        match_result: &ChapterMatch,
        relationship: ComicProgressSubjectRelationship,
    ) -> Result<ComicProgressSubjectId, AppError> {
        let source_subject = self
            .read_subject_for_media_item(source_ref.media_item_id)
            .await?;
        let target_subject = if source_ref.media_item_id == target_ref.media_item_id {
            source_subject.clone()
        } else {
            self.read_subject_for_media_item(target_ref.media_item_id)
                .await?
        };

        if let (Some((source, _)), Some((target, _))) = (&source_subject, &target_subject) {
            if source.id == target.id {
                // 已经是同一个 active Subject 时，绝不因一次较弱的重新观察
                // 把既有 active 关系降级为 Candidate。
                return Ok(source.id);
            }
            if relationship == ComicProgressSubjectRelationship::Equivalent {
                return self
                    .merge_reconciled_subjects(source, target, source_ref, target_ref, match_result)
                    .await;
            }

            // 低置信度关系不合并两个已有 Subject。将候选证据挂在 source
            // Subject 上，target 自己的 active Subject 保持不变。
            let mut subject = source.clone();
            let mut members = source.members().to_vec();
            let candidate = reconciliation_member(
                subject.id,
                target_ref.media_item_id,
                ComicProgressSubjectRelationship::Candidate,
                &match_result.evidence,
            );
            upsert_member(&mut members, candidate);
            subject
                .load_members(members.clone())
                .map_err(|error| invalid_subject(error.to_string()))?;
            self.refresh_authoritative_pointer(&mut subject).await?;
            self.unit_of_work.run_checked_comic_progress_subject_write(
                &ComicProgressSubjectWritePlan {
                    subject,
                    members,
                    page_identity_write: None,
                    progress_writes: Vec::new(),
                    migration_snapshot: None,
                    refresh_receipt: None,
                },
                &ComicProgressSubjectWritePrecondition::ExactSnapshot {
                    subject: Box::new(source.clone()),
                    members: source.members().to_vec(),
                    require_authoritative_progress_none: false,
                    require_progress_absent_for_media_item: None,
                    require_progress_revision_for_media_item: None,
                },
            )?;
            return Ok(source.id);
        }

        if let Some((existing, _)) = source_subject.or(target_subject) {
            let is_source_subject = existing.members().iter().any(|member| {
                member.media_item_id == source_ref.media_item_id
                    && member.state == ComicProgressSubjectMemberState::Active
            });
            let new_media_item_id = if is_source_subject {
                target_ref.media_item_id
            } else {
                source_ref.media_item_id
            };
            if new_media_item_id == existing.canonical_media_item_id
                && existing.members().iter().any(|member| {
                    member.media_item_id == new_media_item_id
                        && member.state == ComicProgressSubjectMemberState::Active
                })
            {
                return Ok(existing.id);
            }
            let mut subject = existing.clone();
            let old_members = existing.members().to_vec();
            let mut members = old_members.clone();
            upsert_member(
                &mut members,
                reconciliation_member(
                    subject.id,
                    new_media_item_id,
                    relationship,
                    &match_result.evidence,
                ),
            );
            subject
                .load_members(members.clone())
                .map_err(|error| invalid_subject(error.to_string()))?;
            self.refresh_authoritative_pointer(&mut subject).await?;
            let subject_id = subject.id;
            self.unit_of_work.run_checked_comic_progress_subject_write(
                &ComicProgressSubjectWritePlan {
                    subject,
                    members,
                    page_identity_write: None,
                    progress_writes: Vec::new(),
                    migration_snapshot: None,
                    refresh_receipt: None,
                },
                &ComicProgressSubjectWritePrecondition::ExactSnapshot {
                    subject: Box::new(existing),
                    members: old_members,
                    require_authoritative_progress_none: false,
                    require_progress_absent_for_media_item: None,
                    require_progress_revision_for_media_item: None,
                },
            )?;
            return Ok(subject_id);
        }

        let mut subject = ComicProgressSubject::new(
            work_id,
            edition_id,
            source_ref.media_item_id,
            UtcMillis::now(),
        );
        // 新 Subject 在本次关系落库前不应带一个尚未存在的 Progress 指针。
        subject.authoritative_progress_media_item_id = None;
        let mut members = vec![canonical_member(subject.id, source_ref.media_item_id)];
        if source_ref.media_item_id != target_ref.media_item_id {
            members.push(reconciliation_member(
                subject.id,
                target_ref.media_item_id,
                relationship,
                &match_result.evidence,
            ));
        }
        subject
            .load_members(members.clone())
            .map_err(|error| invalid_subject(error.to_string()))?;
        self.refresh_authoritative_pointer(&mut subject).await?;
        let subject_id = subject.id;
        self.unit_of_work.run_checked_comic_progress_subject_write(
            &ComicProgressSubjectWritePlan {
                subject,
                members,
                page_identity_write: None,
                progress_writes: Vec::new(),
                migration_snapshot: None,
                refresh_receipt: None,
            },
            &ComicProgressSubjectWritePrecondition::AbsentActiveMembers {
                media_item_ids: dedup_media_item_ids([
                    source_ref.media_item_id,
                    target_ref.media_item_id,
                ]),
            },
        )?;
        Ok(subject_id)
    }

    async fn merge_reconciled_subjects(
        &self,
        source: &ComicProgressSubject,
        target: &ComicProgressSubject,
        source_ref: &ChapterSourceRef,
        target_ref: &ChapterSourceRef,
        match_result: &ChapterMatch,
    ) -> Result<ComicProgressSubjectId, AppError> {
        if source.work_id != target.work_id || source.edition_id != target.edition_id {
            return Err(edition_conflict());
        }
        let survivor_id = select_subject_survivor(&[source.clone(), target.clone()])
            .ok_or_else(|| invalid_subject("合并漫画进度主体时缺少 survivor".to_owned()))?;
        let (survivor_before, loser_before) = if source.id == survivor_id {
            (source, target)
        } else {
            (target, source)
        };

        let mut survivor_members =
            merge_members_for_survivor(survivor_id, source.members(), target.members());
        upsert_member(
            &mut survivor_members,
            reconciliation_member(
                survivor_id,
                source_ref.media_item_id,
                if source_ref.media_item_id == survivor_before.canonical_media_item_id {
                    ComicProgressSubjectRelationship::Canonical
                } else {
                    ComicProgressSubjectRelationship::Equivalent
                },
                &match_result.evidence,
            ),
        );
        if target_ref.media_item_id != source_ref.media_item_id {
            upsert_member(
                &mut survivor_members,
                reconciliation_member(
                    survivor_id,
                    target_ref.media_item_id,
                    if target_ref.media_item_id == survivor_before.canonical_media_item_id {
                        ComicProgressSubjectRelationship::Canonical
                    } else {
                        ComicProgressSubjectRelationship::Equivalent
                    },
                    &match_result.evidence,
                ),
            );
        }

        let mut survivor = survivor_before.clone();
        survivor
            .load_members(survivor_members.clone())
            .map_err(|error| invalid_subject(error.to_string()))?;
        self.refresh_authoritative_pointer(&mut survivor).await?;

        let mut redirected = loser_before.clone();
        redirected
            .redirect_to(survivor_id)
            .map_err(|error| invalid_subject(error.to_string()))?;
        let survivor_id = survivor.id;
        self.unit_of_work
            .run_comic_progress_subject_merge(&ComicProgressSubjectMergePlan {
                survivor,
                survivor_members,
                redirected_subjects: vec![redirected],
                expected_subjects: vec![
                    ComicProgressSubjectMergeExpected {
                        expected_subject: source.clone(),
                        expected_members: source.members().to_vec(),
                    },
                    ComicProgressSubjectMergeExpected {
                        expected_subject: target.clone(),
                        expected_members: target.members().to_vec(),
                    },
                ],
                progress_writes: Vec::new(),
                migration_snapshot: None,
            })?;
        Ok(survivor_id)
    }

    async fn refresh_authoritative_pointer(
        &self,
        subject: &mut ComicProgressSubject,
    ) -> Result<(), AppError> {
        let pointer = self
            .select_authoritative_progress(subject)
            .await?
            .map(|progress| progress.media_item_id);
        set_authoritative_pointer(subject, pointer)
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

    /// 只读解析当前 MediaItem 视角的 Progress。
    ///
    /// 与 `ensure_for_media_item` 不同，本方法绝不创建 Subject、回填 pointer
    /// 或写入 Progress，供 Library/Home/Work 等列表投影使用。目标已有 Progress
    /// 时优先返回目标行；目标没有行时才把 Subject 的权威行投影为当前 locator。
    pub async fn progress_for_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Option<Progress>, AppError> {
        let (media_item, edition) = self.load_comic_context(media_item_id).await?;
        let target_progress =
            ProgressRepository::get_for_media_item(&*self.ports, media_item_id).await?;
        if let Some(progress) = target_progress {
            ensure_comic_progress(&progress, &media_item, &edition)?;
            return Ok(Some(progress));
        }

        let Some((subject, _member)) = self.read_subject_for_media_item(media_item_id).await?
        else {
            return Ok(None);
        };
        let Some(authoritative) = self.authoritative_progress(&subject).await? else {
            return Ok(None);
        };
        if authoritative.media_item_id == media_item_id {
            ensure_comic_progress(&authoritative, &media_item, &edition)?;
            return Ok(Some(authoritative));
        }
        self.project_progress_for_media_item(&authoritative, &media_item, &edition)
            .await
    }

    /// 当前 MediaItem 的进度摘要只读投影。该入口不会因为列表加载而物化
    /// Subject/Progress，且摘要中的 mediaItemId/Comic locator 使用当前条目。
    pub async fn progress_summary_for_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Option<crate::wire::ProgressSummaryDto>, AppError> {
        self.progress_for_media_item(media_item_id)
            .await?
            .as_ref()
            .map(progress_summary)
            .transpose()
    }

    /// 返回当前 MediaItem 所属 active Subject ID，仅读取，不创建 Subject。
    pub async fn subject_id_for_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Option<ComicProgressSubjectId>, AppError> {
        Ok(
            ComicProgressSubjectRepository::get_for_media_item(&*self.ports, media_item_id)
                .await?
                .map(|member| member.subject_id),
        )
    }

    /// 返回 Subject 当前权威 Progress 原行，供首页 Continue 去重使用。
    /// 不改变 pointer，也不把来源行的 locator 伪装成其他 MediaItem。
    pub async fn authoritative_progress_for_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Option<Progress>, AppError> {
        let (_media_item, _edition) = self.load_comic_context(media_item_id).await?;
        let Some((subject, _member)) = self.read_subject_for_media_item(media_item_id).await?
        else {
            return ProgressRepository::get_for_media_item(&*self.ports, media_item_id).await;
        };
        self.authoritative_progress(&subject).await
    }

    /// 显式 Session/保存路径使用的物化入口。
    ///
    /// 目标已有 Progress 时只切换 Subject pointer，不覆盖目标 payload；目标没有
    /// Progress 时按页面身份/页数执行现有最佳努力算法，并把 Subject、目标行和
    /// migration snapshot 放入同一个 Immediate 事务。
    pub async fn materialize_for_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<ComicProgressSubjectProgress, AppError> {
        let resolution = self.ensure_for_media_item(media_item_id).await?;
        let (target_item, target_edition) = self.load_comic_context(media_item_id).await?;
        let target_progress =
            ProgressRepository::get_for_media_item(&*self.ports, media_item_id).await?;
        if let Some(target_progress) = target_progress {
            ensure_comic_progress(&target_progress, &target_item, &target_edition)?;
            if resolution.subject.authoritative_progress_media_item_id != Some(media_item_id) {
                let mut subject = resolution.subject.clone();
                set_authoritative_media_item(&mut subject, media_item_id)?;
                self.write_subject_pointer(&resolution, subject).await?;
            }
            return Ok(ComicProgressSubjectProgress {
                progress: Some(target_progress),
                migration_receipt: None,
            });
        }

        let Some(authoritative) = self.authoritative_progress(&resolution.subject).await? else {
            return Ok(ComicProgressSubjectProgress {
                progress: None,
                migration_receipt: None,
            });
        };
        let Some((page_migration, mut projected)) = self
            .project_progress_with_mapping(&authoritative, &target_item, &target_edition)
            .await?
        else {
            return Ok(ComicProgressSubjectProgress {
                progress: None,
                migration_receipt: None,
            });
        };

        let source_item = MediaItemRepository::get(&*self.ports, authoritative.media_item_id)
            .await?
            .ok_or_else(media_item_not_found)?;
        let source_edition = EditionRepository::get(&*self.ports, source_item.edition_id)
            .await?
            .ok_or_else(edition_not_found)?;
        ensure_comic_progress(&authoritative, &source_item, &source_edition)?;
        let source_revision = progress_revision(&authoritative)?.to_owned();
        let migration_id = ComicProgressMigrationId::new();
        let created_at = UtcMillis::now();
        let snapshot = ComicProgressMigrationSnapshot {
            id: migration_id,
            source_media_item_id: authoritative.media_item_id,
            target_media_item_id: target_item.id,
            source_revision: source_revision.clone(),
            target_revision_before: None,
            old_progress: authoritative.clone(),
            old_target_progress: None,
            new_progress: projected.clone(),
            mode: ProgressMigrationMode::OneTime,
            confidence: page_migration.confidence,
            strategy: page_migration.strategy,
            evidence: resolution.member.evidence.clone(),
            created_at,
            applied_revision: None,
            state: ProgressMigrationState::Applied,
            reverted_at: None,
        };

        let mut subject = resolution.subject.clone();
        set_authoritative_media_item(&mut subject, media_item_id)?;
        let result = self.unit_of_work.run_checked_comic_progress_subject_write(
            &ComicProgressSubjectWritePlan {
                subject,
                members: resolution.subject.members().to_vec(),
                page_identity_write: None,
                progress_writes: vec![ComicProgressWriteCandidate {
                    progress: projected.clone(),
                    expected_revision: None,
                }],
                migration_snapshot: Some(snapshot),
                refresh_receipt: None,
            },
            &ComicProgressSubjectWritePrecondition::ExactSnapshot {
                subject: Box::new(resolution.subject.clone()),
                members: resolution.subject.members().to_vec(),
                require_authoritative_progress_none: false,
                require_progress_absent_for_media_item: Some(target_item.id),
                require_progress_revision_for_media_item: None,
            },
        )?;
        let applied_revision = result
            .applied_progress_revisions
            .first()
            .cloned()
            .ok_or_else(|| invalid_subject_write("物化漫画 Progress 未返回 revision"))?;
        projected.revision = Some(applied_revision.clone());
        let receipt = ComicProgressMigrationReceipt {
            migration_id,
            source_media_item_id: authoritative.media_item_id,
            target_media_item_id: target_item.id,
            strategy: page_migration.strategy,
            confidence: page_migration.confidence,
            evidence: resolution.member.evidence,
            source_progress_snapshot: progress_snapshot_view(Some(&authoritative)),
            target_progress_before: None,
            target_progress_after: progress_snapshot_view(Some(&projected)),
            page_mapping: page_migration,
            algorithm_version: COMIC_PROGRESS_MIGRATION_ALGORITHM_VERSION.to_owned(),
            created_at,
            undoable: true,
            applied_revision: Some(applied_revision),
        };
        Ok(ComicProgressSubjectProgress {
            progress: Some(projected),
            migration_receipt: Some(receipt),
        })
    }

    /// 原子同步一个 MediaItem 的页面身份，并把当前 Subject 视角的 Progress
    /// 一起迁移到新页面序列。
    ///
    /// `old_pages` 与 `expected_page_revision` 必须来自同一次页面身份快照；调用
    /// 方不能在这里重新读取页面后再用旧序列计算。目标已有 Progress 时，目标行
    /// 是当前视角并优先被重定位；目标没有 Progress 时，Subject 的权威行会被投影
    /// 到目标 MediaItem。页面身份、Subject、Progress 和迁移快照全部交给同一个
    /// 受检 Immediate UoW，任何一步失败都不会留下部分状态。
    pub async fn synchronize_page_identities_for_media_item(
        &self,
        media_item_id: MediaItemId,
        old_pages: Vec<PageIdentity>,
        new_pages: Vec<PageIdentity>,
        expected_page_revision: Option<String>,
        expected_progress_revision: Option<String>,
    ) -> Result<ComicProgressMigrationResult, AppError> {
        let (target_item, target_edition) = self.load_comic_context(media_item_id).await?;
        let target_progress =
            ProgressRepository::get_for_media_item(&*self.ports, media_item_id).await?;
        let expected_progress_revision = expected_progress_revision
            .map(|value| validate_expected_revision(&value))
            .transpose()?;

        let (mut subject, members, precondition, target_member) = if let Some((subject, member)) =
            self.read_subject_for_media_item(media_item_id).await?
        {
            let members = subject.members().to_vec();
            let precondition = ComicProgressSubjectWritePrecondition::ExactSnapshot {
                subject: Box::new(subject.clone()),
                members: members.clone(),
                require_authoritative_progress_none: false,
                require_progress_absent_for_media_item: None,
                require_progress_revision_for_media_item: None,
            };
            (subject, members, precondition, member)
        } else {
            let now = UtcMillis::now();
            let mut subject = ComicProgressSubject::new(
                target_edition.work_id,
                target_edition.id,
                media_item_id,
                now,
            );
            subject.authoritative_progress_media_item_id = target_progress
                .as_ref()
                .map(|progress| progress.media_item_id);
            let mut member = ComicProgressSubjectMember::active(
                subject.id,
                media_item_id,
                vec![ChapterEvidence::SameRemoteIdentity],
            );
            member.relationship =
                haven_domain::comic_progress_subject::ComicProgressSubjectRelationship::Canonical;
            let members = vec![member.clone()];
            subject
                .load_members(members.clone())
                .map_err(|error| invalid_subject(error.to_string()))?;
            let precondition =
                ComicProgressSubjectWritePrecondition::AbsentActiveMember { media_item_id };
            (subject, members, precondition, member)
        };

        // A persisted pointer is authoritative, but a target-owned row is the current
        // reader view. `authoritative_progress` still validates a dangling pointer
        // before the target preference can take effect.
        let authoritative_progress = self.authoritative_progress(&subject).await?;
        let source_progress = target_progress.clone().or(authoritative_progress);
        let Some(source_progress) = source_progress else {
            let page_migration = no_target_page_migration();
            self.commit_page_identity_only(
                subject,
                members,
                precondition,
                media_item_id,
                new_pages,
                expected_page_revision,
            )
            .await?;
            let receipt = migration_receipt(ComicProgressReceiptInput {
                migration_id: ComicProgressMigrationId::new(),
                source_media_item_id: media_item_id,
                target_media_item_id: media_item_id,
                strategy: page_migration.strategy,
                confidence: page_migration.confidence,
                evidence: target_member.evidence.clone(),
                source_progress_snapshot: None,
                target_progress_before: None,
                target_progress_after: None,
                page_mapping: page_migration.clone(),
                created_at: UtcMillis::now(),
                undoable: false,
                applied_revision: None,
            });
            return Ok(migration_result_with_receipt(
                ComicProgressMigrationStatus::NoSourceProgress,
                None,
                page_migration,
                None,
                None,
                receipt,
            ));
        };

        let source_item = MediaItemRepository::get(&*self.ports, source_progress.media_item_id)
            .await?
            .ok_or_else(media_item_not_found)?;
        let source_edition = EditionRepository::get(&*self.ports, source_item.edition_id)
            .await?
            .ok_or_else(edition_not_found)?;
        ensure_comic_progress(&source_progress, &source_item, &source_edition)?;

        let source_pages = if source_progress.media_item_id == media_item_id {
            materialize_page_identities(old_pages, source_item.page_count)
        } else {
            materialize_page_identities(
                ComicPageIdentityRepository::list(&*self.ports, source_item.id).await?,
                source_item.page_count,
            )
        };
        // An explicitly empty Provider observation means that there is no readable
        // target page yet.  Only a missing historical identity table may use the
        // MediaItem page count as a proportional-fallback input; synthesizing pages
        // for this fresh empty observation would create a false target Progress.
        let target_pages = new_pages.clone();
        let page_migration = migrate_page_index(
            &source_pages,
            &target_pages,
            comic_page_index(&source_progress)?,
        );
        let source_snapshot = progress_snapshot_view(Some(&source_progress));
        let target_before = target_progress
            .as_ref()
            .and_then(|progress| progress_snapshot_view(Some(progress)));

        let Some(target_page_index) = page_migration.target_page_index else {
            // There is no valid target page to bind yet. Preserve an existing
            // cross-media authority; a target-owned Progress remains its own view.
            if target_progress.is_some() {
                set_authoritative_media_item(&mut subject, media_item_id)?;
            }
            self.commit_page_identity_only(
                subject,
                members,
                precondition,
                media_item_id,
                new_pages,
                expected_page_revision,
            )
            .await?;
            let receipt = migration_receipt(ComicProgressReceiptInput {
                migration_id: ComicProgressMigrationId::new(),
                source_media_item_id: source_progress.media_item_id,
                target_media_item_id: media_item_id,
                strategy: page_migration.strategy,
                confidence: page_migration.confidence,
                evidence: target_member.evidence.clone(),
                source_progress_snapshot: source_snapshot,
                target_progress_before: target_before,
                target_progress_after: None,
                page_mapping: page_migration.clone(),
                created_at: UtcMillis::now(),
                undoable: false,
                applied_revision: None,
            });
            return Ok(migration_result_with_receipt(
                ComicProgressMigrationStatus::NoTargetPage,
                None,
                page_migration,
                None,
                None,
                receipt,
            ));
        };

        let target_page_count = u32::try_from(target_pages.len()).map_err(|_| {
            AppError::new(
                "INVALID_COMIC_PAGE_COUNT",
                ErrorKind::Validation,
                "漫画页面数量超出迁移范围",
                false,
            )
        })?;
        let mut projected = translated_progress(
            &source_progress,
            &target_item,
            target_edition.work_id,
            target_page_index,
            target_page_count,
        )?;
        let source_revision = progress_revision(&source_progress)?.to_owned();
        if let Some(expected) = expected_progress_revision.as_deref() {
            if expected != source_revision {
                return Err(revision_conflict());
            }
        }

        // A successful projection is the point at which the target becomes the
        // Subject's authoritative view. The precondition still contains the old
        // Subject snapshot, so pointer and progress are CAS-protected together.
        set_authoritative_media_item(&mut subject, media_item_id)?;
        let mut progress_precondition = precondition.clone();
        if target_progress.is_none() {
            if let ComicProgressSubjectWritePrecondition::ExactSnapshot {
                require_progress_absent_for_media_item,
                ..
            } = &mut progress_precondition
            {
                // The page identity write and target Progress insert share one
                // checked transaction. A concurrent target insert must be rejected
                // before page identities, Subject members, or the migration
                // snapshot are touched.
                *require_progress_absent_for_media_item = Some(media_item_id);
            }
        }
        let target_revision_before =
            (source_progress.media_item_id == media_item_id).then(|| source_revision.clone());
        let migration_id = ComicProgressMigrationId::new();
        let created_at = UtcMillis::now();
        let snapshot = ComicProgressMigrationSnapshot {
            id: migration_id,
            source_media_item_id: source_progress.media_item_id,
            target_media_item_id: media_item_id,
            source_revision: source_revision.clone(),
            target_revision_before: target_revision_before.clone(),
            old_progress: source_progress.clone(),
            old_target_progress: if source_progress.media_item_id == media_item_id {
                Some(source_progress.clone())
            } else {
                None
            },
            new_progress: projected.clone(),
            mode: ProgressMigrationMode::OneTime,
            confidence: page_migration.confidence,
            strategy: page_migration.strategy,
            evidence: target_member.evidence.clone(),
            created_at,
            applied_revision: None,
            state: ProgressMigrationState::Applied,
            reverted_at: None,
        };
        let candidate_expected_revision = target_revision_before.clone();
        let result = self.unit_of_work.run_checked_comic_progress_subject_write(
            &ComicProgressSubjectWritePlan {
                subject,
                members: members.clone(),
                page_identity_write: Some(
                    crate::services::ports::ComicPageIdentityWriteCandidate {
                        media_item_id,
                        pages: new_pages,
                        expected_revision: expected_page_revision,
                    },
                ),
                progress_writes: vec![ComicProgressWriteCandidate {
                    progress: projected.clone(),
                    expected_revision: candidate_expected_revision,
                }],
                migration_snapshot: Some(snapshot),
                refresh_receipt: None,
            },
            &progress_precondition,
        )?;
        let applied_revision = result
            .applied_progress_revisions
            .first()
            .cloned()
            .ok_or_else(|| invalid_subject_write("页面身份迁移未返回 Progress revision"))?;
        projected.revision = Some(applied_revision.clone());
        let receipt = migration_receipt(ComicProgressReceiptInput {
            migration_id,
            source_media_item_id: source_progress.media_item_id,
            target_media_item_id: media_item_id,
            strategy: page_migration.strategy,
            confidence: page_migration.confidence,
            evidence: target_member.evidence,
            source_progress_snapshot: source_snapshot,
            target_progress_before: target_before,
            target_progress_after: progress_snapshot_view(Some(&projected)),
            page_mapping: page_migration.clone(),
            created_at,
            undoable: true,
            applied_revision: Some(applied_revision.clone()),
        });
        Ok(migration_result_with_receipt(
            ComicProgressMigrationStatus::Applied,
            None,
            page_migration,
            Some(migration_id),
            Some(applied_revision),
            receipt,
        ))
    }

    async fn commit_page_identity_only(
        &self,
        subject: ComicProgressSubject,
        members: Vec<ComicProgressSubjectMember>,
        precondition: ComicProgressSubjectWritePrecondition,
        media_item_id: MediaItemId,
        pages: Vec<PageIdentity>,
        expected_page_revision: Option<String>,
    ) -> Result<(), AppError> {
        self.unit_of_work.run_checked_comic_progress_subject_write(
            &ComicProgressSubjectWritePlan {
                subject,
                members,
                page_identity_write: Some(
                    crate::services::ports::ComicPageIdentityWriteCandidate {
                        media_item_id,
                        pages,
                        expected_revision: expected_page_revision,
                    },
                ),
                progress_writes: Vec::new(),
                migration_snapshot: None,
                refresh_receipt: None,
            },
            &precondition,
        )?;
        Ok(())
    }

    /// 在 Subject、目标 Progress 和 pointer 同一个 Immediate 事务中保存漫画进度。
    pub async fn save_progress_for_media_item(
        &self,
        progress: Progress,
        expected_revision: Option<&str>,
    ) -> Result<String, AppError> {
        let target_media_item_id = progress.media_item_id;
        let (media_item, edition) = self.load_comic_context(target_media_item_id).await?;
        ensure_comic_progress(&progress, &media_item, &edition)?;
        let expected_revision = expected_revision
            .map(validate_expected_revision)
            .transpose()?;
        let resolution = self.ensure_for_media_item(target_media_item_id).await?;
        let target_progress =
            ProgressRepository::get_for_media_item(&*self.ports, target_media_item_id).await?;
        let target_was_absent = target_progress.is_none();

        // A read-only cross-MediaItem projection carries the source row's opaque
        // revision in the wire field. It is not a revision of the absent target
        // row, so convert it into a transaction-level source precondition instead
        // of passing it to the target UPDATE predicate.
        let (candidate_expected_revision, require_source_revision) = if target_was_absent {
            match expected_revision.as_deref() {
                None => (None, None),
                Some(expected_revision) => {
                    let Some(source) = resolution
                        .authoritative_progress
                        .as_ref()
                        .filter(|source| source.media_item_id != target_media_item_id)
                    else {
                        return Err(comic_progress_revision_conflict());
                    };
                    let source_revision = progress_revision(source)?.to_owned();
                    if source_revision != expected_revision {
                        return Err(comic_progress_revision_conflict());
                    }
                    (None, Some((source.media_item_id, source_revision)))
                }
            }
        } else {
            (expected_revision, None)
        };
        let mut subject = resolution.subject.clone();
        set_authoritative_media_item(&mut subject, target_media_item_id)?;
        let result = self.unit_of_work.run_checked_comic_progress_subject_write(
            &ComicProgressSubjectWritePlan {
                subject,
                members: resolution.subject.members().to_vec(),
                page_identity_write: None,
                progress_writes: vec![ComicProgressWriteCandidate {
                    progress,
                    expected_revision: candidate_expected_revision,
                }],
                migration_snapshot: None,
                refresh_receipt: None,
            },
            &ComicProgressSubjectWritePrecondition::ExactSnapshot {
                subject: Box::new(resolution.subject.clone()),
                members: resolution.subject.members().to_vec(),
                require_authoritative_progress_none: false,
                require_progress_absent_for_media_item: target_was_absent
                    .then_some(target_media_item_id),
                require_progress_revision_for_media_item: require_source_revision,
            },
        )?;
        result
            .applied_progress_revisions
            .into_iter()
            .next()
            .ok_or_else(|| invalid_subject_write("漫画 Progress 未返回 revision"))
    }

    async fn load_comic_context(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<(MediaItem, Edition), AppError> {
        let item = MediaItemRepository::get(&*self.ports, media_item_id)
            .await?
            .ok_or_else(media_item_not_found)?;
        if item.media_type != MediaType::Comic {
            return Err(AppError::new(
                "COMIC_PROGRESS_SUBJECT_MEDIA_TYPE",
                ErrorKind::Validation,
                "漫画进度主体仅适用于 Comic MediaItem",
                false,
            ));
        }
        let edition = EditionRepository::get(&*self.ports, item.edition_id)
            .await?
            .ok_or_else(edition_not_found)?;
        Ok((item, edition))
    }

    async fn read_subject_for_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Option<(ComicProgressSubject, ComicProgressSubjectMember)>, AppError> {
        let Some(member) =
            ComicProgressSubjectRepository::get_for_media_item(&*self.ports, media_item_id).await?
        else {
            return Ok(None);
        };
        if member.state
            != haven_domain::comic_progress_subject::ComicProgressSubjectMemberState::Active
        {
            return Ok(None);
        }
        let mut subject = ComicProgressSubjectRepository::get(&*self.ports, member.subject_id)
            .await?
            .ok_or_else(subject_not_found)?;
        let members =
            ComicProgressSubjectRepository::list_members(&*self.ports, member.subject_id).await?;
        subject
            .load_members(members)
            .map_err(|error| invalid_subject(error.to_string()))?;
        Ok(Some((subject, member)))
    }

    async fn authoritative_progress(
        &self,
        subject: &ComicProgressSubject,
    ) -> Result<Option<Progress>, AppError> {
        if let Some(media_item_id) = subject.authoritative_progress_media_item_id {
            return ProgressRepository::get_for_media_item(&*self.ports, media_item_id)
                .await?
                .map(Some)
                .ok_or_else(authoritative_progress_missing);
        }
        self.select_authoritative_progress(subject).await
    }

    async fn project_progress_for_media_item(
        &self,
        source: &Progress,
        target_item: &MediaItem,
        target_edition: &Edition,
    ) -> Result<Option<Progress>, AppError> {
        Ok(self
            .project_progress_with_mapping(source, target_item, target_edition)
            .await?
            .map(|(_, progress)| progress))
    }

    async fn project_progress_with_mapping(
        &self,
        source: &Progress,
        target_item: &MediaItem,
        target_edition: &Edition,
    ) -> Result<Option<(haven_domain::comic_identity::PageMigration, Progress)>, AppError> {
        let source_item = MediaItemRepository::get(&*self.ports, source.media_item_id)
            .await?
            .ok_or_else(media_item_not_found)?;
        let source_edition = EditionRepository::get(&*self.ports, source_item.edition_id)
            .await?
            .ok_or_else(edition_not_found)?;
        ensure_comic_progress(source, &source_item, &source_edition)?;
        ensure_comic_progress_target(target_item, target_edition)?;
        let source_pages = materialize_page_identities(
            ComicPageIdentityRepository::list(&*self.ports, source_item.id).await?,
            source_item.page_count,
        );
        let target_pages = materialize_page_identities(
            ComicPageIdentityRepository::list(&*self.ports, target_item.id).await?,
            target_item.page_count,
        );
        let old_page_index = comic_page_index(source)?;
        let page_migration = migrate_page_index(&source_pages, &target_pages, old_page_index);
        let Some(target_page_index) = page_migration.target_page_index else {
            return Ok(None);
        };
        let target_page_count = u32::try_from(target_pages.len()).map_err(|_| {
            AppError::new(
                "INVALID_COMIC_PAGE_COUNT",
                ErrorKind::Validation,
                "漫画页面数量超出迁移范围",
                false,
            )
        })?;
        // This is a read-only view, not a persisted target row. The wire contract
        // requires a non-empty opaque revision, so expose the authoritative source
        // revision as the view's freshness token. `save_progress_for_media_item`
        // never sends this token as the target row's expected revision; it turns it
        // into a source-row precondition and inserts the target row with
        // `expected_revision=None`.
        let source_revision = progress_revision(source)?.to_owned();
        let percentage = (target_page_count > 0).then(|| {
            ((target_page_index.saturating_add(1)) as f32 / target_page_count as f32).min(1.0)
        });
        Ok(Some((
            page_migration,
            Progress {
                id: ProgressId::new(),
                work_id: target_edition.work_id,
                edition_id: target_item.edition_id,
                media_item_id: target_item.id,
                locator: Locator::Comic(ComicLocator {
                    chapter_item_id: target_item.id,
                    page_index: target_page_index,
                    page_progression: comic_progression(source),
                }),
                completion: source.completion,
                percentage,
                last_active_at: source.last_active_at,
                updated_at: source.updated_at,
                revision: Some(source_revision),
                keyframe_uri: None,
            },
        )))
    }

    async fn write_subject_pointer(
        &self,
        resolution: &ComicProgressSubjectResolution,
        subject: ComicProgressSubject,
    ) -> Result<(), AppError> {
        self.unit_of_work.run_checked_comic_progress_subject_write(
            &ComicProgressSubjectWritePlan {
                subject,
                members: resolution.subject.members().to_vec(),
                page_identity_write: None,
                progress_writes: Vec::new(),
                migration_snapshot: None,
                refresh_receipt: None,
            },
            &ComicProgressSubjectWritePrecondition::ExactSnapshot {
                subject: Box::new(resolution.subject.clone()),
                members: resolution.subject.members().to_vec(),
                require_authoritative_progress_none: false,
                require_progress_absent_for_media_item: None,
                require_progress_revision_for_media_item: None,
            },
        )?;
        Ok(())
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
                        subject: Box::new(expected_subject),
                        members: members.clone(),
                        require_authoritative_progress_none: true,
                        require_progress_absent_for_media_item: None,
                        require_progress_revision_for_media_item: None,
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

fn can_activate_reconciliation(match_result: &ChapterMatch) -> bool {
    match_result.kind != ChapterMatchKind::Unrelated
        && match_result.progress_migration == ProgressMigrationMode::Shared
        && match_result.confidence == MatchConfidence::High
}

fn canonical_member(
    subject_id: ComicProgressSubjectId,
    media_item_id: MediaItemId,
) -> ComicProgressSubjectMember {
    let mut member = ComicProgressSubjectMember::active(
        subject_id,
        media_item_id,
        vec![ChapterEvidence::SameRemoteIdentity],
    );
    member.relationship = ComicProgressSubjectRelationship::Canonical;
    member
}

fn reconciliation_member(
    subject_id: ComicProgressSubjectId,
    media_item_id: MediaItemId,
    relationship: ComicProgressSubjectRelationship,
    evidence: &[ChapterEvidence],
) -> ComicProgressSubjectMember {
    if relationship == ComicProgressSubjectRelationship::Candidate {
        let mut member =
            ComicProgressSubjectMember::candidate(subject_id, media_item_id, evidence.to_vec());
        member.confidence = member_confidence(evidence);
        member
    } else {
        let mut member =
            ComicProgressSubjectMember::active(subject_id, media_item_id, evidence.to_vec());
        member.relationship = relationship;
        member
    }
}

fn member_confidence(evidence: &[ChapterEvidence]) -> MatchConfidence {
    if evidence.iter().any(|item| {
        matches!(
            item,
            ChapterEvidence::SameRemoteIdentity
                | ChapterEvidence::AuthoritativeContentKey
                | ChapterEvidence::ExactPageIdentity { matched: 1.. }
        )
    }) {
        MatchConfidence::High
    } else if evidence
        .iter()
        .any(|item| matches!(item, ChapterEvidence::PartialPageIdentity { matched: 1.. }))
    {
        MatchConfidence::Medium
    } else {
        MatchConfidence::Low
    }
}

fn merge_members_for_survivor(
    survivor_id: ComicProgressSubjectId,
    left: &[ComicProgressSubjectMember],
    right: &[ComicProgressSubjectMember],
) -> Vec<ComicProgressSubjectMember> {
    let mut merged = Vec::new();
    for member in left.iter().chain(right.iter()) {
        let mut member = member.clone();
        member.subject_id = survivor_id;
        if let Some(existing) = merged
            .iter()
            .find(|existing: &&ComicProgressSubjectMember| {
                existing.media_item_id == member.media_item_id && existing.state == member.state
            })
        {
            // Keep one deterministic row for each (media_item, state). Active
            // rows are refreshed by the reconciliation evidence below; candidate
            // and retired rows retain the first historical observation.
            let _ = existing;
            continue;
        }
        merged.push(member);
    }
    merged
}

fn upsert_member(
    members: &mut Vec<ComicProgressSubjectMember>,
    mut replacement: ComicProgressSubjectMember,
) {
    members.retain(|existing| {
        !(existing.media_item_id == replacement.media_item_id
            && existing.state == ComicProgressSubjectMemberState::Candidate
            && replacement.state == ComicProgressSubjectMemberState::Active)
    });
    if let Some(existing) = members.iter_mut().find(|existing| {
        existing.media_item_id == replacement.media_item_id && existing.state == replacement.state
    }) {
        replacement.subject_id = existing.subject_id;
        *existing = replacement;
    } else {
        members.push(replacement);
    }
}

fn dedup_media_item_ids<const N: usize>(ids: [MediaItemId; N]) -> Vec<MediaItemId> {
    let mut unique = Vec::with_capacity(N);
    for id in ids {
        if !unique.contains(&id) {
            unique.push(id);
        }
    }
    unique
}

fn ensure_comic_edition(edition: &Edition) -> Result<(), AppError> {
    if edition.edition_type != MediaType::Comic {
        return Err(AppError::new(
            "COMIC_EDITION_REQUIRED",
            ErrorKind::Validation,
            "漫画进度主体只能绑定漫画 Edition",
            false,
        ));
    }
    Ok(())
}

fn chapter_source_ref_not_found(message: &'static str) -> AppError {
    AppError::new(
        "COMIC_SOURCE_REF_NOT_FOUND",
        ErrorKind::NotFound,
        message,
        false,
    )
}

fn comic_work_mismatch() -> AppError {
    AppError::new(
        "COMIC_WORK_MISMATCH",
        ErrorKind::Validation,
        "漫画来源章节不属于同一个 Work，拒绝建立连续性",
        false,
    )
}

fn edition_conflict() -> AppError {
    AppError::new(
        "EDITION_CONFLICT",
        ErrorKind::Conflict,
        "漫画章节属于不同 Edition 或已知版本画像冲突",
        false,
    )
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

fn comic_progress_revision_conflict() -> AppError {
    AppError::new(
        "COMIC_PROGRESS_REVISION_CONFLICT",
        ErrorKind::Conflict,
        "漫画 Progress revision 已被其他会话更新，请刷新后重试",
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

fn ensure_comic_progress(
    progress: &Progress,
    item: &MediaItem,
    edition: &Edition,
) -> Result<(), AppError> {
    if progress.work_id != edition.work_id
        || progress.edition_id != item.edition_id
        || progress.media_item_id != item.id
    {
        return Err(AppError::new(
            "COMIC_PROGRESS_REQUIRED",
            ErrorKind::Validation,
            "进度的 Work、Edition 或媒体条目不一致",
            false,
        ));
    }
    match &progress.locator {
        Locator::Comic(locator) if locator.chapter_item_id == progress.media_item_id => Ok(()),
        _ => Err(AppError::new(
            "COMIC_PROGRESS_REQUIRED",
            ErrorKind::Validation,
            "进度不是与媒体条目一致的 Comic Locator",
            false,
        )),
    }
}

fn ensure_comic_progress_target(item: &MediaItem, edition: &Edition) -> Result<(), AppError> {
    if item.media_type != MediaType::Comic || item.edition_id != edition.id {
        return Err(AppError::new(
            "COMIC_PROGRESS_REQUIRED",
            ErrorKind::Validation,
            "目标必须是属于漫画 Edition 的 MediaItem",
            false,
        ));
    }
    if edition.edition_type != MediaType::Comic {
        return Err(AppError::new(
            "COMIC_PROGRESS_REQUIRED",
            ErrorKind::Validation,
            "目标 Edition 不是漫画版本",
            false,
        ));
    }
    Ok(())
}

fn comic_page_index(progress: &Progress) -> Result<u32, AppError> {
    match &progress.locator {
        Locator::Comic(locator) if locator.chapter_item_id == progress.media_item_id => {
            Ok(locator.page_index)
        }
        _ => Err(AppError::new(
            "COMIC_PROGRESS_REQUIRED",
            ErrorKind::Validation,
            "进度不是与媒体条目一致的 Comic Locator",
            false,
        )),
    }
}

fn comic_progression(progress: &Progress) -> Option<f32> {
    match &progress.locator {
        Locator::Comic(locator) => locator.page_progression,
        _ => None,
    }
}

fn progress_revision(progress: &Progress) -> Result<&str, AppError> {
    progress
        .revision
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::new(
                "PROGRESS_REVISION_MISSING",
                ErrorKind::Database,
                "Progress 缺少持久化 revision",
                false,
            )
        })
}

fn set_authoritative_media_item(
    subject: &mut ComicProgressSubject,
    media_item_id: MediaItemId,
) -> Result<(), AppError> {
    set_authoritative_pointer(subject, Some(media_item_id))
}

fn set_authoritative_pointer(
    subject: &mut ComicProgressSubject,
    pointer: Option<MediaItemId>,
) -> Result<(), AppError> {
    if subject.authoritative_progress_media_item_id == pointer {
        return Ok(());
    }
    let next = subject
        .updated_at
        .0
        .checked_add(1)
        .ok_or_else(|| invalid_subject_write("漫画进度主体 updated_at 已溢出"))?;
    subject.authoritative_progress_media_item_id = pointer;
    subject.updated_at = UtcMillis(UtcMillis::now().0.max(next));
    Ok(())
}

const MAX_SYNTHETIC_PAGE_IDENTITIES: u32 = 5_000;

fn materialize_page_identities(
    pages: Vec<PageIdentity>,
    page_count: Option<u32>,
) -> Vec<PageIdentity> {
    if !pages.is_empty() {
        return pages;
    }
    let Some(page_count) =
        page_count.filter(|count| *count > 0 && *count <= MAX_SYNTHETIC_PAGE_IDENTITIES)
    else {
        return pages;
    };
    vec![PageIdentity::default(); page_count as usize]
}

fn validate_expected_revision(value: &str) -> Result<String, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > 256
        || trimmed
            .chars()
            .any(|character| (character as u32) <= 0x1f || character == '\u{7f}')
        || trimmed.contains("://")
        || trimmed.to_ascii_lowercase().starts_with("data:")
    {
        return Err(AppError::new(
            "INVALID_ARGUMENT",
            ErrorKind::Validation,
            "漫画进度字段 expectedRevision 非法",
            false,
        ));
    }
    Ok(trimmed.to_owned())
}

fn invalid_subject_write(message: impl Into<String>) -> AppError {
    AppError::new(
        "COMIC_PROGRESS_SUBJECT_PLAN_INVALID",
        ErrorKind::Validation,
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
