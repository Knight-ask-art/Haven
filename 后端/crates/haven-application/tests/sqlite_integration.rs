//! 真实 SQLite 组合端口集成测试（BE-PROGRESS-001 / BE-HISTORY-001 验收项）。
//!
//! - file-backed DB reopen 后 Service 状态恢复（"重启恢复"）。
//! - 真实 upsert / 唯一索引行为（并发 record 幂等语义的 DB 保障）。

use std::sync::Arc;

use haven_application::services::history::HistoryService;
use haven_application::services::home::HomeService;
use haven_application::services::library::LibraryService;
use haven_application::services::progress::ProgressService;
use haven_application::services::progress::comic_progress_subject::ComicProgressSubjectService;
use haven_application::services::settings::SettingsService;
use haven_application::services::work::WorkService;
use haven_application::wire::{
    CompletionWire, EditionGetRequest, LibraryListRequest, LibraryListSort, LocatorDto,
    ProgressSaveRequest, QueryCategory, VideoLocatorDto,
};
use haven_domain::comic_identity::ChapterEvidence;
use haven_domain::comic_progress_subject::{
    ComicProgressSubject, ComicProgressSubjectMember, ComicProgressSubjectRelationship,
};
use haven_domain::contracts::{
    ComicProgressSubjectRepository, EditionRepository, FavoriteRepository, MediaItemRepository,
    ProgressRepository, ResourceRepository, StorageLocationRepository, WorkRepository,
};
use haven_domain::entities::{Edition, MediaIndex, MediaItem, Work};
use haven_domain::enums::{
    Availability, AvailabilitySource, MediaItemStatus, MediaType, ResourceType, WorkStatus,
    WorkType,
};
use haven_domain::ids::{EditionId, MediaItemId, ProgressId, ResourceId, WorkId};
use haven_infrastructure::Db;
use haven_infrastructure::db::repos::{SqliteRepositories, SqliteSettingsUoW};
use haven_infrastructure::db::uow::SqliteUnitOfWork;

/// 组装真实 Sqlite 端口（work/edition/media_item/progress/history 全链）。
fn services(db: Arc<Db>) -> (ProgressService, HistoryService) {
    let settings = SettingsService::new(Arc::new(SqliteSettingsUoW::new(db.clone())));
    let repos = Arc::new(SqliteRepositories::new(db));
    let progress = ProgressService::new(repos.clone());
    let history = HistoryService::new(repos, Arc::new(settings));
    (progress, history)
}

/// 通过真实 Repository 建立 work → edition → media_item 链，返回 media_item_id。
async fn seed_chain(repos: &SqliteRepositories) -> MediaItemId {
    let work_id = WorkId::new();
    let edition_id = EditionId::new();
    let media_item_id = MediaItemId::new();
    let now = haven_common::UtcMillis::now();

    repos
        .work
        .save(&Work {
            id: work_id,
            canonical_title: "集成测试作品".into(),
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
            edition_type: MediaType::Movie,
            release_date: None,
            language: None,
            region: None,
            publisher_or_studio: None,
            description: None,
            artwork: Default::default(),
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
            media_type: MediaType::Movie,
            title: "集成电影".into(),
            index: MediaIndex::Movie,
            duration_ms: Some(100_000),
            page_count: None,
            chapter_count: None,
            published_at: None,
            status: MediaItemStatus::Available,
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();
    media_item_id
}

fn video_request(media_item_id: MediaItemId, position_ms: u64) -> ProgressSaveRequest {
    ProgressSaveRequest {
        media_item_id: media_item_id.to_string(),
        locator: LocatorDto::Video(VideoLocatorDto { position_ms }),
        completion: Some(CompletionWire::InProgress),
        expected_revision: None,
        keyframe: None,
    }
}

#[tokio::test]
async fn progress_survives_db_reopen() {
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("haven-integration.db");

    let media_item_id = {
        let db = Arc::new(Db::open(&db_path).unwrap());
        let repos = SqliteRepositories::new(db.clone());
        let media_item_id = seed_chain(&repos).await;
        let (progress, _) = services(db);
        let result = progress
            .save(video_request(media_item_id, 30_000))
            .await
            .unwrap();
        assert!(!result.revision.is_empty());
        media_item_id
    };

    // 重新打开同一 DB 文件（模拟重启）→ 状态必须恢复。
    let db = Arc::new(Db::open(&db_path).unwrap());
    let (progress, _) = services(db);
    let summary = progress
        .get(media_item_id)
        .await
        .unwrap()
        .expect("重启后必须恢复");
    assert_eq!(summary.completion, CompletionWire::InProgress);
    let ratio = summary
        .progress_ratio
        .expect("重启后 ratio 保留（30s/100s）");
    assert!((ratio - 0.3).abs() < 1e-6, "ratio 应为 0.3，实际 {ratio}");
    let json = serde_json::to_string(&summary).unwrap();
    assert!(json.contains("\"mediaItemId\""), "{json}");
}

#[tokio::test]
async fn history_survives_db_reopen_and_clear_works() {
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("haven-integration-history.db");

    let media_item_id = {
        let db = Arc::new(Db::open(&db_path).unwrap());
        let repos = SqliteRepositories::new(db.clone());
        let media_item_id = seed_chain(&repos).await;
        let (_, history) = services(db);
        history.record(media_item_id).await.unwrap();
        media_item_id
    };

    // 重启后历史恢复
    let db = Arc::new(Db::open(&db_path).unwrap());
    let (_, history) = services(db);
    let entries = history.list_for_media_item(media_item_id).await.unwrap();
    assert_eq!(entries.len(), 1, "重启后历史必须恢复");
    assert_eq!(entries[0].media_item_id, media_item_id.to_string());

    // clear 清空（只清历史）
    history.clear().await.unwrap();
    assert!(
        history
            .list_for_media_item(media_item_id)
            .await
            .unwrap()
            .is_empty(),
        "clear 后为空"
    );
}

#[tokio::test]
async fn history_upsert_is_idempotent_at_db_level() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = SqliteRepositories::new(db.clone());
    let media_item_id = seed_chain(&repos).await;
    let (_, history) = services(db);

    history.record(media_item_id).await.unwrap();
    history.record(media_item_id).await.unwrap();
    history.record(media_item_id).await.unwrap();

    // 唯一索引 + upsert：并发 record 也只保留一条（通过 Service 列表验证）。
    let entries = history.list_for_media_item(media_item_id).await.unwrap();
    assert_eq!(entries.len(), 1, "并发 record 只保留一条");
}

// ---- S-04：跨存储删除编排（先删系统凭据，成功后才清 DB ref）----

use haven_application::services::credential::{CredentialDeleteOutcome, CredentialDeletionService};
use haven_common::AppError;
use haven_domain::credential::{CredentialStore, SecretString};
use haven_domain::enums::{StorageProviderType, StorageStatus};
use haven_domain::ids::{CredentialRef, StorageLocationId};

/// 可注入的 mock CredentialStore（失败注入：成功 / NoEntry 语义 / 平台错误）。
/// 记录实际被删除的 target，供交叉错配断言。
struct MockCredentialStore {
    mode: MockMode,
    deleted_targets: Arc<std::sync::Mutex<Vec<String>>>,
}

#[derive(Clone, Copy)]
enum MockMode {
    Deletes,
    Missing,
    PlatformError,
}

#[async_trait::async_trait]
impl CredentialStore for MockCredentialStore {
    async fn set(&self, _target: &CredentialRef, _secret: &SecretString) -> Result<(), AppError> {
        Ok(())
    }
    async fn get(&self, _target: &CredentialRef) -> Result<Option<SecretString>, AppError> {
        Ok(None)
    }
    async fn delete(&self, target: &CredentialRef) -> Result<bool, AppError> {
        match self.mode {
            MockMode::Deletes => {
                self.deleted_targets
                    .lock()
                    .unwrap()
                    .push(target.as_str().to_owned());
                Ok(true)
            }
            MockMode::Missing => Ok(false),
            MockMode::PlatformError => Err(AppError::new(
                "CREDENTIAL_ACCESS_FAILED",
                haven_common::ErrorKind::Security,
                "模拟平台错误",
                true,
            )),
        }
    }
}

async fn seed_location_with_ref(repos: &SqliteRepositories, ref_str: &str) -> StorageLocationId {
    let id = StorageLocationId::new();
    let location = haven_domain::entities::StorageLocation {
        id,
        provider_type: StorageProviderType::Local,
        display_name: "凭据库".into(),
        // 唯一索引 lower(root_ref)（008 迁移）：不同位置必须不同 root_ref。
        root_ref: format!("D:\\Secure\\{ref_str}"),
        credential_ref: Some(ref_str.parse().unwrap()),
        status: StorageStatus::Connected,
        created_at: haven_common::UtcMillis(1_000),
        updated_at: haven_common::UtcMillis(1_000),
    };
    repos.storage_location.save(&location).await.unwrap();
    id
}

async fn credential_ref_of(repos: &SqliteRepositories, id: StorageLocationId) -> Option<String> {
    let loc = repos.storage_location.get(id).await.unwrap().expect("存在");
    loc.credential_ref.map(|r| r.as_str().to_owned())
}

#[tokio::test]
async fn deletion_clears_db_ref_only_after_store_success() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db));
    let target: CredentialRef = "haven:webdav:profile-1".parse().unwrap();
    let location_id = seed_location_with_ref(&repos, target.as_str()).await;

    let store = Arc::new(MockCredentialStore {
        mode: MockMode::Deletes,
        deleted_targets: Arc::new(std::sync::Mutex::new(vec![])),
    });
    let service = CredentialDeletionService::new(store.clone(), repos.clone());
    // R-S04-1：API 只接收 location_id，凭据 target 由 DB 绑定提供。
    let outcome = service.delete(location_id).await.unwrap();
    assert_eq!(outcome, CredentialDeleteOutcome::Deleted);
    assert!(
        credential_ref_of(&repos, location_id).await.is_none(),
        "成功删除后 DB ref 已清"
    );
    assert_eq!(
        store.deleted_targets.lock().unwrap().as_slice(),
        [target.as_str()],
        "只删除该 location 绑定的凭据"
    );
}

#[tokio::test]
async fn deletion_with_missing_entry_is_idempotent_ref_cleared() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db));
    let target: CredentialRef = "haven:webdav:profile-2".parse().unwrap();
    let location_id = seed_location_with_ref(&repos, target.as_str()).await;

    let store = Arc::new(MockCredentialStore {
        mode: MockMode::Missing,
        deleted_targets: Arc::new(std::sync::Mutex::new(vec![])),
    });
    let service = CredentialDeletionService::new(store, repos.clone());
    let outcome = service.delete(location_id).await.unwrap();
    assert_eq!(outcome, CredentialDeleteOutcome::RefCleared);
    assert!(
        credential_ref_of(&repos, location_id).await.is_none(),
        "NoEntry 幂等清 ref"
    );
}

#[tokio::test]
async fn deletion_failure_keeps_db_ref_and_is_retryable() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db));
    let target: CredentialRef = "haven:webdav:profile-3".parse().unwrap();
    let location_id = seed_location_with_ref(&repos, target.as_str()).await;

    let store = Arc::new(MockCredentialStore {
        mode: MockMode::PlatformError,
        deleted_targets: Arc::new(std::sync::Mutex::new(vec![])),
    });
    let service = CredentialDeletionService::new(store, repos.clone());
    let err = service.delete(location_id).await.unwrap_err();
    assert_eq!(err.code().as_str(), "CREDENTIAL_ACCESS_FAILED");
    assert!(err.retryable(), "平台错误应标 retryable");
    assert_eq!(
        credential_ref_of(&repos, location_id).await.as_deref(),
        Some(target.as_str()),
        "删除失败时 DB ref 必须保留"
    );
}

#[tokio::test]
async fn deletion_cannot_cross_mismatch_credentials_between_locations() {
    // R-S04-1：API 不接收 credential_ref，调用 delete(A) 只能影响 A 的绑定凭据。
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db));
    let ref_a: CredentialRef = "haven:webdav:profile-a".parse().unwrap();
    let ref_b: CredentialRef = "haven:webdav:profile-b".parse().unwrap();
    let id_a = seed_location_with_ref(&repos, ref_a.as_str()).await;
    let id_b = seed_location_with_ref(&repos, ref_b.as_str()).await;

    let store = Arc::new(MockCredentialStore {
        mode: MockMode::Deletes,
        deleted_targets: Arc::new(std::sync::Mutex::new(vec![])),
    });
    let service = CredentialDeletionService::new(store.clone(), repos.clone());
    let outcome = service.delete(id_a).await.unwrap();
    assert_eq!(outcome, CredentialDeleteOutcome::Deleted);
    assert!(
        credential_ref_of(&repos, id_a).await.is_none(),
        "A 的 ref 已清"
    );
    assert_eq!(
        credential_ref_of(&repos, id_b).await.as_deref(),
        Some(ref_b.as_str()),
        "B 的 DB ref 不得受影响"
    );
    assert_eq!(
        store.deleted_targets.lock().unwrap().as_slice(),
        [ref_a.as_str()],
        "系统凭据只删除 A 绑定的 target，B 的凭据不可被触碰"
    );
}

#[tokio::test]
async fn deletion_of_missing_location_errors_without_touching_store() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db));
    let store = Arc::new(MockCredentialStore {
        mode: MockMode::Deletes,
        deleted_targets: Arc::new(std::sync::Mutex::new(vec![])),
    });
    let service = CredentialDeletionService::new(store.clone(), repos.clone());
    let err = service.delete(StorageLocationId::new()).await.unwrap_err();
    assert_eq!(err.code().as_str(), "RESOURCE_NOT_FOUND");
    assert!(
        store.deleted_targets.lock().unwrap().is_empty(),
        "location 不存在时不得触碰任何系统凭据"
    );
}

#[tokio::test]
async fn deletion_with_null_ref_is_idempotent_success() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db));
    let id = StorageLocationId::new();
    let location = haven_domain::entities::StorageLocation {
        id,
        provider_type: StorageProviderType::Local,
        display_name: "无凭据库".into(),
        root_ref: "D:\\Open".into(),
        credential_ref: None,
        status: StorageStatus::Connected,
        created_at: haven_common::UtcMillis(1_000),
        updated_at: haven_common::UtcMillis(1_000),
    };
    repos.storage_location.save(&location).await.unwrap();

    let store = Arc::new(MockCredentialStore {
        mode: MockMode::Deletes,
        deleted_targets: Arc::new(std::sync::Mutex::new(vec![])),
    });
    let service = CredentialDeletionService::new(store.clone(), repos.clone());
    let outcome = service.delete(id).await.unwrap();
    assert_eq!(outcome, CredentialDeleteOutcome::RefCleared);
    assert!(
        store.deleted_targets.lock().unwrap().is_empty(),
        "无凭据时不调用 store"
    );
}

/// mock StorageLocationRepository：注入 DB clear 失败。
struct MockStorageRepo {
    location: haven_domain::entities::StorageLocation,
    clear_fails: bool,
}

#[async_trait::async_trait]
impl haven_domain::contracts::StorageLocationRepository for MockStorageRepo {
    async fn get(
        &self,
        _id: StorageLocationId,
    ) -> Result<Option<haven_domain::entities::StorageLocation>, AppError> {
        Ok(Some(self.location.clone()))
    }
    async fn save(
        &self,
        _location: &haven_domain::entities::StorageLocation,
    ) -> Result<(), AppError> {
        Ok(())
    }
    async fn list(&self) -> Result<Vec<haven_domain::entities::StorageLocation>, AppError> {
        Ok(vec![])
    }
    async fn delete(&self, _id: StorageLocationId) -> Result<bool, AppError> {
        Ok(false)
    }
    async fn clear_credential_ref(&self, _id: StorageLocationId) -> Result<bool, AppError> {
        if self.clear_fails {
            Err(AppError::new(
                "DATABASE_ERROR",
                haven_common::ErrorKind::Database,
                "模拟 DB 失败",
                true,
            ))
        } else {
            Ok(true)
        }
    }
}

#[tokio::test]
async fn deletion_db_clear_failure_returns_stable_error() {
    use haven_domain::ids::CredentialRef;
    let target: CredentialRef = "haven:webdav:profile-clear-fail".parse().unwrap();
    let location = haven_domain::entities::StorageLocation {
        id: StorageLocationId::new(),
        provider_type: StorageProviderType::Local,
        display_name: "clear 失败库".into(),
        root_ref: "D:\\Fail".into(),
        credential_ref: Some(target),
        status: StorageStatus::Connected,
        created_at: haven_common::UtcMillis(1_000),
        updated_at: haven_common::UtcMillis(1_000),
    };

    let repo = MockStorageRepo {
        location: location.clone(),
        clear_fails: true,
    };
    let store = Arc::new(MockCredentialStore {
        mode: MockMode::Deletes,
        deleted_targets: Arc::new(std::sync::Mutex::new(vec![])),
    });
    let service = CredentialDeletionService::new(store.clone(), Arc::new(repo));
    let err = service.delete(location.id).await.unwrap_err();
    assert_eq!(err.code().as_str(), "DATABASE_ERROR");
    assert!(err.retryable(), "DB clear 失败可重试（幂等）");
    assert_eq!(
        store.deleted_targets.lock().unwrap().as_slice(),
        [location.credential_ref.as_ref().unwrap().as_str()],
        "系统凭据已删（重试时 delete 返回 NoEntry，仍会清 ref）"
    );
}

// ---- R-FAV-002：真实 SQLite 首次 set(false) 幂等（不写库、不发 Event、重启一致）----

use haven_application::services::favorite::FavoriteService;

async fn seed_simple_work(repos: &SqliteRepositories) -> WorkId {
    let work_id = WorkId::new();
    let now = haven_common::UtcMillis::now();
    repos
        .work
        .save(&Work {
            id: work_id,
            canonical_title: "首次 false 测试".into(),
            original_title: None,
            sort_title: None,
            description: None,
            work_type: haven_domain::enums::WorkType::Standalone,
            release_year: None,
            language: None,
            director: None,
            actor: None,
            status: WorkStatus::Unknown,
            rating_value: None,
            rating_scale: None,
            artwork: Default::default(),
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();
    work_id
}

/// 通过公共 Repository 接口统计 favorites 行数（Db.lock 为 crate 私有，集成测试不可用）。
async fn favorite_row_count(repos: &SqliteRepositories) -> usize {
    repos.favorite.list(u32::MAX, 0).await.unwrap().len()
}

#[tokio::test]
async fn first_false_writes_nothing_and_survives_reopen() {
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("first-false.db");
    let db = Arc::new(Db::open(&db_path).unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let work_id = seed_simple_work(&repos).await;
    let service = FavoriteService::new(repos.clone(), Arc::new(SqliteUnitOfWork::new(db.clone())));

    let outcome = service.set_with_outcome(work_id, false).await.unwrap();
    assert!(!outcome.changed, "首次 false 不得视为状态变化");
    assert!(
        outcome.result.revision.is_none(),
        "无版本历史 → revision=null"
    );
    assert_eq!(favorite_row_count(&repos).await, 0, "不得写入 favorites 行");

    // 重启后行为一致（file-backed reopen；版本行是否写入由 revision=None 隐式验证）
    drop(repos);
    drop(service);
    drop(db);
    let db2 = Arc::new(Db::open(&db_path).unwrap());
    let repos2 = Arc::new(SqliteRepositories::new(db2.clone()));
    let service2 =
        FavoriteService::new(repos2.clone(), Arc::new(SqliteUnitOfWork::new(db2.clone())));
    let again = service2.set_with_outcome(work_id, false).await.unwrap();
    assert!(!again.changed);
    assert!(again.result.revision.is_none(), "重启后仍无版本历史");
    assert_eq!(favorite_row_count(&repos2).await, 0, "重启后仍零行");
}

// ---- 审查批次 P1-3：library_list 多进度去重（真实 SQLite）----
// progress 唯一约束在 media_item_id 而非 work_id：一个 Work 下多个 MediaItem
// 各有进度时，裸 LEFT JOIN 会让同一作品在列表重复出现、与 count 的 total 错位。
// 此前 LibraryService 没有任何真实 SQLite 集成测试（SQL 形状 bug 单测抓不到）。

use haven_application::services::library::MAX_LIMIT;
use haven_domain::entities::Progress;
use haven_domain::enums::CompletionState;
use haven_domain::locator::{ComicLocator, Locator, VideoLocator};

async fn seed_work_two_progress(repos: &SqliteRepositories) {
    let work_id = WorkId::new();
    let edition_id = EditionId::new();
    let now = haven_common::UtcMillis::now();
    repos
        .work
        .save(&Work {
            id: work_id,
            canonical_title: "多进度列表作品".into(),
            original_title: None,
            sort_title: None,
            description: None,
            work_type: WorkType::Standalone,
            release_year: None,
            language: None,
            director: None,
            actor: None,
            status: WorkStatus::Unknown,
            rating_value: None,
            rating_scale: None,
            artwork: Default::default(),
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
            title: "合集版本".into(),
            subtitle: None,
            edition_type: MediaType::Movie,
            release_date: None,
            language: None,
            region: None,
            publisher_or_studio: None,
            description: None,
            artwork: Default::default(),
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();
    for n in 0..2i64 {
        let media_item_id = MediaItemId::new();
        repos
            .media_item
            .save(&MediaItem {
                id: media_item_id,
                edition_id,
                parent_id: None,
                media_type: MediaType::Movie,
                title: format!("条目{n}"),
                index: MediaIndex::Movie,
                duration_ms: Some(100_000),
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
            .progress
            .save(&Progress {
                id: haven_domain::ids::ProgressId::new(),
                work_id,
                edition_id,
                media_item_id,
                locator: Locator::Video(VideoLocator {
                    media_item_id,
                    position_ms: (n * 1000) as u64,
                }),
                completion: CompletionState::InProgress,
                percentage: Some(0.5),
                last_active_at: haven_common::UtcMillis(now.0 + n),
                updated_at: haven_common::UtcMillis(now.0 + n),
                revision: None,
                keyframe_uri: None,
            })
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn library_list_dedups_work_with_multiple_progress_rows() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db));
    seed_work_two_progress(&repos).await;
    let service = LibraryService::new(repos.clone());

    for sort in [
        LibraryListSort::LastActive,
        LibraryListSort::RecentlyAdded,
        LibraryListSort::Title,
        LibraryListSort::ReleaseDate,
    ] {
        let page = service
            .list(LibraryListRequest {
                category: QueryCategory::All,
                media_types: None,
                query: None,
                sort,
                cursor: None,
                limit: MAX_LIMIT,
            })
            .await
            .unwrap();
        assert_eq!(
            page.items.len(),
            1,
            "{sort:?}：一个 Work 多条进度只能出一张卡"
        );
        assert_eq!(page.total, Some(1), "{sort:?}：total 与卡片数一致");
        assert!(page.next_cursor.is_none(), "{sort:?}：单作品无下一页");
    }
}

struct ComicReadOnlyProjectionFixture {
    work_id: WorkId,
    edition_id: EditionId,
    source_media_item_id: MediaItemId,
    target_media_item_id: MediaItemId,
    subject_id: haven_domain::ids::ComicProgressSubjectId,
}

/// 建立一个真实 SQLite 漫画连续性聚合：源章节有一条持久化 Progress，目标
/// 章节只有 active Subject member，没有自己的 Progress。目标资源单独可消费，
/// 这样 Work Header 也必须选择并投影目标章节，而不能借用源章节的 locator。
async fn seed_comic_read_only_projection_fixture(
    repos: &SqliteRepositories,
) -> ComicReadOnlyProjectionFixture {
    let now = haven_common::UtcMillis(1_000);
    let work_id = WorkId::new();
    let edition_id = EditionId::new();
    let source_media_item_id = MediaItemId::new();
    let target_media_item_id = MediaItemId::new();

    repos
        .work
        .save(&Work {
            id: work_id,
            canonical_title: "只读投影漫画".into(),
            original_title: None,
            sort_title: Some("只读投影漫画".into()),
            description: Some("真实 SQLite Subject 投影 fixture".into()),
            work_type: WorkType::Fiction,
            release_year: Some(2026),
            language: Some("zh".into()),
            director: None,
            actor: None,
            status: WorkStatus::Ongoing,
            rating_value: None,
            rating_scale: None,
            artwork: Default::default(),
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
            title: "中文 · Group A · 黑白".into(),
            subtitle: None,
            edition_type: MediaType::Comic,
            release_date: None,
            language: Some("zh-CN".into()),
            region: None,
            publisher_or_studio: Some("Group A".into()),
            description: None,
            artwork: Default::default(),
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();

    let source_item = MediaItem {
        id: source_media_item_id,
        edition_id,
        parent_id: None,
        media_type: MediaType::Comic,
        title: "第 1 话".into(),
        index: MediaIndex::Chapter {
            volume: Some(1.0),
            chapter: 1.0,
        },
        duration_ms: None,
        page_count: Some(12),
        chapter_count: None,
        published_at: Some("2026-09-01T00:00:00Z".into()),
        status: MediaItemStatus::Available,
        created_at: now,
        updated_at: now,
    };
    let target_item = MediaItem {
        id: target_media_item_id,
        edition_id,
        parent_id: None,
        media_type: MediaType::Comic,
        title: "第 2 话".into(),
        index: MediaIndex::Chapter {
            volume: Some(1.0),
            chapter: 2.0,
        },
        duration_ms: None,
        page_count: Some(12),
        chapter_count: None,
        published_at: Some("2026-09-02T00:00:00Z".into()),
        status: MediaItemStatus::Available,
        // The target is newer, so it is also the deterministic selected item for
        // Work Header once its resource is the only actionable resource.
        created_at: haven_common::UtcMillis(1_001),
        updated_at: haven_common::UtcMillis(1_001),
    };
    repos.media_item.save(&source_item).await.unwrap();
    repos.media_item.save(&target_item).await.unwrap();

    let source_progress = Progress {
        // Keep the source Progress ID below the generated projected ID so the
        // WorkCard tie-breaker deterministically selects the target projection.
        id: ProgressId::from_uuid(uuid::Uuid::from_u128(1)),
        work_id,
        edition_id,
        media_item_id: source_media_item_id,
        locator: Locator::Comic(ComicLocator {
            chapter_item_id: source_media_item_id,
            page_index: 4,
            page_progression: Some(0.4),
        }),
        completion: CompletionState::InProgress,
        percentage: Some(0.4),
        last_active_at: haven_common::UtcMillis(2_000),
        updated_at: haven_common::UtcMillis(2_000),
        revision: None,
        keyframe_uri: Some("data:image/png;base64,fixture".into()),
    };
    repos
        .progress
        .save_if_revision(&source_progress, None)
        .await
        .unwrap()
        .expect("源漫画 Progress 必须取得 revision");

    let mut subject = ComicProgressSubject::new(work_id, edition_id, source_media_item_id, now);
    let mut source_member = ComicProgressSubjectMember::active(
        subject.id,
        source_media_item_id,
        vec![ChapterEvidence::SameRemoteIdentity],
    );
    source_member.relationship = ComicProgressSubjectRelationship::Canonical;
    subject.attach_member(source_member).unwrap();
    subject
        .attach_member(ComicProgressSubjectMember::active(
            subject.id,
            target_media_item_id,
            vec![ChapterEvidence::AuthoritativeContentKey],
        ))
        .unwrap();
    let subject_id = subject.id;
    repos.progress_subjects.save(&subject).await.unwrap();

    let source_id =
        haven_application::services::source_import::stable_source_id("mangadex").unwrap();
    repos
        .resource
        .save(&haven_domain::entities::Resource {
            id: ResourceId::new(),
            media_item_id: target_media_item_id,
            resource_type: ResourceType::ComicArchive,
            source_id: Some(source_id),
            storage_location_id: None,
            locator: haven_domain::entities::ResourceLocator::SourceObject {
                source_id,
                remote_id:
                    "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa:cccccccc-cccc-4ccc-8ccc-cccccccccccc"
                        .into(),
            },
            mime_type: Some("application/vnd.comicbook+zip".into()),
            size: None,
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

    ComicReadOnlyProjectionFixture {
        work_id,
        edition_id,
        source_media_item_id,
        target_media_item_id,
        subject_id,
    }
}

fn assert_comic_summary_belongs_to(
    summary: &haven_application::wire::ProgressSummaryDto,
    media_item_id: MediaItemId,
) {
    let expected = media_item_id.to_string();
    assert_eq!(summary.media_item_id, expected);
    assert!(
        !summary.revision.is_empty(),
        "只读投影必须带 opaque revision"
    );
    match &summary.locator {
        LocatorDto::Comic(locator) => {
            assert_eq!(locator.chapter_item_id, expected);
            assert_eq!(locator.page_index, 4);
            let progression = locator
                .page_progression
                .expect("Comic locator 必须保留 page progression");
            assert!(
                (progression - 0.4).abs() < 1e-6,
                "page progression 应保持 0.4，实际为 {progression}"
            );
        }
        other => panic!("漫画进度必须返回 Comic locator，实际为 {other:?}"),
    }
}

#[tokio::test]
async fn comic_read_only_projections_keep_target_locator_and_do_not_materialize_progress() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let fixture = seed_comic_read_only_projection_fixture(&repos).await;

    let subject_before = repos
        .progress_subjects
        .get(fixture.subject_id)
        .await
        .unwrap()
        .expect("fixture Subject 必须存在");
    let members_before = repos
        .progress_subjects
        .list_members(fixture.subject_id)
        .await
        .unwrap();
    let source_before = repos
        .progress
        .get_for_media_item(fixture.source_media_item_id)
        .await
        .unwrap()
        .expect("源 Progress 必须存在");
    assert!(
        repos
            .progress
            .get_for_media_item(fixture.target_media_item_id)
            .await
            .unwrap()
            .is_none()
    );

    let subjects = ComicProgressSubjectService::new(
        repos.clone(),
        Arc::new(SqliteUnitOfWork::new(db.clone())),
    );

    // Library 的 WorkCard 会读取源/目标两个 MediaItem 的 Subject 视角；目标
    // projection 必须通过 progress_summary，而不能触发 legacy backfill。
    let library = LibraryService::new(repos.clone()).with_comic_progress_subjects(subjects.clone());
    let library_page = library
        .list(LibraryListRequest {
            category: QueryCategory::All,
            media_types: None,
            query: None,
            sort: LibraryListSort::Title,
            cursor: None,
            limit: 10,
        })
        .await
        .unwrap();
    assert_eq!(library_page.items.len(), 1);
    let library_card = &library_page.items[0];
    let library_progress = library_card
        .progress
        .as_ref()
        .expect("Library 卡片必须展示最近的漫画进度");
    assert_comic_summary_belongs_to(library_progress, fixture.target_media_item_id);
    assert_eq!(
        library_card
            .primary_action
            .as_ref()
            .and_then(|action| action.media_item_id.as_deref()),
        Some(fixture.target_media_item_id.to_string().as_str())
    );

    // Home Continue 保留权威源行作为继续阅读入口；Recently Added 则与
    // Library 共用目标 MediaItem 视角，二者都必须是自洽的 Comic locator。
    let home = HomeService::new(repos.clone())
        .with_comic_progress_subjects(subjects.clone())
        .get()
        .await
        .unwrap();
    assert_eq!(home.continue_items.len(), 1);
    assert_eq!(
        home.continue_items[0].media_item_id,
        fixture.source_media_item_id.to_string()
    );
    assert_comic_summary_belongs_to(
        &home.continue_items[0].progress,
        fixture.source_media_item_id,
    );
    assert_eq!(home.recently_added.len(), 1);
    assert_comic_summary_belongs_to(
        home.recently_added[0]
            .progress
            .as_ref()
            .expect("Recently Added 必须展示漫画进度"),
        fixture.target_media_item_id,
    );

    // Work Header 的唯一可消费资源属于目标章节，因此它也必须返回目标
    // projection，而不是把源 locator 错配到目标 action。
    let work_service = WorkService::new(repos.clone()).with_comic_progress_subjects(subjects);
    let header = work_service.get(fixture.work_id).await.unwrap();
    assert_eq!(header.counts.available_resources, 1);
    assert_comic_summary_belongs_to(
        header.progress.as_ref().expect("Work Header 必须展示进度"),
        fixture.target_media_item_id,
    );
    assert_eq!(
        header
            .primary_action
            .as_ref()
            .and_then(|action| action.media_item_id.as_deref()),
        Some(fixture.target_media_item_id.to_string().as_str())
    );

    let edition = work_service
        .get_edition(EditionGetRequest {
            edition_id: fixture.edition_id.to_string(),
        })
        .await
        .unwrap();
    assert_eq!(edition.items.len(), 2);
    let source_detail = edition
        .items
        .iter()
        .find(|item| item.media_item_id == fixture.source_media_item_id.to_string())
        .expect("Edition Detail 必须包含源章节");
    assert_comic_summary_belongs_to(
        source_detail.progress.as_ref().expect("源章节必须保留进度"),
        fixture.source_media_item_id,
    );
    let target_detail = edition
        .items
        .iter()
        .find(|item| item.media_item_id == fixture.target_media_item_id.to_string())
        .expect("Edition Detail 必须包含目标章节");
    assert_comic_summary_belongs_to(
        target_detail
            .progress
            .as_ref()
            .expect("目标章节必须返回只读投影进度"),
        fixture.target_media_item_id,
    );
    assert!(target_detail.primary_action.is_some());

    // 四条只读读取路径都结束后，Subject/member、源行和目标行必须保持原状：
    // 列表/首页/详情不能隐式创建目标 Progress、移动 pointer 或更新时间。
    assert_eq!(
        repos
            .progress_subjects
            .get(fixture.subject_id)
            .await
            .unwrap()
            .expect("Subject 仍存在"),
        subject_before
    );
    assert_eq!(
        repos
            .progress_subjects
            .list_members(fixture.subject_id)
            .await
            .unwrap(),
        members_before
    );
    assert_eq!(
        repos
            .progress
            .get_for_media_item(fixture.source_media_item_id)
            .await
            .unwrap()
            .expect("源 Progress 仍存在"),
        source_before
    );
    assert!(
        repos
            .progress
            .get_for_media_item(fixture.target_media_item_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn stale_comic_projection_revision_rejects_save_after_source_progress_update() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("comic-projection-revision-race.db");
    let first_db = Arc::new(Db::open(&path).unwrap());
    let first_repos = Arc::new(SqliteRepositories::new(first_db.clone()));
    let fixture = seed_comic_read_only_projection_fixture(&first_repos).await;
    let subjects = ComicProgressSubjectService::new(
        first_repos.clone(),
        Arc::new(SqliteUnitOfWork::new(first_db.clone())),
    );

    // 读取的是目标视角，但 revision 来自当前 Subject 权威源行。
    let projected = subjects
        .progress_for_media_item(fixture.target_media_item_id)
        .await
        .unwrap()
        .expect("目标只读投影必须存在");
    let stale_revision = projected
        .revision
        .clone()
        .expect("只读投影必须暴露源 revision");
    assert_eq!(projected.media_item_id, fixture.target_media_item_id);

    // 另一个真实 SQLite 连接先推进源 Progress，模拟用户在列表/详情读取
    // 之后继续阅读。旧目标 projection 不得把自己的 locator 写回数据库。
    let second_db = Arc::new(Db::open(&path).unwrap());
    let second_repos = Arc::new(SqliteRepositories::new(second_db));
    let source_before = first_repos
        .progress
        .get_for_media_item(fixture.source_media_item_id)
        .await
        .unwrap()
        .expect("源 Progress 必须存在");
    let mut winner = source_before.clone();
    winner.locator = Locator::Comic(ComicLocator {
        chapter_item_id: fixture.source_media_item_id,
        page_index: 8,
        page_progression: Some(0.8),
    });
    winner.percentage = Some(0.8);
    winner.updated_at = haven_common::UtcMillis(3_000);
    winner.last_active_at = haven_common::UtcMillis(3_000);
    let winner_revision = second_repos
        .progress
        .save_if_revision(&winner, Some(&stale_revision))
        .await
        .unwrap()
        .expect("第二个连接必须用旧源 revision 成功推进源行");

    let error = subjects
        .save_progress_for_media_item(projected, Some(&stale_revision))
        .await
        .expect_err("源 revision 变化后旧目标投影必须冲突");
    assert_eq!(error.code().as_str(), "COMIC_PROGRESS_REVISION_CONFLICT");

    let source_after = first_repos
        .progress
        .get_for_media_item(fixture.source_media_item_id)
        .await
        .unwrap()
        .expect("源 Progress 仍存在");
    assert_eq!(source_after.locator, winner.locator);
    assert_eq!(source_after.percentage, winner.percentage);
    assert_eq!(
        source_after.revision.as_deref(),
        Some(winner_revision.as_str())
    );
    assert!(
        first_repos
            .progress
            .get_for_media_item(fixture.target_media_item_id)
            .await
            .unwrap()
            .is_none()
    );
    let subject_after = first_repos
        .progress_subjects
        .get(fixture.subject_id)
        .await
        .unwrap()
        .expect("Subject 仍存在");
    assert_eq!(
        subject_after.authoritative_progress_media_item_id,
        Some(fixture.source_media_item_id),
        "旧目标写入不得切换 Subject pointer"
    );
}
