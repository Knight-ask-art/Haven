use super::*;

#[tokio::test]
async fn credential_stage_connect_replace_disconnect_cleanup() {
    let repo = SqliteCloudStorageRepository::new(memory_db());
    let cred1 = fresh_cred();
    repo.stage_credential(&cred1).await.unwrap();
    assert!(
        repo.pending_credentials(CLOUD_CREDENTIAL_CLEANUP_BATCH)
            .await
            .unwrap()
            .is_empty(),
        "staged 引用不得进入普通删除列表"
    );

    let account = match repo
        .connect(&connect_request("Acct-Life"), None, &cred1)
        .await
        .unwrap()
    {
        CloudCasOutcome::Applied(account) => account,
        other => panic!("connect 应成功: {other:?}"),
    };
    assert_eq!(account.generation, 1);
    assert!(account.connected);
    assert_eq!(account.credential_ref.as_ref(), Some(&cred1));

    let cred2 = fresh_cred();
    repo.stage_credential(&cred2).await.unwrap();
    match repo
        .replace_credential(&account.id, 1, &cred1, &cred2)
        .await
        .unwrap()
    {
        CloudCasOutcome::Applied(refreshed) => {
            assert_eq!(refreshed.generation, 1, "换发凭据不换代");
            assert_eq!(refreshed.credential_ref.as_ref(), Some(&cred2));
        }
        other => panic!("replace_credential 应成功: {other:?}"),
    }
    let pending = repo
        .pending_credentials(CLOUD_CREDENTIAL_CLEANUP_BATCH)
        .await
        .unwrap();
    assert!(pending.contains(&cred1), "旧引用转入 ready outbox");
    assert!(!pending.contains(&cred2), "新引用已被消耗");

    assert!(
        repo.stage_credential(&cred2).await.is_err(),
        "活动引用不得再 stage"
    );
    assert!(
        repo.retire_staged_credential(&cred2).await.is_err(),
        "活动引用不得 retire"
    );
    assert_eq!(
        repo.finish_credential_cleanup(&cred2)
            .await
            .unwrap_err()
            .code()
            .as_str(),
        "CLOUD_CREDENTIAL_IN_USE"
    );

    match repo.disconnect(&account.id, 1).await.unwrap() {
        CloudCasOutcome::Applied(tombstone) => {
            assert!(!tombstone.connected);
            assert_eq!(tombstone.generation, 2);
            assert_eq!(
                tombstone.credential_ref.as_ref(),
                Some(&cred2),
                "断开保留引用待清理"
            );
        }
        other => panic!("disconnect 应成功: {other:?}"),
    }
    let pending = repo
        .pending_credentials(CLOUD_CREDENTIAL_CLEANUP_BATCH)
        .await
        .unwrap();
    assert!(pending.contains(&cred1) && pending.contains(&cred2));

    assert_eq!(
        repo.finish_credential_cleanup(&cred1).await.unwrap(),
        CloudCredentialCleanupOutcome::RefRetained
    );
    assert_eq!(
        repo.finish_credential_cleanup(&cred2).await.unwrap(),
        CloudCredentialCleanupOutcome::Cleared
    );
    assert!(
        repo.get_account(&account.id)
            .await
            .unwrap()
            .unwrap()
            .credential_ref
            .is_none()
    );
    assert!(
        repo.pending_credentials(CLOUD_CREDENTIAL_CLEANUP_BATCH)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        repo.pending_credentials(CLOUD_CREDENTIAL_CLEANUP_BATCH + 1)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn recover_expired_stage_is_deterministic_and_keeps_new_stage() {
    let db = memory_db();
    let repo = SqliteCloudStorageRepository::new(db.clone());

    let old = fresh_cred();
    repo.stage_credential(&old).await.unwrap();
    db.lock()
        .execute(
            "UPDATE cloud_credential_cleanup SET staged_at = 1000 WHERE credential_ref = ?1",
            rusqlite::params![old.as_str()],
        )
        .unwrap();
    let keep = fresh_cred();
    repo.stage_credential(&keep).await.unwrap();

    let (active, active_cred) = connect_new(&repo, "Acct-Recover").await;

    // 确定性 now：只回收 staged_at=1000 的旧引用，不睡眠、不触碰活动账户。
    let now = UtcMillis(1000 + CLOUD_CREDENTIAL_STAGE_TTL_MS + 1);
    assert_eq!(repo.recover_staged_credentials(now).await.unwrap(), 1);
    let pending = repo
        .pending_credentials(CLOUD_CREDENTIAL_CLEANUP_BATCH)
        .await
        .unwrap();
    assert_eq!(pending, vec![old.clone()], "仅超时暂存被回收为 ready");

    let account = repo.get_account(&active.id).await.unwrap().unwrap();
    assert!(account.connected);
    assert_eq!(account.credential_ref.as_ref(), Some(&active_cred));

    // finish 不得消费仍处于 staged 的新引用。
    assert_eq!(
        repo.finish_credential_cleanup(&keep).await.unwrap(),
        CloudCredentialCleanupOutcome::AlreadyClean
    );
    let ready: i64 = db
        .lock()
        .query_row(
            "SELECT cleanup_ready FROM cloud_credential_cleanup WHERE credential_ref = ?1",
            rusqlite::params![keep.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(ready, 0, "staged 记录保留且未置 ready");
}

#[tokio::test]
async fn file_reopen_keeps_folders_case_and_shared_credential() {
    let dir = tempfile::Builder::new()
        .prefix("haven-cloud-folders-")
        .tempdir()
        .unwrap();
    let path = dir.path().join("cloud.db");
    let cred = fresh_cred();

    let (account_id, loc_a, loc_b) = {
        let db = Arc::new(Db::open(&path).unwrap());
        let repo = SqliteCloudStorageRepository::new(db);
        repo.stage_credential(&cred).await.unwrap();
        let account = match repo
            .connect(&connect_request("Acct-Case"), None, &cred)
            .await
            .unwrap()
        {
            CloudCasOutcome::Applied(account) => account,
            other => panic!("connect 应成功: {other:?}"),
        };
        let folder_a = register_folder(&repo, &account, "1AbCd-Ef", "Folder A").await;
        let folder_b = register_folder(&repo, &account, "1abcd-ef", "Folder B").await;
        assert_ne!(
            folder_a.location_id, folder_b.location_id,
            "大小写敏感：不同 ID 是不同位置"
        );
        assert_eq!(folder_a.provider_folder_id, "1AbCd-Ef");
        assert_eq!(folder_b.provider_folder_id, "1abcd-ef");
        assert_eq!(folder_a.account_id, account.id);
        assert_eq!(folder_b.account_id, account.id);
        // root_ref 是内部 UUID，绝不是 Provider folder ID（lower() 唯一索引不得被折叠）。
        assert_eq!(folder_a.root_ref.len(), 36);
        assert!(uuid::Uuid::parse_str(&folder_a.root_ref).is_ok());
        assert_ne!(folder_a.root_ref, folder_a.provider_folder_id);
        (account.id, folder_a.location_id, folder_b.location_id)
    };

    // 重新打开同一临时文件：账户 / 目录 / 引用全部持久化。
    let db = Arc::new(Db::open(&path).unwrap());
    let repo = SqliteCloudStorageRepository::new(db.clone());
    let locations = SqliteStorageLocationRepository::new(db);
    let account = repo
        .get_account(&account_id)
        .await
        .unwrap()
        .expect("账户持久化");
    assert!(account.connected);
    assert_eq!(account.credential_ref.as_ref(), Some(&cred));
    assert_eq!(
        repo.get_folder(loc_a)
            .await
            .unwrap()
            .unwrap()
            .provider_folder_id,
        "1AbCd-Ef"
    );
    assert_eq!(
        repo.get_folder(loc_b)
            .await
            .unwrap()
            .unwrap()
            .provider_folder_id,
        "1abcd-ef"
    );

    // 移除一个目录：共享账户凭据与同账户另一个目录不受影响。
    assert_eq!(
        repo.remove_folder(loc_a).await.unwrap(),
        CloudFolderRemoveOutcome::Removed
    );
    assert!(repo.get_folder(loc_a).await.unwrap().is_none());
    assert!(locations.get(loc_a).await.unwrap().is_none());
    assert!(repo.get_folder(loc_b).await.unwrap().is_some());
    assert!(locations.get(loc_b).await.unwrap().is_some());
    let account = repo.get_account(&account_id).await.unwrap().unwrap();
    assert!(account.connected);
    assert_eq!(
        account.credential_ref.as_ref(),
        Some(&cred),
        "移除目录不得触碰账户凭据"
    );
}

#[tokio::test]
async fn disconnect_stales_snapshot_and_preserves_availability() {
    let db = memory_db();
    let repo = SqliteCloudStorageRepository::new(db.clone());
    let resources = SqliteResourceRepository::new(db.clone());
    let locations = SqliteStorageLocationRepository::new(db);

    let (account, _) = connect_new(&repo, "Acct-Snap").await;
    let folder = register_folder(&repo, &account, "SnapFolder-1", "Snaps").await;
    let created = import_new(
        &repo,
        &folder,
        account.generation,
        &pdf("File-Snap-1", 1024),
    )
    .await;

    let snapshot = repo
        .object_snapshot(&created.id)
        .await
        .unwrap()
        .expect("快照");
    assert_eq!(snapshot.account_id, account.id);
    assert_eq!(snapshot.account_generation, account.generation);
    assert_eq!(snapshot.location_status, StorageStatus::Connected);
    assert_eq!(snapshot.object.id, created.id);
    repo.assert_current(account.generation, &snapshot)
        .await
        .unwrap();

    // 同位置用户显式状态：断开绝不覆盖。
    let mut user_marked = sample_resource(created.media_item_id, folder.location_id);
    user_marked.resource_type = ResourceType::PublicationFile;
    user_marked.locator = ResourceLocator::StorageObject {
        provider_id: folder.location_id,
        object_id: created.id.clone(),
        path_hint: None,
    };
    user_marked.mime_type = Some("application/pdf".into());
    user_marked.availability = Availability::SourceUnavailable;
    user_marked.availability_source = AvailabilitySource::User;
    resources.save(&user_marked).await.unwrap();

    let now = UtcMillis::now();
    let local = StorageLocation {
        id: StorageLocationId::new(),
        provider_type: StorageProviderType::Local,
        display_name: "本地库".into(),
        root_ref: format!("C:\\haven-cloud-local-{}", uuid::Uuid::new_v4()),
        credential_ref: None,
        status: StorageStatus::Connected,
        created_at: now,
        updated_at: now,
    };
    locations.save(&local).await.unwrap();
    let local_resource = sample_resource(created.media_item_id, local.id);
    resources.save(&local_resource).await.unwrap();

    match repo
        .disconnect(&account.id, account.generation)
        .await
        .unwrap()
    {
        CloudCasOutcome::Applied(tombstone) => {
            assert!(!tombstone.connected);
            assert_eq!(tombstone.generation, 2);
        }
        other => panic!("disconnect 应成功: {other:?}"),
    }

    let stale = repo
        .assert_current(account.generation, &snapshot)
        .await
        .unwrap_err();
    assert_eq!(stale.code().as_str(), "CLOUD_BINDING_STALE");
    let cloud_loc = locations.get(folder.location_id).await.unwrap().unwrap();
    assert_eq!(cloud_loc.status, StorageStatus::Disconnected);

    let imported = resources.get(created.resource_id).await.unwrap().unwrap();
    assert_eq!(imported.availability, Availability::StorageUnavailable);
    assert_eq!(imported.availability_source, AvailabilitySource::Storage);
    let preserved = resources.get(user_marked.id).await.unwrap().unwrap();
    assert_eq!(preserved.availability, Availability::SourceUnavailable);
    assert_eq!(preserved.availability_source, AvailabilitySource::User);

    let local_loc = locations.get(local.id).await.unwrap().unwrap();
    assert_eq!(local_loc.status, StorageStatus::Connected);
    let local_read = resources.get(local_resource.id).await.unwrap().unwrap();
    assert_eq!(local_read.availability, Availability::Available);
    assert_eq!(local_read.storage_location_id, Some(local.id));

    // 非云盘对象（本地资源 / 随机 UUID）不得伪装成云盘快照。
    assert!(
        repo.object_snapshot(&local_resource.id.to_string())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repo.object_snapshot(&uuid::Uuid::new_v4().to_string())
            .await
            .unwrap()
            .is_none()
    );
}
