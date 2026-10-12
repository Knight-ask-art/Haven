//! 持久化清理义务的扫描：有界批量、幂等、跨重启、失败保留、暂存槽过期、归属与在用守卫、
//! 以及「没有租约就没有清理」。
//!
//! 这些用例断言的是落库负载（`credentialCleanup` / `cacheCleanup` / `stagedCredentials`）
//! 与系统凭据库的**终态一致**：没有任何一条义务会被静默丢掉，也没有任何一个仍在使用的
//! 槽位会被删掉，更没有任何一条不属于这里的 target 会被碰。

use haven_application::services::source_config_cache::{
    InMemorySourceConfigCache, SourceConfigCache,
};
use haven_application::services::source_registry::{
    MAX_CREDENTIAL_CLEANUP, SOURCE_CREDENTIAL_CLEANUP_BATCH, SOURCE_CREDENTIAL_STAGE_TTL_MS,
};
use haven_common::UtcMillis;
use haven_domain::ids::CredentialRef;

use super::fixtures::{
    DuringWrite, FailingDeleteCache, FlakySettingsUoW, Harness, MemoryCredentialStore, add_source,
    credential_ref, len_of, source_exists, source_scoped_slot, write_credential,
};

/// 把一批待清理条目写进落库负载。
async fn install_cleanup_queue(harness: &Harness, source_id: &str, references: &[String]) {
    let mut payload = harness.stored_sources().await;
    payload["credentialCleanup"] = serde_json::Value::Array(
        references
            .iter()
            .map(|reference| serde_json::json!({ "reference": reference, "sourceId": source_id }))
            .collect(),
    );
    harness.overwrite_sources(&payload).await;
}

/// 造一个「被中断的写入」：槽位已经写进系统凭据库，暂存记录停在有效期之前很久。
async fn install_abandoned_stage(
    harness: &Harness,
    store: &MemoryCredentialStore,
    source_id: &str,
    now: UtcMillis,
) -> String {
    let reference = source_scoped_slot("opds", source_id);
    store.seed(&reference, "abandoned-secret");
    let mut payload = harness.stored_sources().await;
    let mut staged = payload["stagedCredentials"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    staged.push(serde_json::json!({
        "reference": reference.clone(),
        "sourceId": source_id,
        "stagedAtMs": now.0 - SOURCE_CREDENTIAL_STAGE_TTL_MS - 1,
    }));
    payload["stagedCredentials"] = serde_json::Value::Array(staged);
    harness.overwrite_sources(&payload).await;
    reference
}

/// 反复有界扫描直到队列收敛。每一轮都必须真实前进，否则在有界轮数内就会 panic——
/// 「下一轮就好」不能是一个永远不到来的承诺。
async fn sweep_until_settled(
    harness: &Harness,
    store: &MemoryCredentialStore,
    cache: &InMemorySourceConfigCache,
    now: UtcMillis,
) {
    for _ in 1..=16 {
        let report = harness
            .registry
            .retry_pending_cleanup(store, cache, now)
            .await
            .unwrap();
        if report.credentials_remaining == 0
            && report.staged_remaining == 0
            && report.caches_remaining == 0
        {
            return;
        }
    }
    panic!("清理义务必须在有界轮数内收敛");
}

#[tokio::test]
async fn an_expired_staging_slot_is_retired_and_cleaned_while_a_fresh_one_is_kept() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let cache = InMemorySourceConfigCache::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;

    // 一个被中断的写入：槽位已经写进系统凭据库，暂存记录却停留在很久以前。
    let abandoned = source_scoped_slot("opds", &source_id);
    store.seed(&abandoned, "abandoned-secret");
    // 一个仍然在途的写入：刚刚才暂存。
    let in_flight = source_scoped_slot("opds", &source_id);
    store.seed(&in_flight, "in-flight-secret");

    let now = UtcMillis::now();
    let mut payload = harness.stored_sources().await;
    payload["stagedCredentials"] = serde_json::json!([
        {
            "reference": abandoned.clone(),
            "sourceId": source_id.clone(),
            "stagedAtMs": now.0 - SOURCE_CREDENTIAL_STAGE_TTL_MS - 1,
        },
        {
            "reference": in_flight.clone(),
            "sourceId": source_id.clone(),
            "stagedAtMs": now.0,
        },
    ]);
    harness.overwrite_sources(&payload).await;

    let report = harness
        .registry
        .retry_pending_cleanup(&store, &cache, now)
        .await
        .unwrap();

    assert_eq!(report.stages_retired, 1, "只有过期的那一条被转入清理队列");
    assert_eq!(report.credentials_deleted, 1);
    assert_eq!(report.staged_remaining, 1, "在途的暂存槽不得被回收");
    assert_eq!(report.credentials_remaining, 0);
    assert!(!store.contains(&abandoned), "被中断的写入必须被清干净");
    assert!(store.contains(&in_flight), "在途的写入不得被扫描删掉");

    let stored = harness.stored_sources().await;
    assert_eq!(len_of(&stored, "stagedCredentials"), 1);
    assert_eq!(stored["stagedCredentials"][0]["reference"], in_flight);

    // 再扫一次：nothing left to do，且不会误删在途槽位。
    let report = harness
        .registry
        .retry_pending_cleanup(&store, &cache, now)
        .await
        .unwrap();
    assert_eq!(report.stages_retired, 0);
    assert_eq!(report.credentials_deleted, 0);
    assert!(store.contains(&in_flight));
}

#[tokio::test]
async fn a_sweep_is_idempotent_and_keeps_failed_entries_for_retry() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let cache = InMemorySourceConfigCache::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;

    let deletable = source_scoped_slot("opds", &source_id);
    let stuck = source_scoped_slot("opds", &source_id);
    store.seed(&deletable, "a");
    store.seed(&stuck, "b");
    store.fail_delete_for(&stuck);
    install_cleanup_queue(&harness, &source_id, &[deletable.clone(), stuck.clone()]).await;

    let report = harness
        .registry
        .retry_pending_cleanup(&store, &cache, UtcMillis::now())
        .await
        .unwrap();
    assert_eq!(report.credentials_deleted, 1);
    assert_eq!(report.failed, 1, "删不掉的条目必须如实计入失败");
    assert_eq!(report.credentials_remaining, 1, "失败的条目必须留在队列里");
    assert!(store.contains(&stuck));
    let stored = harness.stored_sources().await;
    assert_eq!(stored["credentialCleanup"][0]["reference"], stuck);

    // 幂等：再扫一次不会误删已经处理完的东西，且失败条目仍然等着重试。
    let report = harness
        .registry
        .retry_pending_cleanup(&store, &cache, UtcMillis::now())
        .await
        .unwrap();
    assert_eq!(report.credentials_deleted, 0);
    assert_eq!(report.credentials_settled, 0);
    assert_eq!(report.credentials_remaining, 1);
}

#[tokio::test]
async fn the_sweep_batch_is_bounded_and_keeps_entries_added_during_the_sweep() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let cache = InMemorySourceConfigCache::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;
    let bystander = add_source(
        &harness.registry,
        "另一个书源",
        "https://two.example.invalid/opds",
    )
    .await;
    write_credential(&harness.registry, &store, &bystander, "bystander-secret")
        .await
        .unwrap();
    let bystander_ref = credential_ref(&harness.registry, &bystander).await.unwrap();

    // 比一次批量上限还多的待清理条目。
    let total = SOURCE_CREDENTIAL_CLEANUP_BATCH + 6;
    let mut references = Vec::new();
    for _ in 0..total {
        let reference = source_scoped_slot("opds", &source_id);
        store.seed(&reference, "secret");
        references.push(reference);
    }
    install_cleanup_queue(&harness, &source_id, &references).await;

    // 扫描进行中：绕过租约的**非合规**写者撤销了另一个来源的凭据，于是队列里真实地多出
    // 一条义务。扫描必须保留它，而不是用读取时的陈旧快照覆盖回去。
    store.during_write(DuringWrite::ForgeRevokedCredential {
        repos: harness.repos.clone(),
        source_id: bystander.clone(),
    });

    let report = harness
        .registry
        .retry_pending_cleanup(&store, &cache, UtcMillis::now())
        .await
        .unwrap();
    assert_eq!(
        report.credentials_deleted,
        SOURCE_CREDENTIAL_CLEANUP_BATCH as u32
    );
    assert_eq!(
        report.credentials_remaining as usize,
        total - SOURCE_CREDENTIAL_CLEANUP_BATCH + 1,
        "未处理完的条目与扫描期间新增的条目都必须留在队列里"
    );
    let stored = harness.stored_sources().await;
    let queued: Vec<String> = stored["credentialCleanup"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["reference"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        queued.contains(&bystander_ref),
        "扫描期间新增的清理义务不得被丢掉: {queued:?}"
    );
    assert_eq!(queued.len(), total - SOURCE_CREDENTIAL_CLEANUP_BATCH + 1);

    // 恢复系统凭据库之后，剩下的义务在下一次扫描里全部做完。
    store.allow_all_deletes();
    let report = harness
        .registry
        .retry_pending_cleanup(&store, &cache, UtcMillis::now())
        .await
        .unwrap();
    assert_eq!(report.credentials_remaining, 0);
    assert!(!store.contains(&bystander_ref));
    assert_eq!(store.live_references(), Vec::<String>::new());
}

/// 清理队列是可以被伪造/被手工改写的落库数据。任何一条**不属于**该来源的条目都必须被
/// 守卫拦下：既不删除、也不出队、更不会被静默丢弃，而且绝不拿它去碰系统凭据库。
#[tokio::test]
async fn a_foreign_or_forged_cleanup_entry_is_neither_deleted_nor_dropped() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let cache = InMemorySourceConfigCache::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;
    let other_source = add_source(
        &harness.registry,
        "另一个书源",
        "https://two.example.invalid/opds",
    )
    .await;

    // 三种都不属于 `source_id`：同种类但作用域是另一个来源的槽位、AI 家族的 target、
    // 以及 provider 段对但 profile 不是本来源 sourceId 的伪造历史引用。
    let cross_source = source_scoped_slot("opds", &other_source);
    let ai_family = CredentialRef::new_scoped("ai", &uuid::Uuid::new_v4().simple().to_string())
        .unwrap()
        .as_str()
        .to_owned();
    let forged_legacy = format!("haven:opds:{}", uuid::Uuid::new_v4().simple());
    let entries = vec![
        cross_source.clone(),
        ai_family.clone(),
        forged_legacy.clone(),
    ];
    for reference in &entries {
        store.seed(reference, "foreign-secret");
    }
    install_cleanup_queue(&harness, &source_id, &entries).await;

    let report = harness
        .registry
        .retry_pending_cleanup(&store, &cache, UtcMillis::now())
        .await
        .unwrap();

    assert_eq!(report.guarded, 3, "归属判定失败的条目必须全部被守卫拦下");
    assert_eq!(report.credentials_deleted, 0);
    assert_eq!(report.credentials_settled, 0);
    assert_eq!(report.credentials_remaining, 3, "待清理义务不得被静默丢弃");
    assert!(
        store.deletes().is_empty(),
        "队列里的任何一条都不得被直接拿去做删除"
    );
    for reference in &entries {
        assert!(store.contains(reference), "外来秘密必须完好: {reference}");
    }
}

/// 同一引用、不同归属来源的两条待清理条目是**两条彼此独立的义务**：伪造的
/// `{R, 别的来源}` 既不得把合法的 `{R, 本来源}` 在登记时吞掉，也不得在合法义务出队时被
/// 连带抹掉。
///
/// 场景：本来源的一次凭据轮换把旧槽位 `R` 变成待清理义务，而外部删除**暂时失败**；队列里
/// 同时还躺着一条伪造的「同引用、别的来源」条目。系统凭据库恢复之后，扫描必须真实删掉 `R`
/// （合法槽位）并**只结清那一条合法义务**，把伪造条目连同另一个凭据家族的秘密原样留下。
#[tokio::test]
async fn obligations_are_matched_by_exact_identity_not_by_reference_alone() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let cache = InMemorySourceConfigCache::new();
    let rightful = add_source(
        &harness.registry,
        "本来源",
        "https://one.example.invalid/opds",
    )
    .await;
    let other = add_source(
        &harness.registry,
        "别的来源",
        "https://two.example.invalid/opds",
    )
    .await;
    // 另一个凭据家族的秘密：任何一条路径都不得碰它。
    let foreign_family =
        CredentialRef::new_scoped("ai", &uuid::Uuid::new_v4().simple().to_string())
            .unwrap()
            .as_str()
            .to_owned();
    store.seed(&foreign_family, "foreign-secret");

    // 本来源写入一条凭据，得到旧槽位 R。
    write_credential(&harness.registry, &store, &rightful, "old-secret")
        .await
        .unwrap();
    let slot = credential_ref(&harness.registry, &rightful).await.unwrap();
    assert!(store.contains(&slot));

    // 伪造：队列里出现一条「同引用 R、别的来源」的条目。
    install_cleanup_queue(&harness, &other, std::slice::from_ref(&slot)).await;

    // 轮换本来源的凭据：旧槽位 R 成为待清理义务，但外部删除暂时失败。
    store.fail_delete_for(&slot);
    write_credential(&harness.registry, &store, &rightful, "new-secret")
        .await
        .unwrap();
    let rotated = credential_ref(&harness.registry, &rightful).await.unwrap();
    assert_ne!(rotated, slot, "轮换必须分配一个新槽位");
    assert!(store.contains(&slot), "删除暂时失败，旧槽位必须还在");

    // 合法义务必须已经登记，并与伪造条目**并存**（伪造条目不得把它吞掉）。
    let stored = harness.stored_sources().await;
    let queued: Vec<(String, String)> = stored["credentialCleanup"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["reference"].as_str().unwrap().to_owned(),
                entry["sourceId"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert!(
        queued.contains(&(slot.clone(), rightful.clone())),
        "合法的精确义务必须被登记，绝不能被伪造条目吞掉: {queued:?}"
    );
    assert!(
        queued.contains(&(slot.clone(), other.clone())),
        "伪造条目必须原样保留: {queued:?}"
    );
    assert_eq!(queued.len(), 2);

    // 系统凭据库恢复：扫描只结清合法那一条，真实删掉 R。
    store.allow_all_deletes();
    let report = harness
        .registry
        .retry_pending_cleanup(&store, &cache, UtcMillis::now())
        .await
        .unwrap();
    assert_eq!(report.credentials_deleted, 1, "只删除合法那一条");
    assert_eq!(report.guarded, 1, "伪造条目必须被守卫拦下");
    assert!(!store.contains(&slot), "合法的旧槽位必须被真实删除");
    assert!(store.contains(&rotated), "当前凭据不得被误删");
    assert!(
        store.contains(&foreign_family),
        "另一个凭据家族的秘密不得被碰"
    );

    let stored = harness.stored_sources().await;
    assert_eq!(len_of(&stored, "credentialCleanup"), 1, "只结清合法义务");
    assert_eq!(stored["credentialCleanup"][0]["reference"], slot);
    assert_eq!(
        stored["credentialCleanup"][0]["sourceId"], other,
        "留下的必须正是那条伪造条目"
    );
}

/// 「删除已经成功、但出队事务失败」必须让义务留在队列里；下一次扫描幂等地补齐，
/// 并且**不把一次「目标本就不存在」虚报成删除**（`credential_deleted` 只计真实删除）。
#[tokio::test]
async fn a_delete_that_succeeded_but_could_not_dequeue_is_finished_by_the_next_sweep() {
    use std::sync::Arc;

    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let cache = InMemorySourceConfigCache::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;
    let slot = source_scoped_slot("opds", &source_id);
    store.seed(&slot, "doomed");
    install_cleanup_queue(&harness, &source_id, std::slice::from_ref(&slot)).await;

    // 第 0 次写事务是暂存槽有效期扫描，第 1 次是这条条目的出队：让它失败。
    let uow = Arc::new(FlakySettingsUoW::new(harness.db.clone()));
    uow.fail_run_at(1);
    let registry = harness.with_uow(uow.clone());

    let report = registry
        .retry_pending_cleanup(&store, &cache, UtcMillis::now())
        .await
        .unwrap();
    assert_eq!(
        report.credentials_deleted, 0,
        "出队失败时不得把它算成一次完成的删除"
    );
    assert_eq!(report.failed, 1);
    assert_eq!(
        report.credentials_remaining, 1,
        "删除已经发生，义务仍然必须留在队列里"
    );
    assert!(!store.contains(&slot), "秘密确实已经被物理删除");

    // 数据库恢复：下一次扫描发现目标本就不存在，幂等地把义务出队，且不虚报删除。
    uow.heal();
    let report = registry
        .retry_pending_cleanup(&store, &cache, UtcMillis::now())
        .await
        .unwrap();
    assert_eq!(
        report.credentials_deleted, 0,
        "目标本就不存在，不得虚报删除"
    );
    assert_eq!(report.credentials_settled, 1);
    assert_eq!(report.failed, 0);
    assert_eq!(report.credentials_remaining, 0);
    assert_eq!(
        len_of(&harness.stored_sources().await, "credentialCleanup"),
        0
    );
}

/// 没有注入租约的装配面：扫描必须 fail closed，一个字节都不碰系统凭据库，义务原样留着。
#[tokio::test]
async fn a_sweep_without_a_lease_fails_closed() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let cache = InMemorySourceConfigCache::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;
    let slot = source_scoped_slot("opds", &source_id);
    store.seed(&slot, "kept");
    install_cleanup_queue(&harness, &source_id, std::slice::from_ref(&slot)).await;

    let error = harness
        .without_lease()
        .retry_pending_cleanup(&store, &cache, UtcMillis::now())
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "SOURCE_CREDENTIAL_LEASE_UNAVAILABLE");
    assert!(store.deletes().is_empty());
    assert!(store.contains(&slot));
    assert_eq!(
        len_of(&harness.stored_sources().await, "credentialCleanup"),
        1
    );
}

#[tokio::test]
async fn cache_cleanup_survives_a_failing_port_and_is_retried_to_completion() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let cache = FailingDeleteCache::new();
    let source_id = add_source(
        &harness.registry,
        "电视源",
        "https://one.example.invalid/opds",
    )
    .await;
    cache
        .inner()
        .put(&source_id, b"{\"sites\":[]}", UtcMillis(1_000))
        .await
        .unwrap();

    let error = harness
        .registry
        .remove_custom_source(&source_id, &store, &cache)
        .await
        .expect_err("缓存清理失败必须如实返回错误");
    assert_eq!(error.code().as_str(), "DATABASE_ERROR");

    // 记录已经移除，但清理义务没有丢：它还在持久化队列里。
    assert!(!source_exists(&harness.registry, &source_id).await);
    let stored = harness.stored_sources().await;
    assert_eq!(len_of(&stored, "cacheCleanup"), 1);
    assert!(
        cache.inner().get(&source_id).await.unwrap().is_some(),
        "外部缓存确实还没有被删除"
    );

    // 重启之后由扫描把义务做完（缓存端口恢复之后）。
    cache.allow_deletes();
    let restarted = harness.restart();
    let report = restarted
        .retry_pending_cleanup(&store, cache.inner(), UtcMillis::now())
        .await
        .unwrap();
    assert_eq!(report.caches_deleted, 1);
    assert_eq!(report.caches_remaining, 0);
    assert_eq!(report.failed, 0);
    assert!(cache.inner().get(&source_id).await.unwrap().is_none());
    assert_eq!(len_of(&harness.stored_sources().await, "cacheCleanup"), 0);
}

// ---------- 清理队列写满时的有界推进（P2） ----------

/// 清理队列**写满**、同时存在过期暂存槽：清扫必须仍然有界地前进。
///
/// 这是一条真实存在过的停滞：退休先于履行，而退休在容量不足时让整次事务失败，于是每一轮
/// 扫描都在同一个点上返回 `SOURCE_CREDENTIAL_CLEANUP_FULL`——即使系统凭据库已经恢复，
/// 队列也永远排不空，新的写入、清除与移除被一起永久卡住。
#[tokio::test]
async fn a_full_queue_still_drains_and_keeps_unretired_stages_owned() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let cache = InMemorySourceConfigCache::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;

    let mut references = Vec::new();
    for _ in 0..MAX_CREDENTIAL_CLEANUP {
        let reference = source_scoped_slot("opds", &source_id);
        store.seed(&reference, "queued-secret");
        references.push(reference);
    }
    install_cleanup_queue(&harness, &source_id, &references).await;
    let now = UtcMillis::now();
    let abandoned = install_abandoned_stage(&harness, &store, &source_id, now).await;

    // 队列写满期间新的写入仍然 fail closed（稳定的 CLEANUP_FULL），且一个字节都不写外部
    // 存储：容量感知的退休不放宽写入的准入门槛，只是让队列能自己排空。
    let writes_before = store.writes().len();
    let blocked = write_credential(&harness.registry, &store, &source_id, "blocked")
        .await
        .expect_err("队列写满时新的写入必须 fail closed");
    assert_eq!(blocked.code().as_str(), "SOURCE_CREDENTIAL_CLEANUP_FULL");
    assert_eq!(
        store.writes().len(),
        writes_before,
        "被拒绝的写入不得触达系统凭据库"
    );
    assert_eq!(
        len_of(&harness.stored_sources().await, "stagedCredentials"),
        1,
        "失败的事务不得改动暂存队列"
    );

    // 第一轮：队列没有空位，过期暂存槽**不**被转走（它仍然是自己的清理所有者），但履行
    // 照常前进——容量不足不是失败，更不是「填一半就中止」。
    let report = harness
        .registry
        .retry_pending_cleanup(&store, &cache, now)
        .await
        .unwrap();
    assert_eq!(report.stages_retired, 0, "没有空位时不退休，而不是整体失败");
    assert_eq!(
        report.staged_remaining, 1,
        "装不下的过期槽位必须继续由暂存记录兜底"
    );
    assert_eq!(
        report.credentials_deleted, SOURCE_CREDENTIAL_CLEANUP_BATCH as u32,
        "队列满也必须真实前进"
    );
    assert_eq!(
        report.credentials_remaining as usize,
        MAX_CREDENTIAL_CLEANUP - SOURCE_CREDENTIAL_CLEANUP_BATCH
    );
    assert!(store.contains(&abandoned), "它这一轮还没轮到");

    // 反复有界扫描：队列腾空，被中断的槽位最终被真实删除。
    sweep_until_settled(&harness, &store, &cache, now).await;
    assert!(!store.contains(&abandoned), "被中断的写入最终必须被清干净");
    assert_eq!(store.live_references(), Vec::<String>::new());
    let stored = harness.stored_sources().await;
    assert_eq!(len_of(&stored, "credentialCleanup"), 0);
    assert_eq!(len_of(&stored, "stagedCredentials"), 0);

    // 队列腾空之后，写入重新可用（不再是 CLEANUP_FULL）。
    write_credential(&harness.registry, &store, &source_id, "fresh")
        .await
        .expect("队列清空之后写入必须重新可用");
    assert!(
        credential_ref(&harness.registry, &source_id)
            .await
            .is_some()
    );
}

/// 过期暂存槽比空位多：能转的照转、转不下的留队，绝不「填一半就中止」，也绝不丢义务。
#[tokio::test]
async fn more_expired_stages_than_capacity_are_retired_in_capacity_sized_steps() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let cache = InMemorySourceConfigCache::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;

    // 只留两个空位。
    let mut references = Vec::new();
    for _ in 0..(MAX_CREDENTIAL_CLEANUP - 2) {
        let reference = source_scoped_slot("opds", &source_id);
        store.seed(&reference, "queued-secret");
        references.push(reference);
    }
    install_cleanup_queue(&harness, &source_id, &references).await;

    let now = UtcMillis::now();
    let mut abandoned = Vec::new();
    for _ in 0..5 {
        abandoned.push(install_abandoned_stage(&harness, &store, &source_id, now).await);
    }

    let report = harness
        .registry
        .retry_pending_cleanup(&store, &cache, now)
        .await
        .unwrap();
    assert_eq!(report.stages_retired, 2, "只退休装得下的那两条");
    assert_eq!(
        report.staged_remaining, 3,
        "转不下的过期槽位继续留在暂存队列里（仍然有主）"
    );
    assert_eq!(
        report.credentials_deleted,
        SOURCE_CREDENTIAL_CLEANUP_BATCH as u32
    );
    let stored = harness.stored_sources().await;
    // 退休 2 条之后队列写满（= 上限），本轮又真实删除了一个批量，因此落库剩余恰好是
    // 上限减去一个批量；报告里的剩余条数与落库必须一致。
    assert_eq!(
        len_of(&stored, "credentialCleanup"),
        MAX_CREDENTIAL_CLEANUP - SOURCE_CREDENTIAL_CLEANUP_BATCH
    );
    assert_eq!(
        len_of(&stored, "credentialCleanup") as u32,
        report.credentials_remaining
    );
    assert_eq!(len_of(&stored, "stagedCredentials"), 3);

    // 收敛之后五条被中断的写入全部被清干净，队列一条不剩。
    sweep_until_settled(&harness, &store, &cache, now).await;
    for reference in &abandoned {
        assert!(
            !store.contains(reference),
            "被中断的写入不得留下任何无主槽位"
        );
    }
    assert_eq!(store.live_references(), Vec::<String>::new());
    assert_eq!(
        len_of(&harness.stored_sources().await, "credentialCleanup"),
        0
    );
    assert_eq!(
        len_of(&harness.stored_sources().await, "stagedCredentials"),
        0
    );
}

// ---------- 公平：队首的不可结项条目不得饿死后面的义务 ----------

/// 队首长期不可结项的条目不得把排在后面的合法义务永远挡在批量窗口之外。
///
/// 守卫拦下或外部删除失败的条目在下一轮被**重排**到队尾——它们原样留在队列里（不遗忘、
/// 不丢弃），只是让后面的条目也有机会被处理。
#[tokio::test]
async fn guarded_entries_rotate_to_the_tail_and_do_not_starve_later_obligations() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let cache = InMemorySourceConfigCache::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;

    // 队首是一个批量还多的**永不出队**条目：伪造的历史引用（provider 段对、profile 不是
    // 本来源），归属判定一律拦下并留在队列里。
    let guarded_total = SOURCE_CREDENTIAL_CLEANUP_BATCH + 6;
    let mut guarded = Vec::new();
    let mut entries = Vec::new();
    for _ in 0..guarded_total {
        let reference = format!("haven:opds:{}", uuid::Uuid::new_v4().simple());
        guarded.push(reference.clone());
        entries.push(reference);
    }
    // 排在它们后面的是三条真正可以删除的合法义务。
    let mut deletable = Vec::new();
    for _ in 0..3 {
        let reference = source_scoped_slot("opds", &source_id);
        store.seed(&reference, "secret");
        deletable.push(reference.clone());
        entries.push(reference);
    }
    install_cleanup_queue(&harness, &source_id, &entries).await;

    // 第一轮：窗口整好被守卫条目占满，合法义务还没轮到——但它们一条都不能丢。
    let report = harness
        .registry
        .retry_pending_cleanup(&store, &cache, UtcMillis::now())
        .await
        .unwrap();
    assert_eq!(report.credentials_deleted, 0);
    assert_eq!(report.guarded as usize, SOURCE_CREDENTIAL_CLEANUP_BATCH);
    assert_eq!(
        report.credentials_remaining as usize,
        guarded_total + 3,
        "守卫条目与合法义务都必须留在队列里"
    );
    for reference in &deletable {
        assert!(
            store.contains(reference),
            "合法义务尚未被处理，但绝不能消失"
        );
    }

    // 第二轮：被守卫拦下的条目已经轮转到队尾，合法义务终于被处理；守卫条目一条不少。
    let report = harness
        .registry
        .retry_pending_cleanup(&store, &cache, UtcMillis::now())
        .await
        .unwrap();
    assert_eq!(
        report.credentials_deleted, 3,
        "轮转必须让后面的合法义务有机会"
    );
    assert_eq!(
        report.credentials_remaining as usize, guarded_total,
        "守卫条目一条都不能被丢弃"
    );
    for reference in &deletable {
        assert!(!store.contains(reference));
    }
    let stored = harness.stored_sources().await;
    let remaining: Vec<String> = stored["credentialCleanup"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["reference"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(remaining.len(), guarded_total);
    for reference in &guarded {
        assert!(
            remaining.contains(reference),
            "被守卫拦下的条目必须原样留在队列里: {reference}"
        );
    }
}

// ---------- 缓存清理的来源所有权（R2） ----------

/// 缓存清理同样受**来源所有权**约束：陈旧或伪造的待清理条目不得删掉一个仍然存在的来源的
/// last-known-good 配置。义务原样留着，等来源真的被移除之后再履行。
#[tokio::test]
async fn a_pending_cache_cleanup_never_deletes_a_live_sources_config() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let cache = InMemorySourceConfigCache::new();
    let source_id = add_source(
        &harness.registry,
        "电视源",
        "https://one.example.invalid/opds",
    )
    .await;
    cache
        .put(&source_id, br#"{"sites":[]}"#, UtcMillis(1_000))
        .await
        .unwrap();

    // 畸形状态：队列里出现一条针对**仍然存在**的来源的缓存清理义务。
    let mut payload = harness.stored_sources().await;
    payload["cacheCleanup"] = serde_json::json!([source_id.clone()]);
    harness.overwrite_sources(&payload).await;

    let report = harness
        .registry
        .retry_pending_cleanup(&store, &cache, UtcMillis::now())
        .await
        .unwrap();
    assert_eq!(report.caches_guarded, 1);
    assert_eq!(report.caches_deleted, 0);
    assert_eq!(report.caches_remaining, 1, "在用判定不得把义务丢掉");
    assert!(
        cache.get(&source_id).await.unwrap().is_some(),
        "仍然存在的来源不得失去 last-known-good 配置"
    );

    // 用户真的删除这个来源：义务照常履行，缓存被清掉。
    harness
        .registry
        .remove_custom_source(&source_id, &store, &cache)
        .await
        .unwrap();
    assert!(cache.get(&source_id).await.unwrap().is_none());
    assert_eq!(len_of(&harness.stored_sources().await, "cacheCleanup"), 0);
}
