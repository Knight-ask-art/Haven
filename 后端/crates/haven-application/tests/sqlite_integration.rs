//! 真实 SQLite 组合端口集成测试（BE-PROGRESS-001 / BE-HISTORY-001 验收项）。
//!
//! - file-backed DB reopen 后 Service 状态恢复（"重启恢复"）。
//! - 真实 upsert / 唯一索引行为（并发 record 幂等语义的 DB 保障）。

use std::sync::Arc;

use haven_application::services::history::HistoryService;
use haven_application::services::progress::ProgressService;
use haven_application::services::settings::SettingsService;
use haven_application::wire::{CompletionWire, LocatorDto, ProgressSaveRequest, VideoLocatorDto};
use haven_domain::contracts::{
    EditionRepository, FavoriteRepository, MediaItemRepository, ProgressRepository,
    StorageLocationRepository, WorkRepository,
};
use haven_domain::entities::{Edition, MediaIndex, MediaItem, Work};
use haven_domain::enums::{MediaItemStatus, MediaType, WorkStatus, WorkType};
use haven_domain::ids::{EditionId, MediaItemId, WorkId};
use haven_infrastructure::Db;
use haven_infrastructure::db::repos::{SqliteRepositories, SqliteSettingsUoW};

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
use haven_infrastructure::db::uow::SqliteUnitOfWork;

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

use haven_application::services::library::{LibraryService, MAX_LIMIT};
use haven_application::wire::{LibraryListRequest, LibraryListSort, QueryCategory};
use haven_domain::entities::Progress;
use haven_domain::enums::CompletionState;
use haven_domain::locator::{Locator, VideoLocator};

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
