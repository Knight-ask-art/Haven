use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use haven_common::UtcMillis;

use crate::comic_identity::{ChapterEvidence, MatchConfidence};
use crate::ids::{ComicProgressSubjectId, EditionId, MediaItemId, WorkId};

const SUBJECT_ALGORITHM_VERSION: &str = "comic-progress-subject-v1";

/// 漫画进度主体的连续性状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComicProgressSubjectState {
    Active,
    Redirected,
}

/// 漫画进度主体成员的生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComicProgressSubjectMemberState {
    Active,
    Candidate,
    Retired,
}

/// 媒体条目与漫画进度主体之间的连续性关系。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComicProgressSubjectRelationship {
    Canonical,
    Equivalent,
    Candidate,
}

/// 漫画进度主体的领域实体。
///
/// 主体只保存连续性关系和当前权威 Progress 行的媒体条目映射；页面索引、
/// 比例、完成状态、keyframe 和 revision 仍属于既有 Progress/Locator。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ComicProgressSubject {
    pub id: ComicProgressSubjectId,
    pub work_id: WorkId,
    pub edition_id: EditionId,
    pub canonical_media_item_id: MediaItemId,
    /// 指向既有 `Progress.media_item_id`，不是一份复制的进度值。
    pub authoritative_progress_media_item_id: Option<MediaItemId>,
    pub state: ComicProgressSubjectState,
    pub redirect_subject_id: Option<ComicProgressSubjectId>,
    pub algorithm_version: String,
    pub created_at: UtcMillis,
    pub updated_at: UtcMillis,
    /// Active 成员的唯一性由 Subject 聚合在内存中维护；持久化时成员仍是独立关系行。
    #[serde(skip)]
    active_member_media_item_ids: HashSet<MediaItemId>,
}

/// 漫画进度主体的一条媒体条目成员关系。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ComicProgressSubjectMember {
    pub subject_id: ComicProgressSubjectId,
    pub media_item_id: MediaItemId,
    pub relationship: ComicProgressSubjectRelationship,
    pub confidence: MatchConfidence,
    pub evidence: Vec<ChapterEvidence>,
    pub state: ComicProgressSubjectMemberState,
    pub algorithm_version: String,
    pub created_at: UtcMillis,
    pub updated_at: UtcMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComicProgressSubjectError {
    DuplicateActiveMember,
    RedirectCycle,
    RedirectedSubjectCannotAcceptMember,
}

impl ComicProgressSubject {
    pub fn new(
        work_id: WorkId,
        edition_id: EditionId,
        canonical_media_item_id: MediaItemId,
        now: UtcMillis,
    ) -> Self {
        Self {
            id: ComicProgressSubjectId::new(),
            work_id,
            edition_id,
            canonical_media_item_id,
            authoritative_progress_media_item_id: Some(canonical_media_item_id),
            state: ComicProgressSubjectState::Active,
            redirect_subject_id: None,
            algorithm_version: SUBJECT_ALGORITHM_VERSION.to_owned(),
            created_at: now,
            updated_at: now,
            active_member_media_item_ids: HashSet::new(),
        }
    }

    pub fn attach_member(
        &mut self,
        member: ComicProgressSubjectMember,
    ) -> Result<(), ComicProgressSubjectError> {
        if self.state == ComicProgressSubjectState::Redirected {
            return Err(ComicProgressSubjectError::RedirectedSubjectCannotAcceptMember);
        }

        if member.subject_id != self.id {
            return Err(ComicProgressSubjectError::DuplicateActiveMember);
        }

        if member.state == ComicProgressSubjectMemberState::Active
            && !self
                .active_member_media_item_ids
                .insert(member.media_item_id)
        {
            return Err(ComicProgressSubjectError::DuplicateActiveMember);
        }

        Ok(())
    }

    pub fn redirect_to(
        &mut self,
        target: ComicProgressSubjectId,
    ) -> Result<(), ComicProgressSubjectError> {
        if target == self.id || self.redirect_subject_id.is_some() {
            return Err(ComicProgressSubjectError::RedirectCycle);
        }

        self.state = ComicProgressSubjectState::Redirected;
        self.redirect_subject_id = Some(target);
        Ok(())
    }
}

impl ComicProgressSubjectMember {
    pub fn active(
        subject_id: ComicProgressSubjectId,
        media_item_id: MediaItemId,
        evidence: Vec<ChapterEvidence>,
    ) -> Self {
        let now = UtcMillis::now();
        Self {
            subject_id,
            media_item_id,
            relationship: ComicProgressSubjectRelationship::Equivalent,
            confidence: MatchConfidence::High,
            evidence,
            state: ComicProgressSubjectMemberState::Active,
            algorithm_version: SUBJECT_ALGORITHM_VERSION.to_owned(),
            created_at: now,
            updated_at: now,
        }
    }

    pub fn candidate(
        subject_id: ComicProgressSubjectId,
        media_item_id: MediaItemId,
        evidence: Vec<ChapterEvidence>,
    ) -> Self {
        let now = UtcMillis::now();
        Self {
            subject_id,
            media_item_id,
            relationship: ComicProgressSubjectRelationship::Candidate,
            confidence: MatchConfidence::Low,
            evidence,
            state: ComicProgressSubjectMemberState::Candidate,
            algorithm_version: SUBJECT_ALGORITHM_VERSION.to_owned(),
            created_at: now,
            updated_at: now,
        }
    }

    pub fn participates_in_active_progress(&self) -> bool {
        self.state == ComicProgressSubjectMemberState::Active
    }
}

#[cfg(test)]
mod tests {
    use crate::comic_identity::{
        ChapterEvidence, ColorMode, EditionMatchKind, EditionProfile, IdentityFacet,
        MatchConfidence, compare_edition_profiles,
    };
    use crate::ids::{EditionId, MediaItemId, WorkId};
    use crate::{
        ComicProgressSubject, ComicProgressSubjectId, ComicProgressSubjectMember,
        ComicProgressSubjectMemberState,
    };
    use haven_common::UtcMillis;

    fn fixture_work_id() -> WorkId {
        WorkId::new()
    }

    fn fixture_edition_id() -> EditionId {
        EditionId::new()
    }

    fn fixture_media_item_id() -> MediaItemId {
        MediaItemId::new()
    }

    fn fixture_subject_id() -> ComicProgressSubjectId {
        ComicProgressSubjectId::new()
    }

    fn fixture_now() -> UtcMillis {
        UtcMillis(1)
    }

    fn fixture_high_evidence() -> Vec<ChapterEvidence> {
        vec![ChapterEvidence::SameRemoteIdentity]
    }

    fn fixture_low_evidence() -> Vec<ChapterEvidence> {
        vec![ChapterEvidence::WeakChapterMetadata]
    }

    fn unknown_profile() -> EditionProfile {
        EditionProfile::default()
    }

    fn known_group(group: &str) -> EditionProfile {
        EditionProfile {
            language: IdentityFacet::known("zh-hans"),
            translation_line: IdentityFacet::unknown(),
            scan_group: crate::comic_identity::ScanGroupFacet::content_line(group),
            color_mode: ColorMode::Unknown,
        }
    }

    #[test]
    fn active_subject_member_is_unique_and_redirect_cannot_cycle() {
        let mut subject = ComicProgressSubject::new(
            fixture_work_id(),
            fixture_edition_id(),
            fixture_media_item_id(),
            fixture_now(),
        );
        let member = ComicProgressSubjectMember::active(
            subject.id,
            fixture_media_item_id(),
            fixture_high_evidence(),
        );
        assert!(subject.attach_member(member.clone()).is_ok());
        assert!(subject.attach_member(member).is_err());
        assert!(subject.redirect_to(subject.id).is_err());
    }

    #[test]
    fn unknown_is_shared_only_with_unknown_and_known_conflicts_stay_distinct() {
        assert_eq!(
            compare_edition_profiles(&unknown_profile(), &unknown_profile()).kind,
            EditionMatchKind::Same
        );
        assert_eq!(
            compare_edition_profiles(&known_group("A"), &unknown_profile()).kind,
            EditionMatchKind::Candidate
        );
        assert_eq!(
            compare_edition_profiles(&known_group("A"), &known_group("B")).kind,
            EditionMatchKind::Distinct
        );
    }

    #[test]
    fn low_confidence_member_is_candidate_and_does_not_drive_progress_read() {
        let member = ComicProgressSubjectMember::candidate(
            fixture_subject_id(),
            fixture_media_item_id(),
            fixture_low_evidence(),
        );
        assert_eq!(member.state, ComicProgressSubjectMemberState::Candidate);
        assert_eq!(member.confidence, MatchConfidence::Low);
        assert!(!member.participates_in_active_progress());
    }
}
