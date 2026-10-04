use super::*;

#[tokio::test]
async fn pdf_import_builds_real_chain_without_provider_id_leak() {
    let db = memory_db();
    let repo = SqliteCloudStorageRepository::new(db.clone());
    let resources = SqliteResourceRepository::new(db.clone());
    let media_items = SqliteMediaItemRepository::new(db.clone());
    let editions = SqliteEditionRepository::new(db.clone());
    let works = SqliteWorkRepository::new(db.clone());

    let (account, _) = connect_new(&repo, "Acct-Pdf").await;
    let folder = register_folder(&repo, &account, "PdfFolder-1", "PDFs").await;
    let file = pdf("File-AbC-1", 4096);
    let created = import_new(&repo, &folder, account.generation, &file).await;
    assert_eq!(created.provider_file_id, "File-AbC-1");
    assert_eq!(created.size_bytes, 4096);
    assert_eq!(created.location_id, folder.location_id);
    assert_eq!(created.id.len(), 36);

    let media = media_items
        .get(created.media_item_id)
        .await
        .unwrap()
        .expect("media item");
    assert_eq!(media.media_type, MediaType::Document);
    let edition = editions
        .get(media.edition_id)
        .await
        .unwrap()
        .expect("edition");
    assert!(
        works.get(edition.work_id).await.unwrap().is_some(),
        "真实内容链"
    );

    let resource = resources
        .get(created.resource_id)
        .await
        .unwrap()
        .expect("resource");
    assert_eq!(resource.resource_type, ResourceType::PublicationFile);
    assert_eq!(resource.mime_type.as_deref(), Some("application/pdf"));
    match &resource.locator {
        ResourceLocator::StorageObject {
            provider_id,
            object_id,
            ..
        } => {
            assert_eq!(*provider_id, folder.location_id);
            assert_eq!(object_id, &created.id, "object_id 是内部对象 UUID");
            assert_ne!(object_id, &created.provider_file_id);
        }
        other => panic!("应为 StorageObject: {other:?}"),
    }
    assert!(
        !serde_json::to_string(&resource.locator)
            .unwrap()
            .contains("File-AbC-1")
    );

    let category: String = db
        .lock()
        .query_row(
            "SELECT category FROM media_items WHERE id = ?1",
            rusqlite::params![created.media_item_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(category, "periodical");

    match repo
        .import_pdf(&folder, account.generation, &file)
        .await
        .unwrap()
    {
        CloudCasOutcome::Applied(CloudPdfImportOutcome::Existing(existing)) => {
            assert_eq!(existing.id, created.id);
            assert_eq!(existing.resource_id, created.resource_id);
        }
        other => panic!("重复导入应为 Existing: {other:?}"),
    }
    assert_eq!(
        resources
            .list_by_media_item(created.media_item_id)
            .await
            .unwrap()
            .len(),
        1
    );
}

/// 显式重导只刷新 Provider 事实；不覆盖用户标题/来源，不复制内容，原样命中不改时间戳。
#[tokio::test]
async fn reimport_refreshes_provider_facts_and_keeps_content_identity() {
    let db = memory_db();
    let repo = SqliteCloudStorageRepository::new(db.clone());
    let resources = SqliteResourceRepository::new(db.clone());
    let media_items = SqliteMediaItemRepository::new(db.clone());
    let editions = SqliteEditionRepository::new(db.clone());
    let works = SqliteWorkRepository::new(db.clone());
    let (account, _) = connect_new(&repo, "Acct-Refresh").await;
    let folder = register_folder(&repo, &account, "RefreshFolder-1", "PDFs").await;
    let first_file = pdf("File-Rfr-1", 4096);
    let first = import_new(&repo, &folder, account.generation, &first_file).await;
    let mut media = media_items.get(first.media_item_id).await.unwrap().unwrap();
    media.title = "我的报告".into();
    media_items.save(&media).await.unwrap();
    let mut edition = editions.get(media.edition_id).await.unwrap().unwrap();
    edition.title = "我的版本".into();
    editions.save(&edition).await.unwrap();
    let mut work = works.get(edition.work_id).await.unwrap().unwrap();
    work.canonical_title = "我的作品".into();
    works.save(&work).await.unwrap();
    let mut before = resources.get(first.resource_id).await.unwrap().unwrap();
    before.updated_at = UtcMillis(4242);
    before.availability = Availability::TemporarilyUnavailable;
    before.availability_source = AvailabilitySource::User;
    resources.save(&before).await.unwrap();

    match repo
        .import_pdf(&folder, account.generation, &first_file)
        .await
        .unwrap()
    {
        CloudCasOutcome::Applied(CloudPdfImportOutcome::Existing(existing)) => {
            assert_eq!(existing, first)
        }
        other => panic!("重复导入应为 Existing: {other:?}"),
    }
    assert_eq!(
        resources.get(first.resource_id).await.unwrap().unwrap(),
        before
    );

    let renamed = CloudPdfCandidate {
        display_name: "改名的报告.pdf".into(),
        ..first_file.clone()
    };
    match repo
        .import_pdf(&folder, account.generation, &renamed)
        .await
        .unwrap()
    {
        CloudCasOutcome::Applied(CloudPdfImportOutcome::Existing(existing)) => {
            assert_eq!(
                existing,
                CloudObjectBinding {
                    display_name: renamed.display_name,
                    ..first.clone()
                }
            );
        }
        other => panic!("改名重导入应为 Existing: {other:?}"),
    }
    assert_eq!(
        resources.get(first.resource_id).await.unwrap().unwrap(),
        before,
        "仅改名不改 Resource"
    );

    match repo
        .import_pdf(&folder, account.generation, &pdf("File-Rfr-1", 9000))
        .await
        .unwrap()
    {
        CloudCasOutcome::Applied(CloudPdfImportOutcome::Existing(existing)) => {
            assert_eq!(
                existing,
                CloudObjectBinding {
                    size_bytes: 9000,
                    ..first.clone()
                }
            );
        }
        other => panic!("变大重导入应为 Existing: {other:?}"),
    }
    let refreshed = resources.get(first.resource_id).await.unwrap().unwrap();
    assert_ne!(refreshed.updated_at, UtcMillis(4242));
    let mut expected = before;
    expected.size = Some(9000);
    expected.updated_at = refreshed.updated_at;
    assert_eq!(
        refreshed, expected,
        "大小与时间戳以外字段完整保留，含 user unavailable"
    );
    assert_eq!(
        media_items.get(first.media_item_id).await.unwrap().unwrap(),
        media
    );
    assert_eq!(
        editions.get(media.edition_id).await.unwrap().unwrap(),
        edition
    );
    assert_eq!(works.get(edition.work_id).await.unwrap().unwrap(), work);
    assert_eq!(
        resources
            .list_by_media_item(first.media_item_id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        repo.get_object(&first.id)
            .await
            .unwrap()
            .unwrap()
            .size_bytes,
        9000
    );
}

#[tokio::test]
async fn cascade_direction_and_remove_folder_purges_index_content() {
    let db = memory_db();
    let repo = SqliteCloudStorageRepository::new(db.clone());
    let resources = SqliteResourceRepository::new(db.clone());
    let media_items = SqliteMediaItemRepository::new(db.clone());
    let editions = SqliteEditionRepository::new(db.clone());
    let works = SqliteWorkRepository::new(db.clone());
    let locations = SqliteStorageLocationRepository::new(db);

    let (account, cred) = connect_new(&repo, "Acct-Remove").await;
    let folder = register_folder(&repo, &account, "RemoveFolder-1", "Docs").await;

    // 外键方向：资源删除 → 对象绑定随之消失；内容链不随绑定消失。
    let first = import_new(&repo, &folder, account.generation, &pdf("File-Del-1", 2048)).await;
    assert!(resources.delete(first.resource_id).await.unwrap());
    assert!(repo.get_object(&first.id).await.unwrap().is_none());
    assert!(
        media_items
            .get(first.media_item_id)
            .await
            .unwrap()
            .is_some()
    );

    // 移除目录：与本地位置 remove **完全同一算法**（purge），不再是 unlink-保留。
    let second = import_new(
        &repo,
        &folder,
        account.generation,
        &pdf("File-Keep-1", 4096),
    )
    .await;
    let media = media_items
        .get(second.media_item_id)
        .await
        .unwrap()
        .expect("条目");
    let edition = editions.get(media.edition_id).await.unwrap().expect("版本");
    let snapshot = repo
        .object_snapshot(&second.id)
        .await
        .unwrap()
        .expect("快照");
    assert_eq!(
        repo.remove_folder(folder.location_id).await.unwrap(),
        CloudFolderRemoveOutcome::Removed
    );

    assert!(repo.get_folder(folder.location_id).await.unwrap().is_none());
    assert!(locations.get(folder.location_id).await.unwrap().is_none());
    assert!(
        repo.get_object(&second.id).await.unwrap().is_none(),
        "对象绑定解除"
    );
    assert!(
        resources.get(second.resource_id).await.unwrap().is_none(),
        "与本地 remove 一致：该位置 Resource 被清理"
    );
    assert!(
        media_items
            .get(second.media_item_id)
            .await
            .unwrap()
            .is_none(),
        "仅由该位置派生的孤儿条目被清理"
    );
    assert!(editions.get(media.edition_id).await.unwrap().is_none());
    assert!(works.get(edition.work_id).await.unwrap().is_none());
    // 远端文件与共享账户凭据不受影响。
    let account_after = repo.get_account(&account.id).await.unwrap().unwrap();
    assert!(account_after.connected);
    assert_eq!(
        account_after.credential_ref,
        Some(cred),
        "移除目录不得触碰账户凭据"
    );
    let stale = repo
        .assert_current(account.generation, &snapshot)
        .await
        .unwrap_err();
    assert_eq!(stale.code().as_str(), "CLOUD_BINDING_STALE");
}

/// 移除云盘目录 = 本地 remove 的同一算法：只清理**该位置**的应用内索引与下载元数据；
/// 远端文件无人触碰（全程无网络 / 无 keystore 调用），共享账户凭据、其他位置与仍被其他
/// 位置引用的共享内容完整保留。
#[tokio::test]
async fn remove_folder_purges_only_its_own_index_content() {
    let db = memory_db();
    let repo = SqliteCloudStorageRepository::new(db.clone());
    let resources = SqliteResourceRepository::new(db.clone());
    let media_items = SqliteMediaItemRepository::new(db.clone());
    let editions = SqliteEditionRepository::new(db.clone());
    let works = SqliteWorkRepository::new(db.clone());
    let locations = SqliteStorageLocationRepository::new(db.clone());

    let (account, cred) = connect_new(&repo, "Acct-Shared").await;
    let folder = register_folder(&repo, &account, "SharedFolder-1", "Docs").await;
    let created = import_new(&repo, &folder, account.generation, &pdf("File-Sha-1", 4096)).await;
    let media = media_items
        .get(created.media_item_id)
        .await
        .unwrap()
        .expect("条目");
    let edition = editions.get(media.edition_id).await.unwrap().expect("版本");

    // 另一个位置（本地库）与同一条目的第二个 Resource：共享内容必须在移除云盘目录后存活。
    let now = UtcMillis::now();
    let local = StorageLocation {
        id: StorageLocationId::new(),
        provider_type: StorageProviderType::Local,
        display_name: "共享本地库".into(),
        root_ref: format!("C:\\haven-cloud-shared-{}", uuid::Uuid::new_v4()),
        credential_ref: None,
        status: StorageStatus::Connected,
        created_at: now,
        updated_at: now,
    };
    locations.save(&local).await.unwrap();
    let mut shared = sample_resource(created.media_item_id, local.id);
    shared.locator = ResourceLocator::LocalPath {
        path: "D:\\local\\shared.mkv".into(),
    };
    resources.save(&shared).await.unwrap();

    // 位置自身的下载元数据随位置清理；其他位置的任务 / 批次保留。
    {
        let conn = db.lock();
        for (task_id, target) in [("task-cloud", folder.location_id), ("task-local", local.id)] {
            conn.execute(
                "INSERT INTO download_tasks
                    (id, source_resource_id, target_storage_id, state, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'completed', ?4, ?4)",
                rusqlite::params![task_id, shared.id.to_string(), target.to_string(), now.0],
            )
            .unwrap();
        }
        for (batch_id, target) in [
            ("batch-cloud", folder.location_id),
            ("batch-local", local.id),
        ] {
            conn.execute(
                "INSERT INTO download_batches
                    (id, title, category, subject_type, subject_id, target_storage_id, state,
                     created_at, updated_at)
                 VALUES (?1, ?1, 'video', 'resource', ?2, ?2, 'completed', ?3, ?3)",
                rusqlite::params![batch_id, target.to_string(), now.0],
            )
            .unwrap();
        }
    }

    assert_eq!(
        repo.remove_folder(folder.location_id).await.unwrap(),
        CloudFolderRemoveOutcome::Removed
    );

    let count = |table: &str| -> i64 {
        db.lock()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    };
    assert_eq!(count("download_tasks"), 1, "只清理目标位置的下载任务");
    assert_eq!(count("download_batches"), 1, "只清理目标位置的下载批次");
    let kept_task: String = db
        .lock()
        .query_row("SELECT id FROM download_tasks", [], |row| row.get(0))
        .unwrap();
    assert_eq!(kept_task, "task-local");
    let kept_batch: String = db
        .lock()
        .query_row("SELECT id FROM download_batches", [], |row| row.get(0))
        .unwrap();
    assert_eq!(kept_batch, "batch-local");

    assert!(repo.get_folder(folder.location_id).await.unwrap().is_none());
    assert!(locations.get(folder.location_id).await.unwrap().is_none());
    assert!(repo.get_object(&created.id).await.unwrap().is_none());
    assert!(
        resources.get(created.resource_id).await.unwrap().is_none(),
        "与本地 remove 一致：目标位置的 Resource 被清理"
    );
    // 共享内容与另一位置的索引完整保留。
    assert!(
        media_items
            .get(created.media_item_id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(editions.get(media.edition_id).await.unwrap().is_some());
    assert!(works.get(edition.work_id).await.unwrap().is_some());
    let kept = resources
        .get(shared.id)
        .await
        .unwrap()
        .expect("共享资源保留");
    assert_eq!(kept.storage_location_id, Some(local.id));
    assert!(locations.get(local.id).await.unwrap().is_some());
    // 无 keystore / 远端副作用：账户仍是原连接，outbox 无待清理项。
    let account_after = repo.get_account(&account.id).await.unwrap().unwrap();
    assert!(account_after.connected);
    assert_eq!(account_after.credential_ref, Some(cred));
    assert_eq!(count("cloud_credential_cleanup"), 0);
}
