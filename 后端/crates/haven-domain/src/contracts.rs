//! 领域契约（Trait）。Application 只依赖这些抽象，不写 SQL。
//!
//! 规范：`plan/TECHNICAL_ARCHITECTURE.md` §26（Repository Pattern）。
//! 说明：当前仅建立最核心契约以确立模式；其余 Repository 在对应 Task 落地。
//! 异步性：Tauri 命令最终由 tokio 驱动，契约以 async 表达；SQLite 实现内部走 spawn_blocking。

use async_trait::async_trait;

use haven_common::{AppError, ErrorKind};

use crate::agent::AgentActionBinding;
use crate::comic_catalog::{ComicCatalogRefreshReceipt, ComicChapterCatalogState};
use crate::comic_identity::{
    ChapterSourceIdentity, ChapterSourceRef, ComicPageIdentitySnapshot,
    ComicProgressMigrationSnapshot, EditionProfile, PageIdentity,
};
use crate::comic_progress_subject::{ComicProgressSubject, ComicProgressSubjectMember};
use crate::entities::*;
use crate::enums::DownloadState;
use crate::ids::*;
use crate::periodical::{
    Issn, Periodical, PeriodicalArticle, PeriodicalIssue, PeriodicalPlacement, PeriodicalVolume,
};
use crate::setting_proposal::{SettingChangeReceipt, SettingProposal};
use crate::settings::PreferenceData;

/// Work 列表排序（domain 概念；wire `LibraryListSort` 由 mapper 转换）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkOrder {
    RecentlyAdded,
    Title,
    LastActive,
    ReleaseDate,
    Rating,
}

/// Work 与来源身份之间的持久化绑定。
///
/// 领域层只保留稳定的来源标识和本地 Work 归属，不携带远端 URL、认证材料
/// 或本地存储路径。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct WorkSourceRef {
    pub provider: String,
    pub external_id: String,
    pub work_id: WorkId,
}

#[async_trait]
pub trait WorkRepository {
    async fn get(&self, id: WorkId) -> Result<Option<Work>, AppError>;
    async fn save(&self, work: &Work) -> Result<(), AppError>;
    async fn list(&self, limit: u32, offset: u32) -> Result<Vec<Work>, AppError>;
    /// 按指定顺序分页（LastActive 依赖 progress 表 LEFT JOIN）。
    async fn list_sorted(
        &self,
        order: WorkOrder,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Work>, AppError>;
    /// 带筛选的分页：category（None=不过滤）、media_types（None=不过滤）、query（标题 LIKE）。
    async fn list_filtered(
        &self,
        order: WorkOrder,
        category: Option<crate::enums::ContentCategory>,
        media_types: Option<&[crate::enums::MediaType]>,
        query: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Work>, AppError>;
    /// 与 list_filtered 相同的筛选条件下的总数（分页 total）。
    async fn count_filtered(
        &self,
        category: Option<crate::enums::ContentCategory>,
        media_types: Option<&[crate::enums::MediaType]>,
        query: Option<&str>,
    ) -> Result<u64, AppError>;

    /// FTS 键集分页：按 (bm25_rank, id) 升序返回，每项附带该行 bm25 分数。
    /// `after_rank`/`after_id` 为上一页末条游标（`WHERE rank > ? OR (rank = ? AND id > ?)`），
    /// 二者必须同时提供或同时为 None；limit 为页大小。
    /// 默认实现退化为 list_filtered(offset=0)，仅保证 trait 兼容。
    async fn list_filtered_fts(
        &self,
        category: Option<crate::enums::ContentCategory>,
        media_types: Option<&[crate::enums::MediaType]>,
        query: &str,
        _after_rank: Option<f64>,
        _after_id: Option<WorkId>,
        limit: u32,
    ) -> Result<Vec<(f64, Work)>, AppError> {
        let works = self
            .list_filtered(
                WorkOrder::Title,
                category,
                media_types,
                Some(query),
                limit,
                0,
            )
            .await?;
        Ok(works.into_iter().map(|work| (0.0, work)).collect())
    }
    async fn delete(&self, id: WorkId) -> Result<bool, AppError>;
    /// 来源引用去重查找（契约 §36.1 去重键的持久化侧）。
    async fn id_for_source_ref(
        &self,
        provider: &str,
        external_id: &str,
    ) -> Result<Option<WorkId>, AppError>;
    /// 该 Work 是否已有任意来源引用（enrichment 判"新作品"用）。
    async fn has_any_source_ref(&self, id: WorkId) -> Result<bool, AppError>;
    /// 读取该 Work 的全部来源引用；不支持读取的端口必须显式返回明确错误。
    async fn list_source_refs(&self, _work_id: WorkId) -> Result<Vec<WorkSourceRef>, AppError> {
        Err(AppError::new(
            "WORK_SOURCE_REFS_UNIMPLEMENTED",
            ErrorKind::Unsupported,
            "当前存储实现不支持读取作品来源引用",
            false,
        ))
    }
    async fn save_source_ref(
        &self,
        provider: &str,
        external_id: &str,
        work_id: WorkId,
    ) -> Result<(), AppError>;
}

#[async_trait]
pub trait WorkRelationRepository: Send + Sync {
    async fn list_relations_by_work(
        &self,
        work_id: WorkId,
    ) -> Result<Vec<crate::entities::WorkRelation>, AppError>;
    async fn save_relation(&self, relation: &crate::entities::WorkRelation)
    -> Result<(), AppError>;
    async fn delete_relation(&self, id: String) -> Result<bool, AppError>;
}

#[async_trait]
pub trait EditionRepository: Send + Sync {
    async fn get(&self, id: EditionId) -> Result<Option<Edition>, AppError>;
    async fn save(&self, edition: &Edition) -> Result<(), AppError>;
    async fn list_by_work(&self, work_id: WorkId) -> Result<Vec<Edition>, AppError>;
    async fn delete(&self, id: EditionId) -> Result<bool, AppError>;

    /// 批量：按多个 Work 一次取回全部 Edition（消除 list 组装 N+1）。
    /// 默认实现逐 work 调用 list_by_work；Sqlite 实现用 IN 子句覆盖。
    async fn list_by_works(&self, work_ids: &[WorkId]) -> Result<Vec<Edition>, AppError> {
        let mut out = Vec::new();
        for id in work_ids {
            out.extend(self.list_by_work(*id).await?);
        }
        Ok(out)
    }
}

/// 漫画 Edition 画像持久化契约。
///
/// `language` 继续复用 `editions.language`；Repository 负责把画像中的
/// language 与现有字段保持一致，翻译线、扫描组和彩色模式保存在独立画像表。
#[async_trait]
pub trait EditionProfileRepository: Send + Sync {
    async fn get(&self, edition_id: EditionId) -> Result<Option<EditionProfile>, AppError>;
    async fn save(&self, edition_id: EditionId, profile: &EditionProfile) -> Result<(), AppError>;
}

/// 远端章节身份绑定契约。一个来源章节身份最多绑定一个 MediaItem，多个来源
/// 身份可以绑定同一个 MediaItem（内容证据成立时共享进度）。
#[async_trait]
pub trait ChapterSourceRepository: Send + Sync {
    async fn get(
        &self,
        identity: &ChapterSourceIdentity,
    ) -> Result<Option<ChapterSourceRef>, AppError>;
    async fn list_for_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Vec<ChapterSourceRef>, AppError>;
    async fn list_for_source_work(
        &self,
        source_key: &str,
        remote_work_id: &str,
    ) -> Result<Vec<ChapterSourceRef>, AppError>;
    async fn refresh_state(
        &self,
        source_key: &str,
        remote_work_id: &str,
    ) -> Result<Option<ComicChapterCatalogState>, AppError>;
    async fn save(&self, reference: &ChapterSourceRef) -> Result<(), AppError>;
}

/// 报刊（期刊）层级持久化契约。
///
/// 只表达 期刊 → 卷 → 期 → 文章 的显式归属。文章通过 `media_item_id` 复用既有
/// MediaItem，因此本契约不重复实现阅读、进度、资源或下载语义；期刊身份由 ISSN
/// 建立（print / electronic 独立），标题只作展示事实。
#[async_trait]
pub trait PeriodicalRepository: Send + Sync {
    async fn get(&self, id: PeriodicalId) -> Result<Option<Periodical>, AppError>;
    /// 按 Work 反查期刊（期刊本身是作品）。
    async fn find_by_work(&self, work_id: WorkId) -> Result<Option<Periodical>, AppError>;
    /// 按 ISSN 反查期刊：print 与 electronic 都参与匹配。
    async fn find_by_issn(&self, issn: &Issn) -> Result<Option<Periodical>, AppError>;
    async fn list_volumes(
        &self,
        periodical_id: PeriodicalId,
    ) -> Result<Vec<PeriodicalVolume>, AppError>;
    async fn list_issues(
        &self,
        volume_id: PeriodicalVolumeId,
    ) -> Result<Vec<PeriodicalIssue>, AppError>;
    async fn list_articles(
        &self,
        issue_id: PeriodicalIssueId,
    ) -> Result<Vec<PeriodicalArticle>, AppError>;
    /// 按来源 opaque 身份反查文章及其完整归属链（导入幂等与增量读取共用）。
    async fn find_article_by_source(
        &self,
        source_key: &str,
        remote_article_id: &str,
    ) -> Result<Option<PeriodicalPlacement>, AppError>;
    async fn save(&self, periodical: &Periodical) -> Result<(), AppError>;
    async fn save_volume(&self, volume: &PeriodicalVolume) -> Result<(), AppError>;
    async fn save_issue(&self, issue: &PeriodicalIssue) -> Result<(), AppError>;
    async fn save_article(&self, article: &PeriodicalArticle) -> Result<(), AppError>;
}

/// 漫画页面序列的稳定身份持久化契约。
///
/// 页面清单可以变化，调用方传入完整新序列后原子替换；没有稳定身份的页面
/// 允许保存空 `PageIdentity`，以便应用层使用可解释的低置信度比例兜底。
#[async_trait]
pub trait ComicPageIdentityRepository: Send + Sync {
    /// 读取页面序列及其整体观察 revision。页面序列和 revision 必须来自
    /// 同一个一致性观察，不能由调用方分别读取后自行拼接。
    async fn get_snapshot(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<ComicPageIdentitySnapshot, AppError>;

    /// 以页面观察 revision 做原子替换。
    /// - `expected_revision=None` 仅允许创建尚不存在的 state；
    /// - `Some(revision)` 必须精确匹配当前 state；
    /// - 返回 None 表示 CAS 冲突，未留下部分写入。
    async fn replace_if_revision(
        &self,
        media_item_id: MediaItemId,
        pages: &[PageIdentity],
        updated_at: haven_common::UtcMillis,
        expected_revision: Option<&str>,
    ) -> Result<Option<String>, AppError>;

    /// 兼容只需要读取页面列表的查询方；需要 CAS 的调用方必须使用快照。
    async fn list(&self, media_item_id: MediaItemId) -> Result<Vec<PageIdentity>, AppError> {
        Ok(self.get_snapshot(media_item_id).await?.pages)
    }

    /// 兼容旧的无显式 revision 调用方，但内部仍先读取快照并执行 CAS，
    /// 不再使用“无条件覆盖”语义。
    async fn replace(
        &self,
        media_item_id: MediaItemId,
        pages: &[PageIdentity],
        updated_at: haven_common::UtcMillis,
    ) -> Result<(), AppError> {
        let snapshot = self.get_snapshot(media_item_id).await?;
        self.replace_if_revision(
            media_item_id,
            pages,
            updated_at,
            snapshot.revision.as_deref(),
        )
        .await?
        .ok_or_else(|| {
            AppError::new(
                "COMIC_PAGE_IDENTITY_REVISION_CONFLICT",
                ErrorKind::Conflict,
                "漫画页面身份已被其他会话更新，请刷新后重试",
                false,
            )
        })?;
        Ok(())
    }
}

/// 漫画进度迁移的原子应用/撤销契约。
///
/// Infrastructure 必须在一个 SQLite 事务中同时校验来源/目标 revision、写入
/// Progress 和保存快照，避免“进度已改但撤销证据没有落库”的半成功状态。
#[async_trait]
pub trait ComicProgressMigrationRepository: Send + Sync {
    async fn apply(
        &self,
        snapshot: &ComicProgressMigrationSnapshot,
        expected_source_revision: &str,
        expected_target_revision: Option<&str>,
    ) -> Result<Option<String>, AppError>;
    async fn get_snapshot(
        &self,
        id: ComicProgressMigrationId,
    ) -> Result<Option<ComicProgressMigrationSnapshot>, AppError>;
    async fn revert(
        &self,
        id: ComicProgressMigrationId,
        expected_applied_revision: &str,
    ) -> Result<bool, AppError>;
}

/// 漫画目录刷新 Receipt 的只读契约。
///
/// Receipt 是 append-only 的来源观察事实。Work 级章节聚合只读取它来投影
/// 聚合根的刷新状态/覆盖度，不会因为读取而触发或伪造一次刷新；没有实现该
/// 契约的组装层保持空 receipts，聚合根回落到 `NeverSynced`。
#[async_trait]
pub trait ComicCatalogRefreshOutcomeRepository: Send + Sync {
    /// Append one refresh observation.
    ///
    /// The default keeps read-only test doubles and older assembly layers
    /// source-compatible. A refresh use case must treat the default error as a
    /// real persistence failure instead of reporting a successful refresh.
    async fn save(&self, _receipt: &ComicCatalogRefreshReceipt) -> Result<(), AppError> {
        Err(AppError::new(
            "COMIC_REFRESH_RECEIPT_UNAVAILABLE",
            ErrorKind::Unsupported,
            "当前存储实现不支持保存漫画目录刷新结果",
            false,
        ))
    }

    async fn list_by_work(
        &self,
        work_id: WorkId,
    ) -> Result<Vec<ComicCatalogRefreshReceipt>, AppError>;
}

/// 漫画进度主体与成员映射的持久化契约。
///
/// Subject 只保存跨 MediaItem 的连续性与权威 Progress 指针；实际阅读位置、
/// 完成状态、时间和 revision 继续只由 `ProgressRepository` 持有。
#[async_trait]
pub trait ComicProgressSubjectRepository: Send + Sync {
    async fn get(
        &self,
        id: ComicProgressSubjectId,
    ) -> Result<Option<ComicProgressSubject>, AppError>;
    async fn get_for_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Option<ComicProgressSubjectMember>, AppError>;
    async fn list_members(
        &self,
        subject_id: ComicProgressSubjectId,
    ) -> Result<Vec<ComicProgressSubjectMember>, AppError>;
    async fn save_subject(&self, subject: &ComicProgressSubject) -> Result<(), AppError>;
    async fn save_member(&self, member: &ComicProgressSubjectMember) -> Result<(), AppError>;
}

#[async_trait]
pub trait MediaItemRepository: Send + Sync {
    async fn get(&self, id: MediaItemId) -> Result<Option<MediaItem>, AppError>;
    async fn save(&self, item: &MediaItem) -> Result<(), AppError>;
    async fn list_by_edition(&self, edition_id: EditionId) -> Result<Vec<MediaItem>, AppError>;
    async fn delete(&self, id: MediaItemId) -> Result<bool, AppError>;

    /// 批量：按多个 Edition 一次取回全部 MediaItem（消除 list 组装 N+1）。
    /// 默认实现逐 edition 调用 list_by_edition；Sqlite 实现用 IN 子句覆盖。
    async fn list_by_editions(
        &self,
        edition_ids: &[EditionId],
    ) -> Result<Vec<MediaItem>, AppError> {
        let mut out = Vec::new();
        for id in edition_ids {
            out.extend(self.list_by_edition(*id).await?);
        }
        Ok(out)
    }
}

#[async_trait]
pub trait ResourceRepository {
    async fn get(&self, id: ResourceId) -> Result<Option<Resource>, AppError>;
    async fn save(&self, resource: &Resource) -> Result<(), AppError>;
    async fn list_by_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Vec<Resource>, AppError>;
    async fn delete(&self, id: ResourceId) -> Result<bool, AppError>;

    /// 批量标记某存储位置下的全部 Resource 为指定可用性（BE-STORAGE-001：disconnect）。
    async fn mark_unavailable_by_storage(
        &self,
        storage_location_id: StorageLocationId,
        availability: crate::enums::Availability,
    ) -> Result<u64, AppError>;

    /// 删除某存储位置下的全部 Resource 索引（BE-STORAGE-001：remove 应用内索引，
    /// 不触碰用户原始文件）。
    async fn delete_by_storage(
        &self,
        storage_location_id: StorageLocationId,
    ) -> Result<u64, AppError>;
}

#[async_trait]
pub trait ProgressRepository {
    async fn get_for_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Option<Progress>, AppError>;
    async fn save(&self, progress: &Progress) -> Result<(), AppError>;
    /// 原子条件写：`expected_revision=Some` 时只有当前版本精确匹配才写入；
    /// 返回 `None` 表示版本冲突，`Some` 为数据库最终持久化的 authoritative revision。
    /// `expected_revision=None` 表示无条件 upsert，但实现仍须保证返回 revision 单调推进。
    async fn save_if_revision(
        &self,
        progress: &Progress,
        expected_revision: Option<&str>,
    ) -> Result<Option<String>, AppError>;
    /// 原子地标记已完成。发生冲突时只修改 `completion`、`updated_at` 和
    /// `revision`，绝不把调用方过期的 Locator/percentage 回写到已有进度。
    /// `progress` 的其余字段仅在尚不存在记录时作为创建候选。
    async fn mark_completed(&self, progress: &Progress) -> Result<String, AppError>;
    /// 最近活跃的进度列表（首页 Continue 数据源）。
    async fn recent(&self, limit: u32) -> Result<Vec<Progress>, AppError>;

    /// 批量：一次取回多个 MediaItem 的进度（键为 MediaItemId，仅含存在的记录；
    /// 消除 list 组装 N+1）。默认实现逐条调用；Sqlite 实现用 IN 子句覆盖。
    async fn get_for_media_items(
        &self,
        media_item_ids: &[MediaItemId],
    ) -> Result<std::collections::HashMap<MediaItemId, Progress>, AppError> {
        let mut out = std::collections::HashMap::new();
        for id in media_item_ids {
            if let Some(p) = self.get_for_media_item(*id).await? {
                out.insert(*id, p);
            }
        }
        Ok(out)
    }
}

#[async_trait]
pub trait MarkerRepository {
    async fn list_for_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Vec<Marker>, AppError>;
    /// 列出所有未软删除标记（足迹页聚合 Query，契约 §23.1）。
    /// 按 created_at 升序；limit 由调用方钳制。
    async fn list_all(&self, limit: u32) -> Result<Vec<Marker>, AppError>;
    async fn save(&self, marker: &Marker) -> Result<(), AppError>;
    /// 软删除（同步场景需要 tombstone，见 DOMAIN_MODEL §44）。
    async fn soft_delete(&self, id: MarkerId) -> Result<bool, AppError>;
}

/// 收藏契约（DOMAIN_MODEL §26：Work / Edition / MediaItem 三选一，互斥表达）。
#[async_trait]
pub trait FavoriteRepository {
    /// 收藏指定 target（重复收藏幂等覆盖）。
    async fn set(&self, target: &FavoriteTarget) -> Result<(), AppError>;
    /// 取消收藏；返回是否实际取消（未收藏返回 `false`）。
    async fn unset(&self, target: &FavoriteTarget) -> Result<bool, AppError>;
    async fn is_favorite(&self, target: &FavoriteTarget) -> Result<bool, AppError>;
    async fn list(&self, limit: u32, offset: u32) -> Result<Vec<Favorite>, AppError>;

    /// 批量：一次取回多个 target 的收藏集合（消除 list 组装 N+1）。
    /// 默认实现逐条调用 is_favorite；Sqlite 实现用 IN 子句覆盖。
    async fn is_favorite_many(
        &self,
        targets: &[FavoriteTarget],
    ) -> Result<std::collections::HashSet<FavoriteTarget>, AppError> {
        let mut out = std::collections::HashSet::new();
        for t in targets {
            if self.is_favorite(t).await? {
                out.insert(*t);
            }
        }
        Ok(out)
    }
}

/// 历史契约（DOMAIN_MODEL §27：历史 ≠ 进度；记录"何时打开过"）。
/// 不变量：写入的 (work_id, edition_id, media_item_id) 必须构成合法层级链
/// （Repository 校验 + 002 迁移触发器兜底，见 repos/hierarchy.rs）。
/// 幂等：media_item_id 唯一（003 迁移唯一索引），save 为 upsert（并发 record 不重复）。
#[async_trait]
pub trait HistoryRepository {
    async fn get(&self, id: HistoryEntryId) -> Result<Option<HistoryEntry>, AppError>;
    async fn save(&self, entry: &HistoryEntry) -> Result<(), AppError>;
    async fn list_for_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Vec<HistoryEntry>, AppError>;
    /// 最近活跃历史（首页 Continue 的补充数据源）。
    async fn recent(&self, limit: u32) -> Result<Vec<HistoryEntry>, AppError>;
    /// 清空全部历史（`history_clear`；单条 DELETE 语句原子，无 10k 上限问题）。
    async fn clear_all(&self) -> Result<(), AppError>;
    async fn delete(&self, id: HistoryEntryId) -> Result<bool, AppError>;
}

/// 搜索历史契约（V02-SETTINGS-PRIVACY-DATA-007）。
///
/// 搜索词是可删除的本地偏好，与播放/阅读历史分表、分命令、分清理范围。
/// Repository 只保存规范化后的词和最近使用时间；排序与数量上限由应用层约束。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHistoryEntry {
    pub term: String,
    pub last_used_at: haven_common::UtcMillis,
}

#[async_trait]
pub trait SearchHistoryRepository: Send + Sync {
    async fn list(&self, limit: u32) -> Result<Vec<SearchHistoryEntry>, AppError>;
    async fn record(&self, term: &str, at: haven_common::UtcMillis) -> Result<(), AppError>;
    async fn delete(&self, term: &str) -> Result<bool, AppError>;
    async fn clear_all(&self) -> Result<u64, AppError>;
}

/// 存储位置契约（LIBRARY_AND_STORAGE §14–§17；扫描器的输入依赖）。
#[async_trait]
pub trait StorageLocationRepository {
    async fn get(&self, id: StorageLocationId) -> Result<Option<StorageLocation>, AppError>;
    async fn save(&self, location: &StorageLocation) -> Result<(), AppError>;
    async fn list(&self) -> Result<Vec<StorageLocation>, AppError>;
    async fn delete(&self, id: StorageLocationId) -> Result<bool, AppError>;
    /// 清除凭据引用（S-04：凭据删除编排的 DB 侧——先删系统凭据，成功后才清 ref）。
    async fn clear_credential_ref(&self, id: StorageLocationId) -> Result<bool, AppError>;
}

/// 持久化下载任务。状态迁移使用 CAS，避免并发命令覆盖 Worker 的最新状态。
#[async_trait]
pub trait DownloadRepository {
    async fn get(&self, id: DownloadTaskId) -> Result<Option<DownloadTask>, AppError>;
    async fn save(&self, task: &DownloadTask) -> Result<(), AppError>;
    async fn list(&self, limit: u32, offset: u32) -> Result<Vec<DownloadTask>, AppError>;
    /// 查找同一来源和目标的可复用任务；失败/取消任务允许重新创建。
    async fn find_active(
        &self,
        source_resource_id: ResourceId,
        target_storage_id: StorageLocationId,
    ) -> Result<Option<DownloadTask>, AppError>;
    /// 查找同一来源和目标的最近完成任务；由应用层再次校验离线文件是否仍有效。
    async fn find_completed(
        &self,
        source_resource_id: ResourceId,
        target_storage_id: StorageLocationId,
    ) -> Result<Option<DownloadTask>, AppError>;
    /// 仅删除终态任务记录；Offline Resource 由独立用例管理。
    async fn delete_terminal(&self, id: DownloadTaskId) -> Result<bool, AppError>;
    /// 在 Verifying 阶段把最终 Offline Resource 绑定到任务。
    async fn associate_offline_resource(
        &self,
        id: DownloadTaskId,
        expected: DownloadState,
        resource_id: ResourceId,
    ) -> Result<bool, AppError>;
    async fn compare_and_set_state(
        &self,
        id: DownloadTaskId,
        expected: DownloadState,
        next: DownloadState,
    ) -> Result<bool, AppError>;
    async fn update_progress(
        &self,
        id: DownloadTaskId,
        expected: DownloadState,
        bytes_total: Option<u64>,
        bytes_downloaded: u64,
        speed_bps: Option<u64>,
        eta_seconds: Option<u64>,
    ) -> Result<bool, AppError>;
    /// 进程启动时把不可能仍在运行的状态收敛为 Interrupted。
    async fn mark_active_interrupted(&self) -> Result<u64, AppError>;
    /// 按批次列出全部子任务（Batch 聚合只读来源）。
    async fn list_by_batch(&self, batch_id: DownloadBatchId)
    -> Result<Vec<DownloadTask>, AppError>;
    /// 列出未终态任务（调度器取待运行任务；limit 由调用方钳制）。
    async fn list_schedulable(
        &self,
        limit: u32,
        now: haven_common::UtcMillis,
    ) -> Result<Vec<DownloadTask>, AppError>;
}

/// 下载批次聚合契约（DOMAIN_MODEL §45：Batch 只聚合子任务状态，不直接执行网络）。
#[async_trait]
pub trait DownloadBatchRepository: Send + Sync {
    async fn get(
        &self,
        id: DownloadBatchId,
    ) -> Result<Option<crate::entities::DownloadBatch>, AppError>;
    async fn save(&self, batch: &crate::entities::DownloadBatch) -> Result<(), AppError>;
    async fn list(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<crate::entities::DownloadBatch>, AppError>;
}

/// Settings 契约（BE-SETTINGS-001）：按 Section 存取（每 Section 一行）。
/// revision 为状态版本：实际变化时新生成；相同值重复更新幂等返回当前 revision。
/// 并发控制不在 Repository 层做（R-MAIN-01 复审：CAS 语义由 Application 层
/// SettingsUoW 承担——读/校验/比较/写必须在同一事务内，见 services/settings.rs）。
#[async_trait]
pub trait SettingsRepository {
    /// 读取指定 Section 的原始行（从未保存返回 None）。
    async fn get(&self, section: &str) -> Result<Option<SettingsRow>, AppError>;
    /// 写入/覆盖指定 Section 行（含新 revision）。
    async fn upsert(&self, row: &SettingsRow) -> Result<(), AppError>;
}

/// 资源内设持久化契约（ADR-RESOURCE-PREF-001）。
/// Repository 只负责已校验 Patch 的存取和数据库层 CAS；effective 合并由 Application 完成。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditionPreference {
    pub edition_id: EditionId,
    pub data: PreferenceData,
    pub revision: String,
    pub updated_at: haven_common::UtcMillis,
}

/// 媒体条目级资源内设。`edition_id` 是**行内记载的归属版本**：读取时必须原样返回，
/// 不得按调用方上下文改写。提案应用路径会把它与 target 的 edition 交叉校验，不一致
/// 即稳定的完整性错误（而不是"顺手改成目标版本"——那会把一条错行静默洗白）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaItemPreference {
    pub media_item_id: MediaItemId,
    pub edition_id: EditionId,
    pub data: PreferenceData,
    pub revision: String,
    pub updated_at: haven_common::UtcMillis,
}

#[async_trait]
pub trait ResourcePreferenceRepository: Send + Sync {
    async fn get_edition(
        &self,
        edition_id: EditionId,
    ) -> Result<Option<EditionPreference>, AppError>;
    async fn get_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Option<MediaItemPreference>, AppError>;
    /// 条件写：首次写入 expected=None；已有行必须精确匹配 expected。
    /// 返回 false 表示 revision 条件未满足。
    async fn cas_upsert_edition(
        &self,
        preference: &EditionPreference,
        expected_revision: Option<&str>,
    ) -> Result<bool, AppError>;
    async fn cas_upsert_media_item(
        &self,
        preference: &MediaItemPreference,
        expected_revision: Option<&str>,
    ) -> Result<bool, AppError>;
}

/// 设置变更提案持久化契约（V02-SETTING-PROPOSAL-001）。
///
/// 只覆盖"提案本身"的存取：创建、读取与回执查询。
/// **应用（apply）刻意不在这里**：真正生效必须走 Application 层的显式
/// `apply_confirmed` 事务路径（重读提案 → 完整性校验 → 条件 CAS → 回执 +
/// 状态迁移必须同一事务），Repository 不提供任何状态迁移或"直接执行"方法。
#[async_trait]
pub trait SettingProposalRepository: Send + Sync {
    /// 保存新提案（status 必须为 `pending`；id 冲突是错误，不做覆盖）。
    async fn create(&self, proposal: &SettingProposal) -> Result<(), AppError>;
    /// 读取并对存储行做完整性校验：canonical JSON 与 digest 必须自洽、
    /// 载荷版本受支持、target/operation 结构匹配、冗余 target 列与载荷一致。
    async fn get(&self, id: SettingProposalId) -> Result<Option<SettingProposal>, AppError>;
    /// 读取变更回执（提案未应用返回 `None`）。
    ///
    /// 返回的回执必须是**自洽的审计事实**：`proposal_digest` 是 64 位 ASCII 小写
    /// 十六进制、`before`/`after` 落在 target 对应的类型上且是 canonical 形式、
    /// `changed` 与 `applied_revision` 互相印证。读取端不得只信列值就返回——
    /// 无法校验或与父提案不一致的行是完整性错误，不是 `None`。
    /// 反过来，`None` 的含义被钉死为"**确实没有应用过**"：提案处于 `applied`
    /// 却读不出回执是存储被改写，必须报完整性错误——把它报成 `None` 会让审计/UI
    /// 与 `apply_confirmed` 对同一状态给出相反结论。
    async fn get_receipt(
        &self,
        id: SettingProposalId,
    ) -> Result<Option<SettingChangeReceipt>, AppError>;
}

/// Agent 动作绑定的只读契约。
///
/// 创建、Approval Token 摘要的签发/消费与状态迁移由 SettingProposal 的同一 UoW
/// 事务负责；原始 token 不经过 Repository，也不作为可序列化领域事实保存。这个
/// Repository 只用于 Agent 动作恢复/执行前读取并校验绑定事实，不提供任何直接写设置
/// 或绕过审批链的状态修改方法。
#[async_trait]
pub trait AgentActionBindingRepository: Send + Sync {
    async fn get(
        &self,
        proposal_id: SettingProposalId,
    ) -> Result<Option<AgentActionBinding>, AppError>;
}

/// 受控图片代理映射（契约 §36 C1：外部海报 URL 不进 IPC）。
/// `register` 幂等：同一来源与 URL 返回同一稳定 id（UUID 字符串）。
#[async_trait]
pub trait ImageProxyRepository: Send + Sync {
    async fn register(&self, source_id: &str, target_url: &str) -> Result<String, AppError>;
    async fn resolve(&self, id: &str) -> Result<Option<String>, AppError>;
}

/// Enrichment 流水线状态记录（契约 §36.8）。
/// 每个 Work 至多一条；匹配失败不回滚扫描，保留原始名并标 failed。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrichmentState {
    pub work_id: WorkId,
    /// pending | enriched | failed（闭合枚举以字符串持久化）。
    pub status: String,
    pub source_id: Option<String>,
    /// 安全文案，不含内部路径/远端响应。
    pub error: Option<String>,
    pub updated_at: haven_common::UtcMillis,
}

#[async_trait]
pub trait EnrichmentRepository: Send + Sync {
    /// 读取单条状态（从未入队返回 None）。
    async fn get(&self, work_id: WorkId) -> Result<Option<EnrichmentState>, AppError>;
    /// 读取全部或按 work 过滤的状态（updated_at 倒序）。
    async fn list(&self, work_id: Option<WorkId>) -> Result<Vec<EnrichmentState>, AppError>;
    /// 读取超过陈旧阈值的 pending 状态，结果有界且按最早更新时间优先。
    async fn list_stale_pending(
        &self,
        cutoff_ms: i64,
        limit: u32,
    ) -> Result<Vec<EnrichmentState>, AppError>;
    /// upsert 单条状态。
    async fn upsert(&self, state: &EnrichmentState) -> Result<(), AppError>;
}

/// settings 表原始行（data_json 只含 Typed DTO 序列化；Secret 禁止入内）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsRow {
    pub section: String,
    pub schema_version: u32,
    pub revision: String,
    pub data_json: String,
    pub updated_at: haven_common::UtcMillis,
}
