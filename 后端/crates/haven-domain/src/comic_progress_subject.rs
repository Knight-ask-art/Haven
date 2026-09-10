use std::collections::HashSet;
use std::fmt;

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
    /// 成员关系是可恢复的聚合数据；SQLite 仍可把它们作为独立关系行持久化，
    /// 加载后通过 `load_members` 重新执行同一套 Domain 校验。
    #[serde(default)]
    members: Vec<ComicProgressSubjectMember>,
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
    InvalidRedirectState,
    UpdatedAtOverflow,
    RedirectedSubjectCannotAcceptMember,
    MemberBelongsToAnotherSubject,
    InvalidMemberRelationship,
    InvalidMemberConfidence,
    InvalidMemberEvidence,
    InvalidAuthoritativeProgressMember,
}

impl fmt::Display for ComicProgressSubjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::DuplicateActiveMember => "duplicate active comic progress subject member",
            Self::RedirectCycle => "comic progress subject redirect cycle",
            Self::InvalidRedirectState => {
                "comic progress subject state and redirect target are inconsistent"
            }
            Self::UpdatedAtOverflow => "comic progress subject updated_at overflow",
            Self::RedirectedSubjectCannotAcceptMember => {
                "redirected comic progress subject cannot accept a member"
            }
            Self::MemberBelongsToAnotherSubject => {
                "comic progress subject member belongs to another subject"
            }
            Self::InvalidMemberRelationship => "invalid comic progress subject member relationship",
            Self::InvalidMemberConfidence => "invalid comic progress subject member confidence",
            Self::InvalidMemberEvidence => "invalid comic progress subject member evidence",
            Self::InvalidAuthoritativeProgressMember => {
                "authoritative progress media item is not a valid subject progress mapping"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ComicProgressSubjectError {}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ComicProgressSubjectData {
    id: ComicProgressSubjectId,
    work_id: WorkId,
    edition_id: EditionId,
    canonical_media_item_id: MediaItemId,
    authoritative_progress_media_item_id: Option<MediaItemId>,
    state: ComicProgressSubjectState,
    redirect_subject_id: Option<ComicProgressSubjectId>,
    algorithm_version: String,
    created_at: UtcMillis,
    updated_at: UtcMillis,
    #[serde(default)]
    members: Vec<ComicProgressSubjectMember>,
}

impl<'de> Deserialize<'de> for ComicProgressSubject {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let data = ComicProgressSubjectData::deserialize(deserializer)?;
        let mut subject = Self {
            id: data.id,
            work_id: data.work_id,
            edition_id: data.edition_id,
            canonical_media_item_id: data.canonical_media_item_id,
            authoritative_progress_media_item_id: data.authoritative_progress_media_item_id,
            state: data.state,
            redirect_subject_id: data.redirect_subject_id,
            algorithm_version: data.algorithm_version,
            created_at: data.created_at,
            updated_at: data.updated_at,
            members: Vec::new(),
        };
        subject
            .validate_redirect_state()
            .map_err(|error| serde::de::Error::custom(error.to_string()))?;
        subject
            .load_members(data.members)
            .map_err(|error| serde::de::Error::custom(error.to_string()))?;
        Ok(subject)
    }
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
            members: Vec::new(),
        }
    }

    /// 返回已加载并通过 Domain 校验的成员关系。
    pub fn members(&self) -> &[ComicProgressSubjectMember] {
        &self.members
    }

    /// 返回当前 Subject 中可以驱动共享进度的高置信度成员。
    ///
    /// Redirected Subject 即使保留历史成员，也不会暴露任何 active 成员；
    /// 成员自身还必须通过关系、置信度和证据一致性检查。
    pub fn active_progress_members(&self) -> impl Iterator<Item = &ComicProgressSubjectMember> {
        let subject_can_drive_progress =
            self.state == ComicProgressSubjectState::Active && self.redirect_subject_id.is_none();
        self.members.iter().filter(move |member| {
            subject_can_drive_progress && member.participates_in_active_progress()
        })
    }

    /// 返回当前 Subject 中可以驱动共享进度的 MediaItem ID。
    pub fn active_progress_media_items(&self) -> impl Iterator<Item = MediaItemId> + '_ {
        self.active_progress_members()
            .map(|member| member.media_item_id)
    }

    /// 校验 Subject 状态与 redirect target 是否一致。
    ///
    /// 自环始终报告 `RedirectCycle`；其他不一致状态是可诊断的恢复错误。
    pub fn validate_redirect_state(&self) -> Result<(), ComicProgressSubjectError> {
        if self.redirect_subject_id == Some(self.id) {
            return Err(ComicProgressSubjectError::RedirectCycle);
        }

        match (self.state, self.redirect_subject_id) {
            (ComicProgressSubjectState::Active, None)
            | (ComicProgressSubjectState::Redirected, Some(_)) => Ok(()),
            (ComicProgressSubjectState::Active, Some(_))
            | (ComicProgressSubjectState::Redirected, None) => {
                Err(ComicProgressSubjectError::InvalidRedirectState)
            }
        }
    }

    /// 从独立持久化的 Subject member 行恢复成员关系。
    ///
    /// 校验在替换当前成员前完成，因此失败不会留下半加载状态。调用方应把
    /// 持久化的全部成员行一次性交给本方法，而不是自行维护另一份唯一性缓存。
    pub fn load_members<I>(&mut self, members: I) -> Result<(), ComicProgressSubjectError>
    where
        I: IntoIterator<Item = ComicProgressSubjectMember>,
    {
        self.validate_redirect_state()?;
        let loaded_members: Vec<_> = members.into_iter().collect();
        self.validate_members(&loaded_members)?;

        if !self.authoritative_mapping_is_valid_for(&loaded_members) {
            return Err(ComicProgressSubjectError::InvalidAuthoritativeProgressMember);
        }

        self.members = loaded_members;
        Ok(())
    }

    pub fn attach_member(
        &mut self,
        member: ComicProgressSubjectMember,
    ) -> Result<(), ComicProgressSubjectError> {
        self.validate_redirect_state()?;
        if self.state == ComicProgressSubjectState::Redirected {
            return Err(ComicProgressSubjectError::RedirectedSubjectCannotAcceptMember);
        }

        self.validate_members(&self.members)?;

        if member.subject_id != self.id {
            return Err(ComicProgressSubjectError::MemberBelongsToAnotherSubject);
        }

        self.validate_member(&member)?;

        if member.state == ComicProgressSubjectMemberState::Active
            && self.members.iter().any(|existing| {
                existing.state == ComicProgressSubjectMemberState::Active
                    && existing.media_item_id == member.media_item_id
            })
        {
            return Err(ComicProgressSubjectError::DuplicateActiveMember);
        }

        let has_active_member = self
            .members
            .iter()
            .any(|existing| existing.state == ComicProgressSubjectMemberState::Active);
        let proposed_authoritative_progress_media_item_id = if member.state
            == ComicProgressSubjectMemberState::Active
            && !has_active_member
            && self.authoritative_progress_media_item_id == Some(self.canonical_media_item_id)
            && member.media_item_id != self.canonical_media_item_id
        {
            Some(member.media_item_id)
        } else {
            self.authoritative_progress_media_item_id
        };
        let mut proposed_members = self.members.clone();
        proposed_members.push(member);
        if !self.authoritative_mapping_is_valid_for_with_id(
            &proposed_members,
            proposed_authoritative_progress_media_item_id,
        ) {
            return Err(ComicProgressSubjectError::InvalidAuthoritativeProgressMember);
        }
        let updated_at = next_updated_at(self.updated_at)?;

        self.authoritative_progress_media_item_id = proposed_authoritative_progress_media_item_id;
        self.members = proposed_members;
        self.updated_at = updated_at;
        Ok(())
    }

    /// 校验当前 Subject 的成员关系和权威 Progress 映射。
    pub fn validate(&self) -> Result<(), ComicProgressSubjectError> {
        self.validate_redirect_state()?;
        self.validate_members(&self.members)?;
        if !self.authoritative_mapping_is_valid_for(&self.members) {
            return Err(ComicProgressSubjectError::InvalidAuthoritativeProgressMember);
        }
        Ok(())
    }

    /// 校验当前权威 Progress 映射是否指向 Subject 的 active 成员。
    ///
    /// `None` 表示当前还没有既有 Progress 行，可以通过；没有任何 active
    /// 成员时，`canonical_media_item_id` 或已退休成员的 ID 可以作为初始/历史
    /// 映射。一旦加载 active 成员，映射必须显式指向其中一条；本方法不读取或
    /// 复制 Progress 的页面、完成度、keyframe 或 revision。
    pub fn validate_authoritative_progress_media_item_id(
        &self,
    ) -> Result<(), ComicProgressSubjectError> {
        self.validate_redirect_state()?;
        self.validate_members(&self.members)?;
        if self.authoritative_mapping_is_valid_for(&self.members) {
            Ok(())
        } else {
            Err(ComicProgressSubjectError::InvalidAuthoritativeProgressMember)
        }
    }

    fn validate_member(
        &self,
        member: &ComicProgressSubjectMember,
    ) -> Result<(), ComicProgressSubjectError> {
        member.validate()
    }

    fn validate_members(
        &self,
        members: &[ComicProgressSubjectMember],
    ) -> Result<(), ComicProgressSubjectError> {
        let mut active_member_media_item_ids = HashSet::new();
        for member in members {
            if member.subject_id != self.id {
                return Err(ComicProgressSubjectError::MemberBelongsToAnotherSubject);
            }
            if self.state == ComicProgressSubjectState::Redirected
                && member.state == ComicProgressSubjectMemberState::Active
            {
                return Err(ComicProgressSubjectError::InvalidRedirectState);
            }
            self.validate_member(member)?;
            if member.state == ComicProgressSubjectMemberState::Active
                && !active_member_media_item_ids.insert(member.media_item_id)
            {
                return Err(ComicProgressSubjectError::DuplicateActiveMember);
            }
        }
        Ok(())
    }

    fn authoritative_mapping_is_valid_for(&self, members: &[ComicProgressSubjectMember]) -> bool {
        self.authoritative_mapping_is_valid_for_with_id(
            members,
            self.authoritative_progress_media_item_id,
        )
    }

    fn authoritative_mapping_is_valid_for_with_id(
        &self,
        members: &[ComicProgressSubjectMember],
        authoritative_progress_media_item_id: Option<MediaItemId>,
    ) -> bool {
        let Some(authoritative_progress_media_item_id) = authoritative_progress_media_item_id
        else {
            return true;
        };

        let active_members: Vec<MediaItemId> = members
            .iter()
            .filter(|member| member.state == ComicProgressSubjectMemberState::Active)
            .map(|member| member.media_item_id)
            .collect();
        if active_members.is_empty() {
            authoritative_progress_media_item_id == self.canonical_media_item_id
                || members.iter().any(|member| {
                    member.state == ComicProgressSubjectMemberState::Retired
                        && member.media_item_id == authoritative_progress_media_item_id
                })
        } else {
            active_members.contains(&authoritative_progress_media_item_id)
        }
    }

    /// 将本 Subject 指向另一个 Subject。
    ///
    /// 这里没有仓库上下文，因此只拒绝自环和重复 redirect；多个 Subject
    /// 之间的长链/环由 `resolve_subject_redirect` 使用 visited 集合校验。
    pub fn redirect_to(
        &mut self,
        target: ComicProgressSubjectId,
    ) -> Result<(), ComicProgressSubjectError> {
        if target == self.id {
            return Err(ComicProgressSubjectError::RedirectCycle);
        }
        self.validate_redirect_state()?;
        if self.redirect_subject_id.is_some() {
            return Err(ComicProgressSubjectError::RedirectCycle);
        }

        self.validate_members(&self.members)?;
        if !self.authoritative_mapping_is_valid_for(&self.members) {
            return Err(ComicProgressSubjectError::InvalidAuthoritativeProgressMember);
        }
        let updated_at = next_updated_at(self.updated_at)?;
        let mut retired_member_updated_at = Vec::with_capacity(self.members.len());
        for member in &self.members {
            if member.state == ComicProgressSubjectMemberState::Active {
                retired_member_updated_at.push(Some(next_updated_at(member.updated_at)?));
            } else {
                retired_member_updated_at.push(None);
            }
        }

        self.state = ComicProgressSubjectState::Redirected;
        self.redirect_subject_id = Some(target);
        self.updated_at = updated_at;
        for (member, updated_at) in self.members.iter_mut().zip(retired_member_updated_at) {
            if let Some(updated_at) = updated_at {
                member.state = ComicProgressSubjectMemberState::Retired;
                member.updated_at = updated_at;
            }
        }
        Ok(())
    }
}

fn next_updated_at(previous: UtcMillis) -> Result<UtcMillis, ComicProgressSubjectError> {
    let now = UtcMillis::now();
    let next = previous
        .0
        .checked_add(1)
        .ok_or(ComicProgressSubjectError::UpdatedAtOverflow)?;
    Ok(UtcMillis(now.0.max(next)))
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
            && self.relationship != ComicProgressSubjectRelationship::Candidate
            && self.confidence == MatchConfidence::High
            && evidence_supports_high_confidence(&self.evidence)
    }

    /// 校验公开字段构造出的成员是否与其关系、状态、置信度和证据一致。
    pub fn validate(&self) -> Result<(), ComicProgressSubjectError> {
        if self.evidence.is_empty() {
            return Err(ComicProgressSubjectError::InvalidMemberEvidence);
        }

        match self.state {
            ComicProgressSubjectMemberState::Active => {
                if self.relationship == ComicProgressSubjectRelationship::Candidate {
                    return Err(ComicProgressSubjectError::InvalidMemberRelationship);
                }
                if self.confidence != MatchConfidence::High {
                    return Err(ComicProgressSubjectError::InvalidMemberConfidence);
                }
                if !evidence_supports_high_confidence(&self.evidence) {
                    return Err(ComicProgressSubjectError::InvalidMemberEvidence);
                }
            }
            ComicProgressSubjectMemberState::Candidate => {
                if self.relationship != ComicProgressSubjectRelationship::Candidate {
                    return Err(ComicProgressSubjectError::InvalidMemberRelationship);
                }
                let evidence_confidence = evidence_confidence(&self.evidence);
                if self.confidence == MatchConfidence::High
                    || self.confidence != evidence_confidence
                {
                    return Err(ComicProgressSubjectError::InvalidMemberConfidence);
                }
            }
            ComicProgressSubjectMemberState::Retired => {
                if self.confidence != evidence_confidence(&self.evidence) {
                    return Err(ComicProgressSubjectError::InvalidMemberConfidence);
                }
            }
        }
        Ok(())
    }
}

fn evidence_confidence(evidence: &[ChapterEvidence]) -> MatchConfidence {
    if evidence.iter().any(is_high_confidence_evidence) {
        MatchConfidence::High
    } else if evidence.iter().any(is_medium_confidence_evidence) {
        MatchConfidence::Medium
    } else {
        MatchConfidence::Low
    }
}

fn evidence_supports_high_confidence(evidence: &[ChapterEvidence]) -> bool {
    evidence.iter().any(is_high_confidence_evidence)
        && !evidence.iter().any(|item| {
            matches!(
                item,
                ChapterEvidence::ConflictingAuthoritativeContentKey
                    | ChapterEvidence::EditionConflict
                    | ChapterEvidence::WeakChapterMetadata
            )
        })
}

fn is_high_confidence_evidence(evidence: &ChapterEvidence) -> bool {
    matches!(
        evidence,
        ChapterEvidence::SameRemoteIdentity
            | ChapterEvidence::AuthoritativeContentKey
            | ChapterEvidence::ExactPageIdentity { matched: 1.. }
    )
}

fn is_medium_confidence_evidence(evidence: &ChapterEvidence) -> bool {
    matches!(
        evidence,
        ChapterEvidence::PartialPageIdentity { matched: 1.. }
    )
}

/// 按 `(created_at ASC, subject_id ASC)` 选择合并 survivor。
pub fn select_subject_survivor(
    subjects: &[ComicProgressSubject],
) -> Option<ComicProgressSubjectId> {
    subjects
        .iter()
        .min_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        })
        .map(|subject| subject.id)
}

/// 解析 Subject 的 redirect 链，并用 visited 集合拒绝跨 Subject 环。
///
/// `subjects` 是调用方当前加载到的 Subject 集合；如果 redirect target 不在
/// 集合中，则把该 target 视为解析终点，目标存在性由 Application/Repository
/// 另行校验。本函数只负责链路和环检测。
pub fn resolve_subject_redirect(
    start: ComicProgressSubjectId,
    subjects: &[ComicProgressSubject],
) -> Result<ComicProgressSubjectId, ComicProgressSubjectError> {
    let mut current = start;
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(current) {
            return Err(ComicProgressSubjectError::RedirectCycle);
        }
        let Some(subject) = subjects.iter().find(|subject| subject.id == current) else {
            return Ok(current);
        };
        subject.validate_redirect_state()?;
        let Some(target) = subject.redirect_subject_id else {
            return Ok(current);
        };
        current = target;
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
        ComicProgressSubject, ComicProgressSubjectError, ComicProgressSubjectId,
        ComicProgressSubjectMember, ComicProgressSubjectMemberState, ComicProgressSubjectState,
        resolve_subject_redirect, select_subject_survivor,
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

    #[test]
    fn serialized_subject_restores_members_and_loaded_rows_keep_active_members_unique() {
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
        subject.attach_member(member.clone()).unwrap();

        let json = serde_json::to_string(&subject).unwrap();
        let mut restored: ComicProgressSubject = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.members(), &[member.clone()]);
        assert_eq!(
            restored.attach_member(member.clone()),
            Err(ComicProgressSubjectError::DuplicateActiveMember)
        );
        assert_eq!(
            restored.load_members(vec![member.clone(), member]),
            Err(ComicProgressSubjectError::DuplicateActiveMember)
        );
    }

    #[test]
    fn malformed_member_cannot_be_attached_or_drive_active_progress() {
        let mut subject = ComicProgressSubject::new(
            fixture_work_id(),
            fixture_edition_id(),
            fixture_media_item_id(),
            fixture_now(),
        );

        let mut relationship_conflict = ComicProgressSubjectMember::active(
            subject.id,
            fixture_media_item_id(),
            fixture_high_evidence(),
        );
        relationship_conflict.relationship = crate::ComicProgressSubjectRelationship::Candidate;
        assert!(subject.attach_member(relationship_conflict).is_err());

        let mut confidence_conflict = ComicProgressSubjectMember::active(
            subject.id,
            fixture_media_item_id(),
            fixture_high_evidence(),
        );
        confidence_conflict.confidence = MatchConfidence::Low;
        assert!(subject.attach_member(confidence_conflict).is_err());

        let evidence_conflict = ComicProgressSubjectMember::active(
            subject.id,
            fixture_media_item_id(),
            fixture_low_evidence(),
        );
        assert!(!evidence_conflict.participates_in_active_progress());
        assert!(subject.attach_member(evidence_conflict).is_err());

        let mut state_conflict = ComicProgressSubjectMember::candidate(
            subject.id,
            fixture_media_item_id(),
            fixture_low_evidence(),
        );
        state_conflict.state = ComicProgressSubjectMemberState::Active;
        assert!(!state_conflict.participates_in_active_progress());
        assert!(subject.attach_member(state_conflict).is_err());
    }

    #[test]
    fn redirect_updates_timestamp_and_resolver_rejects_a_two_subject_cycle() {
        let mut left = ComicProgressSubject::new(
            fixture_work_id(),
            fixture_edition_id(),
            fixture_media_item_id(),
            fixture_now(),
        );
        let mut right = ComicProgressSubject::new(
            fixture_work_id(),
            fixture_edition_id(),
            fixture_media_item_id(),
            fixture_now(),
        );
        let before_redirect = left.updated_at;

        left.redirect_to(right.id).unwrap();
        assert_eq!(left.state, ComicProgressSubjectState::Redirected);
        assert!(left.updated_at > before_redirect);
        assert_eq!(
            left.redirect_to(right.id),
            Err(ComicProgressSubjectError::RedirectCycle)
        );

        right.redirect_to(left.id).unwrap();
        let subjects = vec![left, right];
        assert_eq!(
            resolve_subject_redirect(subjects[0].id, &subjects),
            Err(ComicProgressSubjectError::RedirectCycle)
        );
    }

    #[test]
    fn survivor_selection_uses_created_at_then_subject_id_and_redirect_preserves_mapping() {
        let mut older = ComicProgressSubject::new(
            fixture_work_id(),
            fixture_edition_id(),
            fixture_media_item_id(),
            UtcMillis(1),
        );
        let first_tie = ComicProgressSubject::new(
            fixture_work_id(),
            fixture_edition_id(),
            fixture_media_item_id(),
            UtcMillis(2),
        );
        let second_tie = ComicProgressSubject::new(
            fixture_work_id(),
            fixture_edition_id(),
            fixture_media_item_id(),
            UtcMillis(2),
        );
        let newer = ComicProgressSubject::new(
            fixture_work_id(),
            fixture_edition_id(),
            fixture_media_item_id(),
            UtcMillis(3),
        );

        assert_eq!(
            select_subject_survivor(&[second_tie.clone(), first_tie.clone()]),
            Some(std::cmp::min(first_tie.id, second_tie.id))
        );
        assert_eq!(
            select_subject_survivor(&[newer, second_tie, older.clone(), first_tie,]),
            Some(older.id)
        );

        let original_authoritative = older.authoritative_progress_media_item_id;
        let original_canonical = older.canonical_media_item_id;
        older.redirect_to(fixture_subject_id()).unwrap();
        assert_eq!(
            older.authoritative_progress_media_item_id,
            original_authoritative
        );
        assert_eq!(older.canonical_media_item_id, original_canonical);
        assert_eq!(older.state, ComicProgressSubjectState::Redirected);
    }

    #[test]
    fn authoritative_progress_mapping_requires_an_active_member_or_initial_canonical() {
        let canonical_media_item_id = fixture_media_item_id();
        let mut subject = ComicProgressSubject::new(
            fixture_work_id(),
            fixture_edition_id(),
            canonical_media_item_id,
            fixture_now(),
        );
        assert!(
            subject
                .validate_authoritative_progress_media_item_id()
                .is_ok()
        );

        let canonical_member = ComicProgressSubjectMember::active(
            subject.id,
            canonical_media_item_id,
            fixture_high_evidence(),
        );
        subject.attach_member(canonical_member).unwrap();
        subject.authoritative_progress_media_item_id = Some(fixture_media_item_id());
        assert!(
            subject
                .validate_authoritative_progress_media_item_id()
                .is_err()
        );

        subject.authoritative_progress_media_item_id = Some(canonical_media_item_id);
        assert!(
            subject
                .validate_authoritative_progress_media_item_id()
                .is_ok()
        );

        let mut invalid_initial_mapping = ComicProgressSubject::new(
            fixture_work_id(),
            fixture_edition_id(),
            fixture_media_item_id(),
            fixture_now(),
        );
        invalid_initial_mapping.authoritative_progress_media_item_id =
            Some(fixture_media_item_id());
        assert!(
            invalid_initial_mapping
                .validate_authoritative_progress_media_item_id()
                .is_err()
        );
    }

    #[test]
    fn redirect_retires_active_members_and_hides_them_from_progress_queries() {
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
        let member_media_item_id = member.media_item_id;
        subject.attach_member(member).unwrap();
        let member_updated_at_before_redirect = subject.members()[0].updated_at;
        assert_eq!(subject.active_progress_members().count(), 1);
        assert_eq!(subject.active_progress_media_items().count(), 1);

        subject.redirect_to(fixture_subject_id()).unwrap();
        assert_eq!(
            subject.members()[0].state,
            ComicProgressSubjectMemberState::Retired
        );
        assert!(subject.members()[0].updated_at > member_updated_at_before_redirect);
        assert_eq!(subject.active_progress_members().count(), 0);
        assert_eq!(subject.active_progress_media_items().count(), 0);
        assert_eq!(
            subject.authoritative_progress_media_item_id,
            Some(member_media_item_id)
        );

        let json = serde_json::to_string(&subject).unwrap();
        let restored: ComicProgressSubject = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored.members()[0].state,
            ComicProgressSubjectMemberState::Retired
        );
        assert_eq!(restored.active_progress_members().count(), 0);
    }

    #[test]
    fn redirected_subject_with_active_member_is_rejected_when_loaded_or_deserialized() {
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
        subject.state = ComicProgressSubjectState::Redirected;
        subject.redirect_subject_id = Some(fixture_subject_id());

        assert_eq!(
            subject.load_members(vec![member.clone()]),
            Err(ComicProgressSubjectError::InvalidRedirectState)
        );

        let mut value = serde_json::to_value(&subject).unwrap();
        value["members"] = serde_json::json!([member]);
        assert!(serde_json::from_value::<ComicProgressSubject>(value).is_err());
    }

    #[test]
    fn subject_state_and_redirect_target_must_be_consistent_everywhere() {
        let mut active_with_target = ComicProgressSubject::new(
            fixture_work_id(),
            fixture_edition_id(),
            fixture_media_item_id(),
            fixture_now(),
        );
        active_with_target.redirect_subject_id = Some(fixture_subject_id());
        assert_eq!(
            active_with_target.validate(),
            Err(ComicProgressSubjectError::InvalidRedirectState)
        );
        assert_eq!(
            active_with_target.load_members(Vec::new()),
            Err(ComicProgressSubjectError::InvalidRedirectState)
        );
        assert_eq!(
            resolve_subject_redirect(active_with_target.id, &[active_with_target.clone()]),
            Err(ComicProgressSubjectError::InvalidRedirectState)
        );

        let mut redirected_without_target = ComicProgressSubject::new(
            fixture_work_id(),
            fixture_edition_id(),
            fixture_media_item_id(),
            fixture_now(),
        );
        redirected_without_target.state = ComicProgressSubjectState::Redirected;
        assert_eq!(
            redirected_without_target.validate(),
            Err(ComicProgressSubjectError::InvalidRedirectState)
        );
        assert_eq!(
            redirected_without_target.load_members(Vec::new()),
            Err(ComicProgressSubjectError::InvalidRedirectState)
        );
        assert_eq!(
            resolve_subject_redirect(
                redirected_without_target.id,
                &[redirected_without_target.clone()]
            ),
            Err(ComicProgressSubjectError::InvalidRedirectState)
        );

        let json = serde_json::to_value(&redirected_without_target).unwrap();
        assert!(serde_json::from_value::<ComicProgressSubject>(json).is_err());
    }

    #[test]
    fn subject_updates_are_strict_and_reject_timestamp_overflow_without_mutation() {
        let mut subject = ComicProgressSubject::new(
            fixture_work_id(),
            fixture_edition_id(),
            fixture_media_item_id(),
            fixture_now(),
        );
        subject.updated_at = UtcMillis(i64::MAX - 2);
        subject
            .attach_member(ComicProgressSubjectMember::candidate(
                subject.id,
                fixture_media_item_id(),
                fixture_low_evidence(),
            ))
            .unwrap();
        assert_eq!(subject.updated_at, UtcMillis(i64::MAX - 1));

        subject
            .attach_member(ComicProgressSubjectMember::candidate(
                subject.id,
                fixture_media_item_id(),
                fixture_low_evidence(),
            ))
            .unwrap();
        assert_eq!(subject.updated_at, UtcMillis(i64::MAX));
        let before_subject_overflow = subject.clone();
        assert_eq!(
            subject.attach_member(ComicProgressSubjectMember::candidate(
                subject.id,
                fixture_media_item_id(),
                fixture_low_evidence(),
            )),
            Err(ComicProgressSubjectError::UpdatedAtOverflow)
        );
        assert_eq!(subject, before_subject_overflow);

        let mut redirect_subject_overflow = ComicProgressSubject::new(
            fixture_work_id(),
            fixture_edition_id(),
            fixture_media_item_id(),
            fixture_now(),
        );
        redirect_subject_overflow.updated_at = UtcMillis(i64::MAX);
        let before_redirect_subject_overflow = redirect_subject_overflow.clone();
        assert_eq!(
            redirect_subject_overflow.redirect_to(fixture_subject_id()),
            Err(ComicProgressSubjectError::UpdatedAtOverflow)
        );
        assert_eq!(redirect_subject_overflow, before_redirect_subject_overflow);

        let mut redirect_member_overflow = ComicProgressSubject::new(
            fixture_work_id(),
            fixture_edition_id(),
            fixture_media_item_id(),
            fixture_now(),
        );
        redirect_member_overflow
            .attach_member(ComicProgressSubjectMember::active(
                redirect_member_overflow.id,
                fixture_media_item_id(),
                fixture_high_evidence(),
            ))
            .unwrap();
        redirect_member_overflow.members[0].updated_at = UtcMillis(i64::MAX);
        let before_redirect_member_overflow = redirect_member_overflow.clone();
        assert_eq!(
            redirect_member_overflow.redirect_to(fixture_subject_id()),
            Err(ComicProgressSubjectError::UpdatedAtOverflow)
        );
        assert_eq!(redirect_member_overflow, before_redirect_member_overflow);
    }
}
