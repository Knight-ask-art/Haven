//! 凭据生命周期的终态事实（真实 SQLite + 真实设置事务）。
//!
//! 覆盖：每次写入分配独立版本槽、旧槽退休、清除只影响当前槽、外部写入失败不损伤任何
//! 已存在的凭据、历史稳定引用的读取与轮换、伪造/跨 provider/串源引用的准入判定。

use haven_application::services::source_registry::SourceRegistryService;
use haven_domain::credential::CredentialStore;
use haven_domain::ids::CredentialRef;

use super::fixtures::{
    DuringWrite, FailingDeleteCache, Harness, MemoryCredentialStore, add_source, clear_credential,
    credential_ref, credential_request, current_revision, descriptor_of, len_of,
    scoped_provider_for, source_exists, write_credential,
};

#[tokio::test]
async fn each_write_allocates_an_independent_slot_and_retires_the_previous_one() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;

    write_credential(&harness.registry, &store, &source_id, "first")
        .await
        .unwrap();
    let first = credential_ref(&harness.registry, &source_id).await.unwrap();
    assert!(
        first.starts_with(&format!(
            "haven:{}:",
            scoped_provider_for("opds", &source_id)
        )),
        "槽位必须同时绑定来源种类与来源身份（provider 段 = <种类>_<来源作用域>）: {first}"
    );
    assert!(
        !first.ends_with(&source_id),
        "版本槽不得由 sourceId 拼接而成: {first}"
    );
    assert_eq!(
        store.writes(),
        vec![first.clone()],
        "外部写入只发生在刚分配的槽位上"
    );

    write_credential(&harness.registry, &store, &source_id, "second")
        .await
        .unwrap();
    let second = credential_ref(&harness.registry, &source_id).await.unwrap();
    assert_ne!(first, second, "每次写入都必须分配新的版本槽");
    assert_eq!(store.live_references(), vec![second.clone()]);
    assert!(
        !store.contains(&first),
        "被替换的旧槽位必须从系统凭据库删除"
    );
    assert!(store.deletes().contains(&first));
    assert_eq!(
        store
            .get(&second.parse::<CredentialRef>().expect("槽位必须是合法引用"))
            .await
            .unwrap()
            .unwrap()
            .expose(),
        "second"
    );

    // 采纳完成之后不留任何暂存槽或清理义务。
    let stored = harness.stored_sources().await;
    assert_eq!(len_of(&stored, "stagedCredentials"), 0);
    assert_eq!(len_of(&stored, "credentialCleanup"), 0);
    assert_eq!(len_of(&stored, "cacheCleanup"), 0);

    // 注册表只投影「是否已配置」，凭据变更推进配置版本（旧观测作废）。
    assert!(
        descriptor_of(&harness.registry, &source_id)
            .await
            .credential_configured
    );
    assert!(
        current_revision(&harness.registry, &source_id)
            .await
            .is_some()
    );
}

#[tokio::test]
async fn legacy_stable_reference_is_read_and_rotated_into_a_new_slot() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let source_id = add_source(
        &harness.registry,
        "旧书源",
        "https://legacy.example.invalid/opds",
    )
    .await;

    // 历史数据：记录里保存的是稳定引用（`haven:opds:<sourceId>`），系统凭据库里也有
    // 对应的秘密。这条路径在升级后必须继续可读。
    let legacy = format!("haven:opds:{source_id}");
    let mut payload = harness.stored_sources().await;
    let record = payload["customSources"]
        .as_array_mut()
        .expect("customSources 必须是数组")
        .iter_mut()
        .find(|record| record["sourceId"] == source_id.as_str())
        .expect("已登记的来源必须出现在负载里");
    record["credentialRef"] = serde_json::Value::String(legacy.clone());
    harness.overwrite_sources(&payload).await;
    store.seed(&legacy, "legacy-secret");

    assert_eq!(
        credential_ref(&harness.registry, &source_id).await,
        Some(legacy.clone()),
        "旧记录里的稳定引用照常被读出"
    );
    assert!(
        descriptor_of(&harness.registry, &source_id)
            .await
            .credential_configured
    );

    // 下一次写入轮换到新版本槽，旧引用被登记进清理队列并实际删除。
    write_credential(&harness.registry, &store, &source_id, "fresh")
        .await
        .unwrap();
    let rotated = credential_ref(&harness.registry, &source_id).await.unwrap();
    assert_ne!(rotated, legacy, "写入必须换成新的版本槽");
    assert!(
        store.deletes().contains(&legacy),
        "旧稳定引用必须被退休删除"
    );
    assert!(!store.contains(&legacy));
    assert_eq!(store.live_references(), vec![rotated]);
    assert_eq!(
        len_of(&harness.stored_sources().await, "credentialCleanup"),
        0
    );
}

#[tokio::test]
async fn clear_revokes_only_the_current_slot_and_leaves_other_sources_alone() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let first = add_source(&harness.registry, "A", "https://a.example.invalid/opds").await;
    let second = add_source(&harness.registry, "B", "https://b.example.invalid/opds").await;
    write_credential(&harness.registry, &store, &first, "a-secret")
        .await
        .unwrap();
    write_credential(&harness.registry, &store, &second, "b-secret")
        .await
        .unwrap();
    let first_ref = credential_ref(&harness.registry, &first).await.unwrap();
    let second_ref = credential_ref(&harness.registry, &second).await.unwrap();
    let revision_before = current_revision(&harness.registry, &second).await;

    clear_credential(&harness.registry, &store, &first)
        .await
        .unwrap();

    assert_eq!(credential_ref(&harness.registry, &first).await, None);
    assert!(!store.contains(&first_ref), "被撤销的槽位必须删除");
    assert_eq!(store.deletes(), vec![first_ref.clone()]);
    assert!(store.contains(&second_ref), "另一个来源的凭据必须完好");
    assert_eq!(
        credential_ref(&harness.registry, &second).await,
        Some(second_ref),
        "串源删除会造成另一个来源静默失去凭据"
    );
    assert_eq!(
        current_revision(&harness.registry, &second).await,
        revision_before,
        "清除一个来源的凭据不得推进其它来源的配置版本"
    );
    assert!(
        !descriptor_of(&harness.registry, &first)
            .await
            .credential_configured
    );
    assert_eq!(
        len_of(&harness.stored_sources().await, "credentialCleanup"),
        0
    );

    // 已经清空之后重复清除是幂等的：不写库、不产生新版本、不触碰任何东西。
    let revision_after = current_revision(&harness.registry, &first).await;
    clear_credential(&harness.registry, &store, &first)
        .await
        .unwrap();
    assert_eq!(
        current_revision(&harness.registry, &first).await,
        revision_after
    );
    assert_eq!(store.deletes(), vec![first_ref]);
}

#[tokio::test]
async fn a_failed_external_write_damages_neither_the_previous_credential_nor_the_record() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;
    write_credential(&harness.registry, &store, &source_id, "keeper")
        .await
        .unwrap();
    let kept = credential_ref(&harness.registry, &source_id).await.unwrap();
    let revision_before = current_revision(&harness.registry, &source_id).await;

    store.fail_writes(true);
    let error = write_credential(&harness.registry, &store, &source_id, "rejected")
        .await
        .expect_err("外部写入失败必须如实返回错误");
    store.fail_writes(false);
    assert_eq!(error.code().as_str(), "CREDENTIAL_ACCESS_FAILED");

    assert_eq!(
        credential_ref(&harness.registry, &source_id).await,
        Some(kept.clone()),
        "失败的写入不得改动当前引用"
    );
    assert_eq!(
        store.live_references(),
        vec![kept.clone()],
        "失败的写入既不能留下自己的槽位，也不能删掉旧凭据"
    );
    assert_eq!(
        store.deletes().len(),
        1,
        "失败方只能删除自己刚分配的那一个槽位"
    );
    assert!(!store.deletes().contains(&kept));
    assert_eq!(
        current_revision(&harness.registry, &source_id).await,
        revision_before,
        "失败的采纳不得推进配置版本（旧观测仍然描述当前来源）"
    );
    let stored = harness.stored_sources().await;
    assert_eq!(len_of(&stored, "stagedCredentials"), 0, "暂存槽必须被清理");
    assert_eq!(
        len_of(&stored, "credentialCleanup"),
        0,
        "没写进去的槽位不留清理义务"
    );
}

/// 外部写入期间来源被改配置（端点变了 → 配置版本推进）：这次写入作废，只删自己刚写进
/// 去的那个槽位；已经存在的凭据完全不受影响（没有任何「还原旧秘密」的分支）。
#[tokio::test]
async fn a_config_change_during_the_external_write_invalidates_only_the_new_slot() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;
    write_credential(&harness.registry, &store, &source_id, "keeper")
        .await
        .unwrap();
    let keeper = credential_ref(&harness.registry, &source_id).await.unwrap();

    store.during_write(DuringWrite::UpdateEndpoint {
        registry: harness.registry.clone(),
        source_id: source_id.clone(),
        endpoint: "https://two.example.invalid/opds".to_owned(),
    });
    let error = write_credential(&harness.registry, &store, &source_id, "rejected")
        .await
        .expect_err("配置版本已变时这次写入必须作废");
    assert_eq!(error.code().as_str(), "SOURCE_CONFIG_STALE");

    assert_eq!(
        credential_ref(&harness.registry, &source_id).await,
        Some(keeper.clone()),
        "旧凭据必须保持不动"
    );
    assert_eq!(store.live_references(), vec![keeper.clone()]);
    assert!(!store.deletes().contains(&keeper));
    assert_eq!(
        store.deletes().len(),
        1,
        "作废的写入只能删除自己刚写进去的那一个槽位"
    );
    assert_eq!(
        harness
            .registry
            .endpoint(&source_id)
            .await
            .unwrap()
            .as_deref(),
        Some("https://two.example.invalid/opds"),
        "并发的配置写入必须落地"
    );
    let stored = harness.stored_sources().await;
    assert_eq!(len_of(&stored, "stagedCredentials"), 0);
    assert_eq!(len_of(&stored, "credentialCleanup"), 0);
}

/// 数据库在**暂存**这一步就失败：外部存储一个字节都不许写，旧凭据毫发无损。
#[tokio::test]
async fn a_database_failure_before_staging_never_touches_the_credential_store() {
    use std::sync::Arc;

    use super::fixtures::FlakySettingsUoW;

    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;
    write_credential(&harness.registry, &store, &source_id, "keeper")
        .await
        .unwrap();
    let keeper = credential_ref(&harness.registry, &source_id).await.unwrap();

    let uow = Arc::new(FlakySettingsUoW::new(harness.db.clone()));
    // 第一次写事务就是暂存：它失败，于是这次写入在任何外部 IO 之前就结束。
    uow.fail_run_at(0);
    let registry = harness.with_uow(uow.clone());

    let error = write_credential(&registry, &store, &source_id, "rejected")
        .await
        .expect_err("暂存失败必须如实返回错误");
    assert_eq!(error.code().as_str(), "DATABASE_ERROR");
    assert_eq!(
        store.writes().len(),
        1,
        "外部存储只应该有先前那一次成功写入，暂存失败后一次都不能再写"
    );
    assert_eq!(
        credential_ref(&registry, &source_id).await,
        Some(keeper.clone())
    );
    assert_eq!(store.live_references(), vec![keeper]);

    // 数据库恢复之后，同一次操作可以从头再来。
    uow.heal();
    write_credential(&registry, &store, &source_id, "retried")
        .await
        .unwrap();
    assert_ne!(
        credential_ref(&registry, &source_id).await,
        None,
        "恢复后必须能正常写入"
    );
}

/// 数据库在**采纳**这一步失败：暂存记录留下（中断的写入仍有清理所有者），只有这次
/// 写入自己的槽位被删掉，已经存在的凭据完全不受影响。
#[tokio::test]
async fn a_database_failure_at_adoption_only_removes_its_own_slot() {
    use std::sync::Arc;

    use super::fixtures::FlakySettingsUoW;

    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;
    write_credential(&harness.registry, &store, &source_id, "keeper")
        .await
        .unwrap();
    let keeper = credential_ref(&harness.registry, &source_id).await.unwrap();

    let uow = Arc::new(FlakySettingsUoW::new(harness.db.clone()));
    // 第 0 次是暂存（成功），第 1 次是采纳（失败）。
    uow.fail_run_at(1);
    let registry = harness.with_uow(uow.clone());

    let error = write_credential(&registry, &store, &source_id, "rejected")
        .await
        .expect_err("采纳失败必须如实返回错误");
    assert_eq!(error.code().as_str(), "DATABASE_ERROR");

    assert_eq!(
        credential_ref(&registry, &source_id).await,
        Some(keeper.clone()),
        "旧凭据必须保持不动"
    );
    assert_eq!(store.live_references(), vec![keeper.clone()]);
    assert!(!store.deletes().contains(&keeper));
    assert_eq!(
        store.deletes().len(),
        1,
        "作废的写入只能删除自己刚写进去的那一个槽位"
    );
    let stored = harness.stored_sources().await;
    assert_eq!(
        len_of(&stored, "stagedCredentials"),
        0,
        "补偿会把这次写入的暂存记录撤下来"
    );
    assert_eq!(
        len_of(&stored, "credentialCleanup"),
        0,
        "补偿删除成功之后不留清理义务"
    );
}

#[tokio::test]
async fn a_failed_external_delete_keeps_a_durable_obligation_across_restart() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let cache = FailingDeleteCache::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;
    write_credential(&harness.registry, &store, &source_id, "pw")
        .await
        .unwrap();
    let revoked = credential_ref(&harness.registry, &source_id).await.unwrap();

    store.fail_deletes(true);
    let error = clear_credential(&harness.registry, &store, &source_id)
        .await
        .expect_err("外部删除失败必须如实返回错误，而不是谎报已清除");
    assert_eq!(error.code().as_str(), "CREDENTIAL_ACCESS_FAILED");
    store.fail_deletes(false);

    assert_eq!(
        credential_ref(&harness.registry, &source_id).await,
        None,
        "撤销是权威事实：引用已经在同一事务里被清掉"
    );
    assert!(store.contains(&revoked), "外部删除确实没有发生");
    let stored = harness.stored_sources().await;
    assert_eq!(
        len_of(&stored, "credentialCleanup"),
        1,
        "清理义务必须已经落库"
    );
    assert_eq!(
        stored["credentialCleanup"][0]["reference"],
        serde_json::Value::String(revoked.clone())
    );

    // 重启等价：新实例在同一个数据库上继续把义务做完。
    let restarted = harness.restart();
    let report = restarted
        .retry_pending_cleanup(&store, cache.inner(), haven_common::UtcMillis::now())
        .await
        .unwrap();
    assert_eq!(report.credentials_deleted, 1);
    assert_eq!(report.credentials_settled, 1);
    assert_eq!(report.failed, 0);
    assert_eq!(report.credentials_remaining, 0);
    assert!(!store.contains(&revoked), "重试之后秘密必须真的不在");
    assert_eq!(
        len_of(&harness.stored_sources().await, "credentialCleanup"),
        0
    );
}

#[tokio::test]
async fn removing_a_source_keeps_cleanup_retryable_and_reports_the_truth() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let cache = FailingDeleteCache::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;
    write_credential(&harness.registry, &store, &source_id, "pw")
        .await
        .unwrap();
    let credential = credential_ref(&harness.registry, &source_id).await.unwrap();

    store.fail_deletes(true);
    harness
        .registry
        .remove_custom_source(&source_id, &store, cache.inner())
        .await
        .expect_err("外部删除失败必须如实返回错误");
    store.fail_deletes(false);

    // 记录已经按用户意图移除，清理义务仍然留在持久化队列里。
    assert!(!source_exists(&harness.registry, &source_id).await);
    assert!(store.contains(&credential));
    let stored = harness.stored_sources().await;
    assert_eq!(len_of(&stored, "credentialCleanup"), 1);

    // 重试：记录已经不存在，但义务还在——这次调用必须把它做完，而不是回 RESOURCE_NOT_FOUND。
    let removed = harness
        .registry
        .remove_custom_source(&source_id, &store, cache.inner())
        .await
        .expect("遗留清理义务必须仍然可以被重试");
    assert!(removed.credential_deleted);
    assert!(!store.contains(&credential));
    assert_eq!(
        len_of(&harness.stored_sources().await, "credentialCleanup"),
        0
    );
    assert!(!source_exists(&harness.registry, &source_id).await);
}

#[tokio::test]
async fn removing_an_unknown_source_without_obligations_is_not_found() {
    use haven_application::services::source_config_cache::InMemorySourceConfigCache;

    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let cache = InMemorySourceConfigCache::new();
    let error = harness
        .registry
        .remove_custom_source("custom_missing0000", &store, &cache)
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "RESOURCE_NOT_FOUND");
    assert!(store.deletes().is_empty());
}

#[tokio::test]
async fn forged_and_cross_provider_references_are_neither_read_nor_deleted() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let cache = FailingDeleteCache::new();
    let forged_owner = add_source(
        &harness.registry,
        "被篡改的书源",
        "https://tampered.example.invalid/opds",
    )
    .await;
    let honest = add_source(
        &harness.registry,
        "正常书源",
        "https://honest.example.invalid/opds",
    )
    .await;
    write_credential(&harness.registry, &store, &honest, "honest-secret")
        .await
        .unwrap();
    let honest_ref = credential_ref(&harness.registry, &honest).await.unwrap();

    // 畸形状态：OPDS 书源的记录里被塞进一条 komga 命名空间的引用，而且系统凭据库里
    // 真的存在这个 target。读取与清理都不许把它当作这里的能力。
    let foreign =
        CredentialRef::new_scoped("komga", &uuid::Uuid::new_v4().simple().to_string()).unwrap();
    store.seed(foreign.as_str(), "foreign-secret");
    let mut payload = harness.stored_sources().await;
    let record = payload["customSources"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|record| record["sourceId"] == forged_owner.as_str())
        .unwrap();
    record["credentialRef"] = serde_json::Value::String(foreign.as_str().to_owned());
    harness.overwrite_sources(&payload).await;

    let error = harness
        .registry
        .custom_credential_ref(&forged_owner)
        .await
        .expect_err("不归属该来源种类的引用必须被拒绝，而不是交给出站请求路径");
    assert_eq!(error.code().as_str(), "INVALID_ARGUMENT");
    assert!(
        !error.user_message().contains(foreign.as_str()),
        "错误文案不得回显凭据 target"
    );
    assert!(
        !descriptor_of(&harness.registry, &forged_owner)
            .await
            .credential_configured,
        "伪造引用不得被投影成一项真实能力"
    );

    let removed = harness
        .registry
        .remove_custom_source(&forged_owner, &store, cache.inner())
        .await
        .unwrap();
    assert!(!removed.credential_deleted);
    assert!(store.contains(foreign.as_str()), "不归属的引用绝不能被删除");
    assert!(
        !store.deletes().contains(&foreign.as_str().to_owned()),
        "删除路径不得碰任何不属于这里的 target"
    );
    assert!(store.contains(&honest_ref), "另一个来源的凭据必须完好");
    assert!(!source_exists(&harness.registry, &forged_owner).await);
    assert!(source_exists(&harness.registry, &honest).await);
}

#[tokio::test]
async fn kinds_without_a_credential_consumer_reject_writes_and_clears() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let feed = harness
        .registry
        .add_feed_source("我的订阅", "https://feeds.example.invalid/rss.xml")
        .await
        .unwrap()
        .source_id;
    let tvbox = harness
        .registry
        .add_tvbox_source("电视源", "https://config.example.invalid/tvbox.json")
        .await
        .unwrap()
        .source_id;

    for source_id in [&feed, &tvbox] {
        for secret in [Some("s3cret"), None] {
            let error = harness
                .registry
                .set_custom_source_credential(&credential_request(source_id, secret), &store)
                .await
                .expect_err("没有凭据消费者的来源必须拒绝凭据路径");
            assert_eq!(error.code().as_str(), "INVALID_ARGUMENT");
        }
    }
    assert!(store.writes().is_empty(), "被拒绝的写入不得触达系统凭据库");
    assert!(store.deletes().is_empty());
    let stored = harness.stored_sources().await;
    assert_eq!(len_of(&stored, "stagedCredentials"), 0);
    assert_eq!(len_of(&stored, "credentialCleanup"), 0);
}

/// 显式停用一个来源不会清理它的凭据：凭据属于来源身份，只随来源删除或用户显式清除。
#[tokio::test]
async fn disabling_a_source_never_touches_its_credential() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;
    write_credential(&harness.registry, &store, &source_id, "pw")
        .await
        .unwrap();
    let reference = credential_ref(&harness.registry, &source_id).await.unwrap();

    harness
        .registry
        .set_custom_source_enabled(&source_id, false)
        .await
        .unwrap();
    assert_eq!(
        credential_ref(&harness.registry, &source_id).await,
        Some(reference.clone())
    );
    assert!(store.contains(&reference));
    assert!(!descriptor_of(&harness.registry, &source_id).await.enabled);
}

/// 注册表在不注入设置事务时也能完成同样的终态（生产装配必须注入；这条只钉住语义）。
#[tokio::test]
async fn credential_lifecycle_without_a_write_channel_still_reaches_the_same_terminal_state() {
    use std::sync::Arc;

    use haven_application::services::source_config_cache::InMemorySourceConfigCache;
    use haven_application::services::source_registry::InMemorySourceCredentialLease;
    use haven_infrastructure::Db;
    use haven_infrastructure::db::repos::SqliteRepositories;

    let db = Arc::new(Db::open_in_memory().unwrap());
    let registry = SourceRegistryService::new(Arc::new(SqliteRepositories::new(db)))
        .with_credential_lease(Arc::new(InMemorySourceCredentialLease::new()));
    let store = MemoryCredentialStore::new();
    let source_id = add_source(
        &registry,
        "无事务装配",
        "https://plain.example.invalid/opds",
    )
    .await;

    write_credential(&registry, &store, &source_id, "first")
        .await
        .unwrap();
    let first = credential_ref(&registry, &source_id).await.unwrap();
    write_credential(&registry, &store, &source_id, "second")
        .await
        .unwrap();
    let second = credential_ref(&registry, &source_id).await.unwrap();
    assert_ne!(first, second);
    assert_eq!(store.live_references(), vec![second]);

    registry
        .remove_custom_source(&source_id, &store, &InMemorySourceConfigCache::new())
        .await
        .unwrap();
    assert!(store.live_references().is_empty());
    assert!(!source_exists(&registry, &source_id).await);
}

/// **另一个来源**的同种类槽位（provider 段形状合法、但来源作用域不属于本来源）：读取、
/// 投影与删除都不许把它当成这里的能力，更不许拿它去删系统凭据库。
#[tokio::test]
async fn a_slot_scoped_to_another_source_of_the_same_kind_is_never_read_or_deleted() {
    use haven_application::services::source_config_cache::InMemorySourceConfigCache;

    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let owner = add_source(
        &harness.registry,
        "真正的书源",
        "https://owner.example.invalid/opds",
    )
    .await;
    let tampered = add_source(
        &harness.registry,
        "被篡改的书源",
        "https://tampered.example.invalid/opds",
    )
    .await;
    write_credential(&harness.registry, &store, &owner, "owner-secret")
        .await
        .unwrap();
    let owner_ref = credential_ref(&harness.registry, &owner).await.unwrap();

    // 畸形状态：把 owner 的**真实**槽位塞进 tampered 的记录里。两条引用的 provider 段
    // 形状完全一样（同为 opds 家族），只有来源作用域不同——这正是「仅凭 provider 推断
    // 归属」会踩中的坑。
    let mut payload = harness.stored_sources().await;
    let record = payload["customSources"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|record| record["sourceId"] == tampered.as_str())
        .unwrap();
    record["credentialRef"] = serde_json::Value::String(owner_ref.clone());
    harness.overwrite_sources(&payload).await;

    let error = harness
        .registry
        .custom_credential_ref(&tampered)
        .await
        .expect_err("不属于本来源的槽位必须被拒绝，而不是交给出站请求路径");
    assert_eq!(error.code().as_str(), "INVALID_ARGUMENT");
    assert!(
        !descriptor_of(&harness.registry, &tampered)
            .await
            .credential_configured,
        "串源引用不得被投影成一项真实能力"
    );

    let removed = harness
        .registry
        .remove_custom_source(&tampered, &store, &InMemorySourceConfigCache::new())
        .await
        .unwrap();
    assert!(!removed.credential_deleted);
    assert!(store.contains(&owner_ref), "另一个来源的凭据必须完好无损");
    assert!(!store.deletes().contains(&owner_ref));
    assert_eq!(
        credential_ref(&harness.registry, &owner).await,
        Some(owner_ref)
    );
    assert_eq!(
        len_of(&harness.stored_sources().await, "credentialCleanup"),
        0,
        "串源引用不得被登记成清理义务"
    );
}

/// **轮换**一条畸形引用：记录里的旧引用指向别的 provider（这里是 AI 家族），系统凭据库里
/// 也真的有它。轮换只替换引用本身，绝不把那条外来秘密登记进清理队列、更不删除它。
#[tokio::test]
async fn rotating_a_foreign_family_reference_leaves_the_foreign_secret_untouched() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let source_id = add_source(
        &harness.registry,
        "被篡改的书源",
        "https://tampered.example.invalid/opds",
    )
    .await;
    let foreign =
        CredentialRef::new_scoped("ai", &uuid::Uuid::new_v4().simple().to_string()).unwrap();
    store.seed(foreign.as_str(), "ai-secret");
    let mut payload = harness.stored_sources().await;
    let record = payload["customSources"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|record| record["sourceId"] == source_id.as_str())
        .unwrap();
    record["credentialRef"] = serde_json::Value::String(foreign.as_str().to_owned());
    harness.overwrite_sources(&payload).await;

    write_credential(&harness.registry, &store, &source_id, "fresh")
        .await
        .unwrap();
    let adopted = credential_ref(&harness.registry, &source_id).await.unwrap();
    assert_ne!(adopted, foreign.as_str(), "写入必须换成新的来源作用域槽位");
    assert!(
        store.contains(foreign.as_str()),
        "轮换一条不属于这里的引用绝不能删除它"
    );
    assert!(
        !store.deletes().contains(&foreign.as_str().to_owned()),
        "删除路径不得碰任何不属于这里的 target"
    );
    assert_eq!(
        len_of(&harness.stored_sources().await, "credentialCleanup"),
        0,
        "外来引用不得被登记成清理义务"
    );
    let mut live = store.live_references();
    live.sort();
    let mut expected = vec![adopted, foreign.as_str().to_owned()];
    expected.sort();
    assert_eq!(live, expected);
}

/// 没有注入租约的装配面：写入、清除与移除一律 fail closed，且不产生任何外部副作用。
#[tokio::test]
async fn every_credential_mutation_fails_closed_without_a_lease() {
    use haven_application::services::source_config_cache::InMemorySourceConfigCache;

    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;
    write_credential(&harness.registry, &store, &source_id, "keeper")
        .await
        .unwrap();
    let keeper = credential_ref(&harness.registry, &source_id).await.unwrap();
    let lease_free = harness.without_lease();

    let mut errors = Vec::new();
    errors.push(
        write_credential(&lease_free, &store, &source_id, "rejected")
            .await
            .unwrap_err(),
    );
    errors.push(
        clear_credential(&lease_free, &store, &source_id)
            .await
            .unwrap_err(),
    );
    errors.push(
        lease_free
            .remove_custom_source(&source_id, &store, &InMemorySourceConfigCache::new())
            .await
            .unwrap_err(),
    );
    for error in &errors {
        assert_eq!(error.code().as_str(), "SOURCE_CREDENTIAL_LEASE_UNAVAILABLE");
    }

    // 只读入口照常可用，且既有凭据毫发无损。
    assert_eq!(
        credential_ref(&lease_free, &source_id).await,
        Some(keeper.clone())
    );
    assert_eq!(store.live_references(), vec![keeper]);
    assert!(store.deletes().is_empty());
    assert!(source_exists(&lease_free, &source_id).await);
}

// ---------- 补偿同时失败时的持久化所有权（R3） ----------

/// 采纳失败 **且** 补偿删除也失败：主错误如实上抛，清理义务留在持久化队列里跨重启存活，
/// 系统凭据库恢复之后由扫描把它做完。
#[tokio::test]
async fn an_adoption_failure_with_a_failing_compensation_delete_still_settles_after_restart() {
    use std::sync::Arc;

    use haven_application::services::source_config_cache::InMemorySourceConfigCache;

    use super::fixtures::FlakySettingsUoW;

    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;
    // 外部删除全部失败：补偿阶段的删除同样删不掉。
    store.fail_deletes(true);

    let uow = Arc::new(FlakySettingsUoW::new(harness.db.clone()));
    // 第 0 次写事务是暂存（成功），第 1 次是采纳（失败）。
    uow.fail_run_at(1);
    let registry = harness.with_uow(uow.clone());

    let error = write_credential(&registry, &store, &source_id, "rejected")
        .await
        .expect_err("采纳失败必须如实返回错误");
    assert_eq!(
        error.code().as_str(),
        "DATABASE_ERROR",
        "主错误与主文案永不被补偿失败替换"
    );

    // 补偿删除失败不丢义务：暂存记录已经换成一条持久化的清理义务。
    let stored = harness.stored_sources().await;
    assert_eq!(
        len_of(&stored, "stagedCredentials"),
        0,
        "补偿事务把这次写入的暂存记录换成了清理义务"
    );
    assert_eq!(
        len_of(&stored, "credentialCleanup"),
        1,
        "补偿删除失败时义务必须留在队列里"
    );
    let pending = stored["credentialCleanup"][0]["reference"]
        .as_str()
        .expect("清理义务必须带引用")
        .to_owned();
    assert!(store.contains(&pending), "秘密确实还没有被物理删除");
    assert_eq!(
        credential_ref(&registry, &source_id).await,
        None,
        "失败的写入不得被采纳"
    );

    // 重建装配面（真实事务 + 恢复可用的系统凭据库）：义务最终被做完。
    uow.heal();
    store.fail_deletes(false);
    let restarted = harness.restart();
    let report = restarted
        .retry_pending_cleanup(
            &store,
            &InMemorySourceConfigCache::new(),
            haven_common::UtcMillis::now(),
        )
        .await
        .unwrap();
    assert_eq!(report.credentials_deleted, 1);
    assert_eq!(report.failed, 0);
    assert_eq!(report.credentials_remaining, 0);
    assert!(!store.contains(&pending), "重试之后秘密必须真的不在");
    assert_eq!(
        len_of(&harness.stored_sources().await, "credentialCleanup"),
        0
    );
}

/// 记录清理义务的那次事务也失败：**原来的暂存记录必须原样保留**——它就是那个已经写进
/// 系统凭据库的槽位的清理所有者。即使补偿删除同样失败，中断的写入仍然有主；租约释放之后
/// 由重建的装配面把它清干净。
#[tokio::test]
async fn a_failed_obligation_recording_keeps_the_original_staged_owner() {
    use std::sync::Arc;

    use haven_application::services::source_config_cache::InMemorySourceConfigCache;
    use haven_application::services::source_registry::SOURCE_CREDENTIAL_STAGE_TTL_MS;

    use super::fixtures::FlakySettingsUoW;

    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;
    store.fail_deletes(true);

    let uow = Arc::new(FlakySettingsUoW::new(harness.db.clone()));
    // 第 0 次是暂存（成功），第 1 次是采纳（失败），第 2 次是补偿里的义务登记（也失败）。
    uow.fail_runs_at(&[1, 2]);
    let registry = harness.with_uow(uow.clone());

    let error = write_credential(&registry, &store, &source_id, "rejected")
        .await
        .expect_err("采纳失败必须如实返回错误");
    assert_eq!(error.code().as_str(), "DATABASE_ERROR");

    // 义务登记失败时**不得**先把暂存记录撤下来：那条记录就是唯一的清理所有者。
    let stored = harness.stored_sources().await;
    assert_eq!(
        len_of(&stored, "stagedCredentials"),
        1,
        "登记义务失败时原来的暂存记录必须原样保留"
    );
    assert_eq!(
        len_of(&stored, "credentialCleanup"),
        0,
        "登记失败的事务不得留下半条义务"
    );
    let staged = stored["stagedCredentials"][0]["reference"]
        .as_str()
        .expect("暂存记录必须带引用")
        .to_owned();
    assert!(
        store.contains(&staged),
        "外部写入确实已经发生：秘密还在，必须仍然有主"
    );
    assert_eq!(credential_ref(&registry, &source_id).await, None);

    // 数据库与系统凭据库恢复之后：租约已经随失败的那次调用释放，扫描把这条过期的暂存槽
    // 转入清理队列并真实删除。
    uow.heal();
    store.fail_deletes(false);
    let restarted = harness.restart();
    let far_future = haven_common::UtcMillis(
        haven_common::UtcMillis::now().0 + SOURCE_CREDENTIAL_STAGE_TTL_MS * 1_000,
    );
    let report = restarted
        .retry_pending_cleanup(&store, &InMemorySourceConfigCache::new(), far_future)
        .await
        .expect("租约释放之后扫描必须能运行");
    assert_eq!(report.stages_retired, 1);
    assert_eq!(report.credentials_deleted, 1);
    assert_eq!(report.staged_remaining, 0);
    assert_eq!(report.credentials_remaining, 0);
    assert!(!store.contains(&staged), "被中断的写入必须被清干净");
    assert_eq!(
        len_of(&harness.stored_sources().await, "stagedCredentials"),
        0
    );
}
