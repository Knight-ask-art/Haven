//! StorageLocationService 集成测试（BE-STORAGE-001 验收）。
//!
//! 真实 SQLite + tempdir：正常添加 / 重复添加幂等 / 无效路径 / 路径消失（Missing 迁移与恢复）/
//! 重新绑定 / 断开幂等 + 资源标记 / 未知 ID / 非 Connected 拒绝扫描 / 重启恢复 /
//! 移除不删除原文件。

use std::path::Path;
use std::sync::Arc;

use haven_application::services::storage_location::StorageLocationService;
use haven_domain::contracts::{EditionRepository, MediaItemRepository};
use haven_domain::enums::{AvailabilitySource, StorageStatus};
use haven_infrastructure::Db;
use haven_infrastructure::db::repos::SqliteRepositories;
use haven_infrastructure::db::uow::SqliteStorageUoW;
use haven_infrastructure::scanner::LocalLibraryScanner;

fn service(db: &Arc<Db>) -> StorageLocationService {
    StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())))
}

/// R-MAIN-09B：经生产入口扫描——`get_scan_target` 取得带 token 的 target 后 `scan_target`。
async fn scan_via_target(
    svc: &StorageLocationService,
    scanner: &LocalLibraryScanner,
    id: haven_domain::ids::StorageLocationId,
) -> haven_infrastructure::scanner::ScanReport {
    let target = svc.get_scan_target(id).await.unwrap();
    scanner.scan_target(&target).await.unwrap()
}

#[tokio::test]
async fn add_local_creates_connected_location() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);

    let id = svc.add_local("电影库".into(), dir.path()).await.unwrap();
    let list = svc.list().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, id);
    assert_eq!(list[0].status, StorageStatus::Connected);
    assert_eq!(
        list[0].provider_type,
        haven_domain::enums::StorageProviderType::Local
    );
    assert_eq!(list[0].display_name, "电影库");
    let expected_root = std::fs::canonicalize(dir.path())
        .unwrap()
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_owned();
    assert_eq!(list[0].root_ref, expected_root);
}

#[tokio::test]
async fn add_local_same_directory_is_idempotent() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);

    let first = svc.add_local("库".into(), dir.path()).await.unwrap();
    let second = svc
        .add_local("库（重复）".into(), dir.path())
        .await
        .unwrap();
    assert_eq!(first, second, "同一规范化目录必须幂等返回既有 ID");
    assert_eq!(svc.list().await.unwrap().len(), 1, "不得生成重复位置");
}

#[tokio::test]
async fn add_local_rejects_invalid_paths() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);

    // 相对路径拒绝
    let err = svc
        .add_local("库".into(), Path::new("relative/dir"))
        .await
        .unwrap_err();
    assert_eq!(err.code().as_str(), "INVALID_ARGUMENT");
    // 不存在目录拒绝
    let missing = std::env::temp_dir().join(format!("haven-no-such-{}", std::process::id()));
    let err = svc.add_local("库".into(), &missing).await.unwrap_err();
    assert_eq!(err.code().as_str(), "INVALID_ARGUMENT");
    // 文件路径（非目录）拒绝
    let file = tempfile::NamedTempFile::new().unwrap();
    let err = svc.add_local("库".into(), file.path()).await.unwrap_err();
    assert_eq!(err.code().as_str(), "INVALID_ARGUMENT");
    assert_eq!(svc.list().await.unwrap().len(), 0, "全部拒绝，无残留");
}

#[tokio::test]
async fn scan_target_missing_path_migrates_and_recovers() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);
    let id = svc.add_local("库".into(), &root).await.unwrap();

    // 正常扫描目标
    let target = svc.get_scan_target(id).await.unwrap();
    assert_eq!(target.storage_location_id(), id);
    assert_eq!(target.root_path(), root);

    // 路径消失 → RESOURCE_UNAVAILABLE + 状态迁移 Missing
    drop(dir);
    let err = svc.get_scan_target(id).await.unwrap_err();
    assert_eq!(err.code().as_str(), "RESOURCE_UNAVAILABLE");
    let list = svc.list().await.unwrap();
    assert_eq!(
        list[0].status,
        StorageStatus::Missing,
        "目录消失应迁移 Missing"
    );

    // 路径恢复 → 自动回 Connected
    let dir2 = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(&root).unwrap();
    let _ = dir2;
    let target = svc.get_scan_target(id).await.unwrap();
    assert_eq!(target.root_path(), root);
    let list = svc.list().await.unwrap();
    assert_eq!(
        list[0].status,
        StorageStatus::Connected,
        "路径恢复应自动回 Connected"
    );
}

#[tokio::test]
async fn rebind_local_updates_root_and_is_idempotent() {
    let dir_a = tempfile::TempDir::new().unwrap();
    let dir_b = tempfile::TempDir::new().unwrap();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);
    let id = svc.add_local("库".into(), dir_a.path()).await.unwrap();

    // 幂等：相同路径直接成功
    svc.rebind_local(id, dir_a.path()).await.unwrap();

    // 重新绑定到新目录
    svc.rebind_local(id, dir_b.path()).await.unwrap();
    let list = svc.list().await.unwrap();
    let expected_b = std::fs::canonicalize(dir_b.path())
        .unwrap()
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_owned();
    assert_eq!(list[0].root_ref, expected_b);
    assert_eq!(list[0].status, StorageStatus::Connected);
}

#[tokio::test]
async fn disconnect_is_idempotent_and_marks_resources() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);
    let id = svc.add_local("库".into(), dir.path()).await.unwrap();

    svc.disconnect(id).await.unwrap();
    let list = svc.list().await.unwrap();
    assert_eq!(list[0].status, StorageStatus::Disconnected);

    // 幂等：再次断开成功且状态不变
    svc.disconnect(id).await.unwrap();
    assert_eq!(
        svc.list().await.unwrap()[0].status,
        StorageStatus::Disconnected
    );

    // 断开后拒绝扫描（非 Connected）
    let err = svc.get_scan_target(id).await.unwrap_err();
    assert_eq!(err.code().as_str(), "SECURITY_POLICY_DENIED");
}

#[tokio::test]
async fn unknown_id_returns_not_found() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);
    let unknown = haven_domain::ids::StorageLocationId::new();

    assert_eq!(
        svc.get_scan_target(unknown)
            .await
            .unwrap_err()
            .code()
            .as_str(),
        "RESOURCE_NOT_FOUND"
    );
    assert_eq!(
        svc.disconnect(unknown).await.unwrap_err().code().as_str(),
        "RESOURCE_NOT_FOUND"
    );
    assert_eq!(
        svc.remove(unknown).await.unwrap_err().code().as_str(),
        "RESOURCE_NOT_FOUND"
    );
    let tmp = tempfile::TempDir::new().unwrap();
    assert_eq!(
        svc.rebind_local(unknown, tmp.path())
            .await
            .unwrap_err()
            .code()
            .as_str(),
        "RESOURCE_NOT_FOUND"
    );
}

#[tokio::test]
async fn locations_survive_restart() {
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("locations.db");
    let media = tempfile::TempDir::new().unwrap();

    let id = {
        let db = Arc::new(Db::open(&db_path).unwrap());
        let svc = service(&db);
        svc.add_local("库".into(), media.path()).await.unwrap()
    };

    // 重启
    let db2 = Arc::new(Db::open(&db_path).unwrap());
    let svc2 = service(&db2);
    let list = svc2.list().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, id);
    assert_eq!(list[0].status, StorageStatus::Connected, "重启后位置恢复");
}

#[tokio::test]
async fn remove_keeps_user_files_intact() {
    let dir = tempfile::TempDir::new().unwrap();
    let file_path = dir.path().join("movie.mkv");
    std::fs::write(&file_path, b"media-bytes").unwrap();

    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);
    let id = svc.add_local("库".into(), dir.path()).await.unwrap();

    svc.remove(id).await.unwrap();
    assert_eq!(svc.list().await.unwrap().len(), 0, "应用内位置已删除");
    assert!(file_path.exists(), "remove 绝对禁止删除用户原始媒体文件");
    assert!(dir.path().exists(), "remove 绝对禁止删除用户原始媒体目录");
}

/// 建 1 work + 1 edition + 每位置一个 media_item（resource 挂到对应位置），
/// 用于验证「共享 edition/work 在单侧位置移除后保留」。
async fn seed_shared_edition(
    repos: &SqliteRepositories,
    locations: &[haven_domain::ids::StorageLocationId],
) -> (
    haven_domain::ids::WorkId,
    Vec<haven_domain::ids::MediaItemId>,
) {
    use haven_domain::contracts::{
        EditionRepository, MediaItemRepository, ResourceRepository, WorkRepository,
    };
    use haven_domain::entities::{Edition, MediaItem, Resource, ResourceLocator, Work};
    use haven_domain::enums::{
        Availability, AvailabilitySource, MediaItemStatus, MediaType, ResourceType, WorkStatus,
        WorkType,
    };

    let work_id = haven_domain::ids::WorkId::new();
    let edition_id = haven_domain::ids::EditionId::new();
    let now = haven_common::UtcMillis::now();
    repos
        .work
        .save(&Work {
            id: work_id,
            canonical_title: "共享合集".into(),
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
            title: "共享版本".into(),
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
    let mut media_ids = Vec::new();
    for (i, loc) in locations.iter().enumerate() {
        let media_item_id = haven_domain::ids::MediaItemId::new();
        repos
            .media_item
            .save(&MediaItem {
                id: media_item_id,
                edition_id,
                parent_id: None,
                media_type: MediaType::Movie,
                title: format!("第{}条目", i + 1),
                index: haven_domain::entities::MediaIndex::Movie,
                duration_ms: None,
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
            .resource
            .save(&Resource {
                id: haven_domain::ids::ResourceId::new(),
                media_item_id,
                resource_type: ResourceType::LocalFile,
                source_id: None,
                storage_location_id: Some(*loc),
                locator: ResourceLocator::LocalPath {
                    path: format!("D:\\Shared\\item{i}.mkv"),
                },
                mime_type: None,
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
        media_ids.push(media_item_id);
    }
    (work_id, media_ids)
}

/// INTEGRATION-SLICE-001 真机验收（「选错目录」缺口）：remove 必须把**仅由该位置
/// 派生**的内容链与收藏一并清除；另一位置内容与共享 edition/work 完整保留
/// （含共享作品上的收藏）；原始文件不动。
#[tokio::test]
async fn remove_purges_only_location_content_and_user_state() {
    use haven_application::services::favorite::FavoriteService;
    use haven_domain::contracts::{
        FavoriteRepository, MediaItemRepository, ResourceRepository, WorkRepository,
    };

    let dir_a = tempfile::TempDir::new().unwrap();
    std::fs::write(dir_a.path().join("Movie.A.mkv"), b"a-bytes").unwrap();
    let dir_b = tempfile::TempDir::new().unwrap();
    std::fs::write(dir_b.path().join("Movie.B.mkv"), b"b-bytes").unwrap();

    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let svc = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let scanner = LocalLibraryScanner::new(db.clone());
    let id_a = svc.add_local("库A".into(), dir_a.path()).await.unwrap();
    let id_b = svc.add_local("库B".into(), dir_b.path()).await.unwrap();

    // 两位置各扫出一个独立 Work。
    scan_via_target(&svc, &scanner, id_a).await;
    let work_a = repos.work.list(10, 0).await.unwrap().remove(0).id;
    scan_via_target(&svc, &scanner, id_b).await;
    let works: Vec<_> = repos.work.list(10, 0).await.unwrap();
    assert_eq!(works.len(), 2);
    let work_b = works.iter().map(|w| w.id).find(|w| *w != work_a).unwrap();

    // 收藏 A 的作品（生产路径 FavoriteService）。
    let fav_svc = FavoriteService::new(
        repos.clone(),
        Arc::new(haven_infrastructure::db::uow::SqliteUnitOfWork::new(
            db.clone(),
        )),
    );
    fav_svc.set_with_outcome(work_a, true).await.unwrap();
    let favorites_len = || async { repos.favorite.list(100, 0).await.unwrap().len() };
    assert_eq!(favorites_len().await, 1);

    // 移除 A：其派生 Work/edition/media_item/resource 与收藏一并清除；B 完整保留。
    svc.remove(id_a).await.unwrap();
    assert_eq!(svc.list().await.unwrap().len(), 1, "仅剩库B");
    assert!(
        repos.work.get(work_a).await.unwrap().is_none(),
        "A 的 Work 清除"
    );
    assert!(
        repos.work.get(work_b).await.unwrap().is_some(),
        "B 的 Work 保留"
    );
    assert_eq!(favorites_len().await, 0, "A 作品的收藏随之清除");
    let works_after: Vec<_> = repos.work.list(10, 0).await.unwrap();
    assert_eq!(works_after.len(), 1, "仅剩 B 的 Work");
    let b_media = first_media_item_id(&repos, work_b).await;
    let b_resources = repos.resource.list_by_media_item(b_media).await.unwrap();
    assert_eq!(b_resources.len(), 1, "B 的资源链完整保留");
    assert!(dir_a.path().join("Movie.A.mkv").exists(), "原始文件不动");

    // 共享结构：同一 edition 两个 media_item 分属两个位置；收藏共享 Work。
    let dir_c = tempfile::TempDir::new().unwrap();
    let dir_d = tempfile::TempDir::new().unwrap();
    let id_c = svc.add_local("库C".into(), dir_c.path()).await.unwrap();
    let id_d = svc.add_local("库D".into(), dir_d.path()).await.unwrap();
    let (shared_work, shared_media) = seed_shared_edition(&repos, &[id_c, id_d]).await;
    fav_svc.set_with_outcome(shared_work, true).await.unwrap();
    assert_eq!(favorites_len().await, 1);

    // 移除 C：仅 C 的 media_item 清除；edition/work（仍有 D 的条目）与收藏保留。
    svc.remove(id_c).await.unwrap();
    let remaining: Vec<_> = repos
        .media_item
        .list_by_edition(
            repos
                .edition
                .list_by_work(shared_work)
                .await
                .unwrap()
                .remove(0)
                .id,
        )
        .await
        .unwrap();
    assert_eq!(remaining.len(), 1, "仅剩 D 的 media_item");
    assert_eq!(remaining[0].id, shared_media[1], "C 的条目清除、D 的保留");
    assert!(
        repos.work.get(shared_work).await.unwrap().is_some(),
        "共享 Work 保留"
    );
    assert_eq!(favorites_len().await, 1, "共享 Work 上的收藏保留");
}

/// 通过公共 Repository 接口建 work→edition→media_item→resource 链并挂到指定位置。
/// `source` 决定 seed 资源的 availability_source（None → Unknown，模拟迁移前数据）。
/// `locator_root`：LocalPath 定位的根（真实 root 内路径）；None → 使用固定假路径
/// （仅用于不涉及 rebase 的边界测试；rebind 测试必须传真实 root）。
async fn seed_resource_chain_with_source(
    repos: &SqliteRepositories,
    storage_id: haven_domain::ids::StorageLocationId,
    source: Option<haven_domain::enums::AvailabilitySource>,
    locator_root: Option<&std::path::Path>,
) -> haven_domain::ids::WorkId {
    use haven_domain::contracts::{
        EditionRepository, MediaItemRepository, ResourceRepository, WorkRepository,
    };
    use haven_domain::entities::{Edition, MediaItem, Resource, ResourceLocator, Work};
    use haven_domain::enums::{
        Availability, AvailabilitySource, MediaItemStatus, MediaType, ResourceType, WorkStatus,
        WorkType,
    };

    let work_id = haven_domain::ids::WorkId::new();
    let edition_id = haven_domain::ids::EditionId::new();
    let media_item_id = haven_domain::ids::MediaItemId::new();
    let resource_id = haven_domain::ids::ResourceId::new();
    let now = haven_common::UtcMillis::now();
    let locator_path = match locator_root {
        Some(root) => root.join("test.mkv").to_string_lossy().replace('\\', "/"),
        None => "D:\\Movies\\test.mkv".into(),
    };

    repos
        .work
        .save(&Work {
            id: work_id,
            canonical_title: "断开标记测试".into(),
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
            title: "测试电影".into(),
            index: haven_domain::entities::MediaIndex::Movie,
            duration_ms: None,
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
        .resource
        .save(&Resource {
            id: resource_id,
            media_item_id,
            resource_type: ResourceType::LocalFile,
            source_id: None,
            storage_location_id: Some(storage_id),
            locator: ResourceLocator::LocalPath { path: locator_path },
            mime_type: None,
            size: None,
            hash: None,
            availability: Availability::Available,
            availability_source: source.unwrap_or(AvailabilitySource::Unknown),
            modified_ms: None,
            fingerprint_first: None,
            fingerprint_last: None,
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();
    work_id
}

/// seed 默认：Unknown 来源（模拟迁移前数据，位置级操作可标记）。
async fn seed_resource_chain(
    repos: &SqliteRepositories,
    storage_id: haven_domain::ids::StorageLocationId,
) -> haven_domain::ids::WorkId {
    seed_resource_chain_with_source(repos, storage_id, None, None).await
}

/// 取链上第一个 media_item_id（辅助）。
async fn first_media_item_id(
    repos: &SqliteRepositories,
    work_id: haven_domain::ids::WorkId,
) -> haven_domain::ids::MediaItemId {
    use haven_domain::contracts::{EditionRepository, MediaItemRepository, WorkRepository};
    let work = repos.work.get(work_id).await.unwrap().unwrap();
    let edition = repos.edition.list_by_work(work.id).await.unwrap().remove(0);
    repos
        .media_item
        .list_by_edition(edition.id)
        .await
        .unwrap()
        .remove(0)
        .id
}

#[tokio::test]
async fn disconnect_marks_resources_unavailable_but_keeps_user_data() {
    use haven_domain::contracts::{ResourceRepository, WorkRepository};

    let dir = tempfile::TempDir::new().unwrap();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let svc = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let id = svc.add_local("库".into(), dir.path()).await.unwrap();
    let work_id = seed_resource_chain(&repos, id).await;

    svc.disconnect(id).await.unwrap();

    // 相关 Resource 标记不可用
    let media_item_id = first_media_item_id(&repos, work_id).await;
    let resources = repos
        .resource
        .list_by_media_item(media_item_id)
        .await
        .unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(
        resources[0].availability,
        haven_domain::enums::Availability::StorageUnavailable,
        "断开后资源必须标记 StorageUnavailable"
    );

    // 用户数据（Work/Edition/MediaItem）保留
    assert!(
        repos.work.get(work_id).await.unwrap().is_some(),
        "Work 必须保留"
    );
    let works: Vec<_> = repos.work.list(10, 0).await.unwrap();
    assert_eq!(works.len(), 1, "Work 数量不变");

    // 断开后 remove：位置派生内容与资源索引一并清除（INTEGRATION-SLICE-001 真机验收
    // 「选错目录」缺口：仅删资源会留孤儿 Work，媒体库仍显示已移除位置的内容）。
    svc.remove(id).await.unwrap();
    let after = repos
        .resource
        .list_by_media_item(media_item_id)
        .await
        .unwrap();
    assert!(after.is_empty(), "remove 删除位置资源索引");
    assert!(
        repos.work.get(work_id).await.unwrap().is_none(),
        "仅由该位置派生的 Work 随 remove 一并清除"
    );
}

// ---- 审阅修复测试（P0-5 事务回滚 / P1-6 排己 / P1-7 UNC 拒绝 + Missing 资源标记）----

use haven_application::services::storage_location::{StorageLocationUoW, StorageTxPorts};
use haven_common::AppError;
use haven_domain::enums::Availability;

#[tokio::test]
async fn unc_paths_are_rejected() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);
    // UNC 形式（verbatim 与直接形式）
    for unc in [
        r"\\?\UNC\server\share",
        r"\\server\share",
        r"\\?\UNC\server\share\media",
    ] {
        let err = svc
            .add_local("库".into(), Path::new(unc))
            .await
            .unwrap_err();
        assert_eq!(err.code().as_str(), "INVALID_ARGUMENT", "应拒绝 UNC: {unc}");
    }
    assert_eq!(svc.list().await.unwrap().len(), 0);
}

#[tokio::test]
async fn rebind_to_path_owned_by_other_location_is_rejected() {
    let dir_a = tempfile::TempDir::new().unwrap();
    let dir_b = tempfile::TempDir::new().unwrap();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);
    let id_a = svc.add_local("库A".into(), dir_a.path()).await.unwrap();
    let id_b = svc.add_local("库B".into(), dir_b.path()).await.unwrap();

    // B 重绑到 A 的路径 → 拒绝（排己检查）
    let err = svc.rebind_local(id_b, dir_a.path()).await.unwrap_err();
    assert_eq!(err.code().as_str(), "INVALID_ARGUMENT");

    // A 自己重绑到自己路径 → 幂等成功
    svc.rebind_local(id_a, dir_a.path()).await.unwrap();
    assert_eq!(svc.list().await.unwrap().len(), 2, "无重复位置");
    let _ = id_a;
    let _ = id_b;
}

/// 失败注入 UoW：set_resources_availability 必然失败（验证 disconnect 回滚）。
struct FailingResourceUoW {
    inner: SqliteStorageUoW,
}

impl StorageLocationUoW for FailingResourceUoW {
    fn run(&self, f: &dyn Fn(&dyn StorageTxPorts) -> Result<(), AppError>) -> Result<(), AppError> {
        self.inner.run(&|tx| {
            struct FailOnce<'a>(&'a dyn StorageTxPorts);
            impl StorageTxPorts for FailOnce<'_> {
                fn load_location(
                    &self,
                    id: haven_domain::ids::StorageLocationId,
                ) -> Result<Option<haven_domain::entities::StorageLocation>, AppError>
                {
                    self.0.load_location(id)
                }
                fn load_all(
                    &self,
                ) -> Result<Vec<haven_domain::entities::StorageLocation>, AppError>
                {
                    self.0.load_all()
                }
                fn save_location(
                    &self,
                    location: &haven_domain::entities::StorageLocation,
                ) -> Result<(), AppError> {
                    self.0.save_location(location)
                }
                fn set_resources_availability(
                    &self,
                    _id: haven_domain::ids::StorageLocationId,
                    _a: haven_domain::enums::Availability,
                    _source: haven_domain::enums::AvailabilitySource,
                ) -> Result<(), AppError> {
                    Err(AppError::new(
                        "DATABASE_ERROR",
                        haven_common::ErrorKind::Database,
                        "模拟资源标记失败",
                        true,
                    ))
                }
                fn load_resources(
                    &self,
                    id: haven_domain::ids::StorageLocationId,
                ) -> Result<Vec<haven_domain::entities::Resource>, AppError> {
                    self.0.load_resources(id)
                }
                fn save_resource(
                    &self,
                    resource: &haven_domain::entities::Resource,
                ) -> Result<(), AppError> {
                    self.0.save_resource(resource)
                }
                fn delete_resources(
                    &self,
                    id: haven_domain::ids::StorageLocationId,
                ) -> Result<(), AppError> {
                    self.0.delete_resources(id)
                }
                fn purge_location_content(
                    &self,
                    id: haven_domain::ids::StorageLocationId,
                ) -> Result<(), AppError> {
                    self.0.purge_location_content(id)
                }
                fn delete_location(
                    &self,
                    id: haven_domain::ids::StorageLocationId,
                ) -> Result<bool, AppError> {
                    self.0.delete_location(id)
                }
            }
            f(&FailOnce(tx))
        })
    }
    fn read_location(
        &self,
        id: haven_domain::ids::StorageLocationId,
    ) -> Result<Option<haven_domain::entities::StorageLocation>, AppError> {
        self.inner.read_location(id)
    }
}

#[tokio::test]
async fn disconnect_failure_rolls_back_location_status() {
    use haven_domain::contracts::StorageLocationRepository;
    let dir = tempfile::TempDir::new().unwrap();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let svc = StorageLocationService::new(Arc::new(FailingResourceUoW {
        inner: SqliteStorageUoW::new(db.clone()),
    }));
    let id = svc.add_local("库".into(), dir.path()).await.unwrap();

    // Resource 标记失败 → 整个事务回滚 → 位置保持 Connected（重试不会漏副作用）
    let err = svc.disconnect(id).await.unwrap_err();
    assert_eq!(err.code().as_str(), "DATABASE_ERROR");
    let loc = repos.storage_location.get(id).await.unwrap().unwrap();
    assert_eq!(
        loc.status,
        haven_domain::enums::StorageStatus::Connected,
        "失败必须回滚，位置不得半提交 Disconnected"
    );

    // 修复后重试成功（幂等路径不再误判）
    let svc2 = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    svc2.disconnect(id).await.unwrap();
    let loc = repos.storage_location.get(id).await.unwrap().unwrap();
    assert_eq!(loc.status, haven_domain::enums::StorageStatus::Disconnected);
}

/// R-MAIN-08 测试 3：路径消失 → 资源 Missing/storage；
/// **只重建空 root 并 get_scan_target：Location 恢复 Connected，但 Resource 仍 Missing**（空目录不得复活）；
/// 恢复原文件并跑真实 Scanner 后才 Available/user。
#[tokio::test]
async fn empty_root_recovery_keeps_resources_missing_until_rescanned() {
    use haven_domain::contracts::{ResourceRepository, WorkRepository};

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let file = root.join("Movie.A.mkv");
    std::fs::write(&file, b"fake-video-bytes").unwrap();

    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let svc = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let scanner = LocalLibraryScanner::new(db.clone());
    let id = svc.add_local("库".into(), &root).await.unwrap();

    // 真实 Scanner 建立 Available/user 资源。
    scan_via_target(&svc, &scanner, id).await;
    let works = repos.work.list(10, 0).await.unwrap();
    assert_eq!(works.len(), 1);
    let resource = {
        let edition = repos
            .edition
            .list_by_work(works[0].id)
            .await
            .unwrap()
            .remove(0);
        let media_item = repos
            .media_item
            .list_by_edition(edition.id)
            .await
            .unwrap()
            .remove(0);
        repos
            .resource
            .list_by_media_item(media_item.id)
            .await
            .unwrap()
            .remove(0)
    };
    assert_eq!(
        resource.availability_source,
        AvailabilitySource::User,
        "Scanner 新资源必须 user 来源"
    );

    // 路径消失 → 位置 Missing + 资源 Missing/storage。
    drop(dir);
    let err = svc.get_scan_target(id).await.unwrap_err();
    assert_eq!(err.code().as_str(), "RESOURCE_UNAVAILABLE");
    let media_item_id = first_media_item_id(&repos, works[0].id).await;
    let resources = repos
        .resource
        .list_by_media_item(media_item_id)
        .await
        .unwrap();
    assert_eq!(
        resources[0].availability,
        Availability::Missing,
        "路径消失后资源必须同步 Missing"
    );
    assert_eq!(
        resources[0].availability_source,
        AvailabilitySource::Storage,
        "位置级标记后来源归位 storage"
    );

    // 只重建**空** root → Location 恢复 Connected，但 Resource 仍 Missing（不复活不存在的文件）。
    std::fs::create_dir_all(&root).unwrap();
    let target = svc.get_scan_target(id).await.unwrap();
    assert_eq!(target.storage_location_id(), id);
    let list = svc.list().await.unwrap();
    assert_eq!(
        list[0].status,
        StorageStatus::Connected,
        "空 root 应恢复 Connected"
    );
    let resources = repos
        .resource
        .list_by_media_item(media_item_id)
        .await
        .unwrap();
    assert_eq!(
        resources[0].availability,
        Availability::Missing,
        "空目录不得虚假复活文件"
    );
    assert_eq!(
        resources[0].availability_source,
        AvailabilitySource::Storage
    );

    // 恢复原文件 + 真实 Scanner 逐项验证 → Available/user。
    std::fs::write(&file, b"fake-video-bytes").unwrap();
    let report = scan_via_target(&svc, &scanner, id).await;
    assert_eq!(report.new, 0, "恢复扫描不得新增实体");
    assert!(
        report.updated >= 1,
        "恢复的资源必须被 Scanner 恢复为 updated"
    );
    let resources = repos
        .resource
        .list_by_media_item(media_item_id)
        .await
        .unwrap();
    assert_eq!(
        resources[0].availability,
        Availability::Available,
        "Scanner 逐项验证后必须恢复 Available"
    );
    assert_eq!(resources[0].availability_source, AvailabilitySource::User);
}

/// R-MAIN-08 测试 2：user 来源的 **非 Available 显式标记**（SourceUnavailable /
/// TemporarilyUnavailable / Unknown / 资源自身 Missing）经过 disconnect / path-missing /
/// recovery / rebind 全部保持原状态（覆盖规则 a/b 禁止触碰）。
#[tokio::test]
async fn user_explicit_unavailable_resources_survive_all_location_ops() {
    use haven_domain::contracts::ResourceRepository;
    use haven_domain::enums::{Availability, AvailabilitySource};

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let svc = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let id = svc.add_local("库".into(), &root).await.unwrap();

    // 四种 user 显式状态分别建资源（locator 在 root 内，rebind 可安全 rebase）。
    let states = [
        Availability::SourceUnavailable,
        Availability::TemporarilyUnavailable,
        Availability::Unknown,
        Availability::Missing,
    ];
    let mut probes = Vec::new();
    for state in states {
        let work_id = seed_resource_chain_with_source(
            &repos,
            id,
            Some(AvailabilitySource::User),
            Some(&root),
        )
        .await;
        let item = first_media_item_id(&repos, work_id).await;
        let mut r = repos
            .resource
            .list_by_media_item(item)
            .await
            .unwrap()
            .remove(0);
        r.availability = state;
        r.availability_source = AvailabilitySource::User;
        repos.resource.save(&r).await.unwrap();
        probes.push((item, state));
    }
    let current = || async {
        let mut out = Vec::new();
        for (item, _) in &probes {
            let r = repos
                .resource
                .list_by_media_item(*item)
                .await
                .unwrap()
                .remove(0);
            out.push(r);
        }
        out
    };

    // ① disconnect：全保持
    svc.disconnect(id).await.unwrap();
    for (r, (_, state)) in current().await.into_iter().zip(probes.iter()) {
        assert_eq!(r.availability, *state, "disconnect 不得覆盖 user 显式标记");
        assert_eq!(r.availability_source, AvailabilitySource::User);
    }

    // ② rebind（同路径，Disconnected → 重连）：保持
    svc.rebind_local(id, &root).await.unwrap();
    let list = svc.list().await.unwrap();
    assert_eq!(
        list[0].status,
        StorageStatus::Connected,
        "同路径 rebind 重连"
    );
    for (r, (_, state)) in current().await.into_iter().zip(probes.iter()) {
        assert_eq!(r.availability, *state, "rebind 重连不得覆盖 user 显式标记");
    }

    // ③ rebind（新路径）：保持
    let dir_b = tempfile::TempDir::new().unwrap();
    let path_b = dir_b.path().to_path_buf();
    svc.rebind_local(id, &path_b).await.unwrap();
    for (r, (_, state)) in current().await.into_iter().zip(probes.iter()) {
        assert_eq!(
            r.availability, *state,
            "rebind 新路径不得覆盖 user 显式标记"
        );
    }

    // ④ path-missing（root 消失）→ recovery：保持
    drop(dir_b);
    let err = svc.get_scan_target(id).await.unwrap_err();
    assert_eq!(err.code().as_str(), "RESOURCE_UNAVAILABLE");
    std::fs::create_dir_all(&path_b).unwrap();
    let _ = svc.get_scan_target(id).await.unwrap();
    for (r, (_, state)) in current().await.into_iter().zip(probes.iter()) {
        assert_eq!(
            r.availability, *state,
            "path-missing/recovery 不得覆盖 user 显式标记"
        );
    }
}

/// R-MAIN-08：rebind 换路径后，unknown/storage 来源资源被无效化（Missing/storage，等待重扫）；
/// user 显式标记（非 Available）不受影响；locator 按相对路径 rebase 到新 root。
#[tokio::test]
async fn rebind_invalidates_storage_resources_for_rescan() {
    use haven_domain::contracts::ResourceRepository;
    use haven_domain::enums::{Availability, AvailabilitySource};

    let dir_a = tempfile::TempDir::new().unwrap();
    let dir_b = tempfile::TempDir::new().unwrap();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let svc = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let id = svc.add_local("库".into(), dir_a.path()).await.unwrap();
    // unknown 来源（规则 b 可覆盖）+ user 显式 SourceUnavailable（不可覆盖），locator 均在旧 root 内。
    let work_id = seed_resource_chain_with_source(&repos, id, None, Some(dir_a.path())).await;
    let work_user = seed_resource_chain_with_source(
        &repos,
        id,
        Some(AvailabilitySource::User),
        Some(dir_a.path()),
    )
    .await;
    let user_item = first_media_item_id(&repos, work_user).await;
    let mut user_resource = repos
        .resource
        .list_by_media_item(user_item)
        .await
        .unwrap()
        .remove(0);
    user_resource.availability = Availability::SourceUnavailable;
    repos.resource.save(&user_resource).await.unwrap();

    svc.rebind_local(id, dir_b.path()).await.unwrap();

    let media_item_id = first_media_item_id(&repos, work_id).await;
    let resources = repos
        .resource
        .list_by_media_item(media_item_id)
        .await
        .unwrap();
    assert_eq!(
        resources[0].availability,
        Availability::Missing,
        "rebind 后 storage/unknown 资源必须无效化（等待重扫）"
    );
    assert_eq!(
        resources[0].availability_source,
        AvailabilitySource::Storage,
        "无效化后来源归位 storage"
    );

    // user 显式标记不被无效化
    let user_resources = repos.resource.list_by_media_item(user_item).await.unwrap();
    assert_eq!(
        user_resources[0].availability,
        Availability::SourceUnavailable,
        "rebind 不得覆盖 user 显式标记"
    );
}

/// R-MAIN-08：路径恢复只把 Location 回 Connected，**绝不批量恢复 Resource**——
/// storage 来源资源保持 Missing（由 Scanner 逐项恢复），user 显式标记保持。
#[tokio::test]
async fn restore_does_not_resurrect_user_marked_unavailable() {
    use haven_domain::contracts::ResourceRepository;
    use haven_domain::enums::{Availability, AvailabilitySource};

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let svc = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let id = svc.add_local("库".into(), &root).await.unwrap();

    // storage 来源资源（locator 在 root 内）+ user 显式 SourceUnavailable 资源。
    let work_storage = seed_resource_chain(&repos, id).await;
    let work_user =
        seed_resource_chain_with_source(&repos, id, Some(AvailabilitySource::User), Some(&root))
            .await;
    let user_item = first_media_item_id(&repos, work_user).await;
    {
        let mut r = repos
            .resource
            .list_by_media_item(user_item)
            .await
            .unwrap()
            .remove(0);
        r.availability = Availability::SourceUnavailable;
        r.availability_source = AvailabilitySource::User;
        repos.resource.save(&r).await.unwrap();
    }

    // 路径消失 → 恢复：Location Connected，但**两个资源都不被批量恢复**。
    drop(dir);
    let _ = svc.get_scan_target(id).await.unwrap_err();
    std::fs::create_dir_all(&root).unwrap();
    let _ = svc.get_scan_target(id).await.unwrap();

    let user_resources = repos.resource.list_by_media_item(user_item).await.unwrap();
    assert_eq!(
        user_resources[0].availability,
        Availability::SourceUnavailable,
        "恢复不得复活 user 显式标记的 SourceUnavailable"
    );

    let storage_item = first_media_item_id(&repos, work_storage).await;
    let storage_resources = repos
        .resource
        .list_by_media_item(storage_item)
        .await
        .unwrap();
    assert_eq!(
        storage_resources[0].availability,
        Availability::Missing,
        "恢复绝不批量恢复 Resource（须 Scanner 逐项验证）"
    );
}

/// R-MAIN-08 测试 1：**真实 Scanner** 创建的 Available/user 资源，disconnect 后
/// 变为 StorageUnavailable/storage（覆盖规则 a：位置不可达时有效可用性必须失效）。
#[tokio::test]
async fn scanner_resources_are_marked_unavailable_on_disconnect() {
    use haven_domain::contracts::{ResourceRepository, WorkRepository};

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::write(root.join("Movie.A.mkv"), b"fake-video-bytes").unwrap();

    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let svc = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let scanner = LocalLibraryScanner::new(db.clone());
    let id = svc.add_local("库".into(), &root).await.unwrap();

    scan_via_target(&svc, &scanner, id).await;
    let works = repos.work.list(10, 0).await.unwrap();
    assert_eq!(works.len(), 1);
    let edition = repos
        .edition
        .list_by_work(works[0].id)
        .await
        .unwrap()
        .remove(0);
    let media_item = repos
        .media_item
        .list_by_edition(edition.id)
        .await
        .unwrap()
        .remove(0);
    let resources = repos
        .resource
        .list_by_media_item(media_item.id)
        .await
        .unwrap();
    assert_eq!(resources[0].availability, Availability::Available);
    assert_eq!(resources[0].availability_source, AvailabilitySource::User);

    svc.disconnect(id).await.unwrap();

    let resources = repos
        .resource
        .list_by_media_item(media_item.id)
        .await
        .unwrap();
    assert_eq!(
        resources[0].availability,
        Availability::StorageUnavailable,
        "Scanner 创建的 Available/user 资源在 disconnect 后必须失效"
    );
    assert_eq!(
        resources[0].availability_source,
        AvailabilitySource::Storage,
        "失效后来源归位 storage"
    );
}

/// R-MAIN-08 测试 4：**Disconnected + 同路径 rebind** → Location 重新 Connected；
/// get_scan_target 可返回；**Resource 不被虚假恢复**（保持 storage overlay，等 Scanner）。
#[tokio::test]
async fn disconnected_same_path_rebind_reconnects_without_resurrection() {
    use haven_domain::contracts::{ResourceRepository, WorkRepository};

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::write(root.join("Movie.A.mkv"), b"fake-video-bytes").unwrap();

    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let svc = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let scanner = LocalLibraryScanner::new(db.clone());
    let id = svc.add_local("库".into(), &root).await.unwrap();
    scan_via_target(&svc, &scanner, id).await;

    svc.disconnect(id).await.unwrap();
    let works = repos.work.list(10, 0).await.unwrap();
    let edition = repos
        .edition
        .list_by_work(works[0].id)
        .await
        .unwrap()
        .remove(0);
    let media_item = repos
        .media_item
        .list_by_edition(edition.id)
        .await
        .unwrap()
        .remove(0);
    let resources = || async {
        repos
            .resource
            .list_by_media_item(media_item.id)
            .await
            .unwrap()
            .remove(0)
    };
    assert_eq!(
        resources().await.availability,
        Availability::StorageUnavailable
    );

    // Disconnected + 同路径 rebind → 重新 Connected；资源保持 overlay。
    svc.rebind_local(id, &root).await.unwrap();
    let list = svc.list().await.unwrap();
    assert_eq!(
        list[0].status,
        StorageStatus::Connected,
        "同路径 rebind 必须重连"
    );
    let resource = resources().await;
    assert_eq!(
        resource.availability,
        Availability::StorageUnavailable,
        "重连不得虚假恢复资源"
    );
    assert_eq!(resource.availability_source, AvailabilitySource::Storage);

    // get_scan_target 可返回（Connected + 路径可达）。
    let target = svc.get_scan_target(id).await.unwrap();
    assert_eq!(target.storage_location_id(), id);

    // Scanner 逐项验证后才恢复 Available/user。
    let report = scan_via_target(&svc, &scanner, id).await;
    assert_eq!(report.new, 0, "重连后扫描不得新增实体");
    let resource = resources().await;
    assert_eq!(
        resource.availability,
        Availability::Available,
        "Scanner 恢复 Available"
    );
    assert_eq!(resource.availability_source, AvailabilitySource::User);
}

/// R-MAIN-08 测试 5：**new-root rebind**——LocalPath 按相对路径 rebase 到新 root，
/// ResourceId/WorkId 不变；把文件移到新 root 后扫描新 root 匹配**旧 Resource**并恢复
/// Available/user，**不新增 Work/Resource**。
#[tokio::test]
async fn rebind_new_root_rebases_and_scanner_recovers_without_duplicates() {
    use haven_domain::contracts::{ResourceRepository, WorkRepository};

    let dir_a = tempfile::TempDir::new().unwrap();
    let root_a = dir_a.path().to_path_buf();
    let dir_b = tempfile::TempDir::new().unwrap();
    let root_b = dir_b.path().to_path_buf();
    std::fs::write(root_a.join("Movie.A.mkv"), b"fake-video-bytes").unwrap();

    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let svc = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let scanner = LocalLibraryScanner::new(db.clone());
    let id = svc.add_local("库".into(), &root_a).await.unwrap();
    scan_via_target(&svc, &scanner, id).await;

    // 记录 rebind 前的身份（ResourceId/WorkId/MediaItemId）。
    let before_works = repos.work.list(10, 0).await.unwrap();
    let before_work = repos.work.get(before_works[0].id).await.unwrap().unwrap();
    let before_edition = repos
        .edition
        .list_by_work(before_work.id)
        .await
        .unwrap()
        .remove(0);
    let before_media = repos
        .media_item
        .list_by_edition(before_edition.id)
        .await
        .unwrap()
        .remove(0);
    let before_resources = repos
        .resource
        .list_by_media_item(before_media.id)
        .await
        .unwrap();
    let before_resource_id = before_resources[0].id;

    // new-root rebind：locator 按相对路径 rebase 到新 root + 资源无效化 Missing/storage。
    svc.rebind_local(id, &root_b).await.unwrap();

    let list = svc.list().await.unwrap();
    assert_eq!(list[0].status, StorageStatus::Connected);
    assert_eq!(
        list[0].root_ref,
        std::fs::canonicalize(&root_b)
            .unwrap()
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
    );

    // 身份不变；locator 已 rebase 到新 root。
    let after_works = repos.work.list(10, 0).await.unwrap();
    assert_eq!(after_works.len(), 1, "rebase 不得新建 Work");
    assert_eq!(after_works[0].id, before_work.id, "WorkId 必须保持");
    let after_edition = repos
        .edition
        .list_by_work(before_work.id)
        .await
        .unwrap()
        .remove(0);
    let after_media = repos
        .media_item
        .list_by_edition(after_edition.id)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(after_media.id, before_media.id, "MediaItemId 必须保持");
    let after_resources = repos
        .resource
        .list_by_media_item(after_media.id)
        .await
        .unwrap();
    assert_eq!(after_resources.len(), 1);
    assert_eq!(
        after_resources[0].id, before_resource_id,
        "ResourceId 必须保持"
    );
    match &after_resources[0].locator {
        haven_domain::entities::ResourceLocator::LocalPath { path } => {
            let expected = root_b
                .join("Movie.A.mkv")
                .to_string_lossy()
                .replace('\\', "/");
            assert_eq!(*path, expected, "locator 必须按相对路径 rebase 到新 root");
        }
        _ => panic!("必须是 LocalPath"),
    }
    assert_eq!(
        after_resources[0].availability,
        Availability::Missing,
        "rebind 后资源必须无效化（等待 Scanner）"
    );
    assert_eq!(
        after_resources[0].availability_source,
        AvailabilitySource::Storage
    );

    // 把文件移到新 root，Scanner 扫新 root → 匹配旧 Resource 恢复，不新增 Work/Resource。
    std::fs::rename(root_a.join("Movie.A.mkv"), root_b.join("Movie.A.mkv")).unwrap();
    let report = scan_via_target(&svc, &scanner, id).await;
    assert_eq!(report.new, 0, "扫描新 root 不得新建 Work/Resource");
    assert!(report.updated >= 1, "旧 Resource 必须被恢复（updated）");

    let works = repos.work.list(10, 0).await.unwrap();
    assert_eq!(works.len(), 1, "不得重复建 Work");
    let resources = repos
        .resource
        .list_by_media_item(after_media.id)
        .await
        .unwrap();
    assert_eq!(resources.len(), 1, "不得重复建 Resource");
    assert_eq!(resources[0].id, before_resource_id, "ResourceId 保持");
    assert_eq!(
        resources[0].availability,
        Availability::Available,
        "Scanner 恢复 Available"
    );
    assert_eq!(resources[0].availability_source, AvailabilitySource::User);
}

/// R-MAIN-08 测试 6：rebase 遇到**非法/越界 locator**（不在旧 root 内）→ 整个事务回滚：
/// root_ref / status / resource locators 均不半更新。
#[tokio::test]
async fn rebind_rebase_rolls_back_on_out_of_root_locator() {
    use haven_domain::contracts::ResourceRepository;

    let dir_a = tempfile::TempDir::new().unwrap();
    let root_a = dir_a.path().to_path_buf();
    let dir_b = tempfile::TempDir::new().unwrap();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let svc = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let id = svc.add_local("库".into(), &root_a).await.unwrap();
    // 越界 locator（D:\Movies\test.mkv 不在 root_a 内）——不允许 rebase。
    let work_id = seed_resource_chain(&repos, id).await;
    let media_item_id = first_media_item_id(&repos, work_id).await;
    let before = repos
        .resource
        .list_by_media_item(media_item_id)
        .await
        .unwrap()
        .remove(0);

    // rebind 必须整体失败并回滚。
    let err = svc.rebind_local(id, dir_b.path()).await.unwrap_err();
    assert_eq!(
        err.code().as_str(),
        "INVALID_ARGUMENT",
        "越界 locator 必须拒绝"
    );
    assert!(
        err.user_message().contains("rebase"),
        "错误须说明 rebase 失败"
    );

    // 无半更新：root_ref / status / locators 全部保持。
    let list = svc.list().await.unwrap();
    assert_eq!(
        list[0].root_ref,
        root_a.to_string_lossy().trim_start_matches(r"\\?\"),
        "root_ref 不得被修改"
    );
    assert_eq!(
        list[0].status,
        StorageStatus::Connected,
        "status 不得被修改"
    );
    let after = repos
        .resource
        .list_by_media_item(media_item_id)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(after.id, before.id, "ResourceId 保持");
    assert_eq!(after.locator, before.locator, "locator 不得被半更新");
    assert_eq!(
        after.availability, before.availability,
        "availability 不得被修改"
    );
}

/// R-MAIN-08A 阻塞 1：**`old_root/../outside/file`（含 ParentDir）必须拒绝**——
/// 不得把越界路径返回为 relative 并 join 出新 root。
#[tokio::test]
async fn rebind_rejects_parent_dir_escape_locator() {
    use haven_domain::contracts::ResourceRepository;
    use haven_domain::entities::ResourceLocator;

    let dir_a = tempfile::TempDir::new().unwrap();
    let root_a = dir_a.path().to_path_buf();
    let dir_b = tempfile::TempDir::new().unwrap();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let svc = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let id = svc.add_local("库".into(), &root_a).await.unwrap();
    let work_id = seed_resource_chain_with_source(&repos, id, None, Some(&root_a)).await;
    let media_item_id = first_media_item_id(&repos, work_id).await;
    let before = repos
        .resource
        .list_by_media_item(media_item_id)
        .await
        .unwrap()
        .remove(0);

    // 篡改 locator 为 old_root/../outside/file（ParentDir 越界）。
    let escape = root_a
        .join("..")
        .join("outside")
        .join("file.mkv")
        .to_string_lossy()
        .replace('\\', "/");
    let mut malicious = before.clone();
    malicious.locator = ResourceLocator::LocalPath { path: escape };
    repos.resource.save(&malicious).await.unwrap();

    let err = svc.rebind_local(id, dir_b.path()).await.unwrap_err();
    assert_eq!(
        err.code().as_str(),
        "INVALID_ARGUMENT",
        "ParentDir 越界必须拒绝"
    );
    assert!(err.user_message().contains("rebase"));

    // 无半更新：root_ref/status/locator 保持。
    let list = svc.list().await.unwrap();
    assert_eq!(
        list[0].root_ref,
        root_a.to_string_lossy().trim_start_matches(r"\\?\"),
        "root_ref 不得被修改"
    );
    let after = repos
        .resource
        .list_by_media_item(media_item_id)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(
        after.locator, malicious.locator,
        "非法 locator 保持（回滚）"
    );
}

/// R-MAIN-08A 阻塞 1：**locator 恰好等于 old_root（LocalFile 指向目录）必须拒绝**（空 relative）。
#[tokio::test]
async fn rebind_rejects_root_itself_as_locator() {
    use haven_domain::contracts::ResourceRepository;
    use haven_domain::entities::ResourceLocator;

    let dir_a = tempfile::TempDir::new().unwrap();
    let root_a = dir_a.path().to_path_buf();
    let dir_b = tempfile::TempDir::new().unwrap();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let svc = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let id = svc.add_local("库".into(), &root_a).await.unwrap();
    let work_id = seed_resource_chain_with_source(&repos, id, None, Some(&root_a)).await;
    let media_item_id = first_media_item_id(&repos, work_id).await;

    let root_as_file = root_a.to_string_lossy().replace('\\', "/");
    let mut malicious = repos
        .resource
        .list_by_media_item(media_item_id)
        .await
        .unwrap()
        .remove(0);
    malicious.locator = ResourceLocator::LocalPath { path: root_as_file };
    repos.resource.save(&malicious).await.unwrap();

    let err = svc.rebind_local(id, dir_b.path()).await.unwrap_err();
    assert_eq!(
        err.code().as_str(),
        "INVALID_ARGUMENT",
        "root 自身作为 locator（空 relative）必须拒绝"
    );
    assert!(err.user_message().contains("rebase"));
    let list = svc.list().await.unwrap();
    assert_eq!(
        list[0].status,
        StorageStatus::Connected,
        "status 不得被修改"
    );
}

/// R-MAIN-08A 阻塞 1：**混合资源回滚**——至少两个 LocalPath：
/// 较小 ID 资源 locator 有效（rebind 闭包内会先执行 save），较大 ID 资源含 ParentDir 非法；
/// rebind 失败后**已保存的第一个 locator 也必须回滚**（非法 locator / root_ref / status /
/// availability 均不变）。依赖 `load_resources ORDER BY id` 的确定性顺序。
#[tokio::test]
async fn rebind_mixed_rebase_rolls_back_first_save() {
    use haven_domain::contracts::ResourceRepository;
    use haven_domain::entities::ResourceLocator;

    let dir_a = tempfile::TempDir::new().unwrap();
    let root_a = dir_a.path().to_path_buf();
    let dir_b = tempfile::TempDir::new().unwrap();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let svc = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let id = svc.add_local("库".into(), &root_a).await.unwrap();

    // 两个资源（各自 Work 链），locator 均初始在 root 内。
    let work_a = seed_resource_chain_with_source(&repos, id, None, Some(&root_a)).await;
    let work_b = seed_resource_chain_with_source(&repos, id, None, Some(&root_a)).await;
    let item_a = first_media_item_id(&repos, work_a).await;
    let item_b = first_media_item_id(&repos, work_b).await;
    let res_a = repos
        .resource
        .list_by_media_item(item_a)
        .await
        .unwrap()
        .remove(0);
    let res_b = repos
        .resource
        .list_by_media_item(item_b)
        .await
        .unwrap()
        .remove(0);

    // 按 id 确定性：较小 ID → 有效 locator（先被 rebind 处理并 save）；
    // 较大 ID → ParentDir 越界（后处理 → Err → 整体回滚）。
    let a_is_smaller = res_a.id < res_b.id;
    let (small, large) = if a_is_smaller {
        (res_a, res_b)
    } else {
        (res_b, res_a)
    };
    let valid_path = root_a
        .join("inside.mkv")
        .to_string_lossy()
        .replace('\\', "/");
    let escape_path = root_a
        .join("..")
        .join("outside.mkv")
        .to_string_lossy()
        .replace('\\', "/");
    let mut small_edit = small.clone();
    small_edit.locator = ResourceLocator::LocalPath {
        path: valid_path.clone(),
    };
    repos.resource.save(&small_edit).await.unwrap();
    let mut large_edit = large.clone();
    large_edit.locator = ResourceLocator::LocalPath {
        path: escape_path.clone(),
    };
    repos.resource.save(&large_edit).await.unwrap();

    let err = svc.rebind_local(id, dir_b.path()).await.unwrap_err();
    assert_eq!(
        err.code().as_str(),
        "INVALID_ARGUMENT",
        "混合 rebase 必须整体失败"
    );
    assert!(err.user_message().contains("rebase"));

    // ① root_ref / status 未变
    let list = svc.list().await.unwrap();
    assert_eq!(
        list[0].root_ref,
        root_a.to_string_lossy().trim_start_matches(r"\\?\"),
        "root_ref 不得被修改"
    );
    assert_eq!(
        list[0].status,
        StorageStatus::Connected,
        "status 不得被修改"
    );

    // ② 有效 locator（较小 ID）也必须回滚：仍是 rebind 前的提交值（inside.mkv），
    //    不得被 rebase 到 new_root（证明先执行 save 后回滚）。
    let after_small = repos
        .resource
        .list_by_media_item(item_a)
        .await
        .unwrap()
        .remove(0);
    let after_big = repos
        .resource
        .list_by_media_item(item_b)
        .await
        .unwrap()
        .remove(0);
    let (after_small, after_big) = if a_is_smaller {
        (after_small, after_big)
    } else {
        (after_big, after_small)
    };
    match after_small.locator {
        ResourceLocator::LocalPath { path } => {
            assert_eq!(
                path, valid_path,
                "已保存的有效 locator 必须随事务回滚（不得 rebase 到 new_root）"
            );
        }
        _ => panic!(),
    }
    // ③ 非法 locator / availability / source 均不变
    match after_big.locator {
        ResourceLocator::LocalPath { path } => assert_eq!(path, escape_path),
        _ => panic!(),
    }
    assert_eq!(after_small.availability, Availability::Available);
    assert_eq!(after_small.availability_source, AvailabilitySource::Unknown);
    assert_eq!(after_big.availability, Availability::Available);
    assert_eq!(after_big.availability_source, AvailabilitySource::Unknown);
}

/// R-MAIN-08B（Windows 专项）：`root/C:/outside` 中段的 `Normal("C:")` 组件经
/// `PathBuf::from_iter` 会重解释为 drive-relative prefix（`C:outside`），
/// 若放行则 `new_root.join(relative)` 替换整个路径越出 new_root。
/// 必须 `INVALID_ARGUMENT`，且 root/status/locator 无半提交。
#[cfg(windows)]
#[tokio::test]
async fn rebind_rejects_drive_relative_reinterpretation_locator() {
    use haven_domain::contracts::ResourceRepository;
    use haven_domain::entities::ResourceLocator;

    let dir_a = tempfile::TempDir::new().unwrap();
    let root_a = dir_a.path().to_path_buf();
    let dir_b = tempfile::TempDir::new().unwrap();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let svc = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let id = svc.add_local("库".into(), &root_a).await.unwrap();
    let work_id = seed_resource_chain_with_source(&repos, id, None, Some(&root_a)).await;
    let media_item_id = first_media_item_id(&repos, work_id).await;
    let before = repos
        .resource
        .list_by_media_item(media_item_id)
        .await
        .unwrap()
        .remove(0);

    // 构造 `root/C:/outside`：中段 "C:" 在原始 components 中为 Normal，
    // 但 PathBuf 重解释为 drive-relative，join 会越出 new_root。
    let base = root_a.to_string_lossy().replace('\\', "/");
    let drive_relative = format!("{base}/C:/outside");
    let mut malicious = before.clone();
    malicious.locator = ResourceLocator::LocalPath {
        path: drive_relative,
    };
    repos.resource.save(&malicious).await.unwrap();

    let err = svc.rebind_local(id, dir_b.path()).await.unwrap_err();
    assert_eq!(
        err.code().as_str(),
        "INVALID_ARGUMENT",
        "drive-relative 重解释必须拒绝"
    );
    assert!(err.user_message().contains("rebase"));

    // 无半提交：root_ref / status / locator / availability 均不变。
    let list = svc.list().await.unwrap();
    assert_eq!(
        list[0].root_ref,
        root_a.to_string_lossy().trim_start_matches(r"\\?\"),
        "root_ref 不得被修改"
    );
    assert_eq!(
        list[0].status,
        StorageStatus::Connected,
        "status 不得被修改"
    );
    let after = repos
        .resource
        .list_by_media_item(media_item_id)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(after.locator, malicious.locator, "locator 保持（回滚）");
    assert_eq!(after.availability, before.availability);
    assert_eq!(after.availability_source, before.availability_source);
}

// ---- R-MAIN-09A：get_scan_target 并发快路径复核 + UNC / mapped-drive 预拒绝 ----

use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use haven_application::services::storage_location::{DefaultRootProbe, ProbeOutcome, RootProbe};

/// 测试用 probe：可注入 hook（在 probe 与短事务复核之间确定性执行 disconnect/rebind），
/// 并跟踪内层 DefaultRootProbe 的 FS canonicalize/read_dir 调用次数（证明预拒绝零 FS）。
struct HookProbe {
    inner: DefaultRootProbe,
    hook: StdMutex<Option<Box<dyn Fn() + Send>>>,
    fs_calls: AtomicUsize,
}

impl HookProbe {
    fn new() -> Self {
        Self {
            inner: DefaultRootProbe::new(),
            hook: StdMutex::new(None),
            fs_calls: AtomicUsize::new(0),
        }
    }
    fn set_hook(&self, f: Box<dyn Fn() + Send>) {
        *self.hook.lock().unwrap() = Some(f);
    }
}

impl RootProbe for HookProbe {
    fn probe(&self, root_ref: &str) -> ProbeOutcome {
        // R-MAIN-09A1 阻塞 5：先完成内层 probe 并记录 fs_calls，**之后**执行 hook——
        // 真正覆盖「FS probe 结束 → 短事务开启」之间的窗口（hook 表示并发 disconnect/rebind 已提交）。
        let out = self.inner.probe(root_ref);
        self.fs_calls
            .store(self.inner.last_fs_calls(), Ordering::SeqCst);
        if let Some(hook) = self.hook.lock().unwrap().take() {
            hook();
        }
        out
    }
    fn last_fs_calls(&self) -> usize {
        self.fs_calls.load(Ordering::SeqCst)
    }
}

/// 模拟 mapped network drive 预拒绝的注入 probe（覆盖生产 `mapped_drive::is_remote_drive`
/// 命中 DRIVE_REMOTE 的分支；生产真逻辑在 `DefaultRootProbe` 且对本地盘不误拒）。
struct SimMappedDriveProbe;

impl RootProbe for SimMappedDriveProbe {
    fn probe(&self, _root_ref: &str) -> ProbeOutcome {
        ProbeOutcome::PolicyDenied
    }
    fn last_fs_calls(&self) -> usize {
        0
    }
}

/// R-MAIN-09A：**concurrent disconnect**——Connected+reachable 原快路径在 probe 后直接返回；
/// 现在所有分支进短事务复核，hook 在 probe 与复核之间确定性断开 → 明确 SECURITY_POLICY_DENIED，
/// 不返回旧 target，且资源不被误标 Missing。
#[tokio::test]
async fn concurrent_disconnect_rejects_stale_scan_target() {
    use haven_domain::contracts::{ResourceRepository, WorkRepository};
    use haven_domain::enums::{Availability, StorageStatus};

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::write(root.join("Movie.A.mkv"), b"fake-video-bytes").unwrap();

    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let probe = Arc::new(HookProbe::new());
    let svc = StorageLocationService::with_probe(
        Arc::new(SqliteStorageUoW::new(db.clone())),
        probe.clone(),
    );
    let id = svc.add_local("库".into(), &root).await.unwrap();
    let scanner = LocalLibraryScanner::new(db.clone());
    scan_via_target(&svc, &scanner, id).await;

    let hook_uow = Arc::new(SqliteStorageUoW::new(db.clone()));
    probe.set_hook(Box::new(move || {
        hook_uow
            .run(&|tx| {
                // 阻塞 6：同一事务内 save_location(Disconnected) + 资源 StorageUnavailable/Storage
                //（等价真实 disconnect 的原子副作用）。
                let mut loc = tx.load_location(id).unwrap().unwrap();
                loc.status = StorageStatus::Disconnected;
                loc.updated_at = haven_common::UtcMillis::now();
                tx.save_location(&loc)?;
                tx.set_resources_availability(
                    id,
                    Availability::StorageUnavailable,
                    AvailabilitySource::Storage,
                )
            })
            .unwrap();
    }));

    let err = svc.get_scan_target(id).await.unwrap_err();
    assert_eq!(
        err.code().as_str(),
        "SECURITY_POLICY_DENIED",
        "probe 期间被断开必须明确拒绝（不返回旧 target）"
    );

    // 位置 Disconnected（hook 提交）。
    let list = svc.list().await.unwrap();
    assert_eq!(list[0].status, StorageStatus::Disconnected);
    // 资源必须是 hook 写入的 StorageUnavailable/Storage（不是 Available，也不是 Missing）。
    let works = repos.work.list(10, 0).await.unwrap();
    let edition = repos
        .edition
        .list_by_work(works[0].id)
        .await
        .unwrap()
        .remove(0);
    let media_item = repos
        .media_item
        .list_by_edition(edition.id)
        .await
        .unwrap()
        .remove(0);
    let resources = repos
        .resource
        .list_by_media_item(media_item.id)
        .await
        .unwrap();
    assert_eq!(
        resources[0].availability,
        Availability::StorageUnavailable,
        "竞态后资源必须是断开写入的 StorageUnavailable"
    );
    assert_eq!(
        resources[0].availability_source,
        AvailabilitySource::Storage,
        "竞态后来源必须是 storage"
    );
}

/// R-MAIN-09A：**concurrent rebind**——hook 在 probe 与复核之间改 root_ref；
/// 原快路径会返回旧 target；现在 retryable，位置无半提交。
#[tokio::test]
async fn concurrent_rebind_rejects_stale_scan_target() {
    use haven_domain::enums::StorageStatus;

    let dir_a = tempfile::TempDir::new().unwrap();
    let root_a = dir_a.path().to_path_buf();
    std::fs::write(root_a.join("Movie.A.mkv"), b"fake-video-bytes").unwrap();
    let dir_b = tempfile::TempDir::new().unwrap();
    let new_root = std::fs::canonicalize(dir_b.path())
        .unwrap()
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_owned();

    let db = Arc::new(Db::open_in_memory().unwrap());
    let probe = Arc::new(HookProbe::new());
    let svc = StorageLocationService::with_probe(
        Arc::new(SqliteStorageUoW::new(db.clone())),
        probe.clone(),
    );
    let id = svc.add_local("库".into(), &root_a).await.unwrap();

    let hook_uow = Arc::new(SqliteStorageUoW::new(db.clone()));
    let new_root_hook = new_root.clone();
    probe.set_hook(Box::new(move || {
        hook_uow
            .run(&|tx| {
                let mut loc = tx.load_location(id).unwrap().unwrap();
                loc.root_ref = new_root_hook.clone();
                loc.status = StorageStatus::Connected;
                loc.updated_at = haven_common::UtcMillis::now();
                tx.save_location(&loc)
            })
            .unwrap();
    }));

    let err = svc.get_scan_target(id).await.unwrap_err();
    assert_eq!(
        err.code().as_str(),
        "DATABASE_ERROR",
        "probe 期间 rebind（root_ref 变化）必须 retryable，不得返回旧 target"
    );
    assert!(err.retryable(), "快照变化必须是可重试错误");

    // 位置 root_ref = hook 提交的新路径；未被旧路径逻辑覆盖。
    let list = svc.list().await.unwrap();
    assert_eq!(
        list[0].root_ref, new_root,
        "root_ref 为并发提交值（无半提交）"
    );
    assert_eq!(list[0].status, StorageStatus::Connected);
}

/// R-MAIN-09A：**Missing+unreachable 快路径快照变化**——位置已 Missing 且路径仍不可达，
/// hook 在 probe 期间改 root_ref → 快照变化 → retryable（而非静默返回旧路径或稳定错误）。
#[tokio::test]
async fn missing_unreachable_snapshot_change_returns_retryable() {
    use haven_domain::enums::StorageStatus;

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let probe = Arc::new(HookProbe::new());
    let svc = StorageLocationService::with_probe(
        Arc::new(SqliteStorageUoW::new(db.clone())),
        probe.clone(),
    );
    let id = svc.add_local("库".into(), &root).await.unwrap();

    // 先让路径消失 → 位置迁移 Missing（无 hook）。
    drop(dir);
    let err = svc.get_scan_target(id).await.unwrap_err();
    assert_eq!(err.code().as_str(), "RESOURCE_UNAVAILABLE");
    let list = svc.list().await.unwrap();
    assert_eq!(list[0].status, StorageStatus::Missing);

    // Missing+unreachable 快路径：probe 期间 hook 改 root_ref → 快照变化 → retryable。
    let dir_new = tempfile::TempDir::new().unwrap();
    let new_root = std::fs::canonicalize(dir_new.path())
        .unwrap()
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_owned();
    let hook_uow = Arc::new(SqliteStorageUoW::new(db.clone()));
    let new_root_hook = new_root.clone();
    probe.set_hook(Box::new(move || {
        hook_uow
            .run(&|tx| {
                let mut loc = tx.load_location(id).unwrap().unwrap();
                loc.root_ref = new_root_hook.clone();
                loc.updated_at = haven_common::UtcMillis::now();
                tx.save_location(&loc)
            })
            .unwrap();
    }));

    let err = svc.get_scan_target(id).await.unwrap_err();
    assert_eq!(
        err.code().as_str(),
        "DATABASE_ERROR",
        "Missing+unreachable 快路径在快照变化时也必须 retryable"
    );
    assert!(err.retryable());
    let list = svc.list().await.unwrap();
    assert_eq!(
        list[0].root_ref, new_root,
        "root_ref 为并发提交值（无半提交）"
    );
}

/// R-MAIN-09A：**UNC 预拒绝（probe 层）**——明文与 verbatim UNC 在任何 canonicalize/read_dir
/// 前被词法拒绝，FS 调用计数必须为 0；verbatim 本地盘符不误拒。
#[test]
fn unc_probe_pre_rejects_before_any_fs() {
    let probe = DefaultRootProbe::new();
    for unc in [
        r"\\server\share",
        r"\\?\UNC\server\share",
        r"\\?\UNC\server\share\media",
    ] {
        assert_eq!(
            probe.probe(unc),
            ProbeOutcome::PolicyDenied,
            "UNC 必须词法预拒绝: {unc}"
        );
        assert_eq!(
            probe.last_fs_calls(),
            0,
            "UNC 预拒绝阶段不得触发 canonicalize/read_dir: {unc}"
        );
    }
    // verbatim 本地盘符（\\?\C:\...）不是 UNC，也不是 mapped drive → 不是 PolicyDenied。
    let out = probe.probe(r"\\?\C:\haven-no-such-dir-xyz");
    assert_ne!(
        out,
        ProbeOutcome::PolicyDenied,
        "verbatim 本地盘符不得被误判为策略拒绝"
    );
}

/// R-MAIN-09A：**service 级 UNC 预拒绝**——位置 root_ref 为 UNC 时，get_scan_target
/// 返回稳定 SECURITY_POLICY_DENIED，且**位置不被标 Missing**；probe FS 计数为 0。
#[tokio::test]
async fn get_scan_target_unc_pre_rejects_without_marking_missing() {
    use haven_domain::enums::StorageStatus;

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let probe = Arc::new(HookProbe::new());
    let svc = StorageLocationService::with_probe(
        Arc::new(SqliteStorageUoW::new(db.clone())),
        probe.clone(),
    );
    let id = svc.add_local("库".into(), &root).await.unwrap();

    // 预先（get_scan_target 前）把位置 root_ref 篡改为 UNC —— 模拟已入库的 UNC 路径。
    let hook_uow = Arc::new(SqliteStorageUoW::new(db.clone()));
    hook_uow
        .run(&|tx| {
            let mut loc = tx.load_location(id).unwrap().unwrap();
            loc.root_ref = r"\\server\share\media".into();
            loc.updated_at = haven_common::UtcMillis::now();
            tx.save_location(&loc)
        })
        .unwrap();

    let err = svc.get_scan_target(id).await.unwrap_err();
    assert_eq!(err.code().as_str(), "SECURITY_POLICY_DENIED");
    assert_eq!(
        probe.last_fs_calls(),
        0,
        "UNC 策略拒绝不得触发任何 canonicalize/read_dir"
    );
    let list = svc.list().await.unwrap();
    assert_eq!(
        list[0].status,
        StorageStatus::Connected,
        "policy deny 不得把位置误标 Missing"
    );
}

/// R-MAIN-09A：**mapped drive 模拟预拒绝（service 级）**——注入 probe 命中 DRIVE_REMOTE
/// 拒绝分支；get_scan_target 稳定 SECURITY_POLICY_DENIED，位置/资源不标 Missing，FS 计数 0。
#[tokio::test]
async fn mapped_drive_simulated_pre_reject_zero_fs() {
    use haven_domain::enums::StorageStatus;

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = StorageLocationService::with_probe(
        Arc::new(SqliteStorageUoW::new(db.clone())),
        Arc::new(SimMappedDriveProbe),
    );
    let id = svc.add_local("库".into(), &root).await.unwrap();

    let err = svc.get_scan_target(id).await.unwrap_err();
    assert_eq!(
        err.code().as_str(),
        "SECURITY_POLICY_DENIED",
        "mapped drive 拒绝必须返回稳定 policy 错误"
    );
    let list = svc.list().await.unwrap();
    assert_eq!(
        list[0].status,
        StorageStatus::Connected,
        "policy deny 不得把位置误标 Missing"
    );
}

/// R-MAIN-09A（Windows 专项）：**生产 DRIVE_REMOTE 逻辑不误拒本地盘**——
/// 真实 `C:\Windows` 不得被判为 mapped drive。
#[cfg(windows)]
#[test]
fn local_drive_is_not_policy_denied() {
    let probe = DefaultRootProbe::new();
    let out = probe.probe("C:\\Windows");
    assert_ne!(
        out,
        ProbeOutcome::PolicyDenied,
        "本地盘（C:\\Windows）不得被误判为 mapped network drive"
    );
}

/// R-MAIN-09A 保底：Connected+reachable 正常快路径（无并发）仍返回验证过的 canonical 路径。
#[tokio::test]
async fn connected_reachable_fast_path_returns_valid_target() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let id = svc.add_local("库".into(), &root).await.unwrap();

    let target = svc.get_scan_target(id).await.unwrap();
    assert_eq!(target.storage_location_id(), id);
    let expected = std::fs::canonicalize(&root)
        .unwrap()
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_owned();
    assert_eq!(
        target.root_path().to_string_lossy(),
        expected,
        "快路径必须返回本次验证过的 canonical 路径"
    );
}

/// R-MAIN-09A1 阻塞 7：**policy-denied race**——初始 DB root 为 UNC，HookProbe 先由
/// DefaultRootProbe 得到 PolicyDenied 且 0 FS，再 hook 在「FS probe 结束→短事务开启」间
/// 把 DB root 改为安全本地 root → 短事务重读 root_ref 变化 → **retryable DATABASE_ERROR**
/// （而非旧的 SECURITY_POLICY_DENIED）；并发提交（新 root）保留。
#[tokio::test]
async fn policy_denied_then_concurrent_rebind_returns_retryable() {
    use haven_domain::enums::StorageStatus;

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let safe_root = std::fs::canonicalize(&root)
        .unwrap()
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_owned();

    let db = Arc::new(Db::open_in_memory().unwrap());
    let probe = Arc::new(HookProbe::new());
    let svc = StorageLocationService::with_probe(
        Arc::new(SqliteStorageUoW::new(db.clone())),
        probe.clone(),
    );
    let id = svc.add_local("库".into(), &root).await.unwrap();

    // 初始 DB root 篡改为 UNC（探测必得 PolicyDenied）。
    let prep = Arc::new(SqliteStorageUoW::new(db.clone()));
    prep.run(&|tx| {
        let mut loc = tx.load_location(id).unwrap().unwrap();
        loc.root_ref = r"\\server\share\media".into();
        loc.updated_at = haven_common::UtcMillis::now();
        tx.save_location(&loc)
    })
    .unwrap();

    // hook：probe（UNC → PolicyDenied，0 FS）完成后把 DB root 改回安全本地。
    let hook_uow = Arc::new(SqliteStorageUoW::new(db.clone()));
    let safe_hook = safe_root.clone();
    probe.set_hook(Box::new(move || {
        hook_uow
            .run(&|tx| {
                let mut loc = tx.load_location(id).unwrap().unwrap();
                loc.root_ref = safe_hook.clone();
                loc.status = StorageStatus::Connected;
                loc.updated_at = haven_common::UtcMillis::now();
                tx.save_location(&loc)
            })
            .unwrap();
    }));

    let err = svc.get_scan_target(id).await.unwrap_err();
    assert_eq!(
        err.code().as_str(),
        "DATABASE_ERROR",
        "PolicyDenied 后快照变化必须 retryable，不得返回旧 SECURITY_POLICY_DENIED"
    );
    assert!(err.retryable(), "快照变化必须是可重试错误");
    assert_eq!(
        probe.last_fs_calls(),
        0,
        "探测输入是 UNC → 预拒绝阶段零 FS 调用"
    );

    // 并发提交（新 root）保留，无半提交。
    let list = svc.list().await.unwrap();
    assert_eq!(list[0].root_ref, safe_root, "并发 rebind 提交的 root 保留");
    assert_eq!(list[0].status, StorageStatus::Connected);
}

/// R-MAIN-09A1 阻塞 7：**provider 快照变化 → retryable**（不得用旧 provider 结果返回）。
#[tokio::test]
async fn provider_change_during_probe_is_retryable() {
    use haven_domain::enums::StorageStatus;

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let probe = Arc::new(HookProbe::new());
    let svc = StorageLocationService::with_probe(
        Arc::new(SqliteStorageUoW::new(db.clone())),
        probe.clone(),
    );
    let id = svc.add_local("库".into(), &root).await.unwrap();

    let hook_uow = Arc::new(SqliteStorageUoW::new(db.clone()));
    probe.set_hook(Box::new(move || {
        hook_uow
            .run(&|tx| {
                let mut loc = tx.load_location(id).unwrap().unwrap();
                loc.provider_type = haven_domain::enums::StorageProviderType::WebDav;
                loc.updated_at = haven_common::UtcMillis::now();
                tx.save_location(&loc)
            })
            .unwrap();
    }));

    let err = svc.get_scan_target(id).await.unwrap_err();
    assert_eq!(
        err.code().as_str(),
        "DATABASE_ERROR",
        "provider 变化必须 retryable（不得用旧 Local 结果返回 target）"
    );
    assert!(err.retryable());
    let list = svc.list().await.unwrap();
    assert_eq!(
        list[0].provider_type,
        haven_domain::enums::StorageProviderType::WebDav,
        "并发提交的 provider 保留（无半提交）"
    );
    assert_eq!(list[0].status, StorageStatus::Connected);
}

// ---- R-MAIN-09B：ScanTarget token / 消费前重探测 / 写事务 guard ----

/// R-MAIN-09B：target 获取后再 **disconnect**（完整 Location+Resource overlay）→
/// scan_target 返回 SCAN_TARGET_STALE 且**不写新实体**。
#[tokio::test]
async fn target_then_disconnect_scan_target_stale_no_writes() {
    use haven_domain::contracts::WorkRepository;
    use haven_domain::enums::StorageStatus;

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let id = svc.add_local("库".into(), &root).await.unwrap();
    let scanner = LocalLibraryScanner::new(db.clone());
    let target = svc.get_scan_target(id).await.unwrap();

    // target 之后 disconnect（完整语义：状态 + 资源 overlay）。
    svc.disconnect(id).await.unwrap();
    assert_eq!(
        svc.list().await.unwrap()[0].status,
        StorageStatus::Disconnected
    );

    let err = scanner.scan_target(&target).await.unwrap_err();
    assert_eq!(err.code().as_str(), "SCAN_TARGET_STALE");
    assert!(err.retryable(), "stale 必须可重试");

    // 目录里有文件，但 0 新实体（token guard 在写前拦截）。
    std::fs::write(root.join("Movie.A.mkv"), b"fake-video-bytes").unwrap();
    let _ = scanner.scan_target(&target).await.unwrap_err();
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let works = repos.work.list(10, 0).await.unwrap();
    assert_eq!(works.len(), 0, "stale 时不得写入任何 Work/Resource");
}

/// R-MAIN-09B：target 后 **rebind** 到新 root → 旧 target stale；旧 root 内容不被写。
#[tokio::test]
async fn target_then_rebind_old_target_stale_old_root_not_written() {
    use haven_domain::contracts::WorkRepository;
    use haven_domain::enums::StorageStatus;

    let dir_a = tempfile::TempDir::new().unwrap();
    let root_a = dir_a.path().to_path_buf();
    let dir_b = tempfile::TempDir::new().unwrap();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let id = svc.add_local("库".into(), &root_a).await.unwrap();
    let scanner = LocalLibraryScanner::new(db.clone());
    let target = svc.get_scan_target(id).await.unwrap();

    // target 之后 rebind 到新 root。
    svc.rebind_local(id, dir_b.path()).await.unwrap();
    let list = svc.list().await.unwrap();
    assert_eq!(list[0].status, StorageStatus::Connected);

    // 旧 root 里放文件（生产上不可能再扫旧 root，但证明旧 target 不会写它）。
    std::fs::write(root_a.join("Movie.Old.mkv"), b"old-video-bytes").unwrap();
    let err = scanner.scan_target(&target).await.unwrap_err();
    assert_eq!(
        err.code().as_str(),
        "SCAN_TARGET_STALE",
        "rebind 后旧 target 必须 stale"
    );
    assert!(err.retryable());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let works = repos.work.list(10, 0).await.unwrap();
    assert_eq!(works.len(), 0, "旧 root 内容不得经旧 target 写入");
}

/// R-MAIN-09B：target 后 **remove** → scan_target stale（位置行不存在）。
#[tokio::test]
async fn target_then_remove_scan_target_stale() {
    use haven_domain::contracts::WorkRepository;

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let id = svc.add_local("库".into(), &root).await.unwrap();
    let scanner = LocalLibraryScanner::new(db.clone());
    let target = svc.get_scan_target(id).await.unwrap();

    svc.remove(id).await.unwrap();
    std::fs::write(root.join("Movie.A.mkv"), b"fake-video-bytes").unwrap();

    let err = scanner.scan_target(&target).await.unwrap_err();
    assert_eq!(
        err.code().as_str(),
        "SCAN_TARGET_STALE",
        "remove 后（行不存在）必须 stale"
    );
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let works = repos.work.list(10, 0).await.unwrap();
    assert_eq!(works.len(), 0);
}

/// R-MAIN-09B：正常 target 扫描成功（token 匹配 + FS 重探测通过）。
#[tokio::test]
async fn normal_target_scan_succeeds() {
    use haven_domain::contracts::WorkRepository;

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let id = svc.add_local("库".into(), &root).await.unwrap();
    std::fs::write(root.join("Movie.A.mkv"), b"fake-video-bytes").unwrap();
    let scanner = LocalLibraryScanner::new(db.clone());
    let target = svc.get_scan_target(id).await.unwrap();

    let report = scanner.scan_target(&target).await.unwrap();
    assert_eq!(report.new, 1, "正常 target 扫描必须成功建立实体");
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    assert_eq!(repos.work.list(10, 0).await.unwrap().len(), 1);
}

/// R-MAIN-09B：消费前 FS 重探测——target root 被删除后 scan_target 失败且 0 写。
/// 同路径删除重建属残余 OS identity 债务（不声称解决）。
#[tokio::test]
async fn target_root_deleted_scan_target_fails_zero_writes() {
    use haven_domain::contracts::WorkRepository;

    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().to_path_buf();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = StorageLocationService::new(Arc::new(SqliteStorageUoW::new(db.clone())));
    let id = svc.add_local("库".into(), &root).await.unwrap();
    let scanner = LocalLibraryScanner::new(db.clone());
    let target = svc.get_scan_target(id).await.unwrap();

    // 删除 root（消费前）→ FS 重探测失败。
    drop(dir);
    let err = scanner.scan_target(&target).await.unwrap_err();
    assert_eq!(
        err.code().as_str(),
        "SCAN_TARGET_STALE",
        "消费前 root 删除必须 stale（重探测失败）"
    );
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    assert_eq!(
        repos.work.list(10, 0).await.unwrap().len(),
        0,
        "root 删除后 0 写"
    );
}
