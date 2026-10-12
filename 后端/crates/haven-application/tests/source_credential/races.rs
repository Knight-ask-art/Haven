//! 并发写入者与删除/清除/扫描的确定性交错（真实 SQLite + 真实设置事务 + 真实租约）。
//!
//! 交错只有三种来源，都不依赖时钟或调度概率：
//! - 写入者停在外部 IO 中间直到测试放行（[`HoldGate`]），用来观测「同一时刻只有一个合规
//!   写者」以及「被挡下的写者一个字节都没写」；
//! - 外部端口在 `set` 期间跑一段真实用例（[`DuringWrite`]）：合规的删除/清除必须被租约
//!   挡下，非合规的直接改库则仍然要被采纳栅栏拦下；
//! - 两个**独立构造**的装配面（各自一条租约、同一把锁文件 / 同一份进程内租约）互相竞争。

use std::sync::Arc;

use haven_application::services::source_config_cache::InMemorySourceConfigCache;
use haven_application::services::source_registry::{
    MAX_STAGED_CREDENTIALS, SOURCE_CREDENTIAL_STAGE_TTL_MS,
};
use haven_common::UtcMillis;

use super::fixtures::{
    DuringWrite, FileHarness, Harness, HoldGate, MemoryCredentialStore, add_source,
    clear_credential, credential_ref, len_of, source_exists, source_scoped_slot, write_credential,
};

/// 外部写入进行中，第二个合规写入者必须立刻拿到 BUSY，且不产生任何外部副作用。
#[tokio::test]
async fn a_second_writer_gets_busy_while_the_first_holds_the_external_write() {
    let harness = Harness::new();
    let store = Arc::new(MemoryCredentialStore::new());
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;

    let gate = Arc::new(HoldGate::new());
    store.hold_next_set(gate.clone());

    let writer_registry = harness.registry.clone();
    let writer_store = store.clone();
    let writer_source = source_id.clone();
    let writer = tokio::spawn(async move {
        write_credential(
            &writer_registry,
            &writer_store,
            &writer_source,
            "first-secret",
        )
        .await
    });

    // 写入者已经过了暂存，正停在系统凭据库的写入里。
    gate.entered().await;

    // 第二个**独立构造**的实例：同一个数据库、同一份租约，因此它必须被挡下。
    let second = harness.independent();
    let error = write_credential(&second, &store, &source_id, "second-secret")
        .await
        .expect_err("外部写入期间第二个写者必须被租约挡下");
    assert_eq!(error.code().as_str(), "SOURCE_CREDENTIAL_BUSY");
    assert!(error.retryable(), "占用是可以直接重试的状态");
    assert_eq!(store.writes().len(), 1, "被挡下的写者不得触达系统凭据库");
    assert!(
        credential_ref(&second, &source_id).await.is_none(),
        "被挡下的写者不得改动持久化引用"
    );

    gate.release();
    writer
        .await
        .expect("写入任务必须正常结束")
        .expect("持租约的写者必须成功");

    let adopted = credential_ref(&harness.registry, &source_id)
        .await
        .expect("必须采纳一个版本槽");
    assert_eq!(store.writes(), vec![adopted.clone()]);
    assert_eq!(store.live_references(), vec![adopted.clone()]);
    let stored = harness.stored_sources().await;
    assert_eq!(len_of(&stored, "stagedCredentials"), 0);
    assert_eq!(len_of(&stored, "credentialCleanup"), 0);

    // 放行之后（锁已释放），被挡下的写者可以照常重试并成功。
    write_credential(&second, &store, &source_id, "second-secret")
        .await
        .expect("锁释放之后必须能写入");
    let retried = credential_ref(&second, &source_id).await.unwrap();
    assert_ne!(retried, adopted, "重试写入必须分配新的版本槽");
    assert_eq!(store.live_references(), vec![retried]);
}

/// 两个**独立构造**的装配面（各自一条租约实例、同一把锁文件）在同一把锁上竞争与释放。
#[tokio::test]
async fn two_independently_constructed_assemblies_contend_on_the_same_lock_file() {
    let harness = FileHarness::new();
    let store = Arc::new(MemoryCredentialStore::new());
    let source_id = add_source(
        &harness.registry,
        "文件库书源",
        "https://file.example.invalid/opds",
    )
    .await;
    let second = harness.independent();
    assert!(
        source_exists(&second, &source_id).await,
        "第二个装配面必须读到同一个数据库里的来源"
    );

    let gate = Arc::new(HoldGate::new());
    store.hold_next_set(gate.clone());
    let writer_registry = harness.registry.clone();
    let writer_store = store.clone();
    let writer_source = source_id.clone();
    let writer = tokio::spawn(async move {
        write_credential(&writer_registry, &writer_store, &writer_source, "first").await
    });
    gate.entered().await;

    let error = write_credential(&second, &store, &source_id, "second")
        .await
        .expect_err("同一把锁文件上的第二个写者必须被挡下");
    assert_eq!(error.code().as_str(), "SOURCE_CREDENTIAL_BUSY");
    assert_eq!(store.writes().len(), 1);

    gate.release();
    writer.await.unwrap().unwrap();
    assert!(
        harness.lock_file().exists(),
        "锁文件必须一直存在（不 unlink、不重建）"
    );

    // 释放之后第二个装配面照常写入。
    write_credential(&second, &store, &source_id, "second")
        .await
        .expect("锁释放之后第二个装配面必须能写入");
    let stored = harness.stored_sources().await;
    assert_eq!(len_of(&stored, "stagedCredentials"), 0);
    assert_eq!(len_of(&stored, "credentialCleanup"), 0);
}

/// 写入者被卡在外部 IO 期间，一个**远远晚于**任何有效期的时间点上的扫描也回收不了它：
/// 扫描必须先拿到租约，于是只能拿到 BUSY。租约是「这条暂存记录还活着吗」的**证明**，
/// 时间不是。
#[tokio::test]
async fn a_blocked_writer_cannot_be_retired_by_ttl_or_capacity_pressure() {
    let harness = Harness::new();
    let store = Arc::new(MemoryCredentialStore::new());
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;

    let gate = Arc::new(HoldGate::new());
    store.hold_next_set(gate.clone());
    let writer_registry = harness.registry.clone();
    let writer_store = store.clone();
    let writer_source = source_id.clone();
    let writer = tokio::spawn(async move {
        write_credential(&writer_registry, &writer_store, &writer_source, "in-flight").await
    });
    gate.entered().await;

    // 它确实已经在暂存队列里留下了一条记录，并且外部写入已经发生。
    let stored = harness.stored_sources().await;
    assert_eq!(len_of(&stored, "stagedCredentials"), 1);
    let staged_ref = stored["stagedCredentials"][0]["reference"]
        .as_str()
        .expect("暂存记录必须有引用")
        .to_owned();
    assert!(store.contains(&staged_ref), "外部写入确实已经发生");

    // 1) TTL：把时间推到远超有效期之后，扫描仍然拿不到租约。
    let far_future = UtcMillis(UtcMillis::now().0 + SOURCE_CREDENTIAL_STAGE_TTL_MS * 1_000);
    let error = harness
        .registry
        .retry_pending_cleanup(
            store.as_ref(),
            &InMemorySourceConfigCache::new(),
            far_future,
        )
        .await
        .expect_err("在途写入持有租约时扫描必须被挡下");
    assert_eq!(error.code().as_str(), "SOURCE_CREDENTIAL_BUSY");

    let stored = harness.stored_sources().await;
    assert_eq!(
        len_of(&stored, "stagedCredentials"),
        1,
        "在途写入的暂存记录不得被回收"
    );
    assert_eq!(stored["stagedCredentials"][0]["reference"], staged_ref);
    assert_eq!(
        len_of(&stored, "credentialCleanup"),
        0,
        "不得给一条活着的写入编造清理义务"
    );
    assert!(store.contains(&staged_ref), "在途写入的秘密不得被删除");
    assert!(store.deletes().is_empty());

    // 2) 容量压力：把暂存队列塞到上限之上。回收只可能发生在**持租约**的暂存事务里，
    //    而被挡下的扫描连租约都拿不到，因此它既回收不了、也遗忘不了任何一条在途写入。
    let filler_source = add_source(
        &harness.registry,
        "填充来源",
        "https://filler.example.invalid/opds",
    )
    .await;
    let mut payload = harness.stored_sources().await;
    let mut staged = payload["stagedCredentials"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(staged.len(), 1, "在途写入的暂存记录必须还在");
    let base = UtcMillis::now().0;
    let filler: Vec<serde_json::Value> = (0..MAX_STAGED_CREDENTIALS + 4)
        .map(|index| {
            serde_json::json!({
                "reference": source_scoped_slot("opds", &filler_source),
                "sourceId": filler_source.clone(),
                "stagedAtMs": base - index as i64,
            })
        })
        .collect();
    staged.extend(filler);
    payload["stagedCredentials"] = serde_json::Value::Array(staged);
    harness.overwrite_sources(&payload).await;

    let error = harness
        .registry
        .retry_pending_cleanup(
            store.as_ref(),
            &InMemorySourceConfigCache::new(),
            far_future,
        )
        .await
        .expect_err("容量压力也不能绕过租约");
    assert_eq!(error.code().as_str(), "SOURCE_CREDENTIAL_BUSY");
    let stored = harness.stored_sources().await;
    assert!(
        stored["stagedCredentials"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["reference"] == staged_ref),
        "容量压力绝不能把一次在途写入弄丢"
    );
    assert!(
        store.contains(&staged_ref),
        "容量压力绝不能删掉一次在途写入的秘密"
    );

    // 放行：写入者正常完成采纳，它自己的暂存记录随之消失，槽位成为当前凭据。
    gate.release();
    writer.await.unwrap().expect("写入必须成功");
    assert_eq!(
        credential_ref(&harness.registry, &source_id).await,
        Some(staged_ref)
    );
}

/// 写入者被**取消**（future 在外部写入之后、采纳之前被丢弃）：暂存记录仍在落库负载里，
/// 它就是这次中断的清理所有者；租约随 future 一起释放，之后的扫描可以安全地把它清干净。
#[tokio::test]
async fn a_cancelled_writer_keeps_a_cleanup_owner_and_is_recovered_after_release() {
    let harness = Harness::new();
    let store = Arc::new(MemoryCredentialStore::new());
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;

    let gate = Arc::new(HoldGate::new());
    store.hold_next_set(gate.clone());
    let writer_registry = harness.registry.clone();
    let writer_store = store.clone();
    let writer_source = source_id.clone();
    let writer = tokio::spawn(async move {
        write_credential(&writer_registry, &writer_store, &writer_source, "cancelled").await
    });
    gate.entered().await;

    let stored = harness.stored_sources().await;
    let staged_ref = stored["stagedCredentials"][0]["reference"]
        .as_str()
        .expect("暂存记录必须已经落库")
        .to_owned();
    assert!(store.contains(&staged_ref), "外部写入确实已经发生");

    // 取消：future 在外部写入之后、采纳之前被丢弃。租约守卫随栈帧一起 Drop。
    writer.abort();
    let cancelled = writer.await;
    assert!(cancelled.is_err(), "任务必须是被取消的");

    // 取消之后没有任何东西采纳它：引用仍是空的，槽位也没有被任何人接管。
    assert!(
        credential_ref(&harness.registry, &source_id)
            .await
            .is_none(),
        "被取消的写入不得留下被采纳的引用"
    );
    let stored = harness.stored_sources().await;
    assert_eq!(
        len_of(&stored, "stagedCredentials"),
        1,
        "被取消的写入必须留下它的清理所有者"
    );
    assert_eq!(stored["stagedCredentials"][0]["reference"], staged_ref);

    // 租约已经随 future 释放：**重建**的服务实例现在可以拿到它，并把这条已经不会再继续
    // 的暂存槽转入清理队列、真实删除。
    let far_future = UtcMillis(UtcMillis::now().0 + SOURCE_CREDENTIAL_STAGE_TTL_MS * 1_000);
    let restarted = harness.restart();
    let report = restarted
        .retry_pending_cleanup(
            store.as_ref(),
            &InMemorySourceConfigCache::new(),
            far_future,
        )
        .await
        .expect("租约释放之后扫描必须能运行");
    assert_eq!(report.stages_retired, 1);
    assert_eq!(report.credentials_deleted, 1);
    assert_eq!(report.staged_remaining, 0);
    assert_eq!(report.credentials_remaining, 0);
    assert!(!store.contains(&staged_ref), "被取消的写入必须被清干净");
}

/// 合规的**移除**不可能插进一次外部写入：它同样要持租约，于是只能拿到 BUSY。
/// 写入完成之后移除照常进行，并且把那一个槽位清干净。
#[tokio::test]
async fn a_compliant_removal_cannot_interleave_with_an_in_flight_write() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;
    store.during_write(DuringWrite::RemoveSource {
        registry: harness.registry.clone(),
        source_id: source_id.clone(),
        cache: Arc::new(InMemorySourceConfigCache::new()),
    });

    write_credential(&harness.registry, &store, &source_id, "keeper")
        .await
        .expect("并发的合规移除不得让这次写入失败");
    assert_eq!(
        store.hook_outcomes(),
        vec!["SOURCE_CREDENTIAL_BUSY".to_owned()],
        "合规移除必须被租约挡下"
    );
    assert!(
        source_exists(&harness.registry, &source_id).await,
        "被挡下的移除不得删掉来源"
    );
    let adopted = credential_ref(&harness.registry, &source_id)
        .await
        .expect("写入必须被采纳");

    // 写入结束（租约释放）之后，移除照常进行并清理掉这一个槽位。
    let removed = harness
        .registry
        .remove_custom_source(&source_id, &store, &InMemorySourceConfigCache::new())
        .await
        .expect("写入结束之后移除必须成功");
    assert!(removed.credential_deleted);
    assert!(!store.contains(&adopted));
    assert!(store.live_references().is_empty());
    assert!(!source_exists(&harness.registry, &source_id).await);
}

/// 合规的**清除**同样插不进一次外部写入；写入完成后清除照常进行，且不伤及旁观来源。
#[tokio::test]
async fn a_compliant_clear_cannot_interleave_with_an_in_flight_write() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let target = add_source(
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
    write_credential(&harness.registry, &store, &target, "original")
        .await
        .unwrap();
    write_credential(&harness.registry, &store, &bystander, "bystander")
        .await
        .unwrap();
    let original = credential_ref(&harness.registry, &target).await.unwrap();
    let bystander_ref = credential_ref(&harness.registry, &bystander).await.unwrap();

    store.during_write(DuringWrite::ClearCredential {
        registry: harness.registry.clone(),
        source_id: target.clone(),
    });
    write_credential(&harness.registry, &store, &target, "in-flight")
        .await
        .expect("并发的合规清除不得让这次写入失败");
    assert_eq!(
        store.hook_outcomes(),
        vec!["SOURCE_CREDENTIAL_BUSY".to_owned()],
        "合规清除必须被租约挡下"
    );

    let in_flight = credential_ref(&harness.registry, &target)
        .await
        .expect("写入必须被采纳");
    assert_ne!(in_flight, original);
    assert!(!store.contains(&original), "被轮换的旧槽位必须被删除");
    assert!(
        !store.deletes().contains(&bystander_ref),
        "任何一条路径都不得删除另一个来源的凭据"
    );

    // 写入结束之后清除照常进行。
    clear_credential(&harness.registry, &store, &target)
        .await
        .expect("写入结束之后清除必须成功");
    assert_eq!(credential_ref(&harness.registry, &target).await, None);
    assert_eq!(store.live_references(), vec![bystander_ref]);
}

/// 唯一还能让来源在一次外部写入期间消失的方式是**非合规**的直接改库（合规移除要持租约）。
/// 采纳栅栏必须继续拦下它，并且补偿只删除自己刚写进去的那一个槽位。
#[tokio::test]
async fn a_forged_removal_during_the_write_still_cannot_leave_an_orphan() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let source_id = add_source(
        &harness.registry,
        "我的书源",
        "https://one.example.invalid/opds",
    )
    .await;
    store.during_write(DuringWrite::ForgeSourceRemoval {
        repos: harness.repos.clone(),
        source_id: source_id.clone(),
    });

    let error = write_credential(&harness.registry, &store, &source_id, "late")
        .await
        .expect_err("来源已消失时这次写入必须作废");
    assert_eq!(error.code().as_str(), "SOURCE_CONFIG_STALE");

    assert!(
        !source_exists(&harness.registry, &source_id).await,
        "迟到的写入不得复活已经删除的来源"
    );
    assert!(
        store.live_references().is_empty(),
        "迟到的写入必须删掉自己刚写进去的槽位，绝不留下无主孤儿"
    );
    assert_eq!(store.deletes().len(), 1);
    let stored = harness.stored_sources().await;
    assert_eq!(len_of(&stored, "stagedCredentials"), 0);
    assert_eq!(len_of(&stored, "credentialCleanup"), 0);
    assert_eq!(len_of(&stored, "cacheCleanup"), 0);
}

/// 扫描的守卫：仍被来源记录引用、或仍属于在途暂存槽的槽位绝不被删除。
#[tokio::test]
async fn a_sweep_never_deletes_a_referenced_or_in_flight_slot() {
    let harness = Harness::new();
    let store = MemoryCredentialStore::new();
    let referenced = add_source(
        &harness.registry,
        "正在使用的书源",
        "https://live.example.invalid/opds",
    )
    .await;
    let in_flight = add_source(
        &harness.registry,
        "写入中的书源",
        "https://staging.example.invalid/opds",
    )
    .await;
    write_credential(&harness.registry, &store, &referenced, "live-secret")
        .await
        .unwrap();
    let live_ref = credential_ref(&harness.registry, &referenced)
        .await
        .unwrap();
    // 在途槽位：已经在系统凭据库里写进去了，但采纳还没有发生。
    let staged_ref = source_scoped_slot("opds", &in_flight);
    store.seed(&staged_ref, "staged-secret");

    // 账实不符的畸形状态：两条都同时出现在清理队列里。
    let now = UtcMillis::now();
    let mut payload = harness.stored_sources().await;
    payload["credentialCleanup"] = serde_json::json!([
        { "reference": live_ref.clone(), "sourceId": referenced.clone() },
        { "reference": staged_ref.clone(), "sourceId": in_flight.clone() },
    ]);
    payload["stagedCredentials"] = serde_json::json!([
        {
            "reference": staged_ref.clone(),
            "sourceId": in_flight.clone(),
            "stagedAtMs": now.0,
        },
    ]);
    harness.overwrite_sources(&payload).await;

    let report = harness
        .registry
        .retry_pending_cleanup(&store, &InMemorySourceConfigCache::new(), now)
        .await
        .unwrap();

    assert_eq!(report.guarded, 2, "两条都必须被守卫拦下");
    assert_eq!(report.credentials_deleted, 0);
    assert_eq!(report.credentials_settled, 0);
    assert_eq!(report.staged_remaining, 1, "在途槽位不得被回收");
    assert!(store.contains(&live_ref), "仍被引用的槽位绝不能被删除");
    assert!(store.contains(&staged_ref), "在途槽位绝不能被删除");
    assert!(store.deletes().is_empty());
    let stored = harness.stored_sources().await;
    assert_eq!(
        len_of(&stored, "credentialCleanup"),
        2,
        "被守卫拦下的条目必须留在队列里继续等待判定"
    );
}
