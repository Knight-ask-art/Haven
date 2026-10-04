use super::*;

/// 快照 / 复核必须拒绝被改写的目录绑定：Provider ID 变化 → IO 后复核失效；root_ref 与位置
/// 不再自洽 → 快照读取本身拒绝，绝不返回半个绑定。
#[tokio::test]
async fn snapshot_rejects_mutated_folder_binding() {
    let db = memory_db();
    let repo = SqliteCloudStorageRepository::new(db.clone());
    let (account, _) = connect_new(&repo, "Acct-Folder-Mut").await;
    let folder = register_folder(&repo, &account, "MutFolder-1", "Docs").await;
    let created = import_new(&repo, &folder, account.generation, &pdf("File-Mut-1", 512)).await;
    let snapshot = repo
        .object_snapshot(&created.id)
        .await
        .unwrap()
        .expect("快照");

    // 目录身份被改写（Provider ID 变化）：复核必须拒绝旧快照。
    db.lock()
        .execute(
            "UPDATE cloud_folder_bindings SET provider_folder_id = 'MutFolder-2'
              WHERE location_id = ?1",
            rusqlite::params![folder.location_id.to_string()],
        )
        .unwrap();
    let stale = repo
        .assert_current(account.generation, &snapshot)
        .await
        .unwrap_err();
    assert_eq!(stale.code().as_str(), "CLOUD_BINDING_STALE");

    // 目录 root_ref 与位置不再自洽：快照读取本身必须拒绝，绝不返回半个绑定。
    db.lock()
        .execute(
            "UPDATE cloud_folder_bindings SET root_ref = ?1 WHERE location_id = ?2",
            rusqlite::params![
                uuid::Uuid::new_v4().to_string(),
                folder.location_id.to_string()
            ],
        )
        .unwrap();
    let invalid = repo.object_snapshot(&created.id).await.unwrap_err();
    assert_eq!(invalid.code().as_str(), "CLOUD_BINDING_INVALID");
}

#[tokio::test]
async fn concurrent_connect_cas_is_atomic_across_two_handles() {
    let dir = tempfile::Builder::new()
        .prefix("haven-cloud-race-")
        .tempdir()
        .unwrap();
    let path = dir.path().join("cloud.db");

    let db0 = Arc::new(Db::open(&path).unwrap());
    let repo0 = SqliteCloudStorageRepository::new(db0.clone());
    let (account, _) = connect_new(&repo0, "Acct-Race").await;
    assert_eq!(account.generation, 1);
    let cred_a = fresh_cred();
    let cred_b = fresh_cred();
    repo0.stage_credential(&cred_a).await.unwrap();
    repo0.stage_credential(&cred_b).await.unwrap();

    // 两个独立打开的 Db 句柄指向同一临时库；Barrier 让两个写者同时进入 CAS 缝。
    let db1 = Arc::new(Db::open(&path).unwrap());
    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for (db, cred) in [(db0.clone(), cred_a.clone()), (db1, cred_b.clone())] {
        let barrier = barrier.clone();
        handles.push(std::thread::spawn(move || {
            let repo = SqliteCloudStorageRepository::new(db);
            let request = connect_request("Acct-Race");
            barrier.wait();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            let outcome = runtime
                .block_on(repo.connect(&request, Some(1), &cred))
                .map_err(|e| e.code().as_str().to_owned());
            (cred, outcome)
        }));
    }
    let results = [
        handles.remove(0).join().unwrap(),
        handles.remove(0).join().unwrap(),
    ];

    let (mut applied, mut stale) = (0, 0);
    let (mut winner, mut loser) = (None, None);
    for (cred, outcome) in results {
        match outcome {
            Ok(CloudCasOutcome::Applied(account)) => {
                applied += 1;
                assert_eq!(account.generation, 2, "胜者必须是旧代际 + 1");
                assert_eq!(account.credential_ref.as_ref(), Some(&cred));
                winner = Some(cred);
            }
            Ok(CloudCasOutcome::Stale) => {
                stale += 1;
                loser = Some(cred);
            }
            Ok(CloudCasOutcome::Missing) => panic!("账户已存在，不得 Missing"),
            Err(code) => panic!("并发 connect 出错: {code}"),
        }
    }
    assert_eq!((applied, stale), (1, 1), "并发 CAS 必须恰好一胜一败");

    // CAS 拒绝方的事务回滚必须保留其 staged 记录（未被消耗）。
    let loser = loser.expect("loser");
    let loser_ready: i64 = db0
        .lock()
        .query_row(
            "SELECT cleanup_ready FROM cloud_credential_cleanup WHERE credential_ref = ?1",
            rusqlite::params![loser.as_str()],
            |row| row.get(0),
        )
        .expect("loser 的 staged 记录仍在");
    assert_eq!(loser_ready, 0);

    // 胜者 staged 被消耗，权威账户行指向胜者引用。
    let winner = winner.expect("winner");
    let winner_rows: i64 = db0
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM cloud_credential_cleanup WHERE credential_ref = ?1",
            rusqlite::params![winner.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(winner_rows, 0);
    let authoritative = repo0.get_account(&account.id).await.unwrap().unwrap();
    assert_eq!(authoritative.generation, 2);
    assert_eq!(authoritative.credential_ref.as_ref(), Some(&winner));
}
